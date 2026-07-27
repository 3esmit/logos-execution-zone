//! Builtin program IDs for the wallet's selected network profile.
//!
//! The Testnet v0.2 values are immutable deployment identities, not values
//! discovered from the remote endpoint. Keeping them local lets health checks
//! reject an incompatible network instead of silently retargeting a signed
//! transaction.

use lee::ProgramId;

#[cfg(feature = "testnet-v0-2")]
const TESTNET_V0_2_AUTHENTICATED_TRANSFER_ID: ProgramId = [
    3_170_810_844,
    2_526_647_253,
    999_807_262,
    1_205_602_179,
    3_401_962_591,
    3_484_055_895,
    2_106_546_407,
    1_900_691_388,
];
#[cfg(feature = "testnet-v0-2")]
const TESTNET_V0_2_TOKEN_ID: ProgramId = [
    2_282_739_141,
    348_907_455,
    1_046_946_228,
    3_735_699_860,
    585_462_133,
    3_426_087_150,
    772_528_164,
    2_090_518_099,
];
#[cfg(feature = "testnet-v0-2")]
const TESTNET_V0_2_AMM_ID: ProgramId = [
    3_938_501_840,
    2_324_858_003,
    1_666_889_367,
    375_716_348,
    780_188_473,
    2_541_850_958,
    134_690_371,
    504_369_919,
];
#[cfg(feature = "testnet-v0-2")]
const TESTNET_V0_2_PRIVACY_PRESERVING_CIRCUIT_ID: ProgramId = [
    1_473_414_827,
    2_830_688_453,
    889_928_182,
    3_392_605_264,
    2_857_892_388,
    2_747_074_170,
    4_080_825_648,
    1_235_408_490,
];

#[cfg(feature = "testnet-v0-2")]
pub const fn authenticated_transfer_id() -> ProgramId {
    TESTNET_V0_2_AUTHENTICATED_TRANSFER_ID
}

#[cfg(not(feature = "testnet-v0-2"))]
pub fn authenticated_transfer_id() -> ProgramId {
    programs::authenticated_transfer().id()
}

#[cfg(feature = "testnet-v0-2")]
pub const fn health_check_program_ids() -> [(&'static str, &'static str, ProgramId); 4] {
    [
        (
            "authenticated transfer",
            "authenticated_transfer",
            TESTNET_V0_2_AUTHENTICATED_TRANSFER_ID,
        ),
        ("token", "token", TESTNET_V0_2_TOKEN_ID),
        (
            "privacy-preserving circuit",
            "privacy_preserving_circuit",
            TESTNET_V0_2_PRIVACY_PRESERVING_CIRCUIT_ID,
        ),
        ("AMM", "amm", TESTNET_V0_2_AMM_ID),
    ]
}

#[cfg(not(feature = "testnet-v0-2"))]
pub fn health_check_program_ids() -> [(&'static str, &'static str, ProgramId); 4] {
    [
        (
            "authenticated transfer",
            "authenticated_transfer",
            programs::authenticated_transfer().id(),
        ),
        ("token", "token", programs::token().id()),
        (
            "privacy-preserving circuit",
            "privacy_preserving_circuit",
            lee::PRIVACY_PRESERVING_CIRCUIT_ID,
        ),
        ("AMM", "amm", programs::amm().id()),
    ]
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "testnet-v0-2")]
    use super::*;

    #[cfg(feature = "testnet-v0-2")]
    #[test]
    fn testnet_v0_2_authenticated_transfer_id_matches_the_deployed_profile() {
        assert_eq!(
            authenticated_transfer_id(),
            [
                3_170_810_844,
                2_526_647_253,
                999_807_262,
                1_205_602_179,
                3_401_962_591,
                3_484_055_895,
                2_106_546_407,
                1_900_691_388,
            ]
        );
    }

    #[cfg(feature = "testnet-v0-2")]
    #[test]
    fn testnet_v0_2_health_check_uses_the_immutable_profile() {
        assert_eq!(
            health_check_program_ids(),
            [
                (
                    "authenticated transfer",
                    "authenticated_transfer",
                    TESTNET_V0_2_AUTHENTICATED_TRANSFER_ID,
                ),
                ("token", "token", TESTNET_V0_2_TOKEN_ID),
                (
                    "privacy-preserving circuit",
                    "privacy_preserving_circuit",
                    TESTNET_V0_2_PRIVACY_PRESERVING_CIRCUIT_ID,
                ),
                ("AMM", "amm", TESTNET_V0_2_AMM_ID),
            ]
        );
    }
}
