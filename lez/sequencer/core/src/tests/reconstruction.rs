#![expect(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    reason = "We don't care about it in tests"
)]

use common::block::Block;
use logos_blockchain_core::mantle::ops::channel::{MsgId, inscribe::Inscription};
use logos_blockchain_zone_sdk::{Slot, ZoneBlock, ZoneMessage};
use storage::sequencer::sequencer_cells::ZoneAnchorRecord;

use super::*;
use crate::{
    SequencerCore, block_store::SequencerStore, config::GenesisAction, mock::MockBlockPublisher,
};

fn block_to_channel_message(block: &Block, slot: u64) -> (ZoneMessage, Slot) {
    let bytes = borsh::to_vec(block).expect("serialize block");
    let message = ZoneMessage::Block(ZoneBlock {
        id: MsgId::from([0_u8; 32]),
        data: Inscription::try_from(bytes.as_slice()).expect("inscription"),
    });
    (message, Slot::from(slot))
}

/// Collects a sequencer's whole chain (genesis..=tip) into a canned channel,
/// one block per slot at `slot_step` spacing.
fn channel_from_store(store: &SequencerStore, slot_step: u64) -> Vec<(ZoneMessage, Slot)> {
    let genesis_id = store.genesis_id();
    let tip_id = store.latest_block_meta().expect("tip").expect("present").id;
    (genesis_id..=tip_id)
        .enumerate()
        .map(|(index, id)| {
            let block = store.get_block_at_id(id).expect("read").expect("present");
            block_to_channel_message(&block, (index as u64 + 1) * slot_step)
        })
        .collect()
}

#[tokio::test]
async fn reconstructs_missing_channel_blocks_into_fresh_store() {
    // Sequencer A produces a few blocks; treat its chain as the channel.
    let config_a = setup_sequencer_config();
    let (mut seq_a, _handle_a) =
        SequencerCoreWithMockClients::start_from_config(config_a.clone()).await;
    seq_a.produce_new_block().await.unwrap();
    seq_a.produce_new_block().await.unwrap();
    let tip_a = seq_a.block_store().latest_block_meta().unwrap().unwrap();

    let messages = channel_from_store(seq_a.block_store(), 10);
    let tip_slot = messages.last().unwrap().1;
    let channel_id = config_a.bedrock_config.channel_id;

    // Sequencer B starts from a fresh store and reconstructs A's chain.
    let config_b = setup_sequencer_config();
    let (mut store_b, mut state_b) =
        SequencerCore::<MockBlockPublisher>::open_or_create_store(&config_b);
    let mock_b = MockBlockPublisher::with_canned_channel(channel_id, Some(tip_slot), messages);

    let channel_was_empty = SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock_b,
        &mut store_b,
        &mut state_b,
        true,
    )
    .await
    .expect("reconstruct");
    assert!(!channel_was_empty);

    let tip_b = store_b.latest_block_meta().unwrap().unwrap();
    assert_eq!(tip_b.id, tip_a.id);
    assert_eq!(tip_b.hash, tip_a.hash);

    // State matches: initial account balances agree with sequencer A.
    for account in initial_public_user_accounts() {
        assert_eq!(
            state_b.get_account_by_id(account.account_id).balance,
            seq_a.state().get_account_by_id(account.account_id).balance,
        );
    }

    let anchor = store_b.get_zone_anchor().unwrap().expect("anchor recorded");
    assert_eq!(anchor.block_id, tip_a.id);
    assert_eq!(anchor.slot, tip_slot.into_inner());

    // Re-running is idempotent: everything is already applied, no error.
    let channel_was_empty = SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock_b,
        &mut store_b,
        &mut state_b,
        true,
    )
    .await
    .expect("reconstruct idempotent");
    assert!(!channel_was_empty);
    assert_eq!(store_b.latest_block_meta().unwrap().unwrap().id, tip_a.id);
}

