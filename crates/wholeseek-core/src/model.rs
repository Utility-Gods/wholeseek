//! Plain data the UI renders. Nothing here borrows from `soulseek_rs`, so the
//! UI crate never needs the protocol crate as a dependency.

use std::path::PathBuf;

/// Identifies one search issued through [`crate::Core::search`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SearchId(pub u64);

/// Identifies one transfer (play fetch or download) issued through [`crate::Core`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(pub u64);

/// Audio attributes a peer advertises for a file (Soulseek attribute codes
/// 0 bitrate, 1 duration, 2 VBR, 4 sample rate, 5 bit depth). Every field is
/// optional because peers send any subset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioAttrs {
    pub bitrate_kbps: Option<u32>,
    pub duration_secs: Option<u32>,
    pub vbr: Option<bool>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u32>,
}

impl AudioAttrs {
    /// Build from `(code, value)` pairs, as both search results and browse
    /// listings carry them.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let mut a = Self::default();
        for (code, value) in pairs {
            match code {
                0 => a.bitrate_kbps = Some(value),
                1 => a.duration_secs = Some(value),
                2 => a.vbr = Some(value != 0),
                4 => a.sample_rate_hz = Some(value),
                5 => a.bit_depth = Some(value),
                _ => {}
            }
        }
        a
    }
}

/// One file on a remote peer. `path` is the peer's full virtual path
/// (backslash-separated), exactly what the protocol needs to request it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub username: String,
    pub path: String,
    pub size: u64,
    pub attrs: AudioAttrs,
}

impl RemoteFile {
    /// Last path component (`01 Track.flac`).
    #[must_use]
    pub fn basename(&self) -> &str {
        crate::paths::remote_basename(&self.path)
    }

    /// Parent folder's virtual path (`Music\Album`), empty for a top-level file.
    #[must_use]
    pub fn folder(&self) -> &str {
        crate::paths::remote_parent(&self.path)
    }

    /// Lower-case extension without the dot (`flac`), empty if none.
    #[must_use]
    pub fn extension(&self) -> String {
        crate::rank::extension(&self.path)
    }
}

/// One search hit with the responding peer's slot/speed figures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub file: RemoteFile,
    /// Peer reports a free upload slot (no queue).
    pub free_slot: bool,
    /// Peer's advertised upload speed, bytes/second.
    pub speed: u32,
}

/// One folder of a browsed user's shares; `files` are direct children.
/// What is known about another user (header of a browse). Server parts
/// (presence, stats) and peer parts (description, slots, queue) arrive
/// separately; `None` = not (yet) known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserProfile {
    /// Server presence (away counts as online).
    pub online: Option<bool>,
    /// B/s, server stats.
    pub avg_speed: Option<u32>,
    pub shared_files: Option<u32>,
    pub shared_dirs: Option<u32>,
    /// Peer UserInfo reply.
    pub description: Option<String>,
    /// Raw image bytes from the peer, if any. Always `None` with the pinned
    /// soulseek-rs, which parses past the picture without keeping it.
    pub picture: Option<Vec<u8>>,
    pub upload_slots: Option<u32>,
    pub free_slot: Option<bool>,
    pub queue_size: Option<u32>,
}

impl UserProfile {
    /// Nothing known at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseDir {
    /// Full virtual path (`Music\Album (2019)`).
    pub path: String,
    pub files: Vec<RemoteFile>,
}

/// Why a transfer exists; decides its target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    /// ▶ Play: bytes go to the cache dir only, never to downloads.
    Play,
    /// ↓ Download: bytes go to the downloads dir.
    Download,
}

/// What [`crate::Core::play_fetch`] hands back immediately, before any byte
/// has arrived. The player opens `part_path` (growing) and switches to
/// `final_path` after [`crate::CoreEvent::TransferDone`]. On Linux an fd
/// already open on `part_path` stays valid across the library's rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayTicket {
    pub id: TransferId,
    pub part_path: PathBuf,
    pub final_path: PathBuf,
}

/// Upload state, mirrored from the library's `UploadStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadState {
    Queued { place: u32 },
    InProgress,
    Completed,
    Cancelled,
    Failed(String),
}

/// One row in the uploads half of the transfers panel.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadRow {
    pub username: String,
    pub path: String,
    pub size: u64,
    pub bytes_sent: u64,
    pub speed: f64,
    pub state: UploadState,
}

/// Login credentials. The password is only held in memory (the app loads it
/// from the OS keyring); `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Everything the settings screen edits. Applied as a whole by
/// [`crate::Core::apply_settings`]; fields that need a reconnect (listen
/// port) make the core reconnect. Persisted as TOML by [`crate::config`];
/// missing keys fall back to [`Settings::default`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub download_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// Cache size cap in bytes; oldest finished entries are evicted beyond it.
    pub cache_cap_bytes: u64,
    pub share_dirs: Vec<PathBuf>,
    pub listen_port: u16,
    /// Concurrent upload slots. There is deliberately no upload speed limit:
    /// the library has none and it is deferred out of the MVP (docs/core-api.md, G1).
    pub upload_slots: usize,
    /// Map `listen_port` on the router via UPnP-IGD, falling back to NAT-PMP.
    /// Implemented by wholeseek-core itself in CP1 (docs/core-api.md, G2).
    pub port_mapping: bool,
    /// A ▶ that has received no data after this many seconds fails
    /// (`TransferFailed`, reason starts with [`crate::PLAY_STALLED`]) and is
    /// cancelled, so the app can try `rank::alternates`. ↓ is unaffected.
    pub play_start_timeout_secs: u64,
}

impl Default for Settings {
    /// XDG locations: `$XDG_DOWNLOAD_DIR/<APP_NAME>` (else `~/Downloads/<APP_NAME>`)
    /// and `$XDG_CACHE_HOME/<APP_NAME>` (else `~/.cache/<APP_NAME>`), with
    /// [`crate::config::APP_NAME`].
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            download_dir: dirs::download_dir()
                .unwrap_or_else(|| home.join("Downloads"))
                .join(crate::config::APP_NAME),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| home.join(".cache"))
                .join(crate::config::APP_NAME),
            cache_cap_bytes: 1024 * 1024 * 1024,
            share_dirs: Vec::new(),
            listen_port: 2234,
            upload_slots: 10,
            port_mapping: true,
            play_start_timeout_secs: 8,
        }
    }
}
