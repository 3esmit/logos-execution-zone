use common::test_utils::produce_dummy_block;
use lee::{Account, AccountId};
use tempfile::tempdir;

use super::*;

fn marker_id() -> AccountId {
    AccountId::new([1; 32])
}

/// A state distinguishable by the marker account's balance, so tests can tell
/// which snapshot a write persisted.
///
/// TODO: is this a bit too much of a hot-fix for test snapshot?
fn state_with_balance(balance: u128) -> V03State {
    V03State::new().with_public_accounts([(
        marker_id(),
        Account {
            balance,
            ..Account::default()
        },
    )])
}

fn dbio_with_genesis(path: &Path) -> (RocksDBIO, Block) {
    let genesis = produce_dummy_block(1, None, vec![]);
    let dbio = RocksDBIO::create(path, &genesis, &state_with_balance(100)).unwrap();
    (dbio, genesis)
}

fn stored_balance(dbio: &RocksDBIO) -> u128 {
    dbio.get_lee_state()
        .unwrap()
        .get_account_by_id(marker_id())
        .balance
}

#[test]
fn store_followed_block_persists_new_block_and_state() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2, &state_with_balance(200), false)
        .unwrap();

    let stored = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert_eq!(stored.header.hash, block2.header.hash);
    assert!(matches!(stored.bedrock_status, BedrockStatus::Pending));
    assert_eq!(
        dbio.latest_block_meta().unwrap().expect("meta is set").id,
        2
    );
    assert_eq!(stored_balance(&dbio), 200);
}

#[test]
fn store_followed_block_finalized_marks_block() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2, &state_with_balance(200), true)
        .unwrap();

    let stored = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert!(matches!(stored.bedrock_status, BedrockStatus::Finalized));
}

#[test]
fn store_followed_block_redelivery_is_a_noop_and_keeps_finalized() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2, &state_with_balance(200), true)
        .unwrap();
    dbio.store_followed_block(&block2, &state_with_balance(300), false)
        .unwrap();

    let stored = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert!(
        matches!(stored.bedrock_status, BedrockStatus::Finalized),
        "re-delivery must not demote a finalized block"
    );
    assert_eq!(
        stored_balance(&dbio),
        200,
        "re-delivery must not overwrite the persisted state"
    );
}

#[test]
fn store_followed_blocks_batch_lands_meta_and_state_on_last_block() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    // Block 2 is already stored (own production); one update then finalizes it
    // and adopts block 3.
    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2, &state_with_balance(200), false)
        .unwrap();

    let block3 = produce_dummy_block(3, Some(block2.header.hash), vec![]);
    let head_tip = BlockMeta {
        id: 3,
        hash: block3.header.hash,
    };
    dbio.store_followed_blocks(
        &[(&block2, true), (&block3, false)],
        Some(&head_tip),
        &state_with_balance(300),
        None,
    )
    .unwrap();

    let stored2 = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert!(matches!(stored2.bedrock_status, BedrockStatus::Finalized));
    let stored3 = dbio.get_block(3).unwrap().expect("block 3 is stored");
    assert!(matches!(stored3.bedrock_status, BedrockStatus::Pending));

    // Meta and state land together on the last block of the batch.
    let meta = dbio.latest_block_meta().unwrap().expect("meta is set");
    assert_eq!(meta.id, 3);
    assert_eq!(meta.hash, block3.header.hash);
    assert_eq!(stored_balance(&dbio), 300);
}

#[test]
fn final_snapshot_round_trips_and_is_absent_on_fresh_store() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    // Fresh store: no finalization observed yet.
    assert!(dbio.get_final_snapshot().unwrap().is_none());

    // A follow update that finalizes block 2 lands the snapshot in the same batch.
    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    let final_meta = BlockMeta {
        id: 2,
        hash: block2.header.hash,
    };
    dbio.store_followed_blocks(
        &[(&block2, true)],
        Some(&final_meta),
        &state_with_balance(300),
        Some((&state_with_balance(200), &final_meta)),
    )
    .unwrap();

    let (final_state, meta) = dbio
        .get_final_snapshot()
        .unwrap()
        .expect("final snapshot is stored");
    assert_eq!(meta.id, 2);
    assert_eq!(meta.hash, block2.header.hash);
    assert_eq!(final_state.get_account_by_id(marker_id()).balance, 200);
    // The head state is stored independently of the final snapshot.
    assert_eq!(stored_balance(&dbio), 300);
}

