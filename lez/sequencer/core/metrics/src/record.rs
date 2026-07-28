#![expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "It's okay for metrics"
)]

use std::time::Duration;

use common::transaction::TxKind;
use metrics::{Counter, Histogram, Unit, counter, gauge, histogram};
use strum::IntoEnumIterator as _;

use crate::names;

#[derive(Clone, Copy, strum::IntoStaticStr, strum::EnumIter)]
pub enum TransactionOrigin {
    User,
    Sequencer,
}

/// Initialize metrics.
pub fn init() {
    blocks_total_counter().increment(0);
    mempool_failed_transactions_total_counter().increment(0);
    record_mempool_size(0);

    drop(block_creation_time_histogram());
    drop(transactions_per_block_histogram());
    for origin in TransactionOrigin::iter() {
        for kind in TxKind::iter() {
            drop(mempool_transaction_application_time_histogram(origin, kind));
        }
    }
}

fn block_creation_time_histogram() -> Histogram {
    histogram!(
        description: "Time taken to create a block",
        unit: Unit::Seconds,
        names::BLOCK_CREATION_TIME
    )
}

pub fn record_block_creation_time(duration: Duration) {
    block_creation_time_histogram().record(duration.as_secs_f64());
}

fn blocks_total_counter() -> Counter {
    counter!(
        description: "Number of blocks in chain",
        unit: Unit::Count,
        names::BLOCKS_TOTAL
    )
}

pub fn set_blocks_total(value: u64) {
    blocks_total_counter().absolute(value);
}

pub fn increment_blocks_total() {
    blocks_total_counter().increment(1);
}

pub fn record_mempool_size(size: usize) {
    gauge!(
        description: "Size of the mempool",
        unit: Unit::Count,
        names::MEMPOOL_SIZE
    )
    .set(u64::try_from(size).expect("Mempool size should fit into u64") as f64);
}

pub fn record_mempool_max_size(size: usize) {
    gauge!(
        description: "Configured maximum size of the mempool",
        unit: Unit::Count,
        names::MEMPOOL_MAX_SIZE
    )
    .set(u64::try_from(size).expect("Mempool max size should fit into u64") as f64);
}

fn mempool_transaction_application_time_histogram(
    origin: TransactionOrigin,
    kind: TxKind,
) -> Histogram {
    histogram!(
        description: "Time taken to apply a mempool transaction",
        unit: Unit::Seconds,
        names::MEMPOOL_TRANSACTION_APPLICATION_TIME,
        "origin" => <&'static str>::from(origin),
        "kind" => <&'static str>::from(kind),
    )
}

pub fn record_mempool_transaction_application_time(
    origin: TransactionOrigin,
    kind: TxKind,
    duration: Duration,
) {
    mempool_transaction_application_time_histogram(origin, kind).record(duration.as_secs_f64());
}

fn transactions_per_block_histogram() -> Histogram {
    histogram!(
        description: "Number of transactions from mempool included in block",
        unit: Unit::Count,
        names::TRANSACTIONS_PER_BLOCK
    )
}

pub fn record_transactions_per_block(count: usize) {
    transactions_per_block_histogram()
        .record(u64::try_from(count).expect("Block transaction count should fit into u64") as f64);
}

fn mempool_failed_transactions_total_counter() -> Counter {
    counter!(
        description: "Number of transactions from mempool that failed to be included in blocks",
        unit: Unit::Count,
        names::MEMPOOL_FAILED_TRANSACTIONS_TOTAL
    )
}

pub fn increment_mempool_failed_transactions_total() {
    mempool_failed_transactions_total_counter().increment(1);
}