#[tokio::test]
async fn fails_when_channel_serves_a_divergent_block() {
    let config = setup_sequencer_config();
    let (mut store, mut state) = SequencerCore::<MockBlockPublisher>::open_or_create_store(&config);

    // Anchor on the local genesis at some slot.
    let genesis_id = store.genesis_id();
    let genesis = store.get_block_at_id(genesis_id).unwrap().unwrap();
    let anchor_slot = 100_u64;
    store
        .set_zone_anchor(&ZoneAnchorRecord {
            slot: anchor_slot,
            block_id: genesis_id,
            hash: genesis.header.hash,
        })
        .unwrap();

    // The channel serves a different block at the anchor id/slot.
    let mut tampered = genesis.clone();
    tampered.header.hash = HashType([9_u8; 32]);
    let messages = vec![block_to_channel_message(&tampered, anchor_slot)];
    let mock = MockBlockPublisher::with_canned_channel(
        config.bedrock_config.channel_id,
        Some(Slot::from(anchor_slot)),
        messages,
    );

    let result = SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock, &mut store, &mut state, true,
    )
    .await;
    assert!(result.is_err(), "divergent channel must abort startup");
}

#[tokio::test]
async fn fails_when_channel_is_missing() {
    let config = setup_sequencer_config();
    let (mut store, mut state) = SequencerCore::<MockBlockPublisher>::open_or_create_store(&config);
    let genesis_id = store.genesis_id();
    let genesis = store.get_block_at_id(genesis_id).unwrap().unwrap();
    store
        .set_zone_anchor(&ZoneAnchorRecord {
            slot: 100,
            block_id: genesis_id,
            hash: genesis.header.hash,
        })
        .unwrap();

    // Anchor present, but the channel does not exist on the connected chain.
    let mock =
        MockBlockPublisher::with_canned_channel(config.bedrock_config.channel_id, None, vec![]);
    let result = SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock, &mut store, &mut state, true,
    )
    .await;
    assert!(result.is_err(), "missing channel must abort startup");
}

/// A sequencer config whose genesis funds the bridge account, so replayed bridge
/// deposit transactions have a source balance to mint from.
fn bridge_funded_config() -> SequencerConfig {
    let mut config = setup_sequencer_config();
    config.genesis = vec![GenesisAction::SupplyBridgeAccount { balance: 1_000_000 }];
    config
}

/// Builds an unfulfilled pending deposit event for `recipient`, matching the
/// encoding `build_bridge_deposit_tx_from_event` expects.
fn deposit_event_record(
    op_id: [u8; 32],
    amount: u64,
    recipient: lee::AccountId,
) -> PendingDepositEventRecord {
    PendingDepositEventRecord {
        deposit_op_id: HashType(op_id),
        source_tx_hash: HashType([0_u8; 32]),
        amount,
        metadata: borsh::to_vec(&DepositMetadataForEncoding {
            recipient_id: recipient,
        })
        .unwrap(),
        submitted_in_block_id: None,
    }
}

/// Builds a signed public bridge `Withdraw` transaction (the normal user path).
fn build_public_withdraw_tx(
    sender: lee::AccountId,
    nonce: u128,
    amount: u64,
    bedrock_account_pk: [u8; 32],
    signing_key: &lee::PrivateKey,
) -> LeeTransaction {
    let message = lee::public_transaction::Message::try_new(
        programs::bridge().id(),
        vec![sender, system_accounts::bridge_account_id()],
        vec![nonce.into()],
        bridge_core::Instruction::Withdraw {
            amount,
            bedrock_account_pk,
        },
    )
    .unwrap();
    let witness_set = lee::public_transaction::WitnessSet::for_message(&message, &[signing_key]);
    LeeTransaction::Public(lee::PublicTransaction::new(message, witness_set))
}

