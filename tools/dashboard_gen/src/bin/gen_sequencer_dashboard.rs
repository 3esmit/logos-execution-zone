//! Generates the sequencer dashboard and prints it to stdout.

#![expect(
    clippy::print_stdout,
    reason = "CLI tool: emitting the dashboard JSON on stdout is the deliverable"
)]
#![expect(
    clippy::non_ascii_literal,
    reason = "legend separators use `·` intentionally, matching the rendered Grafana labels"
)]

use dashboard_gen::{
    Color, Dashboard, FieldOverride, Panel, Target, Thresholds, Unit, avg, percentiles,
    percentiles_labeled, rate_per_min,
};
use json_pretty_compact::PrettyCompactFormatter;
use serde::Serialize as _;

const PERCENTILES: &[u32] = &[50, 90, 95, 99];

fn sequencer_dashboard() -> Dashboard {
    Dashboard::new("Sequencer", "sequencer")
        .tag("sequencer")
        .row(
            7,
            [
                Panel::stat("Chain height")
                    .width(6)
                    .unit(Unit::Short)
                    .decimals(0)
                    .color(Color::fixed("blue"))
                    .target(
                        Target::new(sequencer_core_metrics::names::BLOCK_COUNT).legend("height"),
                    ),
                Panel::timeseries("Block production rate")
                    .width(18)
                    .unit(Unit::Short)
                    .target(rate_per_min(
                        sequencer_core_metrics::names::BLOCK_COUNT,
                        "blocks/min",
                    )),
            ],
        )
        .row(
            9,
            [Panel::timeseries("Block creation time")
                .width(24)
                .unit(Unit::Seconds)
                .targets(percentiles(
                    sequencer_core_metrics::names::BLOCK_CREATION_TIME,
                    PERCENTILES,
                ))
                .target(avg(sequencer_core_metrics::names::BLOCK_CREATION_TIME))
                .with_override(
                    FieldOverride::by_name("avg")
                        .dashed_line()
                        .color(Color::fixed("text")),
                )],
        )
        .row(
            9,
            [
                Panel::timeseries("Transaction application time")
                    .width(12)
                    .unit(Unit::Seconds)
                    .targets(percentiles_labeled(
                        sequencer_core_metrics::names::MEMPOOL_TRANSACTION_APPLICATION_TIME,
                        PERCENTILES,
                        " · {{kind}} · {{origin}}",
                    )),
                Panel::timeseries("Transactions per block")
                    .width(12)
                    .unit(Unit::Short)
                    .targets(percentiles(
                        sequencer_core_metrics::names::TRANSACTIONS_PER_BLOCK,
                        PERCENTILES,
                    ))
                    .target(avg(sequencer_core_metrics::names::TRANSACTIONS_PER_BLOCK))
                    .with_override(
                        FieldOverride::by_name("avg")
                            .dashed_line()
                            .color(Color::fixed("text")),
                    ),
            ],
        )
        .row(
            8,
            [
                Panel::gauge("Mempool utilization")
                    .width(6)
                    .unit(Unit::Percent)
                    .decimals(1)
                    .min(0.0)
                    .max(100.0)
                    .thresholds(
                        Thresholds::base("green")
                            .step(70.0, "orange")
                            .step(90.0, "red"),
                    )
                    .target(
                        Target::new(format!(
                            "100 * {size} / {max_size}",
                            size = sequencer_core_metrics::names::MEMPOOL_SIZE,
                            max_size = sequencer_core_metrics::names::MEMPOOL_MAX_SIZE,
                        ))
                        .legend("utilization"),
                    ),
                Panel::timeseries("Mempool size vs capacity")
                    .width(18)
                    .unit(Unit::Short)
                    .span_nulls()
                    .min(0.0)
                    .target(
                        Target::new(sequencer_core_metrics::names::MEMPOOL_SIZE).legend("queued"),
                    )
                    .target(
                        Target::new(sequencer_core_metrics::names::MEMPOOL_MAX_SIZE)
                            .legend("capacity"),
                    )
                    .with_override(
                        FieldOverride::by_name("capacity")
                            .dashed_line()
                            .color(Color::fixed("red")),
                    )
                    .with_override(FieldOverride::by_name("queued").color(Color::fixed("blue"))),
            ],
        )
        .row(
            8,
            [
                Panel::gauge("Failed transactions share")
                    .width(6)
                    .unit(Unit::Percent)
                    .decimals(2)
                    .min(0.0)
                    .max(100.0)
                    .thresholds(
                        Thresholds::base("green")
                            .step(1.0, "orange")
                            .step(5.0, "red"),
                    )
                    .target(
                        Target::new(format!(
                            // `clamp_min` keeps an idle window (nothing submitted)
                            // reading as 0% instead of a division by zero.
                            "100 * increase({failed}[$__range]) / clamp_min(increase({submitted}[$__range]), 1)",
                            failed = sequencer_core_metrics::names::FAILED_TRANSACTION_COUNT,
                            submitted = sequencer_service_metrics::names::SUBMITTED_TRANSACTION_COUNT,
                        ))
                        .legend("failed"),
                    ),
                Panel::timeseries("Submitted vs failed transactions (per minute)")
                    .width(18)
                    .unit(Unit::Short)
                    .min(0.0)
                    .target(rate_per_min(
                        sequencer_service_metrics::names::SUBMITTED_TRANSACTION_COUNT,
                        "submitted",
                    ))
                    .target(rate_per_min(
                        sequencer_core_metrics::names::FAILED_TRANSACTION_COUNT,
                        "failed",
                    ))
                    .with_override(FieldOverride::by_name("failed").color(Color::fixed("red")))
                    .with_override(
                        FieldOverride::by_name("submitted").color(Color::fixed("green")),
                    ),
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
