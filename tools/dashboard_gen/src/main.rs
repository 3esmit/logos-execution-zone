//! Generates the sequencer dashboard and prints it to stdout.
//!
//! Compare against the committed file:
//!   cargo run -p dashboard_gen | diff - monitoring/grafana/dashboards/sequencer.json
//! Regenerate it:
//!   cargo run -p dashboard_gen > monitoring/grafana/dashboards/sequencer.json

use dashboard_gen::{
    Dashboard, FieldOverride, Panel, Target, avg, percentiles, percentiles_labeled, rate_per_min,
};
use json_pretty_compact::PrettyCompactFormatter;
use serde::Serialize as _;

// Rendered Prometheus metric names. In a real setup these would be re-exported
// from `sequencer_core::metrics` (base name + unit/counter suffix rule) so that
// a rename in the recorder is a compile error here — that is the whole point of
// generating dashboards from the same codebase that emits the metrics.
const BLOCK_COUNT: &str = "block_count_total";
const BLOCK_CREATION_TIME: &str = "block_creation_time_seconds";
const TX_APPLY_TIME: &str = "mempool_transaction_application_time_seconds";
const MEMPOOL_SIZE: &str = "mempool_size";
const TX_PER_BLOCK: &str = "transactions_per_block";
const SUBMITTED_TX: &str = "submitted_transaction_count_total";
const FAILED_TX: &str = "failed_transaction_count_total";

const PERCENTILES: &[u32] = &[50, 90, 95, 99];

fn sequencer_dashboard() -> Dashboard {
    Dashboard::new("Sequencer", "sequencer")
        .tag("sequencer")
        .row(
            7,
            [
                Panel::stat("Chain height")
                    .width(6)
                    .unit("short")
                    .decimals(0)
                    .fixed_color("blue")
                    .target(Target::new(BLOCK_COUNT).legend("height")),
                Panel::timeseries("Block production rate")
                    .width(18)
                    .unit("short")
                    .target(rate_per_min(BLOCK_COUNT, "blocks/min")),
            ],
        )
        .row(
            9,
            [Panel::timeseries("Block creation time")
                .width(24)
                .unit("s")
                .targets(percentiles(BLOCK_CREATION_TIME, PERCENTILES))
                .target(avg(BLOCK_CREATION_TIME))
                .with_override(
                    FieldOverride::by_name("avg")
                        .dashed_line()
                        .fixed_color("text"),
                )],
        )
        .row(
            9,
            [
                Panel::timeseries("Transaction application time")
                    .width(12)
                    .unit("s")
                    .targets(percentiles_labeled(
                        TX_APPLY_TIME,
                        PERCENTILES,
                        " · {{kind}} · {{origin}}",
                    )),
                Panel::timeseries("Mempool size")
                    .width(12)
                    .unit("short")
                    .span_nulls()
                    .target(Target::new(MEMPOOL_SIZE).legend("mempool size")),
            ],
        )
        .row(
            9,
            [
                Panel::timeseries("Transactions per block")
                    .width(12)
                    .unit("short")
                    .targets(percentiles(TX_PER_BLOCK, PERCENTILES))
                    .target(avg(TX_PER_BLOCK))
                    .with_override(
                        FieldOverride::by_name("avg")
                            .dashed_line()
                            .fixed_color("text"),
                    ),
                Panel::timeseries("Transaction throughput (per minute)")
                    .width(12)
                    .unit("short")
                    .target(rate_per_min(SUBMITTED_TX, "submitted"))
                    .target(rate_per_min(FAILED_TX, "failed"))
                    .with_override(FieldOverride::by_name("failed").fixed_color("red"))
                    .with_override(FieldOverride::by_name("submitted").fixed_color("green")),
            ],
        )
}

fn main() {
    let dashboard = sequencer_dashboard();

    let formatter = PrettyCompactFormatter::new();
    let mut output = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut output, formatter);
    dashboard.serialize(&mut ser).unwrap();

    let json = String::from_utf8(output).unwrap();
    println!("{json}");
}
