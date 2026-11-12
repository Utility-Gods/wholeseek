//! Events the core sends to the UI. One channel, in order; the UI drains it
//! each frame and never blocks on the core.

use crate::model::{
    BrowseDir, RemoteFile, SearchHit, SearchId, TransferId, TransferKind, UploadRow,
};
use std::path::PathBuf;

/// Why the session is not (or no longer) connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Server rejected the credentials. The library does not say why
    /// (gap G7), so this carries no detail. No auto-reconnect.
    LoginRejected,
    /// Another client logged in with this account. No auto-reconnect.
    Displaced,
    /// Network/server dropped; the core retries with backoff.
    Network(String),
    /// [`crate::Core::logout`] or shutdown.
    User,
}

/// Outcome of automatic port mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortMapOutcome {
    /// `method` is `"upnp"` or `"natpmp"`; `external` is `ip:port` when the
    /// router reports it.
    Mapped {
        method: String,
        external: Option<String>,
    },
    /// No UPnP/NAT-PMP router mapped the port; human-readable advice.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    /// Logged in. `listen_port` is the port actually bound (the library falls
    /// back to an ephemeral port if the configured one is taken).
    Connected {
        username: String,
        listen_port: Option<u16>,
    },
    Disconnected {
        reason: DisconnectReason,
        will_retry: bool,
    },

    /// New hits for a search since the previous batch, already filtered and
    /// ranked by the core (audio-only, FLAC first). Batches arrive while the
    /// search window is open; the UI merges and re-sorts on append.
    SearchResults {
        search: SearchId,
        hits: Vec<SearchHit>,
    },
    /// The collection window closed; no more batches for this id.
    SearchFinished {
        search: SearchId,
        total_files: usize,
    },

    /// A browsed user's full share tree.
    BrowseResult {
        username: String,
        dirs: Vec<BrowseDir>,
    },
    /// One folder's listing (used to expand a folder download from a search hit).
    FolderContents {
        username: String,
        folder: String,
        dirs: Vec<BrowseDir>,
    },
    BrowseFailed {
        username: String,
        reason: String,
    },

    /// A transfer was accepted; paths are known before any byte arrives.
    TransferQueued {
        id: TransferId,
        kind: TransferKind,
        file: RemoteFile,
        part_path: PathBuf,
    },
    /// Remote peer queued us (no slot yet).
    TransferRemoteQueued {
        id: TransferId,
        place: Option<u32>,
    },
    /// Bytes on disk in `part_path` (the library reports every 128 KiB, plus
    /// once right after it opens the `.part`).
    TransferProgress {
        id: TransferId,
        bytes: u64,
        total: u64,
        speed: f64,
        part_path: PathBuf,
    },
    /// Complete and renamed to `path`.
    TransferDone {
        id: TransferId,
        path: PathBuf,
    },
    TransferFailed {
        id: TransferId,
        reason: String,
    },
    TransferCancelled {
        id: TransferId,
    },

    /// Shares were (re)indexed and announced to the server.
    ShareCounts {
        folders: u32,
        files: u32,
    },
    /// Full snapshot of uploads (active, finished this session, then queued in
    /// serve order). Sent when it changes, at most a few times a second.
    Uploads(Vec<UploadRow>),

    /// Result of automatic router port mapping for the bound listen port
    /// (sent once per session when `Settings::port_mapping` is on). Additive (CP1).
    PortMapping {
        port: u16,
        outcome: PortMapOutcome,
    },

    /// A command could not be carried out (bad state, I/O error), or a
    /// non-fatal problem the user should know about. `context` names the
    /// command or subsystem (`"search"`, `"keyring"`, `"config"`, …).
    Error {
        context: String,
        message: String,
    },
}
