use std::{ffi::CString, ptr, slice};

use common::transaction::LeeTransaction;
use lee::{program::Program, ProgramDeploymentTransaction};
use sequencer_service_rpc::RpcClient as _;

use crate::{
    block_on,
    error::{print_error, WalletFfiError},
    generic_transaction::{FfiProgram, FfiTransactionResult},
    wallet::get_wallet,
    WalletHandle,
};

/// Send a program deployment transaction.
///
/// Publishes program for future use.
///
/// # Parameters
/// - `handle`: Valid wallet handle
/// - `elf_data`: Valid pointer to elf data in bytes
/// - `elf_size`: Size of elf data
/// - `out_result`: Output pointer for transfer result
///
/// # Returns
/// - `Success` if deployment was submitted successfully
/// - Error code on other failures
///
/// # Memory
/// The result must be freed with `wallet_ffi_free_transaction_result()`.
///
/// # Safety
/// - `handle` must be a valid wallet handle from `wallet_ffi_create_new` or `wallet_ffi_open`
/// - `elf_data` must be a valid pointer to elf data
/// - `out_result` must be a valid pointer to a `FfiTransferResult` struct
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_program_deployment(
    handle: *mut WalletHandle,
    elf_data: *const u8,
    elf_size: usize,
    out_result: *mut FfiTransactionResult,
) -> WalletFfiError {
    let wrapper = match get_wallet(handle) {
        Ok(w) => w,
        Err(e) => return e,
    };

    if elf_data.is_null() || out_result.is_null() {
        print_error("Null pointer argument");
        return WalletFfiError::NullPointer;
    }

    let wallet = match wrapper.core.lock() {
        Ok(w) => w,
        Err(e) => {
            print_error(format!("Failed to lock wallet: {e}"));
            return WalletFfiError::InternalError;
        }
    };

    let elf = unsafe { slice::from_raw_parts(elf_data, elf_size) }.to_vec();

    let message = lee::program_deployment_transaction::Message::new(elf);
    let transaction = ProgramDeploymentTransaction::new(message);

    match block_on(
        wallet
            .sequencer_client
            .send_transaction(LeeTransaction::ProgramDeployment(transaction)),
    ) {
        Ok(tx_hash) => {
            let tx_hash = CString::new(tx_hash.to_string())
                .map_or(ptr::null_mut(), std::ffi::CString::into_raw);

            unsafe {
                (*out_result).tx_hash = tx_hash;
                (*out_result).success = true;
            }
            WalletFfiError::Success
        }
        Err(e) => {
            print_error(format!("Deployment failed: {e:?}"));
            unsafe {
                (*out_result).tx_hash = ptr::null_mut();
                (*out_result).success = false;
            }
            WalletFfiError::NetworkError
        }
    }
}

/// Writes elf data of authenticated transfer program into buffer.
///
/// WARNING: Result is not consisent and change between versions, use for testing purposes only.
///
/// # Parameters
/// - `ffi_program`: Valid pointer to `FfiProgram`
///
/// # Returns
/// - `Success` if deployment was submitted successfully
/// - Error code on other failures
///
/// # Memory
/// - `FfiProgram` can be freed with corresponding `wallet_ffi_free_ffi_program` function
///
/// # Safety
/// - `ffi_program` must be a non-null pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_transfer_elf(ffi_program: *mut FfiProgram) -> WalletFfiError {
    if ffi_program.is_null() {
        print_error("Null pointer argument");
        return WalletFfiError::NullPointer;
    }

    let elf = Program::authenticated_transfer_program().elf().to_vec();

    let (raw_elf_data, raw_elf_size, _) = elf.into_raw_parts();

    unsafe {
        (*ffi_program).elf_data = raw_elf_data;
        (*ffi_program).elf_size = raw_elf_size;
    };

    WalletFfiError::Success
}

/// Writes elf data of authenticated token program into buffer.
///
/// WARNING: Result is not consisent and change between versions, use for testing purposes only.
///
/// # Parameters
/// - `ffi_program`: Valid pointer to `FfiProgram`
///
/// # Returns
/// - `Success` if deployment was submitted successfully
/// - Error code on other failures
///
/// # Memory
/// - `FfiProgram` can be freed with corresponding `wallet_ffi_free_ffi_program` function
///
/// # Safety
/// - `ffi_program` must be a non-null pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_token_elf(ffi_program: *mut FfiProgram) -> WalletFfiError {
    if ffi_program.is_null() {
        print_error("Null pointer argument");
        return WalletFfiError::NullPointer;
    }

    let elf = Program::token().elf().to_vec();

    let (raw_elf_data, raw_elf_size, _) = elf.into_raw_parts();

    unsafe {
        (*ffi_program).elf_data = raw_elf_data;
        (*ffi_program).elf_size = raw_elf_size;
    };

    WalletFfiError::Success
}

/// Writes elf data of amm into buffer.
///
/// WARNING: Result is not consisent and change between versions, use for testing purposes only.
///
/// # Parameters
/// - `ffi_program`: Valid pointer to `FfiProgram`
///
/// # Returns
/// - `Success` if deployment was submitted successfully
/// - Error code on other failures
///
/// # Memory
/// - `FfiProgram` can be freed with corresponding `wallet_ffi_free_ffi_program` function
///
/// # Safety
/// - `ffi_program` must be a non-null pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_amm_elf(ffi_program: *mut FfiProgram) -> WalletFfiError {
    if ffi_program.is_null() {
        print_error("Null pointer argument");
        return WalletFfiError::NullPointer;
    }

    let elf = Program::amm().elf().to_vec();

    let (raw_elf_data, raw_elf_size, _) = elf.into_raw_parts();

    unsafe {
        (*ffi_program).elf_data = raw_elf_data;
        (*ffi_program).elf_size = raw_elf_size;
    };

    WalletFfiError::Success
}

/// Writes elf data of ata into buffer.
///
/// WARNING: Result is not consisent and change between versions, use for testing purposes only.
///
/// # Parameters
/// - `ffi_program`: Valid pointer to `FfiProgram`
///
/// # Returns
/// - `Success` if deployment was submitted successfully
/// - Error code on other failures
///
/// # Memory
/// - `FfiProgram` can be freed with corresponding `wallet_ffi_free_ffi_program` function
///
/// # Safety
/// - `ffi_program` must be a non-null pointer
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_ata_elf(ffi_program: *mut FfiProgram) -> WalletFfiError {
    if ffi_program.is_null() {
        print_error("Null pointer argument");
        return WalletFfiError::NullPointer;
    }

    let elf = Program::ata().elf().to_vec();

    let (raw_elf_data, raw_elf_size, _) = elf.into_raw_parts();

    unsafe {
        (*ffi_program).elf_data = raw_elf_data;
        (*ffi_program).elf_size = raw_elf_size;
    };

    WalletFfiError::Success
}

/// Free a ffi program returned by functions `wallet_ffi_*_elf`.
///
/// # Safety
/// The result must be either null or a valid result from a elf getter function.
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_free_ffi_program(ffi_program: *mut FfiProgram) {
    if ffi_program.is_null() {
        return;
    }

    unsafe {
        let ffi_program = &*ffi_program;

        if !ffi_program.elf_data.is_null() {
            let elf = std::slice::from_raw_parts_mut(
                ffi_program.elf_data.cast_mut(),
                ffi_program.elf_size,
            );
            drop(Box::from_raw(std::ptr::from_mut::<[u8]>(elf)));
        }
    }
}
