use std::ffi::c_void;

use indexer_core::IndexerCore;
use tokio::task::JoinHandle;

use crate::Runtime;

/// FFI-owned indexer.
///
/// Has three fields behind `c_void` (so that cbindgen never needs to see their Rust layout):
/// - An [`IndexerCore`] used to answer queries
/// - The background task [`JoinHandle`] that drives ingestion (consuming the block stream so the
///   store stays populated)
/// - The [`Runtime`] they run on. It owns the underlying tokio runtime when we created it (and
///   drops it on teardown) and merely borrows it when the caller supplied one.
#[repr(C)]
pub struct IndexerServiceFFI {
    core: *mut c_void,
    ingest_handle: *mut c_void,
    runtime: *mut c_void,
}

impl IndexerServiceFFI {
    #[must_use]
    pub fn new(core: IndexerCore, ingest_handle: JoinHandle<()>, runtime: Runtime) -> Self {
        Self {
            core: Box::into_raw(Box::new(core)).cast::<c_void>(),
            ingest_handle: Box::into_raw(Box::new(ingest_handle)).cast::<c_void>(),
            runtime: Box::into_raw(Box::new(runtime)).cast::<c_void>(),
        }
    }

    /// Borrow the [`IndexerCore`] to run a query against its store.
    #[must_use]
    pub const fn core(&self) -> &IndexerCore {
        unsafe {
            self.core
                .cast::<IndexerCore>()
                .as_ref()
                .expect("IndexerCore must be a non-null pointer")
        }
    }

    /// Borrow the runtime to `block_on` an async store query.
    #[must_use]
    pub const fn runtime(&self) -> &Runtime {
        unsafe {
            self.runtime
                .cast::<Runtime>()
                .as_ref()
                .expect("Runtime must be a non-null pointer")
        }
    }
}

// Implement Drop to stop ingestion and free the boxed resources.
impl Drop for IndexerServiceFFI {
    fn drop(&mut self) {
        let Self {
            core,
            ingest_handle,
            runtime,
        } = self;

        if !ingest_handle.is_null() {
            // Stop the background ingestion task before tearing down the core.
            let handle = unsafe { Box::from_raw(ingest_handle.cast::<JoinHandle<()>>()) };
            handle.abort();
            drop(handle);
        }
        if !core.is_null() {
            drop(unsafe { Box::from_raw(core.cast::<IndexerCore>()) });
        }
        // Dropping the `Runtime` shuts down the tokio runtime only if we own it
        // (a borrowed one is left for its external owner). Done last, and from
        // the consumer thread, so it never drops from within a runtime worker.
        if !runtime.is_null() {
            drop(unsafe { Box::from_raw(runtime.cast::<Runtime>()) });
        }
    }
}
