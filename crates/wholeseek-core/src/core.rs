//! The `Core` handle the UI holds, and the worker thread behind it.
//!
//! Threading: soulseek-rs-lib is fully synchronous (std threads + `mpsc`,
//! one OS thread per actor/peer; no async runtime). Most of its results are
//! *polled* (search results, browse listings, folder contents, uploads,
//! session loss); only per-download status is pushed on an `mpsc::Receiver`.
//! So the core owns one worker thread that holds the `soulseek_rs::Client`,
//! takes [`Command`]s from the UI, polls the library on a short tick and turns
//! all of it into [`CoreEvent`]s on one channel. Every `Core` method returns
//! immediately.
//!
//! Timeouts and policies (search window, browse/play timeouts, reconnect
//! backoff) are documented in docs/core-api.md.

use crate::event::CoreEvent;
use crate::model::{Credentials, PlayTicket, RemoteFile, SearchId, Settings, TransferId};
use crate::net::{Connector, RealConnector};
use crate::status::{CoreStatus, SearchInfo};
use crate::worker::{Msg, Shared, Tunables, Worker};
use crate::{cache, paths};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Former CP0 skeleton marker. No command answers with it any more; kept
/// because it is part of the frozen public surface.
pub const NOT_IMPLEMENTED: &str = "not implemented yet (CP1)";

/// How often the worker polls the library when no command arrives.
pub const POLL_TICK: Duration = Duration::from_millis(150);

/// Commands from the UI to the worker. Public so the UI can be tested
/// against a fake core, but normally sent via [`Core`]'s methods.
#[derive(Debug, Clone)]
pub enum Command {
    Login(Credentials),
    Logout,
    Search {
        id: SearchId,
        query: String,
    },
    CancelSearch(SearchId),
    Browse {
        username: String,
    },
    Fetch {
        id: TransferId,
        file: RemoteFile,
        dir: PathBuf,
        kind: crate::model::TransferKind,
    },
    DownloadFolder {
        username: String,
        folder: String,
    },
    /// Additive: like `DownloadFolder`, into `<dir>/<remote folder name>/`.
    DownloadFolderTo {
        username: String,
        folder: String,
        dir: PathBuf,
    },
    Cancel(TransferId),
    Retry(TransferId),
    SetShares(Vec<PathBuf>),
    RescanShares,
    ApplySettings(Settings),
    /// Additive (CP1): audio-only filter for search batches (default on).
    SetAudioOnly(bool),
    /// Additive (CP1): stop an upload to a peer.
    CancelUpload {
        username: String,
        path: String,
    },
    /// Additive: fetch a user's profile (server stats + peer UserInfo).
    UserProfile(String),
    /// Additive: delete cached plays except those in use.
    ClearCache,
    /// Additive: replace the wishlist (`Settings::wishlist`).
    SetWishlist(Vec<String>),
    Shutdown,
}

/// The UI's handle on the Soulseek session. Cheap to call from the UI thread:
/// every method enqueues a command and returns.
pub struct Core {
    tx: Sender<Msg>,
    next_id: Arc<AtomicU64>,
    settings: Settings,
    status: Arc<Mutex<CoreStatus>>,
    infos: SearchInfos,
    worker: Option<JoinHandle<()>>,
}

/// Shared with the worker: what happened to recent searches.
pub(crate) type SearchInfos = Arc<Mutex<std::collections::BTreeMap<SearchId, SearchInfo>>>;

impl Core {
    /// Start the worker thread. Returns the handle and the event stream.
    /// Nothing touches the network until [`Core::login`]; the only disk
    /// access is reading/evicting `settings.cache_dir`. Settings are not
    /// persisted here — the app uses [`crate::config`].
    ///
    /// # Errors
    /// If the worker thread cannot be spawned.
    pub fn start(settings: Settings) -> std::io::Result<(Self, Receiver<CoreEvent>)> {
        Self::start_with(settings, Arc::new(RealConnector), Tunables::default())
    }

    pub(crate) fn start_with(
        settings: Settings,
        connector: Arc<dyn Connector>,
        tunables: Tunables,
    ) -> std::io::Result<(Self, Receiver<CoreEvent>)> {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        // Never reuse a t<id> dir (and a stale .part in it) from an earlier run.
        let staging = settings.download_dir.join(paths::STAGING_DIR_NAME);
        let first = cache::max_transfer_id(&[&settings.cache_dir, &staging]) + 1;
        let ids = Arc::new(AtomicU64::new(first));
        let status = Arc::new(Mutex::new(CoreStatus::default()));
        let infos = SearchInfos::default();
        let worker = Worker::new(
            ev_tx,
            tx.clone(),
            connector,
            tunables,
            settings.clone(),
            Shared {
                ids: ids.clone(),
                status: status.clone(),
                infos: infos.clone(),
            },
        );
        let handle = std::thread::Builder::new()
            .name("wholeseek-core".into())
            .spawn(move || worker.run(&rx))?;
        Ok((
            Self {
                tx,
                next_id: ids,
                settings,
                status,
                infos,
                worker: Some(handle),
            },
            ev_rx,
        ))
    }

    fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, cmd: Command) {
        // A dead worker has already emitted its last event; nothing to report.
        let _ = self.tx.send(Msg::Cmd(cmd));
    }

    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Connect and log in; auto-reconnects on network loss until [`Core::logout`].
    pub fn login(&self, credentials: Credentials) {
        self.send(Command::Login(credentials));
    }

    pub fn logout(&self) {
        self.send(Command::Logout);
    }

    /// Start a network search; results arrive as `SearchResults` batches.
    #[must_use]
    /// Search the network for `query`. What goes on the wire is
    /// [`crate::query::normalize_query`]`(query)`; if that finds no audio, the
    /// search is re-run once with [`crate::query::broaden_query`] under the
    /// same id (see [`Core::search_info`]). Results: `SearchResults`, then one
    /// `SearchFinished`.
    pub fn search(&self, query: &str) -> SearchId {
        let id = SearchId(self.next());
        self.send(Command::Search {
            id,
            query: query.trim().to_string(),
        });
        id
    }

    pub fn cancel_search(&self, id: SearchId) {
        self.send(Command::CancelSearch(id));
    }

    /// Fetch a user's share tree; answered by `BrowseResult` or `BrowseFailed`.
    pub fn browse(&self, username: &str) {
        self.send(Command::Browse {
            username: username.to_string(),
        });
    }

    /// ▶ Fetch `file` into its own cache directory. The ticket's paths are
    /// valid immediately; the player waits for `TransferProgress` before
    /// opening `part_path`. Never touches the downloads folder.
    #[must_use]
    pub fn play_fetch(&self, file: &RemoteFile) -> PlayTicket {
        let id = TransferId(self.next());
        let dir = paths::cache_transfer_dir(&self.settings.cache_dir, id);
        let ticket = PlayTicket {
            id,
            part_path: paths::part_path(&dir, &file.path),
            final_path: paths::final_path(&dir, &file.path),
        };
        self.send(Command::Fetch {
            id,
            file: file.clone(),
            dir,
            kind: crate::model::TransferKind::Play,
        });
        ticket
    }

    /// ↓ Save `file` under `downloads/<remote folder>/`. If the same file is
    /// fully cached from playing, the worker moves it instead of fetching.
    #[must_use]
    pub fn download(&self, file: &RemoteFile) -> TransferId {
        let id = TransferId(self.next());
        let dir = paths::download_dir_for(&self.settings.download_dir, &file.path);
        self.send(Command::Fetch {
            id,
            file: file.clone(),
            dir,
            kind: crate::model::TransferKind::Download,
        });
        id
    }

    /// Additive: ↓ `file` into `<dir>/<remote folder name>/` instead of the
    /// downloads folder ("Download to…"). Staging, `.part` handling and
    /// name-collision rules are those of [`Core::download`]: the file streams
    /// in `<download_dir>/.wholeseek-partial/t<id>/` and is moved (rename, or
    /// copy + delete across filesystems) when complete; `TransferDone { path }`
    /// is the final file. `dir` becomes the first of
    /// `Settings::recent_download_dirs` (persist with `config::save_settings`).
    pub fn download_to(&mut self, file: &RemoteFile, dir: &Path) -> TransferId {
        self.settings.remember_download_dir(dir);
        let id = TransferId(self.next());
        self.send(Command::Fetch {
            id,
            file: file.clone(),
            dir: paths::download_dir_for(dir, &file.path),
            kind: crate::model::TransferKind::Download,
        });
        id
    }

    /// Additive: [`Core::download_folder`] into `<dir>/<remote folder name>/`.
    /// Updates `Settings::recent_download_dirs` like [`Core::download_to`].
    pub fn download_folder_to(&mut self, username: &str, folder: &str, dir: &Path) {
        self.settings.remember_download_dir(dir);
        self.send(Command::DownloadFolderTo {
            username: username.to_string(),
            folder: folder.to_string(),
            dir: dir.to_path_buf(),
        });
    }

    /// ↓ a whole remote folder (non-recursive, as Soulseek folder listings are).
    /// Each file then appears as its own `TransferQueued`.
    pub fn download_folder(&self, username: &str, folder: &str) {
        self.send(Command::DownloadFolder {
            username: username.to_string(),
            folder: folder.to_string(),
        });
    }

    pub fn cancel(&self, id: TransferId) {
        self.send(Command::Cancel(id));
    }

    pub fn retry(&self, id: TransferId) {
        self.send(Command::Retry(id));
    }

    /// Replace shared folders, rescan, announce counts (`ShareCounts`).
    pub fn set_shares(&mut self, dirs: Vec<PathBuf>) {
        self.settings.share_dirs.clone_from(&dirs);
        self.send(Command::SetShares(dirs));
    }

    pub fn rescan_shares(&self) {
        self.send(Command::RescanShares);
    }

    /// Additive (CP1): turn the audio-only filter for search batches on/off
    /// (on by default). Applies to batches sent after the call.
    pub fn set_audio_only(&self, on: bool) {
        self.send(Command::SetAudioOnly(on));
    }

    /// Additive (CP1): cancel an upload to `username` of `path` (the
    /// `UploadRow::path`). An `Uploads` snapshot follows.
    pub fn cancel_upload(&self, username: &str, path: &str) {
        self.send(Command::CancelUpload {
            username: username.to_string(),
            path: path.to_string(),
        });
    }

    /// Additive: ask the server (presence, stats) and the peer (description,
    /// slots, queue) about `username`. Answers arrive as one or more
    /// `CoreEvent::UserProfile`, or `UserProfileFailed` if nothing came back
    /// within 10 s.
    pub fn user_profile(&self, username: &str) {
        self.send(Command::UserProfile(username.to_string()));
    }

    /// Additive: the current [`CoreStatus`] snapshot (reconnect countdown,
    /// listen port really bound and why, running searches' progress, cache
    /// usage). Cheap; meant to be read every frame.
    #[must_use]
    pub fn status(&self) -> CoreStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Additive: delete cached plays (`cache/t*`), keeping the current play
    /// and plays still being fetched. `status().cache` shows the result.
    pub fn clear_cache(&self) {
        self.send(Command::ClearCache);
    }

    /// Apply new settings. A changed listen port reconnects the session.
    /// Additive: what happened to a recent search — the query sent, whether it
    /// was broadened (and to what), whether it is a wishlist search, counts.
    /// Kept for the newest [`crate::status::SEARCH_INFO_KEEP`] searches.
    #[must_use]
    pub fn search_info(&self, id: SearchId) -> Option<SearchInfo> {
        self.infos.lock().ok()?.get(&id).cloned()
    }

    /// Additive: the wishlist (`Settings::wishlist`).
    #[must_use]
    pub fn wishlist(&self) -> &[String] {
        &self.settings.wishlist
    }

    /// Additive: add a wishlist query. Returns false for an empty query or one
    /// already on the list (case-insensitive). Persist with
    /// `config::save_settings(core.settings())`.
    pub fn wishlist_add(&mut self, query: &str) -> bool {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if q.is_empty()
            || self
                .settings
                .wishlist
                .iter()
                .any(|w| w.to_lowercase() == q.to_lowercase())
        {
            return false;
        }
        self.settings.wishlist.push(q);
        self.send(Command::SetWishlist(self.settings.wishlist.clone()));
        true
    }

    /// Additive: remove a wishlist query (case-insensitive). Returns whether it
    /// was there. Persist with `config::save_settings(core.settings())`.
    pub fn wishlist_remove(&mut self, query: &str) -> bool {
        let q = query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let before = self.settings.wishlist.len();
        self.settings.wishlist.retain(|w| w.to_lowercase() != q);
        let removed = self.settings.wishlist.len() != before;
        if removed {
            self.send(Command::SetWishlist(self.settings.wishlist.clone()));
        }
        removed
    }

    pub fn apply_settings(&mut self, settings: Settings) {
        self.settings = settings.clone();
        self.send(Command::ApplySettings(settings));
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioAttrs;

    fn file(path: &str) -> RemoteFile {
        RemoteFile {
            username: "alice".into(),
            path: path.into(),
            size: 10,
            attrs: AudioAttrs::default(),
        }
    }

    fn settings() -> Settings {
        Settings {
            download_dir: "/dl".into(),
            cache_dir: "/cache".into(),
            ..Settings::default()
        }
    }

    // The library's Client must move into the worker thread.
    #[test]
    fn library_client_is_send() {
        const fn assert_send<T: Send>() {}
        assert_send::<soulseek_rs::Client>();
    }

    #[test]
    fn play_fetch_targets_cache_never_downloads() {
        let (core, _ev) = Core::start(settings()).unwrap();
        let t = core.play_fetch(&file(r"Music\Album\01 a.flac"));
        assert!(t.part_path.starts_with("/cache"));
        assert!(!t.part_path.starts_with("/dl"));
        assert_eq!(t.part_path.file_name().unwrap(), "01 a.flac.part");
        assert_eq!(t.final_path.parent(), t.part_path.parent());
        let t2 = core.play_fetch(&file(r"Music\Other\01 a.flac"));
        assert_ne!(
            t.part_path, t2.part_path,
            "same basename must not share a .part"
        );
    }

    #[test]
    fn ids_are_unique_across_kinds() {
        let (core, _ev) = Core::start(settings()).unwrap();
        let a = core.search("x");
        let b = core.download(&file("a.flac"));
        let c = core.search("y");
        assert!(a.0 < b.0 && b.0 < c.0);
    }

    #[test]
    fn offline_commands_answer_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let (core, ev) = Core::start(Settings {
            download_dir: tmp.path().join("dl"),
            cache_dir: tmp.path().join("cache"),
            ..Settings::default()
        })
        .unwrap();
        core.browse("bob");
        match ev.recv_timeout(Duration::from_secs(2)).unwrap() {
            CoreEvent::BrowseFailed { username, reason } => {
                assert_eq!(username, "bob");
                assert_eq!(reason, "not connected");
            }
            other => panic!("unexpected {other:?}"),
        }
        drop(core); // joins the worker
    }

    #[test]
    fn credentials_debug_redacts_password() {
        let c = Credentials {
            username: "u".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{c:?}").contains("hunter2"));
    }
}
