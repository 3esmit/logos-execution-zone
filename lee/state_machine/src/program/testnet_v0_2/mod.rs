//! Host wire compatibility for the source-attested Testnet v0.2 programs.
//!
//! Those guests encode account owners as eight u32 words, unlike current guests'
//! `AccountId` string encoding. Select the schema by attested image identity before
//! execution; never retry a failed program under a different schema. State storage,
//! current guest schemas, instruction bytes, and execution validation are unchanged.
//! This adapter does not provide private-circuit input compatibility.

use lee_core::{
    account::AccountWithMetadata,
    program::{ProgramId, ProgramOutput},
};
use risc0_zkvm::{ExecutorEnvBuilder, Journal};

use crate::error::LeeError;

mod abi;
pub mod ids;

pub(super) fn uses_legacy_abi(program_id: ProgramId) -> bool {
    [
        ids::AUTHENTICATED_TRANSFER_ID,
        ids::TOKEN_ID,
        ids::AMM_ID,
        ids::CLOCK_ID,
        ids::ASSOCIATED_TOKEN_ACCOUNT_ID,
        ids::VAULT_ID,
        ids::FAUCET_ID,
        ids::BRIDGE_ID,
        ids::PINATA_ID,
    ]
    .contains(&program_id)
}

pub(super) fn write_pre_states(
    pre_states: &[AccountWithMetadata],
    builder: &mut ExecutorEnvBuilder,
) -> Result<(), LeeError> {
    let legacy: Vec<abi::Metadata> = pre_states.iter().map(Into::into).collect();
    builder
        .write(&legacy)
        .map_err(|error| LeeError::ProgramWriteInputFailed(error.to_string()))?;
    Ok(())
}

pub(super) fn decode_output(journal: &Journal) -> Result<ProgramOutput, LeeError> {
    // RISC Zero's byte-slice decoder panics on a non-word-aligned journal.
    // Validate the boundary and decode owned words, without alignment assumptions.
    let (chunks, remainder) = journal.bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return Err(LeeError::ProgramExecutionFailed(
            "legacy program journal length is not a multiple of 4".into(),
        ));
    }
    let words: Vec<u32> = chunks
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    risc0_zkvm::serde::from_slice::<abi::Output, u32>(&words)
        .map(Into::into)
        .map_err(|error| LeeError::ProgramExecutionFailed(error.to_string()))
}

#[cfg(test)]
mod tests;
