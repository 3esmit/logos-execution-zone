//! Reconstructing and verifying L2 chain state from a Bedrock (L1) channel.

pub use apply::{BlockIngestError, Tip, apply_block, validate_against_tip};
pub use consistency::{
    Anchor, AnchorConsistencyCheck, ChainConsistency, ChainMismatch, verify_chain_consistency,
};

pub mod apply;
pub mod consistency;
