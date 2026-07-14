use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, bail};
use common::{block::Block, transaction::LeeTransaction};
use cross_zone::{build_dispatch_from_emission, extract_emission};
use cross_zone_inbox_core::{
    CrossZoneMessage, Instruction as InboxInstruction, MessageKey, ZoneId, message_key,
};
use futures::StreamExt as _;
use lee::PublicKey;
use log::{error, info};
use logos_blockchain_core::mantle::ops::channel::ChannelId;
use logos_blockchain_zone_sdk::{
    CommonHttpClient, ZoneMessage, adapter::NodeHttpClient, indexer::ZoneIndexer,
};
use tokio::sync::RwLock;

use crate::config::IndexerConfig;

/// How often the verifier logs that it is still waiting on a lagging peer reader,
/// so a stuck wait is observable without rejecting a legitimate message.
const LAG_LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Cache of finalized peer-zone blocks, filled by per-peer reader tasks and read
/// by the verifier to re-derive cross-zone dispatch transactions.
#[derive(Clone, Default)]
struct PeerBlocks {
    chains: Arc<RwLock<HashMap<ZoneId, HashMap<u64, Block>>>>,
}

impl PeerBlocks {
    async fn insert(&self, zone: ZoneId, block: Block) {
        self.chains
            .write()
            .await
            .entry(zone)
            .or_default()
            .insert(block.header.block_id, block);
    }

    async fn get(&self, zone: ZoneId, block_id: u64) -> Option<Block> {
        self.chains
            .read()
            .await
            .get(&zone)
            .and_then(|chain| chain.get(&block_id).cloned())
    }

    /// The highest block id this reader has finalized for `zone`, or `None` if it
    /// has read nothing yet.
    async fn highest_seen(&self, zone: ZoneId) -> Option<u64> {
        self.chains
            .read()
            .await
            .get(&zone)
            .and_then(|chain| chain.keys().copied().max())
    }
}

/// The indexer-side Option B verifier.
///
/// For every cross-zone dispatch in a block it re-derives the transaction from
/// the peer's finalized block and rejects it if the bytes differ (a forgery), so
/// delivery no longer relies on trusting the sequencer. A replay of an
/// already-delivered message is accepted, since the inbox no-ops it on chain.
#[derive(Clone)]
pub struct CrossZoneVerifier {
    self_zone: ZoneId,
    /// Pinned block-signing key per peer zone, enforced during re-derivation.
    /// One key per peer is sufficient while a zone has a single sequencer; key
    /// sets with rotation come in with decentralized sequencing. The pin is
    /// largely redundant given Bedrock's turn-based write authorization, so it is
    /// optional: a peer with no configured key is not signature-checked.
    peer_pubkeys: HashMap<ZoneId, PublicKey>,
    peers: PeerBlocks,
    seen: Arc<RwLock<HashSet<MessageKey>>>,
}

