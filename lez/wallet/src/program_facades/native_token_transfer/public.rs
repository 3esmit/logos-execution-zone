use authenticated_transfer_core::Instruction as AuthTransferInstruction;
use common::HashType;
use lee::{ProgramId, program::Program};

use super::NativeTokenTransfer;
use crate::{
    AccountIdentity, ExecutionFailureKind,
    program_facades::native_token_transfer::auth_transfer_preparation,
};

impl NativeTokenTransfer<'_> {
    pub async fn send_public_transfer(
        &self,
        from: AccountIdentity,
        to: AccountIdentity,
        balance_to_move: u128,
    ) -> Result<HashType, ExecutionFailureKind> {
        self.send_public_transfer_with_program_id(
            from,
            to,
            balance_to_move,
            crate::network_profile::authenticated_transfer_id(),
        )
        .await
    }

    pub async fn send_public_transfer_local(
        &self,
        from: AccountIdentity,
        to: AccountIdentity,
        balance_to_move: u128,
    ) -> Result<HashType, ExecutionFailureKind> {
        self.send_public_transfer_with_program_id(
            from,
            to,
            balance_to_move,
            local_registration_program_id(),
        )
        .await
    }

    async fn send_public_transfer_with_program_id(
        &self,
        from: AccountIdentity,
        to: AccountIdentity,
        balance_to_move: u128,
        program_id: ProgramId,
    ) -> Result<HashType, ExecutionFailureKind> {
        let (instruction_data, _program, tx_pre_check) = auth_transfer_preparation(balance_to_move);

        self.0
            .send_pub_tx_with_pre_check(vec![from, to], instruction_data, program_id, tx_pre_check)
            .await
    }

    pub async fn register_account(
        &self,
        account: AccountIdentity,
    ) -> Result<HashType, ExecutionFailureKind> {
        self.register_account_with_program_id(account, profile_registration_program_id())
            .await
    }

    pub async fn register_account_local(
        &self,
        account: AccountIdentity,
    ) -> Result<HashType, ExecutionFailureKind> {
        self.register_account_with_program_id(account, local_registration_program_id())
            .await
    }

    async fn register_account_with_program_id(
        &self,
        account: AccountIdentity,
        program_id: ProgramId,
    ) -> Result<HashType, ExecutionFailureKind> {
        let instruction_data = Program::serialize_instruction(AuthTransferInstruction::Initialize)?;

        self.0
            .send_pub_tx(vec![account], instruction_data, program_id)
            .await
    }
}

#[cfg_attr(
    feature = "testnet-v0-2",
    expect(
        clippy::missing_const_for_fn,
        reason = "shared helper must also support the non-const default-profile build"
    )
)]
fn profile_registration_program_id() -> ProgramId {
    crate::network_profile::authenticated_transfer_id()
}

fn local_registration_program_id() -> ProgramId {
    programs::authenticated_transfer().id()
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "testnet-v0-2")]
    use super::local_registration_program_id;
    use super::profile_registration_program_id;

    #[cfg(feature = "testnet-v0-2")]
    #[test]
    fn profile_registration_stays_pinned_to_the_testnet_profile() {
        assert_eq!(
            profile_registration_program_id(),
            crate::network_profile::authenticated_transfer_id()
        );
    }

    #[cfg(feature = "testnet-v0-2")]
    #[test]
    fn local_registration_uses_the_compiled_program_identity() {
        assert_eq!(
            local_registration_program_id(),
            programs::authenticated_transfer().id()
        );
    }

    #[cfg(not(feature = "testnet-v0-2"))]
    #[test]
    fn default_profile_registration_uses_the_compiled_program_identity() {
        assert_eq!(
            profile_registration_program_id(),
            programs::authenticated_transfer().id()
        );
    }
}
