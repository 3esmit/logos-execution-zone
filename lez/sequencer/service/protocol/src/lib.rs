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
