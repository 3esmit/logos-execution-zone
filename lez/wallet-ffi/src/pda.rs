use lee::AccountId;

use crate::{FfiBytes32, FfiNullifierPublicKey, FfiPdaSeed, FfiProgramId, FfiU128};

/// Produce account id for public PDA.
///
/// # Parameters
/// - `program_id`: Id of a owner program
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

/// Produce account id for public PDA.
///
/// # Parameters
/// - `program_id`: Id of a owner program
/// - `pda_seed`: 32 byte seed
/// - `npk`: 32 byte nullifier public key(can be get from `wallet_ffi_get_private_account_keys`)
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
