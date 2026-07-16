//! Immutable artifacts deployed by the current Testnet chain.
//!
//! These are intentionally separate from generated development artifacts. Their
//! provenance and digests live in `testnet_initial_state/testnet-v0.2/manifest.json`.

use std::borrow::Cow;

use lee::program::Program;

pub const AUTHENTICATED_TRANSFER_ID: [u32; 8] = [
    3_170_810_844,
    2_526_647_253,
    999_807_262,
    1_205_602_179,
    3_401_962_591,
    3_484_055_895,
    2_106_546_407,
    1_900_691_388,
];
pub const TOKEN_ID: [u32; 8] = [
    2_282_739_141,
    348_907_455,
    1_046_946_228,
    3_735_699_860,
    585_462_133,
    3_426_087_150,
    772_528_164,
    2_090_518_099,
];
pub const AMM_ID: [u32; 8] = [
    3_938_501_840,
    2_324_858_003,
    1_666_889_367,
    375_716_348,
    780_188_473,
    2_541_850_958,
    134_690_371,
    504_369_919,
];
pub const CLOCK_ID: [u32; 8] = [
    979_979_912,
    3_730_255_152,
    96_781_338,
    501_898_186,
    3_738_241_015,
    2_113_460_497,
    2_222_463_973,
    1_670_293_850,
];
pub const ASSOCIATED_TOKEN_ACCOUNT_ID: [u32; 8] = [
    3_357_312_149,
    3_615_960_253,
    3_351_583_505,
    2_234_166_003,
    4_153_433_811,
    2_743_238_177,
    2_886_052_503,
    4_160_755_157,
];
pub const VAULT_ID: [u32; 8] = [
    1_168_813_120,
    241_877_831,
    3_407_559_972,
    2_131_462_206,
    1_965_161_891,
    2_000_235_008,
    2_574_408_698,
    1_333_126_597,
];
pub const FAUCET_ID: [u32; 8] = [
    3_202_488_003,
    2_265_373_137,
    3_565_181_875,
    2_136_920_928,
    3_140_485_604,
    4_047_263_442,
    237_953_438,
    138_790_662,
];
pub const BRIDGE_ID: [u32; 8] = [
    4_055_902_857,
    3_504_899_920,
    3_002_912_689,
    171_582_257,
    1_364_142_713,
    2_858_044_526,
    2_885_477_314,
    3_931_248_408,
];
pub const PINATA_ID: [u32; 8] = [
    2_376_463_974,
    3_277_439_378,
    1_302_675_137,
    2_326_148_894,
    3_540_995_969,
    3_074_751_928,
    386_292_947,
    4_166_403_244,
];
pub const PRIVACY_PRESERVING_CIRCUIT_ID: [u32; 8] = [
    1_473_414_827,
    2_830_688_453,
    889_928_182,
    3_392_605_264,
    2_857_892_388,
    2_747_074_170,
    4_080_825_648,
    1_235_408_490,
];

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
