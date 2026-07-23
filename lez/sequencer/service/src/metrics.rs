//! This module provides all metrics exposed by the sequencer service crate.

use metrics::{Unit, counter};

mod names {
    pub const SUBMITTED_TRANSACTION_COUNT: &str = "submitted_transaction_count";
}

pub fn increment_submitted_transaction_count() {
    counter!(
        description: "Number of transactions submitted",
        unit: Unit::Count,
        names::SUBMITTED_TRANSACTION_COUNT
    )
    .increment(1);
}
