//! The v0.2 guest Serde schema, from source revision
//! 1a94c8612fa7fcc4acecde44b973d942eab19f11. Only account owner representation
//! differs; conversions preserve every authorization, claim, call, and validity field.

use lee_core::{
    account::{Account, AccountId, AccountWithMetadata, Data, Nonce},
    program::{
        AccountPostState, BlockValidityWindow, ChainedCall, Claim, InstructionData, PdaSeed,
        ProgramId, ProgramOutput, TimestampValidityWindow,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct LegacyAccount {
    pub program_owner: ProgramId,
    pub balance: u128,
    pub data: Data,
    pub nonce: Nonce,
}

impl From<&Account> for LegacyAccount {
    fn from(account: &Account) -> Self {
        Self {
            program_owner: account.program_owner.into(),
            balance: account.balance,
            data: account.data.clone(),
            nonce: account.nonce,
        }
    }
}

impl From<LegacyAccount> for Account {
    fn from(account: LegacyAccount) -> Self {
        Self {
            program_owner: account.program_owner.into(),
            balance: account.balance,
            data: account.data,
            nonce: account.nonce,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Metadata {
    pub account: LegacyAccount,
    pub is_authorized: bool,
    pub account_id: AccountId,
}

impl From<&AccountWithMetadata> for Metadata {
    fn from(metadata: &AccountWithMetadata) -> Self {
        Self {
            account: (&metadata.account).into(),
            is_authorized: metadata.is_authorized,
            account_id: metadata.account_id,
        }
    }
}

impl From<Metadata> for AccountWithMetadata {
    fn from(metadata: Metadata) -> Self {
        Self::new(
            metadata.account.into(),
            metadata.is_authorized,
            metadata.account_id,
        )
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
pub(super) struct PostState {
    pub account: LegacyAccount,
    pub claim: Option<Claim>,
}

impl From<PostState> for AccountPostState {
    fn from(post: PostState) -> Self {
        match post.claim {
            Some(claim) => Self::new_claimed(post.account.into(), claim),
            None => Self::new(post.account.into()),
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
pub(super) struct Call {
    pub program_id: ProgramId,
    pub pre_states: Vec<Metadata>,
    pub instruction_data: InstructionData,
    pub pda_seeds: Vec<PdaSeed>,
}

impl From<Call> for ChainedCall {
    fn from(call: Call) -> Self {
        Self {
            program_id: call.program_id,
            pre_states: call.pre_states.into_iter().map(Into::into).collect(),
            instruction_data: call.instruction_data,
            pda_seeds: call.pda_seeds,
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
pub(super) struct Output {
    pub self_program_id: ProgramId,
    pub caller_program_id: Option<ProgramId>,
    pub instruction_data: InstructionData,
    pub pre_states: Vec<Metadata>,
    pub post_states: Vec<PostState>,
    pub chained_calls: Vec<Call>,
    pub block_validity_window: BlockValidityWindow,
    pub timestamp_validity_window: TimestampValidityWindow,
}

impl From<Output> for ProgramOutput {
    fn from(output: Output) -> Self {
        Self {
            self_program_id: output.self_program_id,
            caller_program_id: output.caller_program_id,
            instruction_data: output.instruction_data,
            pre_states: output.pre_states.into_iter().map(Into::into).collect(),
            post_states: output.post_states.into_iter().map(Into::into).collect(),
            chained_calls: output.chained_calls.into_iter().map(Into::into).collect(),
            block_validity_window: output.block_validity_window,
            timestamp_validity_window: output.timestamp_validity_window,
        }
    }
}
