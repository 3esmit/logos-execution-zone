/// Initializes the FFI's logger.
///
/// Wires up `env_logger`, so the library's `log::*` output is controlled by the
/// `RUST_LOG` environment variable (e.g. `RUST_LOG=info`). Without this, the
/// FFI's log calls go nowhere — and since failures are otherwise reported only
/// as numeric [`OperationStatus`](crate::errors::OperationStatus) codes, there
/// is no other way to see *why* a call failed.
///
/// Safe to call multiple times and from any consumer: if a global logger is
/// already set, the call is a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn init_logger() {
    let _ignore_me = env_logger::try_init();
}
