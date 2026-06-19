#![expect(
    clippy::tests_outside_test_module,
    clippy::arithmetic_side_effects,
    reason = "We don't care about these in tests"
)]

use std::collections::BTreeMap;

use cross_zone_inbox_core::{
    CrossZoneMessage, InboxConfig, Instruction, inbox_config_account_id,
    inbox_seen_shard_account_id,
};
use lee_core::account::Account;
use ping_core::{ReceiverInstruction, ping_record_pda};

use crate::{
    V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
    validated_state_diff::ValidatedStateDiff,
};

/// Drives `cross_zone_inbox::Dispatch` directly through the state machine
/// (no watcher) and asserts the message is delivered to `ping_receiver`, which
/// records the payload into its own PDA.
#[test]
fn inbox_dispatch_delivers_payload_to_ping_receiver() {
    let mut state = V03State::new_with_genesis_accounts(&[], vec![], 0);
    // ping_receiver is a throwaway demo target, registered only in this test state.
    state.insert_program(Program::ping_receiver());

    let inbox_id = Program::cross_zone_inbox().id();
    let receiver_id = Program::ping_receiver().id();

    let self_zone = [1_u8; 32];
    let src_zone = [2_u8; 32];
    let src_block_id = 5;

    // Seed the inbox config account (inbox-owned) allowing src_zone -> ping_receiver.
    let mut allowed_targets = BTreeMap::new();
    allowed_targets.insert(src_zone, vec![receiver_id]);
    let config = InboxConfig {
        self_zone,
        allowed_peers: BTreeMap::new(),
        allowed_targets,
    };
    let config_id = inbox_config_account_id(inbox_id);
    state.force_insert_account(
        config_id,
        Account {
            program_owner: inbox_id,
            balance: 0_u128,
            data: config
                .to_bytes()
                .try_into()
                .expect("config fits in account data"),
            nonce: 0_u128.into(),
        },
    );

    // The payload is the ping_receiver instruction, serialized as risc0 words in
    // little-endian bytes (the contract the inbox reverses when forwarding).
    let inner = b"hello-cross-zone".to_vec();
    let words = risc0_zkvm::serde::to_vec(&ReceiverInstruction::Record {
        payload: inner.clone(),
    })
    .expect("serialize ping instruction");
    let payload: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();

    let msg = CrossZoneMessage {
        src_zone,
        src_block_id,
        src_tx_index: 0,
        src_program_id: [9_u32; 8],
        target_program_id: receiver_id,
        payload,
        l1_inclusion_witness: None,
    };

    let seen_id = inbox_seen_shard_account_id(inbox_id, &src_zone, src_block_id);
    let record_id = ping_record_pda(receiver_id);

    let message = Message::try_new(
        inbox_id,
        vec![config_id, seen_id, record_id],
        vec![],
        Instruction::Dispatch(msg),
    )
    .expect("build dispatch message");
    let tx = crate::PublicTransaction::new(message, WitnessSet::from_raw_parts(vec![]));

    let diff = ValidatedStateDiff::from_public_transaction(&tx, &state, 1, 0)
        .expect("dispatch must validate and execute");
    let public_diff = diff.public_diff();

    let record = public_diff
        .get(&record_id)
        .expect("ping record account must change");
    assert_eq!(
        record.data.clone().into_inner(),
        inner,
        "ping_receiver must record the delivered payload"
    );
}
