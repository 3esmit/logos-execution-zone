use std::{collections::HashMap, ffi::CString};

use nssa::{privacy_preserving_transaction::circuit::ProgramWithDependencies, program::Program};

use crate::{
    block_on,
    error::{print_error, WalletFfiError},
    map_execution_error,
    wallet::get_wallet,
    FfiAccountIdentity, FfiTransferResult, WalletHandle,
};

#[repr(C)]
pub struct SerializationHelperResult {
    pub instruction_words: *mut u32,
    pub instruction_words_size: usize,
    pub error: WalletFfiError,
}

impl SerializationHelperResult {
    fn from_err(error: WalletFfiError) -> Self {
        Self {
            instruction_words: std::ptr::null_mut(),
            instruction_words_size: 0,
            error,
        }
    }
}

/// Serialize sequence of bytes into RISC0 readable words
///
/// # Parameters
/// - `input_instruction_data`: Valid pointer to a sequence of bytes
/// - `input_instruction_data_size`: Size of `input_instruction_data`
///
/// # Returns
/// - `Success` on successful creation
/// - Error code on failure
///
/// # Safety
/// - `input_instruction_data` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_serialization_helper(
    input_instruction_data: *const u8,
    input_instruction_data_size: usize,
) -> SerializationHelperResult {
    if input_instruction_data.is_null() {
        print_error("Null input pointer for instruction_data");
        return SerializationHelperResult::from_err(WalletFfiError::NullPointer);
    }

    let input_slice =
        unsafe { std::slice::from_raw_parts(input_instruction_data, input_instruction_data_size) };
    let res_vec_u32 = match risc0_zkvm::serde::to_vec(input_slice).map_err(|err| {
        print_error(format!(
            "Failed to serialize input into words with err {err}"
        ));
        WalletFfiError::SerializationError
    }) {
        Ok(res) => res,
        Err(err) => return SerializationHelperResult::from_err(err),
    };
    let res_len = res_vec_u32.len();
    let res_boxed = res_vec_u32.into_boxed_slice();
    let res_ptr = Box::into_raw(res_boxed).cast::<u32>();

    SerializationHelperResult {
        instruction_words: res_ptr,
        instruction_words_size: res_len,
        error: WalletFfiError::Success,
    }
}

#[repr(C)]
/// Intended to be created manually
pub struct FfiProgram {
    pub elf_data: *const u8,
    pub elf_size: usize,
}

impl TryFrom<&FfiProgram> for Program {
    type Error = WalletFfiError;

    fn try_from(value: &FfiProgram) -> Result<Self, Self::Error> {
        let mut elf = Vec::with_capacity(value.elf_size);

        // Alignment will be different, we need to read elements one-by-one
        for i in 0..value.elf_size {
            elf.push(unsafe { *value.elf_data.add(i) });
        }

        Self::new(elf).map_err(|err| {
            print_error(format!("Invalid program bytecode, err: {err}"));
            WalletFfiError::InvalidBytecode
        })
    }
}

impl From<Program> for FfiProgram {
    fn from(value: Program) -> Self {
        let elf_size = value.elf.len();
        let elf_data = Box::into_raw(value.elf.into_boxed_slice()) as *const u8;

        Self { elf_data, elf_size }
    }
}

#[repr(C)]
/// Intended to be created manually
pub struct FfiProgramWithDependencies {
    pub program: FfiProgram,
    pub deps: *const FfiProgram,
    pub deps_size: usize,
}

impl TryFrom<FfiProgramWithDependencies> for ProgramWithDependencies {
    type Error = WalletFfiError;

    fn try_from(value: FfiProgramWithDependencies) -> Result<Self, Self::Error> {
        let mut program_map = HashMap::new();

        let orig_program = (&value.program).try_into()?;

        // Alignment will be different, we need to read elements one-by-one
        for i in 0..value.deps_size {
            let program_dep: Program = unsafe { value.deps.add(i).as_ref() }
                .ok_or(WalletFfiError::NullPointer)?
                .try_into()?;

            program_map.insert(program_dep.id(), program_dep);
        }

        Ok(Self {
            program: orig_program,
            dependencies: program_map,
        })
    }
}

