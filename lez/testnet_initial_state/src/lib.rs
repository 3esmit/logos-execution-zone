use std::collections::HashMap;

use key_protocol::key_management::{
    KeyChain, key_tree::chain_index::ChainIndex, secret_holders::SecretSpendingKey,
};
use lee::{Account, AccountId, Data, PrivateKey, PublicKey, V03State, program::Program};
use serde::{Deserialize, Serialize};

#[cfg(feature = "testnet")]
mod testnet_v0_2;

const PRIVATE_KEY_PUB_ACC_A: [u8; 32] = [
    16, 162, 106, 154, 236, 125, 52, 184, 35, 100, 238, 174, 69, 197, 41, 77, 187, 10, 118, 75, 0,
    11, 148, 238, 185, 181, 133, 17, 220, 72, 124, 77,
];

const PRIVATE_KEY_PUB_ACC_B: [u8; 32] = [
    113, 121, 64, 177, 204, 85, 229, 214, 178, 6, 109, 191, 29, 154, 63, 38, 242, 18, 244, 219, 8,
    208, 35, 136, 23, 127, 207, 237, 216, 169, 190, 27,
];

const SSK_PRIV_ACC_A: [u8; 32] = [
    93, 13, 190, 240, 250, 33, 108, 195, 176, 40, 144, 61, 4, 28, 58, 112, 53, 161, 42, 238, 155,
    27, 23, 176, 208, 121, 15, 229, 165, 180, 99, 143,
];

const SSK_PRIV_ACC_B: [u8; 32] = [
    48, 175, 124, 10, 230, 240, 166, 14, 249, 254, 157, 226, 208, 124, 122, 177, 203, 139, 192,
    180, 43, 120, 55, 151, 50, 21, 113, 22, 254, 83, 148, 56,
];

const DEFAULT_PROGRAM_OWNER: [u32; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

const PUB_ACC_A_INITIAL_BALANCE: u128 = 10000;
const PUB_ACC_B_INITIAL_BALANCE: u128 = 20000;

const PRIV_ACC_A_INITIAL_BALANCE: u128 = 10000;
const PRIV_ACC_B_INITIAL_BALANCE: u128 = 20000;

const DEVELOPMENT_FIXTURE_TESTNET_ERROR: &str = "initial_state_profile `development_fixture` is unsupported in binaries compiled with the `testnet` feature";

/// Selects the builtin state used to initialize a sequencer or indexer store.
///
/// [`Self::Default`] preserves the state selected by the running binary's
/// feature set. [`Self::DevelopmentFixture`] is deliberately limited to the
/// end-to-end test fixture: it adds the development Pinata program and its
/// account so a fresh indexer can replay the committed development fixture.
/// Binaries compiled with `testnet` must reject it because their clock and
/// builtin program transactions still target the Testnet program IDs.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialStateProfile {
    /// Preserve the state selected by the running binary's feature set.
    #[default]
    Default,
    /// Development state required to replay the committed end-to-end fixture.
    ///
    /// Unsupported in binaries compiled with `testnet`.
    DevelopmentFixture,
}

