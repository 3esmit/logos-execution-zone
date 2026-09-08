//! Immutable artifacts deployed by the current Testnet chain.
//!
//! These are intentionally separate from generated development artifacts. Their
//! provenance and digests live in `testnet_initial_state/testnet-v0.2/manifest.json`.

use std::borrow::Cow;

use lee::program::Program;
pub use lee::program::testnet_v0_2::ids::{
    AMM_ID, ASSOCIATED_TOKEN_ACCOUNT_ID, AUTHENTICATED_TRANSFER_ID, BRIDGE_ID, CLOCK_ID, FAUCET_ID,
    PINATA_ID, PRIVACY_PRESERVING_CIRCUIT_ID, TOKEN_ID, VAULT_ID,
};

pub const AUTHENTICATED_TRANSFER_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "authenticated_transfer.bin"
));
pub const TOKEN_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "token.bin"
));
pub const AMM_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "amm.bin"
));
pub const CLOCK_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "clock.bin"
));
pub const ASSOCIATED_TOKEN_ACCOUNT_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "associated_token_account.bin"
));
pub const VAULT_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "vault.bin"
));
pub const FAUCET_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "faucet.bin"
));
pub const BRIDGE_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "bridge.bin"
));
pub const PINATA_ELF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../testnet_initial_state/testnet-v0.2/",
    "pinata.bin"
));

#[must_use]
pub const fn authenticated_transfer() -> Program {
    Program::new_unchecked(
        AUTHENTICATED_TRANSFER_ID,
        Cow::Borrowed(AUTHENTICATED_TRANSFER_ELF),
    )
}

#[must_use]
pub const fn token() -> Program {
    Program::new_unchecked(TOKEN_ID, Cow::Borrowed(TOKEN_ELF))
}

#[must_use]
pub const fn amm() -> Program {
    Program::new_unchecked(AMM_ID, Cow::Borrowed(AMM_ELF))
}

#[must_use]
pub const fn clock() -> Program {
    Program::new_unchecked(CLOCK_ID, Cow::Borrowed(CLOCK_ELF))
}

#[must_use]
pub const fn ata() -> Program {
    Program::new_unchecked(
        ASSOCIATED_TOKEN_ACCOUNT_ID,
        Cow::Borrowed(ASSOCIATED_TOKEN_ACCOUNT_ELF),
    )
}

#[must_use]
pub const fn vault() -> Program {
    Program::new_unchecked(VAULT_ID, Cow::Borrowed(VAULT_ELF))
}

#[must_use]
pub const fn faucet() -> Program {
    Program::new_unchecked(FAUCET_ID, Cow::Borrowed(FAUCET_ELF))
}

#[must_use]
pub const fn bridge() -> Program {
    Program::new_unchecked(BRIDGE_ID, Cow::Borrowed(BRIDGE_ELF))
}

#[must_use]
pub const fn pinata() -> Program {
    Program::new_unchecked(PINATA_ID, Cow::Borrowed(PINATA_ELF))
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::*;

    #[test]
    fn artifacts_match_the_attested_testnet_manifest() {
        let cases = [
            (
                AUTHENTICATED_TRANSFER_ELF,
                AUTHENTICATED_TRANSFER_ID,
                "a54b6e7d08f664253d8df784c5d2559f79e19d67fd1719ecbd1c141dbd6ddbcf",
            ),
            (
                TOKEN_ELF,
                TOKEN_ID,
                "0f3671d6d0f77fb5d25d3d3dc5015b748b8a6e0687ac8b457f096f87408ef6fd",
            ),
            (
                AMM_ELF,
                AMM_ID,
                "2d275366687baf11eea27942879242ff8889d14b2246dac000a1f0b6a4e5f610",
            ),
            (
                CLOCK_ELF,
                CLOCK_ID,
                "0447f839e46f584e7125dbd3f67b7d1a0cba17d520247867fc5ef0f5a9eb7a5b",
            ),
            (
                ASSOCIATED_TOKEN_ACCOUNT_ELF,
                ASSOCIATED_TOKEN_ACCOUNT_ID,
                "8ca29768706ae0450b06cd1423eb1f2d87c05cbfb4a5a968d07824da4bcdd2fc",
            ),
            (
                VAULT_ELF,
                VAULT_ID,
                "7ff0b662649ac47cd3135294e13afd0cf0b5f97dc923bfe78b5d92ff13e7f5a4",
            ),
            (
                FAUCET_ELF,
                FAUCET_ID,
                "452b3bd60bc399d63a2abc2fc2ce3cb33cb2c723be23588ca9d50f8ce6920f15",
            ),
            (
                BRIDGE_ELF,
                BRIDGE_ID,
                "c28f556d1220a32ba02f9d6b6d7ede5b0101a55f6d0e9fb7917dacf067311dec",
            ),
            (
                PINATA_ELF,
                PINATA_ID,
                "59807047471e26af11847dcbfd9d87b4ca60cd56149b1cae31f9050944ed3a98",
            ),
        ];

        for (elf, expected_id, expected_sha256) in cases {
            assert_eq!(
                Program::new(elf.into()).map(|program| program.id()).ok(),
                Some(expected_id)
            );
            assert_eq!(format!("{:x}", Sha256::digest(elf)), expected_sha256);
        }
    }

    #[test]
    fn privacy_preserving_circuit_matches_the_attested_testnet_manifest() {
        let elf = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../testnet_initial_state/testnet-v0.2/privacy_preserving_circuit.bin"
        ));

        assert_eq!(
            risc0_binfmt::compute_image_id(elf).ok().map(Into::into),
            Some(PRIVACY_PRESERVING_CIRCUIT_ID)
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(elf)),
            "4c3665913d08a0ec043ee2d476750c59792df8d7c4feacedbd36cac58ffae9c3"
        );
    }
}
