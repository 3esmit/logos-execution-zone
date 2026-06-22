use serde::Serialize;

/// Coarse lifecycle state of the indexer's ingestion loop, so a client can tell
/// "still catching up" apart from "something went wrong".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// Booted; no ingestion cycle has run yet.
    Starting,
    /// Streaming finalized messages toward the L1 frontier.
    Syncing,
    /// Drained the stream up to LIB; idle until new blocks finalize.
    CaughtUp,
    /// The last cycle failed (e.g. the L1 node is unreachable). See `last_error`.
    Error,
}

/// Snapshot of the indexer's sync status.
///
/// `state` is the coarse phase and `last_error` carries the reason when it is
/// `Error`. `indexed_block_id` is the L2 tip, filled from the store at read time
/// (so it is always `None` in the value held behind the status mutex — see
/// `IndexerCore::status`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerStatus {
    pub state: SyncState,
    pub indexed_block_id: Option<u64>,
    pub last_error: Option<String>,
}

impl IndexerStatus {
    /// Initial status before any ingestion cycle has run.
    pub(crate) const fn starting() -> Self {
        Self {
            state: SyncState::Starting,
            indexed_block_id: None,
            last_error: None,
        }
    }
}
