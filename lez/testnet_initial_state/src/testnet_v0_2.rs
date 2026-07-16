//! Source-attested Testnet v0.2 genesis state.

use std::{collections::HashMap, str::FromStr as _};

use lee::{Account, AccountId, V03State, program::Program};

use super::{initial_commitments, initial_public_user_accounts};

const LEGACY_PRIVATE_ACCOUNT_IDS: [&str; 2] = [
    "4eGX3M3rgjHsme8n3sSp89af8JRZtYVTesbJjLqaX1VQ",
    "3m6HQmCgmAvsxZtxAHPqqEqoBG4335fCG8TzxigyW7rE",
];

pub fn initial_state() -> V03State {
    V03State::new()
        .with_public_accounts(initial_public_accounts())
        .with_private_accounts(initial_private_accounts())
        .with_programs(initial_programs())
}

fn initial_public_accounts() -> HashMap<AccountId, Account> {
    let authenticated_transfer_id = programs::testnet::authenticated_transfer().id();

    initial_public_user_accounts()
        .into_iter()
        .map(|account| {
            (
                account.account_id,
                Account {
                    program_owner: authenticated_transfer_id,
                    balance: account.balance,
                    ..Account::default()
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
            (
                system_accounts::pinata_account_id(),
                system_accounts::pinata_account(),
            ),
        ])
        .chain(
            system_accounts::clock_account_ids()
                .into_iter()
                .map(|clock_id| (clock_id, system_accounts::clock_account())),
        )
        .collect()
}

fn initial_private_accounts() -> Vec<(lee_core::Commitment, lee_core::Nullifier)> {
    let authenticated_transfer_id = programs::testnet::authenticated_transfer().id();

    initial_commitments()
        .into_iter()
        .zip(LEGACY_PRIVATE_ACCOUNT_IDS)
        .map(|(initial_data, account_id)| {
            let account_id = AccountId::from_str(account_id)
                .expect("Testnet private account ID should be a valid account ID");
            let mut account = initial_data.account;
            account.program_owner = authenticated_transfer_id;
            (
                lee_core::Commitment::new(&account_id, &account),
                lee_core::Nullifier::for_account_initialization(&account_id),
            )
        })
        .collect()
}

fn initial_programs() -> Vec<Program> {
    vec![
        programs::testnet::authenticated_transfer(),
        programs::testnet::token(),
        programs::testnet::amm(),
        programs::testnet::clock(),
        programs::testnet::ata(),
        programs::testnet::vault(),
        programs::testnet::faucet(),
        programs::testnet::bridge(),
        programs::testnet::pinata(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_program_and_accounts_are_the_deployed_testnet_versions() {
        let mut state = initial_state();
        let clock_program_id = programs::testnet::clock().id();

        for account_id in system_accounts::clock_account_ids() {
            assert_eq!(
                state.get_account_by_id(account_id).program_owner,
                clock_program_id
            );
        }

        let message = lee::public_transaction::Message::try_new(
            clock_program_id,
            system_accounts::clock_account_ids().to_vec(),
            vec![],
            7_u64,
        )
        .expect("Testnet clock invocation should be constructable");
        let transaction = lee::PublicTransaction::new(
            message,
            lee::public_transaction::WitnessSet::from_raw_parts(vec![]),
        );

        assert!(
            state
                .transition_from_public_transaction(&transaction, 1, 7)
                .is_ok()
        );
    }

    #[test]
    fn clock_program_replays_testnet_block_6852() {
        let clock_program_id = programs::testnet::clock().id();
        let mut state = V03State::new()
            .with_public_accounts(
                system_accounts::clock_account_ids()
                    .into_iter()
                    .zip([
                        [195, 26, 0, 0, 0, 0, 0, 0, 105, 243, 32, 49, 159, 1, 0, 0],
                        [194, 26, 0, 0, 0, 0, 0, 0, 178, 8, 32, 49, 159, 1, 0, 0],
                        [194, 26, 0, 0, 0, 0, 0, 0, 178, 8, 32, 49, 159, 1, 0, 0],
                    ])
                    .map(|(account_id, data)| {
                        (
                            account_id,
                            Account {
                                program_owner: clock_program_id,
                                data: data
                                    .to_vec()
                                    .try_into()
                                    .expect("Testnet clock state fits in account data"),
                                ..Account::default()
                            },
                        )
                    }),
            )
            .with_programs([programs::testnet::clock()]);

        let message = lee::public_transaction::Message::try_new(
            clock_program_id,
            system_accounts::clock_account_ids().to_vec(),
            vec![],
            1_783_235_730_974_u64,
        )
        .expect("Testnet clock invocation should be constructable");
        let transaction = lee::PublicTransaction::new(
            message,
            lee::public_transaction::WitnessSet::from_raw_parts(vec![]),
        );

        assert!(
            state
                .transition_from_public_transaction(&transaction, 6_852, 1_783_235_730_974)
                .is_ok()
        );
    }

    #[test]
    fn genesis_contains_legacy_private_commitments() {
        let state = initial_state();
        let authenticated_transfer_id = programs::testnet::authenticated_transfer().id();

        for (initial_data, account_id) in initial_commitments()
            .into_iter()
            .zip(LEGACY_PRIVATE_ACCOUNT_IDS)
        {
            let account_id = AccountId::from_str(account_id)
                .expect("Testnet private account ID should be a valid account ID");
            let commitment_account = Account {
                program_owner: authenticated_transfer_id,
                ..initial_data.account
            };
            let commitment = lee_core::Commitment::new(&account_id, &commitment_account);

            assert!(
                state.get_proof_for_commitment(&commitment).is_some(),
                "Testnet genesis must include commitment for {account_id}"
            );
        }
    }
}
