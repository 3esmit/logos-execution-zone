use lee::AccountId;

use crate::{FfiBytes32, FfiNullifierPublicKey, FfiPdaSeed, FfiProgramId, FfiU128};

/// Produce account id for public PDA.
///
/// # Parameters
/// - `program_id`: Id of the owner program
/// - `pda_seed`: 32 byte seed
///
/// # Returns
/// - `FfiBytes32` representing account id bytes
#[no_mangle]
pub extern "C" fn wallet_ffi_account_id_for_public_pda(
    program_id: FfiProgramId,
    pda_seed: FfiPdaSeed,
) -> FfiBytes32 {
    AccountId::for_public_pda(&program_id.data, &pda_seed.into()).into()
}

/// Produce account id for private PDA.
///
/// # Parameters
/// - `program_id`: Id of the owner program
/// - `pda_seed`: 32 byte seed
/// - `npk`: 32 byte nullifier public key (can be obtained from
///   `wallet_ffi_get_private_account_keys`)
/// - `identifier`: little endian encoded `u128`
///
/// # Returns
/// - `FfiBytes32` representing account id bytes
#[no_mangle]
pub extern "C" fn wallet_ffi_account_id_for_private_pda(
    program_id: FfiProgramId,
    pda_seed: FfiPdaSeed,
    npk: FfiNullifierPublicKey,
    identifier: FfiU128,
) -> FfiBytes32 {
    AccountId::for_private_pda(
        &program_id.data,
        &pda_seed.into(),
        &npk.into(),
        identifier.into(),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use lee::AccountId;
    use lee_core::NullifierPublicKey;
    use vault_core::PdaSeed;

    use crate::pda::{wallet_ffi_account_id_for_private_pda, wallet_ffi_account_id_for_public_pda};

    #[test]
    fn public_pda_consistent_derivation() {
        let program_id = [100_u32, 101, 102, 103, 104, 105, 106, 107];
        let pda_seed = PdaSeed::new([42; 32]);

        let pda_id = AccountId::for_public_pda(&program_id, &pda_seed);
        let ffi_pda_id = wallet_ffi_account_id_for_public_pda(program_id.into(), pda_seed.into());

        assert_eq!(pda_id.into_value(), ffi_pda_id.data);
    }

    #[test]
    fn private_pda_consistent_derivation() {
        let program_id = [100_u32, 101, 102, 103, 104, 105, 106, 107];
        let pda_seed = PdaSeed::new([42; 32]);
        let npk = NullifierPublicKey([43; 32]);
        let identifier = 100_000_u128;

        let pda_id = AccountId::for_private_pda(&program_id, &pda_seed, &npk, identifier);
        let ffi_pda_id = wallet_ffi_account_id_for_private_pda(
            program_id.into(),
            pda_seed.into(),
            npk.into(),
            identifier.into(),
        );

        assert_eq!(pda_id.into_value(), ffi_pda_id.data);
    }
}