impl CrossZoneVerifier {
    /// Builds the verifier and spawns one peer reader per configured peer.
    /// Returns `None` when cross-zone messaging is disabled.
    pub fn start(config: &IndexerConfig) -> Option<Self> {
        let cross_zone = config.cross_zone.as_ref()?;
        let self_zone: ZoneId = *config.channel_id.as_ref();
        let peers = PeerBlocks::default();
        let mut peer_pubkeys = HashMap::new();

        for peer in &cross_zone.peers {
            let node = NodeHttpClient::new(
                CommonHttpClient::new(config.bedrock_config.auth.clone().map(Into::into)),
                config.bedrock_config.addr.clone(),
            );
            if let Some(bytes) = peer.expected_block_signing_pubkey {
                let pubkey = PublicKey::try_new(bytes)
                    .expect("configured peer block-signing pubkey is a valid key");
                peer_pubkeys.insert(peer.channel_id, pubkey);
            }
            tokio::spawn(read_peer(
                ZoneIndexer::new(ChannelId::from(peer.channel_id), node),
                peer.channel_id,
                peers.clone(),
                config.consensus_info_polling_interval,
            ));
        }

        Some(Self {
            self_zone,
            peer_pubkeys,
            peers,
            seen: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Verifies every cross-zone dispatch in a block, returning `Err` on the first
    /// forged dispatch (the caller halts ingestion) or the keys to mark seen.
    ///
    /// The caller MUST record the returned keys via [`Self::record_seen`] only
    /// after the block applies, so the seen-set mirrors the inbox's on-chain
    /// seen-shard. Marking a key from a block that never applies would let a later
    /// forged dispatch reuse it to skip re-derivation while the inbox delivers the
    /// forgery. A key already seen is a replay the inbox no-ops, so it is accepted
    /// without re-derivation rather than halting on a legitimate re-delivery.
    pub async fn verify_block(&self, block: &Block) -> Result<Vec<MessageKey>> {
        let mut verified = Vec::new();
        for tx in &block.body.transactions {
            let Some(msg) = Self::decode_dispatch(tx) else {
                continue;
            };

            let key = message_key(&msg.src_zone, msg.src_block_id, msg.src_tx_index);
            if self.seen.read().await.contains(&key) {
                continue;
            }

            let expected = self.rederive(&msg).await?;
            if LeeTransaction::Public(expected) != *tx {
                bail!(
                    "forged cross-zone dispatch from zone {} block {} tx {}: re-derivation mismatch",
                    hex::encode(msg.src_zone),
                    msg.src_block_id,
                    msg.src_tx_index
                );
            }

            info!(
                "Verified cross-zone dispatch from zone {} block {} tx {}",
                hex::encode(msg.src_zone),
                msg.src_block_id,
                msg.src_tx_index
            );
            verified.push(key);
        }
        Ok(verified)
    }

    /// Marks the given dispatch keys seen, so a later replay of them is accepted
    /// without re-derivation. Call only after the block that carried them has been
    /// applied on chain (see [`Self::verify_block`]).
    pub async fn record_seen(&self, keys: Vec<MessageKey>) {
        if keys.is_empty() {
            return;
        }
        self.seen.write().await.extend(keys);
    }

    /// Decodes a transaction into the cross-zone message it dispatches, or `None`
    /// if it is not an inbox dispatch.
    fn decode_dispatch(tx: &LeeTransaction) -> Option<CrossZoneMessage> {
        let LeeTransaction::Public(public_tx) = tx else {
            return None;
        };
        if public_tx.message().program_id != programs::cross_zone_inbox().id() {
            return None;
        }
        match risc0_zkvm::serde::from_slice::<InboxInstruction, _>(
            &public_tx.message().instruction_data,
        ) {
            Ok(InboxInstruction::Dispatch(msg)) => Some(msg),
            Err(_) => None,
        }
    }

    /// Re-derives the dispatch transaction the watcher should have injected for
    /// `msg`, reading the source emission from the peer's finalized block.
    async fn rederive(&self, msg: &CrossZoneMessage) -> Result<lee::PublicTransaction> {
        let peer_block = self
            .wait_for_peer_block(msg.src_zone, msg.src_block_id)
            .await?;

        // Equivocation defense: the source block must be signed by the peer's
        // pinned block-signing key, not merely inscribed on the channel.
        if let Some(expected) = self.peer_pubkeys.get(&msg.src_zone)
            && !peer_block.is_signed_by(expected)
        {
            bail!(
                "forged cross-zone dispatch: peer zone {} block {} is not signed by the pinned block-signing key",
                hex::encode(msg.src_zone),
                msg.src_block_id
            );
        }

        let emission_tx = peer_block
            .body
            .transactions
            .get(usize::try_from(msg.src_tx_index).expect("u32 index fits in usize"))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "src_tx_index {} out of range in peer block",
                    msg.src_tx_index
                )
            })?;

        let LeeTransaction::Public(emission_tx) = emission_tx else {
            bail!("peer emission transaction is not public");
        };
        let message = emission_tx.message();
        let emission =
            extract_emission(message.program_id, &message.instruction_data).ok_or_else(|| {
                anyhow::anyhow!("peer transaction at src_tx_index is not a recognized emitter")
            })?;

        if emission.target_zone != self.self_zone {
            bail!("peer emission targets a different zone");
        }

        Ok(build_dispatch_from_emission(
            msg.src_zone,
            msg.src_block_id,
            msg.src_tx_index,
            message.program_id,
            emission.target_program_id,
            &emission.target_accounts,
            emission.payload,
        ))
    }

