//! Reexports of types used by sequencer rpc specification.

use std::{fmt::Display, str::FromStr};

pub use common::{HashType, block::Block, transaction::LeeTransaction};
pub use lee::{Account, AccountId, ProgramId};
pub use lee_core::{BlockId, Commitment, CommitmentSetDigest, MembershipProof, account::Nonce};
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};

/// Maximum number of account identifiers returned by a local public transaction receipt.
pub const MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_ACCOUNTS: usize = 128;

/// Maximum number of successor blocks returned to confirm a local public transaction receipt.
pub const MAX_LOCAL_PUBLIC_TRANSACTION_RECEIPT_CONFIRMATIONS: u8 = 32;

/// Maximum number of blocks returned by one local public history request.
pub const MAX_LOCAL_PUBLIC_BLOCK_HISTORY_BLOCKS: u8 = 32;

/// Maximum number of public transactions returned by one local public history page.
pub const MAX_LOCAL_PUBLIC_BLOCK_HISTORY_TRANSACTIONS: usize = 256;

/// Maximum number of public account identifiers returned by one local public history page.
pub const MAX_LOCAL_PUBLIC_BLOCK_HISTORY_ACCOUNT_IDS: usize = 1024;

/// Maximum number of public instruction words returned by one local public history page.
pub const MAX_LOCAL_PUBLIC_BLOCK_HISTORY_INSTRUCTION_WORDS: usize = 0x4000;

/// A local sequencer chain header, represented without its transaction payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBlockHeaderReceiptV1 {
    pub block_id: BlockId,
    pub block_hash: HashType,
    pub previous_block_hash: HashType,
}

/// A bounded receipt issued by a local sequencer for a public transaction.
///
/// `instruction_data_sha256` is SHA-256 over the little-endian bytes of each instruction word,
/// in order. `confirmation_chain` contains exactly the requested number of successor headers;
/// each header must link to the previous header, starting with `inclusion`.
///
/// This is a trusted response from the local sequencer. It is neither an independently
/// verifiable cryptographic inclusion proof nor Bedrock finality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPublicTransactionReceiptV1 {
    pub transaction_hash: HashType,
    pub program_id: ProgramId,
    pub account_ids: Vec<AccountId>,
    pub instruction_word_count: u32,
    pub instruction_data_sha256: HashType,
    pub inclusion: LocalBlockHeaderReceiptV1,
    pub confirmation_chain: Vec<LocalBlockHeaderReceiptV1>,
}

/// A request for one bounded, trusted local public-history page.
///
/// The first request omits `expected_tip`. Later requests must echo the
/// preceding page's `snapshot_tip`; the server rejects a page if its tip
/// changed rather than composing history from different local-chain snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPublicBlockHistoryRequestV1 {
    /// `0` asks the sequencer to begin at its stored genesis block. A nonzero
    /// value is an explicit block cursor, such as `next_block_id` from a
    /// previous page.
    pub start_block_id: BlockId,
    pub max_blocks: u8,
    pub expected_tip: Option<LocalBlockHeaderReceiptV1>,
}

/// One public transaction represented structurally for trusted local replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPublicTransactionV1 {
    pub transaction_hash: HashType,
    pub program_id: ProgramId,
    pub account_ids: Vec<AccountId>,
    pub instruction_data: Vec<u32>,
}

/// A local block header and the public transactions selected from its body.
///
/// The header commits the complete block, including any transaction variants
/// omitted from `public_transactions`. This response is trusted local
/// sequencer data, not a cryptographic inclusion proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPublicBlockV1 {
    pub header: LocalBlockHeaderReceiptV1,
    pub public_transactions: Vec<LocalPublicTransactionV1>,
}

/// A bounded, snapshot-bound page of local public block history.
///
/// Callers begin with `expected_tip = null`, then use the returned
/// `snapshot_tip` with each following request. `next_block_id` is absent when
/// the page reaches that snapshot's tip.
///
/// This is a trusted response from a local sequencer. It is neither public
/// network finality nor an independently verifiable transaction proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPublicBlockHistoryPageV1 {
    pub snapshot_tip: LocalBlockHeaderReceiptV1,
    pub blocks: Vec<LocalPublicBlockV1>,
    pub next_block_id: Option<BlockId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, SerializeDisplay, DeserializeFromStr)]
pub struct ChannelId(pub [u8; 32]);

impl Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex_string = hex::encode(self.0);
        write!(f, "{hex_string}")
    }
}

impl FromStr for ChannelId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0_u8; 32];
        hex::decode_to_slice(s, &mut bytes)?;
        Ok(Self(bytes))
    }
}
