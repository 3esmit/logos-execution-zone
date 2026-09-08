use lee_core::{
    account::{Account, AccountId, Nonce},
    program::{
        AccountPostState, BlockValidityWindow, ChainedCall, Claim, PdaSeed, TimestampValidityWindow,
    },
};

use super::*;

#[test]
fn only_attested_public_programs_select_legacy_abi() {
    for id in [
        ids::AUTHENTICATED_TRANSFER_ID,
        ids::TOKEN_ID,
        ids::AMM_ID,
        ids::CLOCK_ID,
        ids::ASSOCIATED_TOKEN_ACCOUNT_ID,
        ids::VAULT_ID,
        ids::FAUCET_ID,
        ids::BRIDGE_ID,
        ids::PINATA_ID,
    ] {
        assert!(uses_legacy_abi(id));
        let mut changed = id;
        changed[0] ^= 1;
        assert!(!uses_legacy_abi(changed));
    }
    assert!(!uses_legacy_abi([0; 8]));
    assert!(!uses_legacy_abi(ids::PRIVACY_PRESERVING_CIRCUIT_ID));
}

#[test]
fn owner_words_and_account_fields_round_trip_losslessly() {
    let owner = [0x0102_0304, u32::MAX, 2, 3, 4, 5, 6, 7];
    let account = Account {
        program_owner: owner.into(),
        balance: u128::MAX - 1,
        data: vec![0, 1, 127, 255].try_into().unwrap(),
        nonce: Nonce(u128::MAX - 2),
    };
    let legacy = abi::LegacyAccount::from(&account);
    let words = risc0_zkvm::serde::to_vec(&legacy).unwrap();
    assert_eq!(&words[..8], &owner);
    let decoded: abi::LegacyAccount = risc0_zkvm::serde::from_slice(&words).unwrap();
    assert_eq!(Account::from(decoded), account);
}

#[test]
fn output_conversion_preserves_authorization_claims_calls_and_windows() {
    let account = Account {
        program_owner: [1, 2, 3, 4, 5, 6, 7, 8].into(),
        balance: 91,
        data: vec![3, 2, 1].try_into().unwrap(),
        nonce: Nonce(47),
    };
    let authorized = AccountWithMetadata::new(account.clone(), true, AccountId::new([11; 32]));
    let unauthorized = AccountWithMetadata::new(account.clone(), false, AccountId::new([12; 32]));
    let seed = PdaSeed::new([13; 32]);
    let claims = [None, Some(Claim::Authorized), Some(Claim::Pda(seed))];
    let legacy = abi::Output {
        self_program_id: ids::CLOCK_ID,
        caller_program_id: Some(ids::TOKEN_ID),
        instruction_data: vec![1, 8, 5],
        pre_states: vec![(&authorized).into(), (&unauthorized).into()],
        post_states: claims
            .into_iter()
            .map(|claim| abi::PostState {
                account: (&account).into(),
                claim,
            })
            .collect(),
        chained_calls: vec![abi::Call {
            program_id: ids::VAULT_ID,
            pre_states: vec![(&unauthorized).into(), (&authorized).into()],
            instruction_data: vec![99, 100],
            pda_seeds: vec![seed],
        }],
        block_validity_window: BlockValidityWindow::try_from(20..40).unwrap(),
        timestamp_validity_window: TimestampValidityWindow::try_from(60..80).unwrap(),
    };
    let words = risc0_zkvm::serde::to_vec(&legacy).unwrap();
    let journal = Journal::new(words.into_iter().flat_map(u32::to_le_bytes).collect());
    let output = decode_output(&journal).unwrap();
    let expected = ProgramOutput {
        self_program_id: ids::CLOCK_ID,
        caller_program_id: Some(ids::TOKEN_ID),
        instruction_data: vec![1, 8, 5],
        pre_states: vec![authorized.clone(), unauthorized.clone()],
        post_states: vec![
            AccountPostState::new(account.clone()),
            AccountPostState::new_claimed(account.clone(), Claim::Authorized),
            AccountPostState::new_claimed(account, Claim::Pda(seed)),
        ],
        chained_calls: vec![ChainedCall {
            program_id: ids::VAULT_ID,
            pre_states: vec![unauthorized, authorized],
            instruction_data: vec![99, 100],
            pda_seeds: vec![seed],
        }],
        block_validity_window: BlockValidityWindow::try_from(20..40).unwrap(),
        timestamp_validity_window: TimestampValidityWindow::try_from(60..80).unwrap(),
    };
    assert_eq!(output, expected);
}

#[test]
fn malformed_legacy_journal_is_an_execution_error() {
    for bytes in [vec![], vec![0, 1, 2], vec![0; 4], vec![0; 40]] {
        assert!(matches!(
            decode_output(&Journal::new(bytes)),
            Err(LeeError::ProgramExecutionFailed(_))
        ));
    }
}
