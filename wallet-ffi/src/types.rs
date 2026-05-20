//! C-compatible type definitions for the FFI layer.

use core::slice;
use std::{ffi::c_char, ptr};

use nssa::Data;
use nssa_core::{encryption::shared_key_derivation::Secp256k1Point, NullifierPublicKey};
use wallet::AccountIdentity;

use crate::error::WalletFfiError;

/// Opaque pointer to the Wallet instance.
///
/// This type is never instantiated directly - it's used as an opaque handle
/// to hide the internal wallet structure from C code.
#[repr(C)]
pub struct WalletHandle {
    _private: [u8; 0],
}

/// 32-byte array type for `AccountId`, keys, hashes, etc.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiBytes32 {
    pub data: [u8; 32],
}

/// Program ID - 8 u32 values (32 bytes total).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiProgramId {
    pub data: [u32; 8],
}

/// U128 - 16 bytes little endian.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiU128 {
    pub data: [u8; 16],
}

/// Account data structure - C-compatible version of nssa Account.
///
/// Note: `balance` and `nonce` are u128 values represented as little-endian
/// byte arrays since C doesn't have native u128 support.
#[repr(C)]
pub struct FfiAccount {
    pub program_owner: FfiProgramId,
    /// Balance as little-endian [u8; 16].
    pub balance: FfiU128,
    /// Pointer to account data bytes.
    pub data: *const u8,
    /// Length of account data.
    pub data_len: usize,
    /// Nonce as little-endian [u8; 16].
    pub nonce: FfiU128,
}

impl Default for FfiAccount {
    fn default() -> Self {
        Self {
            program_owner: FfiProgramId::default(),
            balance: FfiU128::default(),
            data: std::ptr::null(),
            data_len: 0,
            nonce: FfiU128::default(),
        }
    }
}

/// Public keys for a private account (safe to expose).
#[repr(C)]
pub struct FfiPrivateAccountKeys {
    /// Nullifier public key (32 bytes).
    pub nullifier_public_key: FfiBytes32,
    /// viewing public key (compressed secp256k1 point).
    pub viewing_public_key: *const u8,
    /// Length of viewing public key (typically 33 bytes).
    pub viewing_public_key_len: usize,
}

impl Default for FfiPrivateAccountKeys {
    fn default() -> Self {
        Self {
            nullifier_public_key: FfiBytes32::default(),
            viewing_public_key: std::ptr::null(),
            viewing_public_key_len: 0,
        }
    }
}

/// Public key info for a public account.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiPublicAccountKey {
    pub public_key: FfiBytes32,
}

/// Single entry in the account list.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiAccountListEntry {
    pub account_id: FfiBytes32,
    pub is_public: bool,
}

/// List of accounts returned by `wallet_ffi_list_accounts`.
#[repr(C)]
pub struct FfiAccountList {
    pub entries: *mut FfiAccountListEntry,
    pub count: usize,
}

impl Default for FfiAccountList {
    fn default() -> Self {
        Self {
            entries: std::ptr::null_mut(),
            count: 0,
        }
    }
}

/// Result of a transfer operation.
#[repr(C)]
pub struct FfiTransferResult {
    // TODO: Replace with HashType FFI representation
    /// Transaction hash (null-terminated string, or null on failure).
    pub tx_hash: *mut c_char,
    /// Whether the transfer succeeded.
    pub success: bool,
}

impl Default for FfiTransferResult {
    fn default() -> Self {
        Self {
            tx_hash: std::ptr::null_mut(),
            success: false,
        }
    }
}

// Helper functions to convert between Rust and FFI types

impl FfiBytes32 {
    /// Create from a 32-byte array.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { data: bytes }
    }

    /// Create from an `AccountId`.
    #[must_use]
    pub const fn from_account_id(id: nssa::AccountId) -> Self {
        Self { data: *id.value() }
    }
}

impl FfiPrivateAccountKeys {
    #[must_use]
    pub const fn npk(&self) -> nssa_core::NullifierPublicKey {
        nssa_core::NullifierPublicKey(self.nullifier_public_key.data)
    }

