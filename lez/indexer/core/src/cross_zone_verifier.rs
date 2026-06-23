use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use common::{block::Block, transaction::LeeTransaction};
use cross_zone_inbox_core::{
    CrossZoneMessage, Instruction as InboxInstruction, MessageKey, ZoneId,
    build_dispatch_from_emission, message_key,
};
use futures::StreamExt as _;
use lee::program::Program;
use lee_core::program::ProgramId;
use log::{error, info};
use logos_blockchain_core::mantle::ops::channel::ChannelId;
use logos_blockchain_zone_sdk::{
    CommonHttpClient, ZoneMessage, adapter::NodeHttpClient, indexer::ZoneIndexer,
};
use ping_core::SenderInstruction;
use tokio::sync::RwLock;

use crate::config::IndexerConfig;

/// How long the verifier waits for a referenced peer block to finalize before
/// rejecting the dispatch as referencing a nonexistent block.
const PEER_BLOCK_WAIT: Duration = Duration::from_secs(60);

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
}

/// The indexer-side Option B verifier. For every cross-zone dispatch in a block
/// it re-derives the transaction from the peer's finalized block and rejects it
/// if the bytes differ (a forgery) or the message was already delivered (a
/// replay), so delivery no longer relies on trusting the sequencer.
#[derive(Clone)]
pub struct CrossZoneVerifier {
    self_zone: ZoneId,
    inbox_id: ProgramId,
    emitter_id: ProgramId,
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

        for peer in &cross_zone.peers {
            let node = NodeHttpClient::new(
                CommonHttpClient::new(config.bedrock_config.auth.clone().map(Into::into)),
                config.bedrock_config.addr.clone(),
            );
            tokio::spawn(read_peer(
                ZoneIndexer::new(ChannelId::from(peer.channel_id), node),
                peer.channel_id,
                peers.clone(),
                config.consensus_info_polling_interval,
            ));
        }

