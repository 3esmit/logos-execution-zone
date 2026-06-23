use serde::Serialize;

/// Coarse lifecycle state of the indexer's ingestion loop, so a client can tell
/// "still catching up" apart from "something went wrong".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexerSyncState {
    /// Booted; no ingestion cycle has run yet.
    Starting,
    /// Streaming finalized messages toward the L1 frontier.
    Syncing,
    /// Drained the stream up to LIB; idle until new blocks finalize.
    CaughtUp,
    /// The last cycle failed (e.g. the L1 node is unreachable). See `last_error`.
    Error,
}

/// Live ingestion status owned by the ingest loop: the coarse `state` plus the
/// reason when it is `Error`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerSyncStatus {
    pub state: IndexerSyncState,
    pub last_error: Option<String>,
}

impl IndexerSyncStatus {
    /// Initial status before any ingestion cycle has run.
    pub(crate) const fn starting() -> Self {
        Self {
            state: IndexerSyncState::Starting,
            last_error: None,
        }
    }

    /// Actively streaming finalized messages toward the L1 frontier.
    pub(crate) const fn syncing() -> Self {
        Self {
            state: IndexerSyncState::Syncing,
            last_error: None,
        }
    }

    /// Drained the stream up to LIB; idle until new blocks finalize.
    pub(crate) const fn caught_up() -> Self {
        Self {
            state: IndexerSyncState::CaughtUp,
            last_error: None,
        }
    }

    /// The last cycle failed; `reason` explains why.
    pub(crate) const fn error(reason: String) -> Self {
        Self {
            state: IndexerSyncState::Error,
            last_error: Some(reason),
        }
    }
}

/// Full status snapshot returned to callers (FFI/RPC): the live [`SyncStatus`]
/// plus the L2 tip (`indexed_block_id`) read fresh from the store at query time.
///
/// The tip is tracked by the store, not the ingest loop, so it lives here on the
/// returned snapshot rather than inside the shared [`SyncStatus`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerStatus {
    #[serde(flatten)]
    pub sync: IndexerSyncStatus,
    pub indexed_block_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexer_status_serializes_to_flat_object() {
        let status = IndexerStatus {
            sync: IndexerSyncStatus::error("boom".to_owned()),
            indexed_block_id: Some(7),
        };
        let value = serde_json::to_value(&status).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "state": "error",
                "lastError": "boom",
                "indexedBlockId": 7,
            })
        );
    }

    #[test]
    fn caught_up_clears_error() {
        let value = serde_json::to_value(IndexerSyncStatus::caught_up()).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({ "state": "caught_up", "lastError": null })
        );
    }
}