    pub fn vpk(&self) -> Result<nssa_core::encryption::ViewingPublicKey, WalletFfiError> {
        if self.viewing_public_key_len == 33 {
            let slice = unsafe {
                slice::from_raw_parts(self.viewing_public_key, self.viewing_public_key_len)
            };
            Ok(Secp256k1Point(slice.to_vec()))
        } else {
            Err(WalletFfiError::InvalidKeyValue)
        }
    }
}

/// Enumeration to represent kinds of FfiAccountManagerAccountIdentity
#[repr(C)]
pub enum FfiAccountIdentityKind {
    Public = 0,
    PublicNoSign = 1,
    PrivateOwned = 2,
    PrivateForeign = 3,
    PrivatePdaOwned = 4,
    PrivatePdaForeign = 5,
    PrivateShared = 6,
    PrivatePdaShared = 7,
}

/// Struct representing of account identity, given to `AccountManager` at intialization
#[repr(C)]
pub struct FfiAccountIdentity {
    kind: FfiAccountIdentityKind,
    pub account_id: FfiBytes32,
    pub nullifier_secret_key: FfiBytes32,
    pub nullifier_public_key: FfiBytes32,
    pub viewing_public_key: *const u8,
    pub viewing_public_key_len: usize,
    pub identifier: FfiU128,
}

impl Default for FfiAccountIdentity {
    fn default() -> Self {
        Self {
            kind: FfiAccountIdentityKind::Public,
            account_id: FfiBytes32::default(),
            nullifier_secret_key: FfiBytes32::default(),
            nullifier_public_key: FfiBytes32::default(),
            viewing_public_key: std::ptr::null(),
            viewing_public_key_len: 0,
            identifier: FfiU128::default(),
        }
    }
}

impl From<u128> for FfiU128 {
    fn from(value: u128) -> Self {
        Self {
            data: value.to_le_bytes(),
        }
    }
}

impl From<FfiU128> for u128 {
    fn from(value: FfiU128) -> Self {
        Self::from_le_bytes(value.data)
    }
}

impl From<nssa::AccountId> for FfiBytes32 {
    fn from(id: nssa::AccountId) -> Self {
        Self::from_account_id(id)
    }
}

impl From<[u8; 32]> for FfiBytes32 {
    fn from(value: [u8; 32]) -> Self {
        Self { data: value }
    }
}

impl From<FfiBytes32> for nssa::AccountId {
    fn from(bytes: FfiBytes32) -> Self {
        Self::new(bytes.data)
    }
}

impl From<nssa::Account> for FfiAccount {
    #[expect(
        clippy::as_conversions,
        reason = "We need to convert to byte arrays for FFI"
    )]
    fn from(value: nssa::Account) -> Self {
        // Convert account data to FFI type
        let data_vec: Vec<u8> = value.data.into();
        let data_len = data_vec.len();
        let data = if data_len > 0 {
            let data_boxed = data_vec.into_boxed_slice();
            Box::into_raw(data_boxed) as *const u8
        } else {
            ptr::null()
        };

        let program_owner = FfiProgramId {
            data: value.program_owner,
        };
        Self {
            program_owner,
            balance: value.balance.into(),
            data,
            data_len,
            nonce: value.nonce.0.into(),
        }
    }
}

impl TryFrom<&FfiAccount> for nssa::Account {
    type Error = WalletFfiError;

    fn try_from(value: &FfiAccount) -> Result<Self, Self::Error> {
        let data = if value.data_len > 0 {
            unsafe {
                let slice = slice::from_raw_parts(value.data, value.data_len);
                Data::try_from(slice.to_vec())
                    .map_err(|_err| WalletFfiError::InvalidTypeConversion)?
            }
        } else {
            Data::default()
        };
        Ok(Self {
            program_owner: value.program_owner.data,
            balance: value.balance.into(),
            data,
            nonce: nssa_core::account::Nonce(value.nonce.into()),
        })
    }
}

