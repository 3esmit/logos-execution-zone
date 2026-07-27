use metrics::{Unit, counter};

use crate::names;

pub fn increment_submitted_transaction_count() {
    counter!(
        description: "Number of transactions submitted",
        unit: Unit::Count,
        names::SUBMITTED_TRANSACTION_COUNT
    )
    .increment(1);
}