/// The cold-start backfill re-delivers an already-finalized deposit into the
/// mempool before reconstruction applies the same deposit block, and the queued
/// mint cannot be pulled back out. Since the bridge program does not dedup on
/// `l1_deposit_op_id`, block production must skip the already-submitted deposit
/// so the vault is minted exactly once.
#[tokio::test]
async fn reconstructed_deposit_is_not_reminted_after_backfill_redelivery() {
    let recipient = initial_public_user_accounts()[0].account_id;
    let deposit_amount = 500_u64;
    let withdraw_amount = 100_u64;
    let bedrock_account_pk = [0x22_u8; 32];
    let deposit_op_id = [0x0d_u8; 32];

    // Sequencer A produces a deposit block then a withdraw block.
    let config_a = bridge_funded_config();
    let (mut seq_a, mempool_a) =
        SequencerCoreWithMockClients::start_from_config(config_a.clone()).await;

    let deposit_record = deposit_event_record(deposit_op_id, deposit_amount, recipient);
    let deposit_tx =
        crate::build_bridge_deposit_tx_from_event(&deposit_record).expect("build deposit tx");
    mempool_a
        .push((TransactionOrigin::Sequencer, deposit_tx.clone()))
        .await
        .unwrap();
    seq_a.produce_new_block().await.unwrap();

    let withdraw_tx = build_public_withdraw_tx(
        recipient,
        0,
        withdraw_amount,
        bedrock_account_pk,
        &create_signing_key_for_account1(),
    );
    mempool_a
        .push((TransactionOrigin::User, withdraw_tx.clone()))
        .await
        .unwrap();
    seq_a.produce_new_block().await.unwrap();

    let tip_a = seq_a.block_store().latest_block_meta().unwrap().unwrap();
    let messages = channel_from_store(seq_a.block_store(), 10);
    let tip_slot = messages.last().unwrap().1;
    let channel_id = config_a.bedrock_config.channel_id;

    let config_b = bridge_funded_config();
    let (mut seq_b, mempool_b) = SequencerCoreWithMockClients::start_from_config(config_b).await;

    // Backfill re-delivery: persist the pending record and queue the mint, as
    // `on_deposit_event` does, before reconstruction runs.
    assert!(
        seq_b
            .block_store()
            .dbio()
            .add_pending_deposit_event(deposit_record.clone())
            .unwrap()
    );
    mempool_b
        .push((TransactionOrigin::Sequencer, deposit_tx))
        .await
        .unwrap();

    let mock_b = MockBlockPublisher::with_canned_channel(channel_id, Some(tip_slot), messages);
    SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock_b,
        &mut seq_b.store,
        &mut seq_b.state,
        true,
    )
    .await
    .expect("reconstruct");
    seq_b.chain_height = seq_b.store.latest_block_meta().unwrap().unwrap().id;

    let tip_b = seq_b.block_store().latest_block_meta().unwrap().unwrap();
    assert_eq!(tip_b.id, tip_a.id);
    assert_eq!(tip_b.hash, tip_a.hash);

    seq_b.produce_new_block().await.unwrap();

    let vault_id = vault_core::compute_vault_account_id(programs::vault().id(), recipient);
    let bridge_id = system_accounts::bridge_account_id();
    for account in [vault_id, bridge_id, recipient] {
        assert_eq!(
            seq_b.state().get_account_by_id(account).balance,
            seq_a.state().get_account_by_id(account).balance,
            "reconstructed balance mismatch for {account:?}",
        );
    }
    assert_eq!(
        seq_b.state().get_account_by_id(vault_id).balance,
        u128::from(deposit_amount),
        "deposit must mint into the recipient vault exactly once, not twice"
    );

    let produced = seq_b
        .block_store()
        .get_block_at_id(tip_b.id + 1)
        .unwrap()
        .expect("produced block present");
    assert!(
        !produced
            .body
            .transactions
            .iter()
            .any(|tx| crate::extract_bridge_deposit_id(tx) == Some(HashType(deposit_op_id))),
        "the re-delivered mint must be skipped, not re-included in a block"
    );

    // A reconstructed withdraw's finalized L1 event was already re-delivered (and
    // dropped) by cold-start backfill, so it will never be consumed again.
    // Reconstruction must not count it, or the count stays phantom-inflated forever.
    let withdraw_arg = crate::extract_bridge_withdraw_data(&withdraw_tx).expect("withdraw data");
    let key = crate::withdraw_event_reconciliation_key(&withdraw_arg.outputs).expect("recon key");
    assert!(
        !seq_b
            .block_store()
            .dbio()
            .consume_unseen_withdraw_count(key)
            .unwrap(),
        "reconstruction must not leave a phantom unseen-withdraw count"
    );
}