        Some(Self {
            self_zone,
            inbox_id: Program::cross_zone_inbox().id(),
            emitter_id: Program::ping_sender().id(),
            peers,
            seen: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Verifies every cross-zone dispatch in a block, returning `Err` on the
    /// first forged or replayed dispatch. The caller halts ingestion on error.
    pub async fn verify_block(&self, block: &Block) -> Result<()> {
        for tx in &block.body.transactions {
            let Some(msg) = self.decode_dispatch(tx) else {
                continue;
            };

            let key = message_key(&msg.src_zone, msg.src_block_id, msg.src_tx_index);
            if self.seen.read().await.contains(&key) {
                bail!("cross-zone replay: message {} re-delivered", hex::encode(key));
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

            self.seen.write().await.insert(key);
            info!(
                "Verified cross-zone dispatch from zone {} block {} tx {}",
                hex::encode(msg.src_zone),
                msg.src_block_id,
                msg.src_tx_index
            );
        }
        Ok(())
    }

    /// Decodes a transaction into the cross-zone message it dispatches, or `None`
    /// if it is not an inbox dispatch.
    fn decode_dispatch(&self, tx: &LeeTransaction) -> Option<CrossZoneMessage> {
        let LeeTransaction::Public(public_tx) = tx else {
            return None;
        };
        if public_tx.message().program_id != self.inbox_id {
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
            .await
            .with_context(|| {
                format!(
                    "no peer block {} from zone {} to verify against",
                    msg.src_block_id,
                    hex::encode(msg.src_zone)
                )
            })?;

        let emission = peer_block
            .body
            .transactions
            .get(msg.src_tx_index as usize)
            .ok_or_else(|| {
                anyhow::anyhow!("src_tx_index {} out of range in peer block", msg.src_tx_index)
            })?;

        let LeeTransaction::Public(emission) = emission else {
            bail!("peer emission transaction is not public");
        };
        if emission.message().program_id != self.emitter_id {
            bail!("peer transaction at src_tx_index is not an emitter transaction");
        }

        let SenderInstruction::Send {
            target_zone,
            target_program_id,
            target_accounts,
            payload,
            ..
        } = risc0_zkvm::serde::from_slice(&emission.message().instruction_data)
            .context("decode peer emission instruction")?;

        if target_zone != self.self_zone {
            bail!("peer emission targets a different zone");
        }

        Ok(build_dispatch_from_emission(
            self.inbox_id,
            msg.src_zone,
            msg.src_block_id,
            msg.src_tx_index,
            self.emitter_id,
            target_program_id,
            &target_accounts,
            payload,
        ))
    }

    /// Polls the peer cache until the referenced block finalizes. A forged
    /// reference to a never-finalized block times out and is rejected.
    async fn wait_for_peer_block(&self, zone: ZoneId, block_id: u64) -> Result<Block> {
        let mut waited = Duration::ZERO;
        loop {
            if let Some(block) = self.peers.get(zone, block_id).await {
                return Ok(block);
            }
            if waited >= PEER_BLOCK_WAIT {
                bail!(
                    "peer block {} from zone {} did not finalize within {:?}",
                    block_id,
                    hex::encode(zone),
                    PEER_BLOCK_WAIT
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            waited += Duration::from_secs(1);
        }
    }
}

/// Reads a peer zone's finalized blocks from Bedrock into the shared cache.
async fn read_peer(
    zone_indexer: ZoneIndexer<NodeHttpClient>,
    peer_zone: ZoneId,
    peers: PeerBlocks,
    poll_interval: Duration,
) {
    info!("Cross-zone peer reader started for {}", hex::encode(peer_zone));

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
        PublicTransaction,
        program::Program,
        public_transaction::{Message, WitnessSet},
    };
    use ping_core::ping_record_pda;

    use super::*;

    const SELF_ZONE: ZoneId = [1; 32];
    const PEER_ZONE: ZoneId = [2; 32];
    const PEER_BLOCK_ID: u64 = 5;

    fn verifier() -> CrossZoneVerifier {
        CrossZoneVerifier {
            self_zone: SELF_ZONE,
            inbox_id: Program::cross_zone_inbox().id(),
            emitter_id: Program::ping_sender().id(),
            peers: PeerBlocks::default(),
            seen: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// A ping_sender emission addressed to `SELF_ZONE` carrying `payload`.
    fn emission(payload: &[u8]) -> LeeTransaction {
        let receiver_id = Program::ping_receiver().id();
        let send = SenderInstruction::Send {
            outbox_program_id: Program::cross_zone_outbox().id(),
            target_zone: SELF_ZONE,
            target_program_id: receiver_id,
            target_accounts: vec![ping_record_pda(receiver_id).into_value()],
            payload: payload.to_vec(),
            ordinal: 0,
        };
        let message = Message::try_new(Program::ping_sender().id(), vec![], vec![], send)
            .expect("emission serializes");
        LeeTransaction::Public(PublicTransaction::new(
            message,
            WitnessSet::from_raw_parts(vec![]),
        ))
    }

    /// The dispatch a watcher would inject for a `PEER_BLOCK_ID` emission of `payload`.
    fn dispatch(payload: &[u8]) -> LeeTransaction {
        let receiver_id = Program::ping_receiver().id();
        LeeTransaction::Public(build_dispatch_from_emission(
            Program::cross_zone_inbox().id(),
            PEER_ZONE,
            PEER_BLOCK_ID,
            0,
            Program::ping_sender().id(),
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
            .insert(PEER_ZONE, produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]))
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
            .insert(PEER_ZONE, produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"real")]))
            .await;

        let block = produce_dummy_block(9, None, vec![dispatch(b"forged")]);
        let err = verifier.verify_block(&block).await.unwrap_err();
        assert!(err.to_string().contains("forged"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn rejects_replayed_dispatch() {
        let verifier = verifier();
        verifier
            .peers
            .insert(PEER_ZONE, produce_dummy_block(PEER_BLOCK_ID, None, vec![emission(b"hi")]))
            .await;

        let first = produce_dummy_block(9, None, vec![dispatch(b"hi")]);
        verifier.verify_block(&first).await.expect("first delivery verifies");

        let replay = produce_dummy_block(10, None, vec![dispatch(b"hi")]);
        let err = verifier.verify_block(&replay).await.unwrap_err();
        assert!(err.to_string().contains("replay"), "unexpected error: {err}");
    }
}
