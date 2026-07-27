//! Reads a single Grafana panel JSON on stdin (Grafana → panel menu → Inspect →
//! Panel JSON) and prints the Rust `Panel::…` builder expression that rebuilds
//! it through `dashboard_gen`, omitting values Grafana supplies by default.
//!
//! Paste the result into a dashboard's `.row(…)` and run `cargo fmt`.

#![expect(
    clippy::print_stderr,
    reason = "CLI tool: diagnostics on stderr are the deliverable"
)]
#![expect(
    clippy::non_ascii_literal,
    reason = "help text mirrors Grafana's `Inspect → Panel JSON` menu path"
)]

use std::{
    io::{self, Read as _, Write as _},
    process::ExitCode,
};

fn main() -> ExitCode {
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        eprintln!("error: failed to read panel JSON from stdin: {err}");
        return ExitCode::FAILURE;
    }

    match dashboard_gen::panel_to_rust_source(&input) {
        Ok(source) => {
            if let Err(err) = io::stdout().write_all(source.as_bytes()) {
                eprintln!("error: failed to write generated source to stdout: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: could not parse panel JSON: {err}");
            eprintln!(
                "hint: paste one panel (Inspect → Panel JSON);\
                 only stat and timeseries are supported."
            );
            eprintln!(
                "hint: this tool supports only the subset of Grafana's panel JSON, you might need \
                 to manually implement support for new fields."
            );

            ExitCode::FAILURE
        }
    }
}