/// A reconstructed withdraw block must not touch the unseen-withdraw counter.
/// Its finalized L1 Withdraw event was already re-delivered (and dropped as a
/// no-op) by cold-start backfill, so counting it during reconstruction would
/// leave a permanent phantom that nothing ever consumes.
#[tokio::test]
async fn reconstructed_withdraw_leaves_no_phantom_unseen_count() {
    let recipient = initial_public_user_accounts()[0].account_id;
    let withdraw_amount = 100_u64;
    let bedrock_account_pk = [0x33_u8; 32];

    // Sequencer A produces a single withdraw block; treat its chain as the channel.
    let config_a = bridge_funded_config();
    let (mut seq_a, mempool_a) =
        SequencerCoreWithMockClients::start_from_config(config_a.clone()).await;
    let withdraw_tx = build_public_withdraw_tx(
        recipient,
        0,
        withdraw_amount,
        bedrock_account_pk,
        &create_signing_key_for_account1(),
    );
    mempool_a
        .push((TransactionOrigin::User, withdraw_tx.clone()))
        .await
        .unwrap();
    seq_a.produce_new_block().await.unwrap();

    let withdraw_arg = crate::extract_bridge_withdraw_data(&withdraw_tx).expect("withdraw data");
    let key = crate::withdraw_event_reconciliation_key(&withdraw_arg.outputs).expect("recon key");
    // Producing the withdraw counts it as unseen, awaiting its L1 event.
    assert!(
        seq_a
            .block_store()
            .dbio()
            .consume_unseen_withdraw_count(key)
            .unwrap(),
        "producing a withdraw must count it as unseen"
    );

    let messages = channel_from_store(seq_a.block_store(), 10);
    let tip_slot = messages.last().unwrap().1;
    let channel_id = config_a.bedrock_config.channel_id;

    // Sequencer B reconstructs A's chain from a fresh store.
    let config_b = bridge_funded_config();
    let (mut store_b, mut state_b) =
        SequencerCore::<MockBlockPublisher>::open_or_create_store(&config_b);
    let mock_b = MockBlockPublisher::with_canned_channel(channel_id, Some(tip_slot), messages);
    SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock_b,
        &mut store_b,
        &mut state_b,
        true,
    )
    .await
    .expect("reconstruct");

    assert!(
        !store_b.dbio().consume_unseen_withdraw_count(key).unwrap(),
        "reconstruction must not leave a phantom unseen-withdraw count"
    );
}

