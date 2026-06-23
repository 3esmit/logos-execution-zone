use std::ffi::c_void;

use indexer_core::IndexerCore;
use tokio::task::JoinHandle;

use crate::Runtime;

/// FFI-owned indexer.
///
/// - An [`IndexerCore`] used to answer queries
/// - The background task [`JoinHandle`] that drives ingestion (consuming the block stream so the
///   store stays populated)
/// - The [`Runtime`] used to run async queries against the store (either owned or borrowed),
///   already FFI-safe.
#[repr(C)]
pub struct IndexerServiceFFI {
    core: *mut c_void,
    ingest_handle: *mut c_void,
    runtime: Runtime,
}

impl IndexerServiceFFI {
    #[must_use]
    pub fn new(core: IndexerCore, ingest_handle: JoinHandle<()>, runtime: Runtime) -> Self {
        Self {
            core: Box::into_raw(Box::new(core)).cast::<c_void>(),
            ingest_handle: Box::into_raw(Box::new(ingest_handle)).cast::<c_void>(),
            runtime,
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
        &self.runtime
    }
}

impl Drop for IndexerServiceFFI {
    fn drop(&mut self) {
        if !self.ingest_handle.is_null() {
            let handle = unsafe { Box::from_raw(self.ingest_handle.cast::<JoinHandle<()>>()) };
            // stop the background ingestion task before tearing down the core.
            handle.abort();
            drop(handle);
        }
        if !self.core.is_null() {
            drop(unsafe { Box::from_raw(self.core.cast::<IndexerCore>()) });
        }

        // `runtime` field is dropped automatically on return here:
        // - if runtime was owned, it is shutdown at this point
        // - if it was borrowed, it continues to live within the external owner
    }
}