impl InitialStateProfile {
    /// Validates that the profile can be used by the current binary.
    pub const fn validate_for_compiled_network(self) -> Result<(), &'static str> {
        match self {
            Self::DevelopmentFixture if cfg!(feature = "testnet") => {
                Err(DEVELOPMENT_FIXTURE_TESTNET_ERROR)
            }
            Self::Default | Self::DevelopmentFixture => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicAccountPublicInitialData {
    pub account_id: AccountId,
    pub balance: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivateAccountPublicInitialData {
    pub npk: lee_core::NullifierPublicKey,
    pub vpk: lee_core::encryption::ViewingPublicKey,
    pub account: lee_core::account::Account,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicAccountPrivateInitialData {
    pub account_id: lee::AccountId,
    pub pub_sign_key: lee::PrivateKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateAccountPrivateInitialData {
    pub account: lee_core::account::Account,
    pub key_chain: KeyChain,
    pub chain_index: Option<ChainIndex>,
    pub identifier: lee_core::Identifier,
}

impl PrivateAccountPrivateInitialData {
    #[must_use]
    pub fn account_id(&self) -> lee::AccountId {
        lee::AccountId::for_regular_private_account(
            &self.key_chain.nullifier_public_key,
            &self.key_chain.viewing_public_key,
            self.identifier,
        )
    }
}

#[must_use]
pub fn initial_pub_accounts_private_keys() -> Vec<PublicAccountPrivateInitialData> {
    let acc1_pub_sign_key = PrivateKey::try_new(PRIVATE_KEY_PUB_ACC_A).unwrap();

    let acc2_pub_sign_key = PrivateKey::try_new(PRIVATE_KEY_PUB_ACC_B).unwrap();

    vec![
        PublicAccountPrivateInitialData {
            account_id: AccountId::from(&PublicKey::new_from_private_key(&acc1_pub_sign_key)),
            pub_sign_key: acc1_pub_sign_key,
        },
        PublicAccountPrivateInitialData {
            account_id: AccountId::from(&PublicKey::new_from_private_key(&acc2_pub_sign_key)),
            pub_sign_key: acc2_pub_sign_key,
        },
    ]
}

fn key_chain_from_ssk(ssk: [u8; 32]) -> KeyChain {
    let secret_spending_key = SecretSpendingKey(ssk);
    let private_key_holder = secret_spending_key.produce_private_key_holder(None);
    let nullifier_public_key = private_key_holder.generate_nullifier_public_key();
    let viewing_public_key = private_key_holder.generate_viewing_public_key();

    KeyChain {
        secret_spending_key,
        private_key_holder,
        nullifier_public_key,
        viewing_public_key,
    }
}

fn initial_priv_accounts_private_keys() -> Vec<PrivateAccountPrivateInitialData> {
    let key_chain_1 = key_chain_from_ssk(SSK_PRIV_ACC_A);
    let key_chain_2 = key_chain_from_ssk(SSK_PRIV_ACC_B);

    vec![
        PrivateAccountPrivateInitialData {
            account: Account {
                program_owner: DEFAULT_PROGRAM_OWNER,
                balance: PRIV_ACC_A_INITIAL_BALANCE,
                data: Data::default(),
                nonce: 0.into(),
            },
            key_chain: key_chain_1,
            chain_index: None,
            identifier: 0,
        },
        PrivateAccountPrivateInitialData {
            account: Account {
                program_owner: DEFAULT_PROGRAM_OWNER,
                balance: PRIV_ACC_B_INITIAL_BALANCE,
                data: Data::default(),
                nonce: 0.into(),
            },
            key_chain: key_chain_2,
            chain_index: None,
            identifier: 0,
        },
    ]
}

fn initial_commitments() -> Vec<PrivateAccountPublicInitialData> {
    initial_priv_accounts_private_keys()
        .into_iter()
        .map(|data| PrivateAccountPublicInitialData {
            npk: data.key_chain.nullifier_public_key,
            vpk: data.key_chain.viewing_public_key.clone(),
            account: data.account,
        })
        .collect()
}

fn initial_private_accounts() -> Vec<(lee_core::Commitment, lee_core::Nullifier)> {
    initial_commitments()
        .iter()
        .map(|init_comm_data| {
            let npk = &init_comm_data.npk;
            let account_id =
                lee::AccountId::for_regular_private_account(npk, &init_comm_data.vpk, 0);

            let mut acc = init_comm_data.account.clone();

            acc.program_owner = programs::authenticated_transfer().id();

            (
                lee_core::Commitment::new(&account_id, &acc),
                lee_core::Nullifier::for_account_initialization(&account_id),
            )
        })
        .collect()
}

#[must_use]
pub fn initial_public_user_accounts() -> Vec<PublicAccountPublicInitialData> {
    let initial_account_ids = initial_pub_accounts_private_keys()
        .into_iter()
        .map(|data| data.account_id)
        .collect::<Vec<_>>();

    vec![
        PublicAccountPublicInitialData {
            account_id: initial_account_ids[0],
            balance: PUB_ACC_A_INITIAL_BALANCE,
        },
        PublicAccountPublicInitialData {
            account_id: initial_account_ids[1],
            balance: PUB_ACC_B_INITIAL_BALANCE,
        },
    ]
}

fn initial_public_accounts() -> HashMap<AccountId, Account> {
    initial_public_user_accounts()
        .iter()
        .map(|acc_data| {
            (
                acc_data.account_id,
                Account {
                    program_owner: programs::authenticated_transfer().id(),
                    balance: acc_data.balance,
                    ..Default::default()
                },
            )
        })
        .chain([
            (
                system_accounts::faucet_account_id(),
                system_accounts::faucet_account(),
            ),
            (
                system_accounts::bridge_account_id(),
                system_accounts::bridge_account(),
            ),
        ])
        .chain(
            system_accounts::clock_account_ids()
                .into_iter()
                .map(|clock_id| (clock_id, system_accounts::clock_account())),
        )
        .chain([(
            system_accounts::sequencer_stake_config_account_id(),
            system_accounts::sequencer_stake_config_account(),
        )])
        .collect()
}

/// Development system accounts used only by the committed end-to-end fixture.
///
/// Do not route these through `system_accounts`' active-profile helpers: the
/// fixture must remain on development program IDs even when tests enable the
/// deployed Testnet feature elsewhere in the workspace.
fn development_fixture_public_accounts() -> HashMap<AccountId, Account> {
    initial_public_user_accounts()
        .iter()
        .map(|acc_data| {
            (
                acc_data.account_id,
                Account {
                    program_owner: programs::authenticated_transfer().id(),
                    balance: acc_data.balance,
                    ..Default::default()
                },
            )
        })
        .chain([
            (
                faucet_core::compute_faucet_account_id(programs::faucet().id()),
                Account {
                    program_owner: programs::authenticated_transfer().id(),
                    balance: u128::MAX,
                    ..Default::default()
                },
            ),
            (
                bridge_core::compute_bridge_account_id(programs::bridge().id()),
                Account {
                    program_owner: programs::authenticated_transfer().id(),
                    ..Default::default()
                },
            ),
            development_fixture_pinata_account(),
        ])
        .chain(
            clock_core::CLOCK_PROGRAM_ACCOUNT_IDS
                .into_iter()
                .map(|clock_id| {
                    (
                        clock_id,
                        Account {
                            program_owner: programs::clock().id(),
                            data: clock_core::ClockAccountData {
                                block_id: 0,
                                timestamp: 0,
                            }
                            .to_bytes()
                            .try_into()
                            .expect("Clock account data should fit"),
                            ..Default::default()
                        },
                    )
                }),
        )
        .chain(std::iter::once(wrapped_token_config_account()))
        .chain(std::iter::once((
            system_accounts::sequencer_stake_config_account_id(),
            system_accounts::sequencer_stake_config_account(),
        )))
        .collect()
}

fn development_fixture_pinata_account() -> (AccountId, Account) {
    (
        system_accounts::pinata_account_id(),
        Account {
            program_owner: programs::pinata().id(),
            balance: 1_500_000,
            data: vec![3; 33]
                .try_into()
                .expect("Pinata account data should fit"),
            ..Default::default()
        },
    )
}

/// The wrapped-token config account.
///
/// Seeded so the `wrapped_token` guest can pin its authorized minter (the
/// cross-zone inbox) without importing the inbox id. Fixed for every zone, so it
/// lives in the shared initial state.
fn wrapped_token_config_account() -> (AccountId, Account) {
    let wrapped_token_id = programs::wrapped_token().id();
    let config = wrapped_token_core::WrappedTokenConfig {
        minter: programs::cross_zone_inbox().id(),
        sources: Vec::new(),
    };
    (
        wrapped_token_core::config_account_id(wrapped_token_id),
        Account {
            program_owner: wrapped_token_id,
            data: config
                .to_bytes()
                .try_into()
                .expect("minter id fits in account data"),
            ..Default::default()
        },
    )
}

fn initial_programs() -> Vec<Program> {
    vec![
        programs::authenticated_transfer(),
        programs::token(),
        programs::amm(),
        programs::clock(),
        programs::ata(),
        programs::vault(),
        programs::faucet(),
        programs::bridge(),
        programs::sequencer_stake(),
        // Cross-zone programs are builtins: their bytecode is baked into every node,
        // so registering them in the base state (rather than shipping ELFs through
        // the genesis block, which exceeds the inscription size limit) keeps the two
        // nodes in lock-step with nothing to desync.
        programs::cross_zone_inbox(),
        programs::cross_zone_outbox(),
        programs::ping_sender(),
        programs::ping_receiver(),
        programs::bridge_lock(),
        programs::wrapped_token(),
    ]
}

#[must_use]
pub fn initial_state() -> V03State {
    lee::V03State::new()
        .with_public_accounts(initial_public_accounts())
        .with_private_accounts(initial_private_accounts())
        .with_programs(initial_programs())
}

/// Builds the initial state selected by a persisted service configuration.
///
/// The default profile intentionally retains the existing feature-selected
/// behavior. The fixture profile is explicit so a restored development dump
/// and a freshly initialized indexer start with the same builtin programs.
#[must_use]
pub fn initial_state_for_profile(profile: InitialStateProfile) -> V03State {
    match profile {
        InitialStateProfile::Default => {
            #[cfg(feature = "testnet")]
            {
                initial_state_testnet()
            }
            #[cfg(not(feature = "testnet"))]
            {
                initial_state()
            }
        }
        InitialStateProfile::DevelopmentFixture => development_fixture_initial_state(),
    }
}

fn development_fixture_initial_state() -> V03State {
    let mut programs = initial_programs();
    programs.push(programs::pinata());

    V03State::new()
        .with_public_accounts(development_fixture_public_accounts())
        .with_private_accounts(initial_private_accounts())
        .with_programs(programs)
}

/// Builds the state required to replay the deployed Testnet protocol.
#[cfg(feature = "testnet")]
#[must_use]
pub fn initial_state_testnet() -> V03State {
    testnet_v0_2::initial_state()
}

/// Compatibility fallback for consumers that do not enable the Testnet protocol feature.
#[cfg(not(feature = "testnet"))]
#[must_use]
pub fn initial_state_testnet() -> V03State {
    initial_state()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use key_protocol::key_management::secret_holders::ViewingSecretKey;

    use super::*;

    const VSK_D_PRIV_ACC_A: [u8; 32] = [
        4, 118, 187, 42, 14, 254, 144, 150, 125, 176, 205, 240, 109, 81, 234, 177, 244, 236, 108,
        71, 107, 10, 107, 169, 95, 134, 75, 193, 213, 57, 81, 218,
    ];

    const VSK_Z_PRIV_ACC_A: [u8; 32] = [
        117, 29, 113, 136, 175, 148, 38, 38, 110, 220, 157, 155, 245, 13, 239, 244, 106, 126, 188,
        90, 204, 28, 82, 70, 200, 16, 219, 33, 43, 210, 125, 239,
    ];

    const VSK_D_PRIV_ACC_B: [u8; 32] = [
        100, 59, 111, 232, 245, 32, 102, 179, 205, 119, 145, 238, 9, 235, 62, 38, 55, 252, 179,
        217, 219, 211, 6, 188, 85, 160, 68, 54, 61, 114, 102, 81,
    ];

    const VSK_Z_PRIV_ACC_B: [u8; 32] = [
        123, 246, 87, 46, 116, 95, 39, 122, 251, 71, 207, 144, 70, 227, 120, 27, 98, 59, 67, 247,
        209, 194, 110, 231, 250, 247, 205, 243, 31, 142, 104, 208,
    ];

    const PUB_ACC_A_TEXT_ADDR: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";
    const PUB_ACC_B_TEXT_ADDR: &str = "7wHg9sbJwc6h3NP1S9bekfAzB8CHifEcxKswCKUt3YQo";

    const PRIV_ACC_A_TEXT_ADDR: &str = "GSx3EttJzQqhFPibttxguyhKXkiD4DJmA2dMmuszEmFv";
    const PRIV_ACC_B_TEXT_ADDR: &str = "Dec1rT4DynCafh6k5pmywLGUU16RpxcxCdrSVYq8ukaN";

    #[test]
    fn default_profile_preserves_current_build_state() {
        let profile_state = initial_state_for_profile(InitialStateProfile::Default);

        #[cfg(feature = "testnet")]
        let expected_state = initial_state_testnet();
        #[cfg(not(feature = "testnet"))]
        let expected_state = initial_state();

        assert_eq!(profile_state.program_ids(), expected_state.program_ids());
        assert_eq!(
            profile_state.get_account_by_id(system_accounts::pinata_account_id()),
            expected_state.get_account_by_id(system_accounts::pinata_account_id()),
        );
    }

    #[cfg(not(feature = "testnet"))]
    #[test]
    fn development_fixture_profile_contains_development_pinata_state() {
        let state = initial_state_for_profile(InitialStateProfile::DevelopmentFixture);
        let expected_pinata_account_id = system_accounts::pinata_account_id();
        let expected_pinata_account = system_accounts::pinata_account();

        assert_eq!(
            programs::pinata().id(),
            expected_pinata_account.program_owner,
        );
        assert_eq!(
            system_accounts::pinata_account_id(),
            expected_pinata_account_id,
        );
        assert!(
            state
                .program_ids()
                .contains(&expected_pinata_account.program_owner)
        );
        assert_eq!(
            state.get_account_by_id(expected_pinata_account_id),
            expected_pinata_account,
        );
    }

    #[cfg(not(feature = "testnet"))]
    #[test]
    fn development_fixture_profile_matches_legacy_development_state() {
        // The committed prebuilt fixture was created before the Testnet profile
        // gate, from this exact development state plus Pinata. Keep the explicit
        // fixture profile byte-for-byte compatible without depending on active
        // Testnet feature selection.
        let mut legacy_public_accounts_map = initial_public_accounts();
        legacy_public_accounts_map.insert(
            system_accounts::pinata_account_id(),
            system_accounts::pinata_account(),
        );
        let mut legacy_programs = initial_programs();
        legacy_programs.push(programs::pinata());

        let legacy_state = V03State::new()
            .with_public_accounts(legacy_public_accounts_map.clone())
            .with_private_accounts(initial_private_accounts())
            .with_programs(legacy_programs);
        let fixture_state = initial_state_for_profile(InitialStateProfile::DevelopmentFixture);

        assert_eq!(fixture_state.program_ids(), legacy_state.program_ids());
        let mut legacy_public_accounts: Vec<_> = legacy_public_accounts_map.into_iter().collect();
        legacy_public_accounts.sort_by_key(|(account_id, _)| *account_id);
        for (account_id, account) in legacy_public_accounts {
            assert_eq!(fixture_state.get_account_by_id(account_id), account);
        }
    }

    #[test]
    fn pub_state_consistency() {
        let init_accs_private_data = initial_pub_accounts_private_keys();
        let init_accs_pub_data = initial_public_user_accounts();

        assert_eq!(
            init_accs_private_data[0].account_id,
            init_accs_pub_data[0].account_id
        );

        assert_eq!(
            init_accs_private_data[1].account_id,
            init_accs_pub_data[1].account_id
        );

        assert_eq!(
            init_accs_pub_data[0],
            PublicAccountPublicInitialData {
                account_id: AccountId::from_str(PUB_ACC_A_TEXT_ADDR).unwrap(),
                balance: PUB_ACC_A_INITIAL_BALANCE,
            }
        );

        assert_eq!(
            init_accs_pub_data[1],
            PublicAccountPublicInitialData {
                account_id: AccountId::from_str(PUB_ACC_B_TEXT_ADDR).unwrap(),
                balance: PUB_ACC_B_INITIAL_BALANCE,
            }
        );
    }

    #[test]
    fn private_state_consistency() {
        let init_private_accs_keys = initial_priv_accounts_private_keys();
        let init_comms = initial_commitments();

        // `nsk`/`npk` carry no constants of their own: the key chains derive from `SSK_*`, and the
        // two address canaries below pin H(PREFIX || npk || vpk || identifier), so drift anywhere
        // in ask -> nsk -> npk or in vsk -> vpk moves one of them. Nothing is left unpinned.
        // `VSK_*` stays pinned separately because it is the last value on the vsk -> vpk leg that
        // a test can compare directly.
        assert_eq!(
            init_private_accs_keys[0]
                .key_chain
                .private_key_holder
                .viewing_secret_key,
            ViewingSecretKey::new(VSK_D_PRIV_ACC_A, VSK_Z_PRIV_ACC_A)
        );
        assert_eq!(
            init_private_accs_keys[1]
                .key_chain
                .private_key_holder
                .viewing_secret_key,
            ViewingSecretKey::new(VSK_D_PRIV_ACC_B, VSK_Z_PRIV_ACC_B)
        );

        assert_eq!(
            init_private_accs_keys[0].account_id().to_string(),
            PRIV_ACC_A_TEXT_ADDR
        );
        assert_eq!(
            init_private_accs_keys[1].account_id().to_string(),
            PRIV_ACC_B_TEXT_ADDR
        );

        assert_eq!(
            init_private_accs_keys[0].key_chain.nullifier_public_key,
            init_comms[0].npk
        );
        assert_eq!(
            init_private_accs_keys[1].key_chain.nullifier_public_key,
            init_comms[1].npk
        );

        assert_eq!(
            init_comms[0],
            PrivateAccountPublicInitialData {
                npk: init_private_accs_keys[0].key_chain.nullifier_public_key,
                vpk: init_private_accs_keys[0]
                    .key_chain
                    .viewing_public_key
                    .clone(),
                account: Account {
                    program_owner: DEFAULT_PROGRAM_OWNER,
                    balance: PRIV_ACC_A_INITIAL_BALANCE,
                    data: Data::default(),
                    nonce: 0.into(),
                },
            }
        );

        assert_eq!(
            init_comms[1],
            PrivateAccountPublicInitialData {
                npk: init_private_accs_keys[1].key_chain.nullifier_public_key,
                vpk: init_private_accs_keys[1]
                    .key_chain
                    .viewing_public_key
                    .clone(),
                account: Account {
                    program_owner: DEFAULT_PROGRAM_OWNER,
                    balance: PRIV_ACC_B_INITIAL_BALANCE,
                    data: Data::default(),
                    nonce: 0.into(),
                },
            }
        );
    }

    #[test]
    fn genesis_system_accounts_have_expected_contents() {
        // System-account IDs must be distinct and non-default, and the genesis
        // faucet/bridge accounts must carry their expected field values.  Catches
        // mutations that replace `system_faucet_account`/`system_bridge_account`
        // with `Default::default()`, delete their `balance`/`program_owner`
        // fields, or replace `system_bridge_account_id` with `Default::default()`.
        let faucet_id = system_accounts::faucet_account_id();
        let bridge_id = system_accounts::bridge_account_id();
        assert_ne!(bridge_id, AccountId::default());
        assert_ne!(faucet_id, bridge_id);

        let state = initial_state();
        let default_owner = Account::default().program_owner;

        let faucet = state.get_account_by_id(faucet_id);
        assert_eq!(faucet.balance, u128::MAX, "faucet must hold u128::MAX");
        assert_ne!(
            faucet.program_owner, default_owner,
            "faucet must have a non-default program_owner"
        );

        let bridge = state.get_account_by_id(bridge_id);
        assert_ne!(
            bridge.program_owner, default_owner,
            "bridge must have a non-default program_owner"
        );
    }

    #[test]
    fn default_profile_is_supported_by_current_binary() {
        assert_eq!(
            InitialStateProfile::Default.validate_for_compiled_network(),
            Ok(())
        );
    }

    #[cfg(feature = "testnet")]
    #[test]
    fn development_fixture_profile_is_rejected_in_testnet_builds() {
        assert_eq!(
            InitialStateProfile::DevelopmentFixture.validate_for_compiled_network(),
            Err(DEVELOPMENT_FIXTURE_TESTNET_ERROR)
        );
    }

    #[cfg(not(feature = "testnet"))]
    #[test]
    fn development_fixture_profile_is_allowed_in_non_testnet_builds() {
        assert_eq!(
            InitialStateProfile::DevelopmentFixture.validate_for_compiled_network(),
            Ok(())
        );
    }
}