/// A deposit whose L1 event was observed (an unfulfilled pending record
/// exists) and whose L2 mint is already contained in a finalized channel block.
/// Reconstruction must reconcile the pending record against that block — marking
/// it submitted so the startup replay does not re-inject it — and apply the mint
/// exactly once.
#[tokio::test]
async fn reconstruction_reconciles_already_finished_deposit() {
    let recipient = initial_public_user_accounts()[0].account_id;
    let deposit_amount = 400_u64;
    let deposit_op_id = [0x1a_u8; 32];

    // Sequencer A: a single block that fully processes the bridge deposit.
    let config_a = bridge_funded_config();
    let (mut seq_a, mempool_a) =
        SequencerCoreWithMockClients::start_from_config(config_a.clone()).await;
    let deposit_record = deposit_event_record(deposit_op_id, deposit_amount, recipient);
    let deposit_tx =
        crate::build_bridge_deposit_tx_from_event(&deposit_record).expect("build deposit tx");
    mempool_a
        .push((TransactionOrigin::Sequencer, deposit_tx))
        .await
        .unwrap();
    seq_a.produce_new_block().await.unwrap();
    let deposit_block_id = seq_a.block_store().latest_block_meta().unwrap().unwrap().id;

    let messages = channel_from_store(seq_a.block_store(), 10);
    let tip_slot = messages.last().unwrap().1;
    let channel_id = config_a.bedrock_config.channel_id;

    // Sequencer B: fresh store, but with the *unfulfilled* pending deposit event
    // pre-seeded, as the cold-start backfill would when it re-observes this
    // already-finalized deposit.
    let config_b = bridge_funded_config();
    let (mut store_b, mut state_b) =
        SequencerCore::<MockBlockPublisher>::open_or_create_store(&config_b);
    assert!(
        store_b
            .dbio()
            .add_pending_deposit_event(deposit_record.clone())
            .unwrap()
    );

    let mock_b = MockBlockPublisher::with_canned_channel(channel_id, Some(tip_slot), messages);
    SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock_b,
        &mut store_b,
        &mut state_b,
        true,
    )
    .await
    .expect("reconstruct");

    // The mint was applied exactly once.
    let vault_id = vault_core::compute_vault_account_id(programs::vault().id(), recipient);
    assert_eq!(
        state_b.get_account_by_id(vault_id).balance,
        u128::from(deposit_amount),
        "already-finished deposit must be applied exactly once"
    );

    // The pending event is now marked submitted in the reconstructed block, so the
    // startup replay would not re-queue it — no double mint on restart.
    let record = store_b
        .get_unfulfilled_deposit_events()
        .unwrap()
        .into_iter()
        .find(|event| event.deposit_op_id == HashType(deposit_op_id))
        .expect("pending deposit event should still be recorded");
    assert_eq!(
        record.submitted_in_block_id,
        Some(deposit_block_id),
        "reconstruction must reconcile the already-finished deposit against its channel block"
    );
}

#[tokio::test]
async fn committed_local_against_missing_channel_fails_without_anchor() {
    // A sequencer that has committed blocks — a non-genesis tip plus a persisted
    // checkpoint — but only ever produced (so it never recorded a per-block
    // anchor). Restarting it against a wiped/missing channel must still fail,
    // driven by the committed-blocks invariant rather than an anchor probe.
    let config = setup_sequencer_config();
    {
        let (mut seq, _handle) =
            SequencerCoreWithMockClients::start_from_config(config.clone()).await;
        seq.produce_new_block().await.unwrap();
        seq.produce_new_block().await.unwrap();
        assert!(seq.block_store().latest_block_meta().unwrap().unwrap().id > 1);
    } // drop releases the store so we can reopen it

    // Reopen: blocks beyond genesis, no anchor. `is_fresh_start = false` stands in
    // for a checkpoint persisted by a prior sync (the mock never emits one).
    let (mut store, mut state) = SequencerCore::<MockBlockPublisher>::open_or_create_store(&config);
    assert!(store.get_zone_anchor().unwrap().is_none());
    assert!(store.latest_block_meta().unwrap().unwrap().id > 1);

    // The channel is gone: no tip, no messages.
    let mock =
        MockBlockPublisher::with_canned_channel(config.bedrock_config.channel_id, None, vec![]);
    let result = SequencerCore::<MockBlockPublisher>::verify_and_reconstruct(
        &mock, &mut store, &mut state, false,
    )
    .await;
    assert!(
        result.is_err(),
        "committed blocks against a missing channel must abort startup"
    );
}
