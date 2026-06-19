use std::ffi::c_void;

use indexer_core::IndexerCore;
use tokio::{runtime::Handle, task::JoinHandle};

/// FFI-owned indexer.
///
/// Has three fields behind `c_void` (so that cbindgen never needs to see their Rust layout):
/// - An [`IndexerCore`] used to answer queries
/// - The background task [`JoinHandle`] that drives ingestion (consuming the block stream so the
///   store stays populated)
/// - A [`Handle`] to the runtime they live on.
#[repr(C)]
pub struct IndexerServiceFFI {
    core: *mut c_void,
    ingest_handle: *mut c_void,
    runtime_handle: *mut c_void,
}

impl IndexerServiceFFI {
    #[must_use]
    pub fn new(core: IndexerCore, ingest_handle: JoinHandle<()>, runtime_handle: Handle) -> Self {
        Self {
            core: Box::into_raw(Box::new(core)).cast::<c_void>(),
            ingest_handle: Box::into_raw(Box::new(ingest_handle)).cast::<c_void>(),
            runtime_handle: Box::into_raw(Box::new(runtime_handle)).cast::<c_void>(),
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

    /// Borrow the runtime handle to `block_on` an async store query.
    #[must_use]
    pub const fn runtime_handle(&self) -> &Handle {
        unsafe {
            self.runtime_handle
                .cast::<Handle>()
                .as_ref()
                .expect("Runtime handle must be a non-null pointer")
        }
    }
}

// Implement Drop to stop ingestion and free the boxed resources.
impl Drop for IndexerServiceFFI {
    fn drop(&mut self) {
        let Self {
            core,
            ingest_handle,
            runtime_handle,
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
        if !runtime_handle.is_null() {
            drop(unsafe { Box::from_raw(runtime_handle.cast::<Handle>()) });
        }
    }
}