    /// Resolves the referenced peer block, distinguishing forgery from lag.
    ///
    /// If the block is cached, return it. If our peer reader has already
    /// finalized past `block_id` and we still do not have it, the reference is to
    /// a block that does not exist on the peer chain, a forgery, so reject now.
    async fn wait_for_peer_block(&self, zone: ZoneId, block_id: u64) -> Result<Block> {
        let mut waited = Duration::ZERO;
        loop {
            if let Some(block) = self.peers.get(zone, block_id).await {
                return Ok(block);
            }
            if self
                .peers
                .highest_seen(zone)
                .await
                .is_some_and(|h| h >= block_id)
            {
                bail!(
                    "forged cross-zone reference: peer zone {} finalized past block {} but it is absent",
                    hex::encode(zone),
                    block_id
                );
            }
            if !waited.is_zero() && waited.as_secs().is_multiple_of(LAG_LOG_INTERVAL.as_secs()) {
                info!(
                    "Waiting for peer zone {} to finalize block {} ({}s); reader is behind",
                    hex::encode(zone),
                    block_id,
                    waited.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            waited = waited.saturating_add(Duration::from_secs(1));
        }
    }
}

/// Reads a peer zone's finalized blocks from Bedrock into the shared cache.
#[expect(
    clippy::infinite_loop,
    reason = "the peer reader runs for the lifetime of the indexer process"
)]
async fn read_peer(
    zone_indexer: ZoneIndexer<NodeHttpClient>,
    peer_zone: ZoneId,
    peers: PeerBlocks,
    poll_interval: Duration,
) {
    info!(
        "Cross-zone peer reader started for {}",
        hex::encode(peer_zone)
    );

    let mut cursor = None;
    loop {
        let stream = match zone_indexer.next_messages(cursor).await {
            Ok(stream) => stream,
            Err(err) => {
                error!(
                    "Peer reader next_messages failed for {}: {err}",
                    hex::encode(peer_zone)
                );
                tokio::time::sleep(poll_interval).await;
                continue;
            }
        };
        let mut stream = std::pin::pin!(stream);

        while let Some((msg, slot)) = stream.next().await {
            if let ZoneMessage::Block(zone_block) = msg {
                match borsh::from_slice::<Block>(&zone_block.data) {
                    Ok(block) => peers.insert(peer_zone, block).await,
                    Err(err) => error!("Peer reader failed to deserialize block: {err}"),
                }
            }
            cursor = Some(slot);
        }

        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use common::test_utils::produce_dummy_block;
    use lee::{
        PrivateKey, PublicKey, PublicTransaction,
        public_transaction::{Message, WitnessSet},
    };
    use ping_core::{SenderInstruction, ping_record_pda};

    use super::*;

    const SELF_ZONE: ZoneId = [1; 32];
    const PEER_ZONE: ZoneId = [2; 32];
    const PEER_BLOCK_ID: u64 = 5;

    fn verifier() -> CrossZoneVerifier {
        verifier_with_pinned_keys(HashMap::new())
    }

    fn verifier_with_pinned_keys(peer_pubkeys: HashMap<ZoneId, PublicKey>) -> CrossZoneVerifier {
        CrossZoneVerifier {
            self_zone: SELF_ZONE,
            peer_pubkeys,
            peers: PeerBlocks::default(),
            seen: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// A `ping_sender` emission addressed to `SELF_ZONE` carrying `payload`.
    fn emission(payload: &[u8]) -> LeeTransaction {
        let receiver_id = programs::ping_receiver().id();
        let send = SenderInstruction::Send {
            outbox_program_id: programs::cross_zone_outbox().id(),
            target_zone: SELF_ZONE,
            target_program_id: receiver_id,
            target_accounts: vec![ping_record_pda(receiver_id).into_value()],
            payload: payload.to_vec(),
            ordinal: 0,
        };
        let message = Message::try_new(programs::ping_sender().id(), vec![], vec![], send)
            .expect("emission serializes");
        LeeTransaction::Public(PublicTransaction::new(
            message,
            WitnessSet::from_raw_parts(vec![]),
        ))
    }

    /// The dispatch a watcher would inject for a `PEER_BLOCK_ID` emission of `payload`.
    fn dispatch(payload: &[u8]) -> LeeTransaction {
        let receiver_id = programs::ping_receiver().id();
        LeeTransaction::Public(build_dispatch_from_emission(
            PEER_ZONE,
            PEER_BLOCK_ID,
            0,
            programs::ping_sender().id(),
            receiver_id,
            &[ping_record_pda(receiver_id).into_value()],
            payload.to_vec(),
        ))
    }

    #[tokio::test]
    async fn verifies_dispatch_matching_a_peer_emission() {
        let verifier = verifier();
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]),
            )
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        verifier
            .verify_block(&block)
            .await
            .expect("dispatch matching the peer emission verifies");
    }

    #[tokio::test]
    async fn rejects_dispatch_with_no_matching_emission() {
        let verifier = verifier();
        // The peer block carries the real emission, but the block claims a
        // different payload, so re-derivation does not reproduce it.
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"real")]),
            )
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"forged")]);
        let err = verifier.verify_block(&block).await.unwrap_err();
        assert!(
            err.to_string().contains("forged"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn verifies_dispatch_signed_by_the_pinned_peer_key() {
        // produce_dummy_block signs with PrivateKey([37; 32]); pin its pubkey.
        let signer = PublicKey::new_from_private_key(&PrivateKey::try_new([37; 32]).unwrap());
        let mut keys = HashMap::new();
        keys.insert(PEER_ZONE, signer);
        let verifier = verifier_with_pinned_keys(keys);
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]),
            )
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        verifier
            .verify_block(&block)
            .await
            .expect("a dispatch from the pinned signer verifies");
    }

    #[tokio::test]
    async fn rejects_dispatch_from_a_block_not_signed_by_the_pinned_key() {
        // Pin a different key than the one that signed the peer block.
        let mut keys = HashMap::new();
        keys.insert(PEER_ZONE, PublicKey::try_new([42; 32]).unwrap());
        let verifier = verifier_with_pinned_keys(keys);
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]),
            )
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        let err = verifier.verify_block(&block).await.unwrap_err();
        assert!(
            err.to_string().contains("pinned"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_reference_to_a_block_the_peer_never_finalized() {
        let verifier = verifier();
        // The reader has finalized past PEER_BLOCK_ID (it holds a later block) but
        // never saw PEER_BLOCK_ID itself, so a dispatch referencing it is a forgery
        // and must be rejected rather than waited on forever.
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID + 1, None, vec![emission(b"hi")]),
            )
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        let err = verifier.verify_block(&block).await.unwrap_err();
        assert!(
            err.to_string().contains("forged"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn accepts_replayed_dispatch_as_noop() {
        let verifier = verifier();
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]),
            )
            .await;

        let first = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        let keys = verifier
            .verify_block(&first)
            .await
            .expect("first delivery verifies");
        // Mark the delivery seen, as the ingest loop does once the block applies.
        verifier.record_seen(keys).await;

        // Replace the peer block with a different emission so re-deriving the
        // replay would mismatch. The replay must still be accepted, proving it is
        // the seen-key short-circuit (the inbox no-ops it on chain) and not a
        // successful re-derivation.
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"different")]),
            )
            .await;

        let replay = produce_dummy_block(10, None, vec![dispatch(b"hi")]);
        verifier
            .verify_block(&replay)
            .await
            .expect("a replay is accepted as an on-chain no-op");
    }

    #[tokio::test]
    async fn unaccepted_dispatch_does_not_poison_seen() {
        // A dispatch verified in a block that never applies (e.g. one that parks)
        // must not be marked seen. Otherwise a later forged dispatch could reuse
        // its key to skip re-derivation, while the inbox, never having recorded
        // the key on chain, would deliver the forgery.
        let verifier = verifier();
        verifier
            .peers
            .insert(
                PEER_ZONE,
                produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]),
            )
            .await;

        // The dispatch verifies, but the block is not applied, so record_seen is
        // not called (the ingest loop records only after an Applied outcome).
        let first = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        verifier
            .verify_block(&first)
            .await
            .expect("dispatch verifies");

        // A forged dispatch reusing the same key (same src zone, block, tx index)
        // with a different payload must still be re-derived and rejected, since
        // its key was never recorded as seen.
        let forged = produce_dummy_block(10, None, vec![dispatch(b"forged")]);
        let err = verifier.verify_block(&forged).await.unwrap_err();
        assert!(
            err.to_string().contains("forged"),
            "unexpected error: {err}"
        );
    }
}
