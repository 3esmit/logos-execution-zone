use lee_core::account::{Account, AccountId, AccountWithMetadata};

use crate::program::Program;

#[test]
fn program_execution() {
    let program = crate::test_methods::simple_balance_transfer();
    let balance_to_move: u128 = 11_223_344_556_677;
    let instruction_data = Program::serialize_instruction(balance_to_move).unwrap();
    let sender = AccountWithMetadata::new(
        Account {
            balance: 77_665_544_332_211,
            ..Account::default()
        },
        true,
        AccountId::new([0; 32]),
    );
    let recipient = AccountWithMetadata::new(Account::default(), false, AccountId::new([1; 32]));

    let expected_sender_post = Account {
        balance: 77_665_544_332_211 - balance_to_move,
        ..Account::default()
    };
    let expected_recipient_post = Account {
        balance: balance_to_move,
        ..Account::default()
    };
    let program_output = program
        .execute(None, &[sender, recipient], &instruction_data)
        .unwrap();

    let [sender_post, recipient_post] = program_output.post_states.try_into().unwrap();

    assert_eq!(sender_post.account(), &expected_sender_post);
    assert_eq!(recipient_post.account(), &expected_recipient_post);
}

#[test]
fn deployed_clock_requires_legacy_guest_wire_schema() {
    use lee_core::{
        account::{Data, Nonce},
        program::{BlockValidityWindow, ChainedCall, Claim, ProgramId, TimestampValidityWindow},
    };
    use risc0_zkvm::{ExecutorEnv, default_executor};
    use serde::{Deserialize, Serialize};

    // Independent test DTOs mirror the source-attested v0.2 guest schema.
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct LegacyAccount {
        program_owner: ProgramId,
        balance: u128,
        data: Data,
        nonce: Nonce,
    }
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct LegacyMetadata {
        account: LegacyAccount,
        is_authorized: bool,
        account_id: AccountId,
    }
    #[derive(Debug, Deserialize)]
    struct LegacyPost {
        account: LegacyAccount,
        claim: Option<Claim>,
    }
    #[derive(Debug, Deserialize)]
    struct LegacyOutput {
        self_program_id: ProgramId,
        caller_program_id: Option<ProgramId>,
        instruction_data: Vec<u32>,
        pre_states: Vec<LegacyMetadata>,
        post_states: Vec<LegacyPost>,
        chained_calls: Vec<ChainedCall>,
        block_validity_window: BlockValidityWindow,
        timestamp_validity_window: TimestampValidityWindow,
    }

    let binary = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../lez/testnet_initial_state/testnet-v0.2/clock.bin"
    ));
    let program = Program::new(binary.as_slice().to_vec().into()).unwrap();
    let program_id = program.id();
    let ids = [
        AccountId::new(*b"/LEZ/ClockProgramAccount/0000001"),
        AccountId::new(*b"/LEZ/ClockProgramAccount/0000010"),
        AccountId::new(*b"/LEZ/ClockProgramAccount/0000050"),
    ];
    let data = [
        [195, 26, 0, 0, 0, 0, 0, 0, 105, 243, 32, 49, 159, 1, 0, 0],
        [194, 26, 0, 0, 0, 0, 0, 0, 178, 8, 32, 49, 159, 1, 0, 0],
        [194, 26, 0, 0, 0, 0, 0, 0, 178, 8, 32, 49, 159, 1, 0, 0],
    ];
    let legacy: Vec<_> = ids
        .into_iter()
        .zip(data)
        .map(|(account_id, data)| LegacyMetadata {
            account: LegacyAccount {
                program_owner: program_id,
                balance: 0,
                data: data.to_vec().try_into().unwrap(),
                nonce: Nonce::default(),
            },
            is_authorized: false,
            account_id,
        })
        .collect();
    let current: Vec<_> = legacy
        .iter()
        .map(|item| {
            AccountWithMetadata::new(
                Account {
                    program_owner: item.account.program_owner.into(),
                    balance: item.account.balance,
                    data: item.account.data.clone(),
                    nonce: item.account.nonce,
                },
                item.is_authorized,
                item.account_id,
            )
        })
        .collect();
    let instruction = Program::serialize_instruction(1_783_235_730_974_u64).unwrap();
    let mut current_builder = ExecutorEnv::builder();
    current_builder.session_limit(Some(32 * 1024 * 1024));
    current_builder.write(&program_id).unwrap();
    current_builder.write(&None::<ProgramId>).unwrap();
    current_builder.write(&current).unwrap();
    current_builder.write(&instruction).unwrap();
    let current_result = default_executor().execute(current_builder.build().unwrap(), binary);
    assert!(
        matches!(&current_result, Err(error) if error.to_string().contains("DeserializeBadBool")),
        "unexpected current-ABI result: {current_result:?}"
    );

    let mut builder = ExecutorEnv::builder();
    builder.session_limit(Some(32 * 1024 * 1024));
    builder.write(&program_id).unwrap();
    builder.write(&None::<ProgramId>).unwrap();
    builder.write(&legacy).unwrap();
    builder.write(&instruction).unwrap();
    let session = default_executor()
        .execute(builder.build().unwrap(), binary)
        .expect("unchanged deployed clock must execute with its attested input schema");
    assert!(
        session
            .journal
            .decode::<lee_core::program::ProgramOutput>()
            .is_err()
    );
    let output: LegacyOutput = session.journal.decode().unwrap();
    #[cfg(feature = "testnet-v0-2")]
    let adapted_output = program.execute(None, &current, &instruction).unwrap();
    #[cfg(feature = "testnet-v0-2")]
    assert_eq!(adapted_output.pre_states, current);
    assert_eq!(output.self_program_id, program_id);
    assert_eq!(output.caller_program_id, None);
    assert_eq!(output.instruction_data, instruction);
    assert_eq!(output.pre_states, legacy);
    assert_eq!(output.post_states.len(), 3);
    assert!(output.chained_calls.is_empty());
    assert_eq!(
        output.block_validity_window,
        BlockValidityWindow::new_unbounded()
    );
    assert_eq!(
        output.timestamp_validity_window,
        TimestampValidityWindow::new_unbounded()
    );
    for (index, post) in output.post_states.iter().enumerate() {
        assert!(post.claim.is_none());
        assert_eq!(post.account.program_owner, program_id);
        assert_eq!(post.account.balance, 0);
        assert_eq!(post.account.nonce, Nonce::default());
        let mut expected = data[index];
        if index == 0 {
            expected[..8].copy_from_slice(&6_852_u64.to_le_bytes());
            expected[8..].copy_from_slice(&1_783_235_730_974_u64.to_le_bytes());
        }
        assert_eq!(post.account.data.as_ref(), expected.as_slice());
        #[cfg(feature = "testnet-v0-2")]
        assert_eq!(
            adapted_output.post_states[index].account().data.as_ref(),
            expected.as_slice()
        );
    }
}
