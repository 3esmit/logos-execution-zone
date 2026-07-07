//! Validating and applying channel L2 blocks to a `V03State`.
//!
//! These primitives are shared by the consumers that reconstruct L2 state from
//! the Bedrock channel.

use common::{
    HashType,
    block::Block,
    transaction::{LeeTransaction, clock_invocation},
};
use lee::{GENESIS_BLOCK_ID, V03State};
use lee_core::BlockId;
use serde::{Deserialize, Serialize};

/// Why a channel L2 block could not be validated or applied.
///
/// Persisted in `RocksDB` (via the Indexer's stall reason), so every variant
/// must be `Clone + Serialize + Deserialize`.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum BlockIngestError {
    #[error("Failed to deserialize L2 block: {0}")]
    /// Here we store the error string that is derived from [`borsh::from_slice`]'s [`Err`].
    Deserialize(String),
    #[error("Unexpected block id: expected {expected}, got {got}")]
    UnexpectedBlockId { expected: BlockId, got: BlockId },
    #[error("Broken chain link: expected prev {expected_prev}, got {got_prev}")]
    BrokenChainLink {
        expected_prev: HashType,
        got_prev: HashType,
    },
    #[error("Block hash mismatch: computed {computed}, header {header}")]
    HashMismatch {
        computed: HashType,
        header: HashType,
    },
    #[error("Block has no transactions")]
    EmptyBlock,
    #[error("Last transaction must be the public clock invocation for the block timestamp")]
    InvalidClockTransaction,
    #[error("Genesis block must contain only public transactions")]
    NonPublicGenesisTransaction,
    #[error("State transition failed at transaction {tx_index}: {reason}")]
    StateTransition {
        /// Index of the failing transaction within the block body.
        tx_index: u64,
        /// Reason string from `lee::Error` to `anyhow::Error` to `{:#}`.
        ///
        /// This is required because `lee::Error` is not `Clone + Serialize + Deserialize`, so we
        /// cannot store it directly.
        reason: String,
    },
}

impl BlockIngestError {
    /// Whether the failure may be transient rather than a property of the block.
    ///
    /// FIXME: `StateTransition` is too coarse — its `reason` string mixes genuine
    /// state-transition rejections with infra failures (risc0 executor teardown,
    /// storage errors). Once it carries a structured cause, narrow this so only
    /// infra failures retry.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::StateTransition { .. })
    }
}

/// The last successfully applied block a candidate must extend.
pub struct Tip {
    pub block_id: BlockId,
    pub hash: HashType,
}

/// Checks that `block` is the valid continuation of `tip`: hash integrity,
/// then block-id continuity, then `prev_block_hash` linkage. A `None` tip
/// (cold store) expects the genesis block.
pub fn validate_against_tip(tip: Option<&Tip>, block: &Block) -> Result<(), BlockIngestError> {
    let computed = block.recompute_hash();
    if computed != block.header.hash {
        return Err(BlockIngestError::HashMismatch {
            computed,
            header: block.header.hash,
        });
    }

    match tip {
        None => {
            if block.header.block_id != GENESIS_BLOCK_ID {
                return Err(BlockIngestError::UnexpectedBlockId {
                    expected: GENESIS_BLOCK_ID,
                    got: block.header.block_id,
                });
            }
        }
        Some(tip) => {
            let expected = tip
                .block_id
                .checked_add(1)
                .expect("block id should not overflow");
            if block.header.block_id != expected {
                return Err(BlockIngestError::UnexpectedBlockId {
                    expected,
                    got: block.header.block_id,
                });
            }
            if block.header.prev_block_hash != tip.hash {
                return Err(BlockIngestError::BrokenChainLink {
                    expected_prev: tip.hash,
                    got_prev: block.header.prev_block_hash,
                });
            }
        }
    }
    Ok(())
}

/// Applies a block's transactions to `state`.
pub fn apply_block(block: &Block, state: &mut V03State) -> Result<(), BlockIngestError> {
    let (clock_tx, user_txs) = block
        .body
        .transactions
        .split_last()
        .ok_or(BlockIngestError::EmptyBlock)?;

    let expected_clock = LeeTransaction::Public(clock_invocation(block.header.timestamp));
    if *clock_tx != expected_clock {
        return Err(BlockIngestError::InvalidClockTransaction);
    }

    let is_genesis = block.header.block_id == GENESIS_BLOCK_ID;
    for (tx_index, transaction) in user_txs.iter().enumerate() {
        let state_transition = |err: anyhow::Error| BlockIngestError::StateTransition {
            tx_index: tx_index.try_into().expect("tx index fits in u64"),
            reason: format!("{err:#}"),
        };
        if is_genesis {
            let LeeTransaction::Public(public_tx) = transaction else {
                return Err(BlockIngestError::NonPublicGenesisTransaction);
            };
            state
                .transition_from_public_transaction(
                    public_tx,
                    block.header.block_id,
                    block.header.timestamp,
                )
                .map_err(|err| state_transition(err.into()))?;
        } else {
            transaction
                .clone()
                .execute_on_state(state, block.header.block_id, block.header.timestamp)
                .map_err(|err| state_transition(err.into()))?;
        }
    }

    let LeeTransaction::Public(clock_public_tx) = clock_tx else {
        return Err(BlockIngestError::InvalidClockTransaction);
    };
    state
        .transition_from_public_transaction(
            clock_public_tx,
            block.header.block_id,
            block.header.timestamp,
        )
        .map_err(|err| BlockIngestError::StateTransition {
            tx_index: user_txs.len().try_into().expect("tx index fits in u64"),
            reason: format!("{:#}", anyhow::Error::from(err)),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_and_round_trips_externally_tagged() {
        let err = BlockIngestError::UnexpectedBlockId {
            expected: 5,
            got: 7,
        };
        let value = serde_json::to_value(&err).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({ "UnexpectedBlockId": { "expected": 5, "got": 7 } })
        );
        let back: BlockIngestError = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            BlockIngestError::UnexpectedBlockId {
                expected: 5,
                got: 7
            }
        ));
    }
}
