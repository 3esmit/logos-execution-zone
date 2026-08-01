//! Trusted local public-history FFI functions.

use std::{
    ffi::{c_char, CString},
    ptr,
};

use sequencer_service_protocol::{HashType, LocalBlockHeaderReceiptV1};

use crate::{
    block_on,
    error::{print_error, WalletFfiError},
    types::{FfiBytes32, WalletHandle},
    wallet::get_wallet,
};

/// A local sequencer block header used to pin a public-history snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FfiLocalBlockHeaderReceiptV1 {
    pub block_id: u64,
    pub block_hash: FfiBytes32,
    pub previous_block_hash: FfiBytes32,
}

impl From<FfiLocalBlockHeaderReceiptV1> for LocalBlockHeaderReceiptV1 {
    fn from(value: FfiLocalBlockHeaderReceiptV1) -> Self {
        Self {
            block_id: value.block_id,
            block_hash: HashType(value.block_hash.data),
            previous_block_hash: HashType(value.previous_block_hash.data),
        }
    }
}

impl From<LocalBlockHeaderReceiptV1> for FfiLocalBlockHeaderReceiptV1 {
    fn from(value: LocalBlockHeaderReceiptV1) -> Self {
        Self {
            block_id: value.block_id,
            block_hash: FfiBytes32::from(value.block_hash.0),
            previous_block_hash: FfiBytes32::from(value.previous_block_hash.0),
        }
    }
}

/// Read one bounded, snapshot-pinned page from the wallet's configured leader.
///
/// This always calls the local public-history RPC method at the configured leader. It accepts no
/// URL, HTTP path, or RPC-method input. The returned JSON is a trusted local sequencer response,
/// not public-network finality.
///
/// # Parameters
/// - `handle`: Valid wallet handle.
/// - `start_block_id`: Cursor for the page; zero asks the sequencer to use its stored genesis.
/// - `expected_tip`: Null for the first page, otherwise the prior page's `snapshot_tip`.
/// - `out_history_json`: Receives an allocated, null-terminated JSON result.
///
/// # Returns
/// - `Success` with `out_history_json` allocated; free it with `wallet_ffi_free_string()`.
/// - `NetworkError` if the configured leader cannot serve the request.
/// - `SerializationError` if the bounded response cannot be encoded as JSON.
///
/// # Safety
/// - `handle` must be a valid wallet handle from `wallet_ffi_create_new` or `wallet_ffi_open`.
/// - If non-null, `expected_tip` must point to a valid `FfiLocalBlockHeaderReceiptV1` for this
///   call.
/// - `out_history_json` must point to writable storage for one `char*` and remains owned by the
///   caller; the returned string must be freed with `wallet_ffi_free_string()`.
#[no_mangle]
pub unsafe extern "C" fn wallet_ffi_get_local_public_block_history(
    handle: *mut WalletHandle,
    start_block_id: u64,
    expected_tip: *const FfiLocalBlockHeaderReceiptV1,
    out_history_json: *mut *mut c_char,
) -> WalletFfiError {
    if out_history_json.is_null() {
        print_error("Null output pointer for local public history JSON");
        return WalletFfiError::NullPointer;
    }

    // SAFETY: The caller contract requires writable storage for one `char*`; reset it before any
    // fallible operation so callers never observe a stale owned allocation on failure.
    unsafe {
        *out_history_json = ptr::null_mut();
    }

    let wrapper = match get_wallet(handle) {
        Ok(wrapper) => wrapper,
        Err(error) => return error,
    };

    let expected_tip = if expected_tip.is_null() {
        None
    } else {
        // SAFETY: The caller contract requires `expected_tip` to point to a valid readable
        // `FfiLocalBlockHeaderReceiptV1`; the value is `Copy`, so it does not borrow caller data.
        Some(unsafe { (*expected_tip).into() })
    };

    let wallet = match wrapper.core.lock() {
        Ok(wallet) => wallet,
        Err(error) => {
            print_error(format!("Failed to lock wallet: {error}"));
            return WalletFfiError::InternalError;
        }
    };

    let page = match block_on(wallet.get_local_public_block_history(start_block_id, expected_tip)) {
        Ok(page) => page,
        Err(error) => {
            print_error(format!("Failed to get local public history: {error}"));
            return WalletFfiError::NetworkError;
        }
    };

    let history_json = match serde_json::to_string(&page) {
        Ok(history_json) => history_json,
        Err(error) => {
            print_error(format!("Failed to serialize local public history: {error}"));
            return WalletFfiError::SerializationError;
        }
    };
    let history_json = match CString::new(history_json) {
        Ok(history_json) => history_json,
        Err(error) => {
            print_error(format!(
                "Local public history JSON contains an interior NUL: {error}"
            ));
            return WalletFfiError::SerializationError;
        }
    };

    // SAFETY: `out_history_json` was checked non-null above and remains caller-owned writable
    // storage for this synchronous call. `CString::into_raw` transfers exactly one allocation to
    // the caller, which is paired with `wallet_ffi_free_string()`.
    unsafe {
        *out_history_json = history_json.into_raw();
    }
    WalletFfiError::Success
}

#[cfg(test)]
mod tests {
    use sequencer_service_protocol::{HashType, LocalBlockHeaderReceiptV1};

    use super::FfiLocalBlockHeaderReceiptV1;

    #[test]
    fn ffi_snapshot_tip_roundtrips_without_layout_conversion() {
        let expected = LocalBlockHeaderReceiptV1 {
            block_id: 9,
            block_hash: HashType([1_u8; 32]),
            previous_block_hash: HashType([2_u8; 32]),
        };

        let ffi: FfiLocalBlockHeaderReceiptV1 = expected.clone().into();

        assert_eq!(ffi.block_id, expected.block_id);
        assert_eq!(ffi.block_hash.data, expected.block_hash.0);
        assert_eq!(ffi.previous_block_hash.data, expected.previous_block_hash.0);
        assert_eq!(LocalBlockHeaderReceiptV1::from(ffi), expected);
    }
}