#[test]
fn store_followed_block_overwrites_competing_block_at_same_id() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2a = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2a, &state_with_balance(200), false)
        .unwrap();

    // A reorg replaces block 2: the competing block wins the slot.
    let block2b = produce_dummy_block(2, Some(HashType([9; 32])), vec![]);
    dbio.store_followed_block(&block2b, &state_with_balance(300), false)
        .unwrap();

    let stored = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert_eq!(stored.header.hash, block2b.header.hash);
    assert!(matches!(stored.bedrock_status, BedrockStatus::Pending));
    assert_eq!(stored_balance(&dbio), 300);

    // The tip meta must follow the reorg winner, or a restart seeds the chain
    // from the orphaned block's hash.
    let meta = dbio.latest_block_meta().unwrap().expect("meta is set");
    assert_eq!(meta.id, 2);
    assert_eq!(meta.hash, block2b.header.hash);
}

#[test]
fn net_shortening_reorg_drops_stale_blocks() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2a = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2a, &state_with_balance(200), false)
        .unwrap();
    let block3 = produce_dummy_block(3, Some(block2a.header.hash), vec![]);
    dbio.store_followed_block(&block3, &state_with_balance(300), false)
        .unwrap();

    // A shorter competing chain wins: block 2 is replaced, block 3 gets no
    // replacement.
    let block2b = produce_dummy_block(2, Some(HashType([9; 32])), vec![]);
    let head_tip = BlockMeta {
        id: 2,
        hash: block2b.header.hash,
    };
    dbio.store_followed_blocks(
        &[(&block2b, false)],
        Some(&head_tip),
        &state_with_balance(400),
        None,
    )
    .unwrap();

    let stored2 = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert_eq!(stored2.header.hash, block2b.header.hash);
    assert!(
        dbio.get_block(3).unwrap().is_none(),
        "stale block above the new head must be deleted, or restart replay panics on its broken link"
    );
    let meta = dbio.latest_block_meta().unwrap().expect("meta is set");
    assert_eq!(meta.id, 2);
    assert_eq!(meta.hash, block2b.header.hash);
    assert_eq!(stored_balance(&dbio), 400);
}

#[test]
fn shrink_only_reorg_rewinds_tip_meta() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2 = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2, &state_with_balance(200), false)
        .unwrap();
    let block3 = produce_dummy_block(3, Some(block2.header.hash), vec![]);
    dbio.store_followed_block(&block3, &state_with_balance(300), false)
        .unwrap();

    // Orphan-only update: block 3 falls off the branch with no replacement.
    let head_tip = BlockMeta {
        id: 2,
        hash: block2.header.hash,
    };
    dbio.store_followed_blocks(&[], Some(&head_tip), &state_with_balance(200), None)
        .unwrap();

    assert!(
        dbio.get_block(3).unwrap().is_none(),
        "the orphaned block must not survive the tip rewind"
    );
    let meta = dbio.latest_block_meta().unwrap().expect("meta is set");
    assert_eq!(meta.id, 2);
    assert_eq!(meta.hash, block2.header.hash);
    assert_eq!(stored_balance(&dbio), 200);
}

#[test]
fn produced_block_below_disk_head_pins_meta_and_prunes() {
    let temp_dir = tempdir().unwrap();
    let (dbio, genesis) = dbio_with_genesis(temp_dir.path());

    let block2a = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.store_followed_block(&block2a, &state_with_balance(200), false)
        .unwrap();
    let block3 = produce_dummy_block(3, Some(block2a.header.hash), vec![]);
    dbio.store_followed_block(&block3, &state_with_balance(300), false)
        .unwrap();

    // Producing at height 2 while the disk head is still 3: the produce path
    // pins the tip meta to the produced block and drops the stale suffix in
    // the same write, mirroring the follow path.
    let block2b = produce_dummy_block(2, Some(genesis.header.hash), vec![]);
    dbio.atomic_update(&block2b, &[], vec![], &state_with_balance(400))
        .unwrap();

    let stored2 = dbio.get_block(2).unwrap().expect("block 2 is stored");
    assert_eq!(stored2.header.hash, block2b.header.hash);
    assert!(dbio.get_block(3).unwrap().is_none());
    let meta = dbio.latest_block_meta().unwrap().expect("meta is set");
    assert_eq!(meta.id, 2);
    assert_eq!(meta.hash, block2b.header.hash);
    assert_eq!(stored_balance(&dbio), 400);
}