impl From<ProgramWithDependencies> for FfiProgramWithDependencies {
    fn from(value: ProgramWithDependencies) -> Self {
        let ffi_program = value.program.into();

        let ffi_deps: Vec<FfiProgram> = value.dependencies.into_values().map(Into::into).collect::<Vec<_>>();

        let deps_size = ffi_deps.len();
        let deps = Box::into_raw(ffi_deps.into_boxed_slice()) as *const FfiProgram;

        Self { program: ffi_program, deps, deps_size }
    }
}

#[repr(C)]
pub enum FfiExecutionFlow {
    Public = 0,
    PrivacyPreserving = 1,
}

/// Send generic public transaction
///
/// # Parameters
/// - `handle`: Valid pointer to wallet handle
/// - `account_identities`: Valid pointer to list of `FfiAccountIdentity`
/// - `instruction_words`: Valid pointer to instruction words
/// - `out_result`: Valid pointer to `FfiTransferResult`
///
/// # Returns
/// - `Success` on successful creation
/// - Error code on failure
///
/// # Safety
/// - `handle` must be a valid pointer
/// - `account_identities` must be a valid pointer
/// - `instruction_words` must be a valid pointer
/// - `out_result` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_send_generic_public_transaction(
    handle: *mut WalletHandle,
    account_identities: *const FfiAccountIdentity,
    account_identities_size: usize,
    instruction_words: *const u32,
    instruction_words_size: usize,
    program_with_dependencies: FfiProgramWithDependencies,
    out_result: *mut FfiTransferResult,
) -> WalletFfiError {
    let wrapper = match get_wallet(handle) {
        Ok(w) => w,
        Err(e) => return e,
    };

    if account_identities.is_null() {
        print_error("Null output pointer for account identities list");
        return WalletFfiError::NullPointer;
    }

    if instruction_words.is_null() {
        print_error("Null output pointer for instruction data");
        return WalletFfiError::NullPointer;
    }

    if out_result.is_null() {
        print_error("Null output pointer return hash");
        return WalletFfiError::NullPointer;
    }

    let wallet = match wrapper.core.lock() {
        Ok(w) => w,
        Err(e) => {
            print_error(format!("Failed to lock wallet: {e}"));
            return WalletFfiError::InternalError;
        }
    };

    let mut accounts = Vec::with_capacity(account_identities_size);
    let mut instruction_data = Vec::with_capacity(instruction_words_size);

    // Alignment will be different, we need to read elements one-by-one
    for i in 0..account_identities_size {
        accounts.push(
            match match unsafe { account_identities.add(i).as_ref() }
                .ok_or(WalletFfiError::NullPointer)
            {
                Ok(v) => v,
                Err(err) => {
                    print_error(format!(
                        "account_identities_size does not match actual size of account_identities"
                    ));
                    return err;
                }
            }
            .try_into()
            {
                Ok(v) => v,
                Err(err) => return err,
            },
        );
    }

    // Alignment will be different, we need to read elements one-by-one
    for i in 0..instruction_words_size {
        instruction_data.push(unsafe { *instruction_words.add(i) });
    }

    let program = match program_with_dependencies.try_into() {
        Ok(v) => v,
        Err(err) => return err,
    };

    match block_on(wallet.send_pub_tx(accounts, instruction_data, &program)) {
        Ok(tx_hash) => {
            let tx_hash = CString::new(tx_hash.to_string())
                .map_or(std::ptr::null_mut(), std::ffi::CString::into_raw);

            unsafe {
                (*out_result).tx_hash = tx_hash;
                (*out_result).success = true;
            }
            WalletFfiError::Success
        }
        Err(e) => {
            print_error(format!("Public send failed: {e:?}"));
            unsafe {
                (*out_result).tx_hash = std::ptr::null_mut();
                (*out_result).success = false;
            }
            map_execution_error(e)
        }
    }
}
