//! A polled snapshot of core state the UI reads each frame with
//! [`crate::Core::status`]. It complements the event stream: events say what
//! happened, the snapshot says how things stand now (time left to a
//! reconnect or a search window, cache usage, the port really bound).
//!
//! Added instead of new `CoreEvent` variants, which would break exhaustive
//! matches in the app.

use crate::model::SearchId;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Everything in the snapshot. Updated by the worker every poll tick
/// (150 ms) and after each command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreStatus {
    /// Set while waiting to reconnect after a network drop or failed login
    /// attempt; `None` when online, connecting, or not retrying.
    pub reconnect: Option<ReconnectStatus>,
    pub listen: ListenStatus,
    /// Searches whose window is still open (newest last). A search leaves
    /// this list when its `SearchFinished` event is sent.
    pub searches: Vec<SearchProgress>,
    pub cache: CacheUsage,
    /// One entry per `Settings::share_dirs` entry, in that order.
    pub shares: Vec<ShareStatus>,
}

/// Counts for one shared directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareStatus {
    pub dir: PathBuf,
    /// Folders (the directory itself and every subfolder holding files, as
    /// the library counts them) and files found by the last scan; 0 until the
    /// first scan finishes or when the directory is missing/unreadable.
    pub folders: u32,
    pub files: u32,
    /// A scan or rescan including this directory is running; the counts are
    /// from the previous scan until it ends.
    pub scanning: bool,
}

/// The pending automatic reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectStatus {
    /// 1 for the first retry after a drop, 2 for the next, …
    pub attempt: u32,
    /// Delay chosen for this attempt (2, 5, 15, 30, then 60 s).
    pub delay: Duration,
    /// When the attempt starts.
    pub at: Instant,
}

impl ReconnectStatus {
    /// Whole seconds until the attempt, rounded up (0 once due).
    #[must_use]
    pub fn in_secs(&self) -> u64 {
        let left = self.at.saturating_duration_since(Instant::now());
        left.as_secs() + u64::from(left.subsec_nanos() > 0)
    }
}

/// The peer-listening port: what settings asked for and what was bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenStatus {
    /// `Settings::listen_port` used for the current/last session.
    pub requested: u16,
    /// Port actually bound (= `Connected { listen_port }`); `None` before the
    /// first login.
    pub bound: Option<u16>,
    /// Why `bound` differs from `requested`, if it does. The library falls
    /// back to an OS-chosen port only when the requested one is in use.
    pub note: Option<String>,
}

/// A running search, for "128 results from 34 users… still searching".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProgress {
    pub id: SearchId,
    pub query: String,
    /// Files delivered so far in `SearchResults` (after the audio filter).
    pub files: usize,
    /// Distinct users among them.
    pub users: usize,
    /// When the search window closes and `SearchFinished` is sent.
    pub ends_at: Instant,
}

impl SearchProgress {
    /// Time left in the search window.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.ends_at.saturating_duration_since(Instant::now())
    }
}

/// Play cache usage (`Settings::cache_dir`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheUsage {
    /// Bytes in `cache/t*` dirs (rescanned every ~2 s and after changes).
    pub used_bytes: u64,
    /// `Settings::cache_cap_bytes`.
    pub cap_bytes: u64,
    /// Part of `used_bytes` that eviction and [`crate::Core::clear_cache`]
    /// keep: the current play and plays still being fetched.
    pub in_use_bytes: u64,
}