impl From<nssa::PublicKey> for FfiPublicAccountKey {
    fn from(value: nssa::PublicKey) -> Self {
        Self {
            public_key: FfiBytes32::from_bytes(*value.value()),
        }
    }
}

impl TryFrom<&FfiPublicAccountKey> for nssa::PublicKey {
    type Error = WalletFfiError;

    fn try_from(value: &FfiPublicAccountKey) -> Result<Self, Self::Error> {
        let public_key = Self::try_new(value.public_key.data)
            .map_err(|_err| WalletFfiError::InvalidTypeConversion)?;
        Ok(public_key)
    }
}

impl From<AccountIdentity> for FfiAccountIdentity {
    fn from(value: AccountIdentity) -> Self {
        match value {
            AccountIdentity::Public(account_id) => Self {
                kind: FfiAccountIdentityKind::Public,
                account_id: account_id.into(),
                ..Default::default()
            },
            AccountIdentity::PublicNoSign(account_id) => Self {
                kind: FfiAccountIdentityKind::PublicNoSign,
                account_id: account_id.into(),
                ..Default::default()
            },
            AccountIdentity::PrivateOwned(account_id) => Self {
                kind: FfiAccountIdentityKind::PrivateOwned,
                account_id: account_id.into(),
                ..Default::default()
            },
            AccountIdentity::PrivateForeign {
                npk,
                vpk,
                identifier,
            } => {
                let vpk_vec = vpk.0;
                let vpk_len = vpk_vec.len();
                let vpk_data = if vpk_len > 0 {
                    let vpk_data_boxed = vpk_vec.into_boxed_slice();
                    Box::into_raw(vpk_data_boxed) as *const u8
                } else {
                    ptr::null()
                };

                Self {
                    kind: FfiAccountIdentityKind::PrivateForeign,
                    nullifier_public_key: npk.0.into(),
                    viewing_public_key: vpk_data,
                    viewing_public_key_len: vpk_len,
                    identifier: identifier.into(),
                    ..Default::default()
                }
            }
            AccountIdentity::PrivatePdaOwned(account_id) => Self {
                kind: FfiAccountIdentityKind::PrivatePdaOwned,
                account_id: account_id.into(),
                ..Default::default()
            },
            AccountIdentity::PrivatePdaForeign {
                account_id,
                npk,
                vpk,
                identifier,
            } => {
                let vpk_vec = vpk.0;
                let vpk_len = vpk_vec.len();
                let vpk_data = if vpk_len > 0 {
                    let vpk_data_boxed = vpk_vec.into_boxed_slice();
                    Box::into_raw(vpk_data_boxed) as *const u8
                } else {
                    ptr::null()
                };

                Self {
                    kind: FfiAccountIdentityKind::PrivatePdaForeign,
                    account_id: account_id.into(),
                    nullifier_public_key: npk.0.into(),
                    viewing_public_key: vpk_data,
                    viewing_public_key_len: vpk_len,
                    identifier: identifier.into(),
                    ..Default::default()
                }
            }
            AccountIdentity::PrivateShared {
                nsk,
                npk,
                vpk,
                identifier,
            } => {
                let vpk_vec = vpk.0;
                let vpk_len = vpk_vec.len();
                let vpk_data = if vpk_len > 0 {
                    let vpk_data_boxed = vpk_vec.into_boxed_slice();
                    Box::into_raw(vpk_data_boxed) as *const u8
                } else {
                    ptr::null()
                };

                Self {
                    kind: FfiAccountIdentityKind::PrivateShared,
                    nullifier_secret_key: nsk.into(),
                    nullifier_public_key: npk.0.into(),
                    viewing_public_key: vpk_data,
                    viewing_public_key_len: vpk_len,
                    identifier: identifier.into(),
                    ..Default::default()
                }
            }
            AccountIdentity::PrivatePdaShared {
                account_id,
                nsk,
                npk,
                vpk,
                identifier,
            } => {
                let vpk_vec = vpk.0;
                let vpk_len = vpk_vec.len();
                let vpk_data = if vpk_len > 0 {
                    let vpk_data_boxed = vpk_vec.into_boxed_slice();
                    Box::into_raw(vpk_data_boxed) as *const u8
                } else {
                    ptr::null()
                };

                Self {
                    kind: FfiAccountIdentityKind::PrivateShared,
                    account_id: account_id.into(),
                    nullifier_secret_key: nsk.into(),
                    nullifier_public_key: npk.0.into(),
                    viewing_public_key: vpk_data,
                    viewing_public_key_len: vpk_len,
                    identifier: identifier.into(),
                }
            }
        }
    }
}

