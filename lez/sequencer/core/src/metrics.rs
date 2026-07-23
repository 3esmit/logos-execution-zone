//! This module provides all metrics exposed by the sequencer core crate.

#![expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "It's okay for metrics"
)]

use std::time::Duration;

use common::transaction::TxKind;
use metrics::{Counter, Unit, counter, gauge, histogram};

use crate::TransactionOrigin;

mod names {
    pub const BLOCK_CREATION_TIME: &str = "block_creation_time";
    pub const BLOCK_COUNT: &str = "block_count";
    pub const MEMPOOL_SIZE: &str = "mempool_size";
    pub const MEMPOOL_TRANSACTION_APPLICATION_TIME: &str = "mempool_transaction_application_time";
    pub const TRANSACTIONS_PER_BLOCK: &str = "transactions_per_block";
    pub const FAILED_TRANSACTION_COUNT: &str = "failed_transaction_count";
}

pub fn record_block_creation_time(duration: Duration) {
    histogram!(
        description: "Time taken to create a block",
        unit: Unit::Seconds,
        names::BLOCK_CREATION_TIME
    )
    .record(duration.as_secs_f64());
}

fn block_count_counter() -> Counter {
    counter!(
        description: "Number of blocks in chain",
        unit: Unit::Count,
        names::BLOCK_COUNT
    )
}

pub fn set_block_count(value: u64) {
    block_count_counter().absolute(value);
}

pub fn increment_block_count() {
    block_count_counter().increment(1);
}

pub fn record_mempool_size(size: usize) {
    gauge!(
        description: "Size of the mempool",
        unit: Unit::Count,
        names::MEMPOOL_SIZE
    )
    .set(u64::try_from(size).expect("Mempool size should fit into u64") as f64);
}

pub fn record_mempool_transaction_application_time(
    origin: TransactionOrigin,
    kind: TxKind,
    duration: Duration,
) {
    histogram!(
        description: "Time taken to apply a mempool transaction",
        unit: Unit::Seconds,
        names::MEMPOOL_TRANSACTION_APPLICATION_TIME,
        "origin" => <&'static str>::from(origin),
        "kind" => <&'static str>::from(kind),
    )
    .record(duration.as_secs_f64());
}

pub fn record_transactions_per_block(count: usize) {
    histogram!(
        description: "Number of transactions included in block",
        unit: Unit::Count,
        names::TRANSACTIONS_PER_BLOCK
    )
    .record(u64::try_from(count).expect("Block transaction count should fit into u64") as f64);
}

pub fn increment_failed_transaction_count() {
    counter!(
        description: "Number of transactions that failed to be included in blocks",
        unit: Unit::Count,
        names::FAILED_TRANSACTION_COUNT
    )
    .increment(1);
}
