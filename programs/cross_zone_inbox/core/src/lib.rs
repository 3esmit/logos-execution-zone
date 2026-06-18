use std::collections::BTreeMap;

use lee_core::{
    account::AccountId,
    program::{PdaSeed, ProgramId},
};
use serde::{Deserialize, Serialize};

/// Raw 32-byte zone (channel) id; the host maps it to the zone-sdk `ChannelId`.
pub type ZoneId = [u8; 32];

/// Block-signing public key pinned per peer zone.
pub type ExpectedPubkey = [u8; 32];

/// Content-addressed replay key for a delivered message.
pub type MessageKey = [u8; 32];

/// Source blocks per seen-set shard, so no single seen account grows without bound.
pub const EPOCH_BLOCKS: u64 = 10_000;

const MESSAGE_KEY_DOMAIN: [u8; 32] = *b"/LEZ/v0.3/CrossZoneMsgKey/00000/";
const INBOX_CONFIG_SEED: [u8; 32] = *b"/LEZ/v0.3/CrossZoneInboxCfg/000/";
const INBOX_SEEN_SEED_DOMAIN: [u8; 32] = *b"/LEZ/v0.3/CrossZoneInboxSeen/00/";

/// A finalized outbound message observed on a peer zone, addressed to a program
/// on this zone. The watcher fills it from the peer's block; it is never
/// self-reported by a user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossZoneMessage {
    pub src_zone: ZoneId,
    pub src_block_id: u64,
    pub src_tx_index: u32,
    pub src_program_id: ProgramId,
    pub target_program_id: ProgramId,
    pub payload: Vec<u8>,
    /// Reserved for a future source-state proof; MUST be `None` in v1.
    pub l1_inclusion_witness: Option<Vec<u8>>,
}

/// Peer and per-peer target allowlists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxConfig {
    pub allowed_peers: BTreeMap<ZoneId, ExpectedPubkey>,
    pub allowed_targets: BTreeMap<ZoneId, Vec<ProgramId>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instruction {
    /// Delivers a finalized peer message to its target program.
    Dispatch(CrossZoneMessage),
}

/// Content-addressed replay key: `(src_zone, src_block_id, src_tx_index)` hashed
/// under a domain separator. Watcher-independent and immune to proof
/// malleability, since it keys on block id plus index rather than a tx hash.
#[must_use]
pub fn message_key(src_zone: &ZoneId, src_block_id: u64, src_tx_index: u32) -> MessageKey {
    use risc0_zkvm::sha::{Impl, Sha256 as _};

    let mut bytes = Vec::with_capacity(MESSAGE_KEY_DOMAIN.len() + 32 + 8 + 4);
    bytes.extend_from_slice(&MESSAGE_KEY_DOMAIN);
    bytes.extend_from_slice(src_zone);
    bytes.extend_from_slice(&src_block_id.to_le_bytes());
    bytes.extend_from_slice(&src_tx_index.to_le_bytes());

    Impl::hash_bytes(&bytes)
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

/// The config account holding the allowlists.
#[must_use]
pub fn inbox_config_account_id(inbox_id: ProgramId) -> AccountId {
    AccountId::for_public_pda(&inbox_id, &PdaSeed::new(INBOX_CONFIG_SEED))
}

/// The seen-set shard for the `(src_zone, epoch)` the message falls in.
#[must_use]
pub fn inbox_seen_shard_account_id(
    inbox_id: ProgramId,
    src_zone: &ZoneId,
    src_block_id: u64,
) -> AccountId {
    AccountId::for_public_pda(&inbox_id, &seen_shard_seed(src_zone, src_block_id))
}

fn seen_shard_seed(src_zone: &ZoneId, src_block_id: u64) -> PdaSeed {
    use risc0_zkvm::sha::{Impl, Sha256 as _};

    let src_epoch = src_block_id / EPOCH_BLOCKS;
    let mut bytes = Vec::with_capacity(INBOX_SEEN_SEED_DOMAIN.len() + 32 + 8);
    bytes.extend_from_slice(&INBOX_SEEN_SEED_DOMAIN);
    bytes.extend_from_slice(src_zone);
    bytes.extend_from_slice(&src_epoch.to_le_bytes());

    let seed: [u8; 32] = Impl::hash_bytes(&bytes)
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| unreachable!());
    PdaSeed::new(seed)
}

/// Builds the sequencer-origin dispatch transaction. Pure, so the watcher's
/// injected tx and the indexer's re-derived tx are byte-identical for the same
/// inputs (the basis of the Option B check). `target_account_ids` are the
/// inbox's chained-call targets; deriving them is target-specific.
#[cfg(feature = "host")]
#[must_use]
pub fn build_inbox_dispatch_tx(
    inbox_id: ProgramId,
    msg: &CrossZoneMessage,
    target_account_ids: Vec<AccountId>,
) -> lee::PublicTransaction {
    let mut account_ids = Vec::with_capacity(2 + target_account_ids.len());
    account_ids.push(inbox_config_account_id(inbox_id));
    account_ids.push(inbox_seen_shard_account_id(
        inbox_id,
        &msg.src_zone,
        msg.src_block_id,
    ));
    account_ids.extend(target_account_ids);

    let message = lee::public_transaction::Message::try_new(
        inbox_id,
        account_ids,
        vec![],
        Instruction::Dispatch(msg.clone()),
    )
    .expect("inbox dispatch instruction must serialize");

    lee::PublicTransaction::new(
        message,
        lee::public_transaction::WitnessSet::from_raw_parts(vec![]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(b: u8) -> ZoneId {
        [b; 32]
    }

    #[test]
    fn message_key_is_stable_and_content_addressed() {
        assert_eq!(message_key(&zone(1), 7, 3), message_key(&zone(1), 7, 3));
        assert_ne!(message_key(&zone(1), 7, 3), message_key(&zone(2), 7, 3));
        assert_ne!(message_key(&zone(1), 7, 3), message_key(&zone(1), 8, 3));
        assert_ne!(message_key(&zone(1), 7, 3), message_key(&zone(1), 7, 4));
    }

    #[test]
    fn seen_shards_split_on_epoch_boundary() {
        let id: ProgramId = [9; 8];
        assert_eq!(
            inbox_seen_shard_account_id(id, &zone(1), 0),
            inbox_seen_shard_account_id(id, &zone(1), EPOCH_BLOCKS - 1),
        );
        assert_ne!(
            inbox_seen_shard_account_id(id, &zone(1), EPOCH_BLOCKS - 1),
            inbox_seen_shard_account_id(id, &zone(1), EPOCH_BLOCKS),
        );
    }

    #[cfg(feature = "host")]
    #[test]
    fn build_inbox_dispatch_tx_is_deterministic() {
        let inbox: ProgramId = [5; 8];
        let msg = CrossZoneMessage {
            src_zone: zone(1),
            src_block_id: 42,
            src_tx_index: 2,
            src_program_id: [6; 8],
            target_program_id: [7; 8],
            payload: vec![1, 2, 3, 4],
            l1_inclusion_witness: None,
        };
        let targets = vec![AccountId::new([8; 32]), AccountId::new([9; 32])];

        let tx1 = build_inbox_dispatch_tx(inbox, &msg, targets.clone());
        let tx2 = build_inbox_dispatch_tx(inbox, &msg, targets);
        assert_eq!(tx1, tx2);
    }
}
