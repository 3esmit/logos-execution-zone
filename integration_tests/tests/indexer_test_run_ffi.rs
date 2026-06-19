#![expect(
    clippy::tests_outside_test_module,
    clippy::undocumented_unsafe_blocks,
    reason = "We don't care about these in tests"
)]

use anyhow::Result;
use integration_tests::L2_TO_L1_TIMEOUT;
use log::info;

#[path = "indexer_ffi_helpers/mod.rs"]
mod indexer_ffi_helpers;

#[test]
fn indexer_test_run_ffi() -> Result<()> {
    // `_ctx` keeps the bedrock/sequencer harness (and its runtime) alive for the
    // duration of the test; the indexer was started on that runtime.
    let (_ctx, indexer_ffi, _indexer_dir) = indexer_ffi_helpers::setup()?;

    // RUN OBSERVATION
    std::thread::sleep(L2_TO_L1_TIMEOUT);

    let last_block_indexer_ffi_res =
        unsafe { indexer_ffi_helpers::query_last_block(&raw const indexer_ffi) };

    assert!(last_block_indexer_ffi_res.error.is_ok());
    assert!(last_block_indexer_ffi_res.is_some);

    let last_block_indexer_ffi = last_block_indexer_ffi_res.block_id;

    info!("Last block on indexer FFI now is {last_block_indexer_ffi}");

    assert!(last_block_indexer_ffi > 0);

    Ok(())
}