impl TryFrom<&FfiAccountIdentity> for AccountIdentity {
    type Error = WalletFfiError;

    fn try_from(value: &FfiAccountIdentity) -> Result<Self, Self::Error> {
        match value.kind {
            FfiAccountIdentityKind::Public => Ok(AccountIdentity::Public(value.account_id.into())),
            FfiAccountIdentityKind::PublicNoSign => {
                Ok(AccountIdentity::PublicNoSign(value.account_id.into()))
            }
            FfiAccountIdentityKind::PrivateOwned => {
                Ok(AccountIdentity::PrivateOwned(value.account_id.into()))
            }
            FfiAccountIdentityKind::PrivateForeign => {
                let vpk = if value.viewing_public_key_len == 33 {
                    let slice = unsafe {
                        slice::from_raw_parts(
                            value.viewing_public_key,
                            value.viewing_public_key_len,
                        )
                    };
                    Ok(Secp256k1Point(slice.to_vec()))
                } else {
                    Err(WalletFfiError::InvalidKeyValue)
                }?;

                Ok(AccountIdentity::PrivateForeign {
                    npk: NullifierPublicKey(value.nullifier_public_key.data),
                    vpk,
                    identifier: value.identifier.into(),
                })
            }
            FfiAccountIdentityKind::PrivatePdaOwned => {
                Ok(AccountIdentity::PrivatePdaOwned(value.account_id.into()))
            }
            FfiAccountIdentityKind::PrivatePdaForeign => {
                let vpk = if value.viewing_public_key_len == 33 {
                    let slice = unsafe {
                        slice::from_raw_parts(
                            value.viewing_public_key,
                            value.viewing_public_key_len,
                        )
                    };
                    Ok(Secp256k1Point(slice.to_vec()))
                } else {
                    Err(WalletFfiError::InvalidKeyValue)
                }?;

                Ok(AccountIdentity::PrivatePdaForeign {
                    account_id: value.account_id.into(),
                    npk: NullifierPublicKey(value.nullifier_public_key.data),
                    vpk,
                    identifier: value.identifier.into(),
                })
            }
            FfiAccountIdentityKind::PrivateShared => {
                let vpk = if value.viewing_public_key_len == 33 {
                    let slice = unsafe {
                        slice::from_raw_parts(
                            value.viewing_public_key,
                            value.viewing_public_key_len,
                        )
                    };
                    Ok(Secp256k1Point(slice.to_vec()))
                } else {
                    Err(WalletFfiError::InvalidKeyValue)
                }?;

                Ok(AccountIdentity::PrivateShared {
                    nsk: value.nullifier_secret_key.data,
                    npk: NullifierPublicKey(value.nullifier_public_key.data),
                    vpk,
                    identifier: value.identifier.into(),
                })
            }
            FfiAccountIdentityKind::PrivatePdaShared => {
                let vpk = if value.viewing_public_key_len == 33 {
                    let slice = unsafe {
                        slice::from_raw_parts(
                            value.viewing_public_key,
                            value.viewing_public_key_len,
                        )
                    };
                    Ok(Secp256k1Point(slice.to_vec()))
                } else {
                    Err(WalletFfiError::InvalidKeyValue)
                }?;

                Ok(AccountIdentity::PrivatePdaShared {
                    account_id: value.account_id.into(),
                    nsk: value.nullifier_secret_key.data,
                    npk: NullifierPublicKey(value.nullifier_public_key.data),
                    vpk,
                    identifier: value.identifier.into(),
                })
            }
        }
    }
}
