//! What the UI talks to: the real [`wholeseek_core::Core`], or the demo's fake event source.
//! Both are driven the same way: commands return at once, events are drained each frame.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use wholeseek_core::{
    Core, CoreEvent, Credentials, DownloadedKeys, HistoryEntry, HistoryQuery, PlayTicket,
    RemoteFile, SearchId, Settings, TransferId,
};

use crate::demo::Call;
use wholeseek_core::status::CoreStatus;

/// Stands in until the saved settings are read at startup: does nothing, says nothing.
#[derive(Default)]
pub struct IdleBackend {
    settings: Settings,
}

impl Backend for IdleBackend {
    fn settings(&self) -> &Settings {
        &self.settings
    }
    fn login(&mut self, _: Credentials) {}
    fn logout(&mut self) {}
    fn search(&mut self, _: &str) -> SearchId {
        SearchId(0)
    }
    fn cancel_search(&mut self, _: SearchId) {}
    fn browse(&mut self, _: &str) {}
    fn play_fetch(&mut self, f: &RemoteFile) -> PlayTicket {
        let dir =
            wholeseek_core::paths::cache_transfer_dir(&self.settings.cache_dir, TransferId(0));
        PlayTicket {
            id: TransferId(0),
            part_path: wholeseek_core::paths::part_path(&dir, &f.path),
            final_path: wholeseek_core::paths::final_path(&dir, &f.path),
        }
    }
    fn download(&mut self, _: &RemoteFile) -> TransferId {
        TransferId(0)
    }
    fn download_folder(&mut self, _: &str, _: &str) {}
    fn download_to(&mut self, _: &RemoteFile, _: &Path) -> TransferId {
        TransferId(0)
    }
    fn download_folder_to(&mut self, _: &str, _: &str, _: &Path) {}
    fn download_album_file(&mut self, _: &RemoteFile, _: Option<&Path>) -> TransferId {
        TransferId(0)
    }
    fn cancel(&mut self, _: TransferId) {}
    fn retry(&mut self, _: TransferId) {}
    fn set_shares(&mut self, _: Vec<PathBuf>) {}
    fn rescan_shares(&mut self) {}
    fn apply_settings(&mut self, _: Settings) {}
    fn cancel_upload(&mut self, _: &str, _: &str) {}
    fn user_profile(&mut self, _: &str) {}
    fn set_audio_only(&mut self, _: bool) {}
    fn drain(&mut self, _: f64) -> Vec<CoreEvent> {
        Vec::new()
    }
    fn status(&self) -> CoreStatus {
        CoreStatus::default()
    }
    fn clear_cache(&mut self) {}
}

/// The UI's view of the Soulseek session (mirrors `Core`'s public methods).
pub trait Backend {
    fn settings(&self) -> &Settings;
    fn login(&mut self, credentials: Credentials);
    fn logout(&mut self);
    fn search(&mut self, query: &str) -> SearchId;
    fn cancel_search(&mut self, id: SearchId);
    fn browse(&mut self, username: &str);
    fn play_fetch(&mut self, file: &RemoteFile) -> PlayTicket;
    fn download(&mut self, file: &RemoteFile) -> TransferId;
    fn download_folder(&mut self, username: &str, folder: &str);
    /// "Download to…": one file into `dir` (flat, or in its album folder with
    /// `Settings::single_file_subfolder`); `dir` heads `Settings::recent_download_dirs`.
    fn download_to(&mut self, file: &RemoteFile, dir: &Path) -> TransferId;
    /// "Download folder to…": `<dir>/<remote folder name>/`; updates the recent folders.
    fn download_folder_to(&mut self, username: &str, folder: &str, dir: &Path);
    /// One track of an album, always in `<root>/<remote folder name>/` whatever the setting
    /// (Keep while an album plays). `root`: `None` the downloads folder; `Some` updates the
    /// recent folders.
    fn download_album_file(&mut self, file: &RemoteFile, root: Option<&Path>) -> TransferId;
    fn cancel(&mut self, id: TransferId);
    fn retry(&mut self, id: TransferId);
    fn set_shares(&mut self, dirs: Vec<PathBuf>);
    fn rescan_shares(&mut self);
    fn apply_settings(&mut self, settings: Settings);
    fn cancel_upload(&mut self, username: &str, path: &str);
    /// Ask for a peer's profile; answers arrive as `UserProfile` / `UserProfileFailed`.
    fn user_profile(&mut self, username: &str);
    fn set_audio_only(&mut self, on: bool);
    /// Every event ready now. Never blocks. `now` is the frame clock (virtual when headless).
    fn drain(&mut self, now: f64) -> Vec<CoreEvent>;
    /// The core's polled snapshot: reconnect wait, listen port, search progress, cache.
    fn status(&self) -> CoreStatus;
    /// Delete cached plays except the current and in-flight ones.
    fn clear_cache(&mut self);
    /// Every `(username, remote path)` downloaded before (this run or earlier): the
    /// "Downloaded" mark. A shared set; fetch once per frame.
    fn downloaded_keys(&self) -> DownloadedKeys {
        DownloadedKeys::default()
    }
    /// Download history, newest first, filtered and paged (`Core::history`).
    fn history(&self, _query: &HistoryQuery) -> Vec<HistoryEntry> {
        Vec::new()
    }
    /// Forget one history entry; whether it was there.
    fn history_remove(&mut self, _id: u64) -> bool {
        false
    }
    /// Forget the whole history (files on disk stay).
    ///
    /// # Errors
    /// The history file couldn't be rewritten (memory is cleared anyway).
    fn history_clear(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Whether a history entry's file is still on disk (a `stat`; visible rows only).
    fn history_on_disk(&self, entry: &HistoryEntry) -> bool {
        entry.exists_on_disk()
    }
    /// What happened to a recent search: what was typed, whether it was broadened (and to
    /// what).
    fn search_info(&self, _id: SearchId) -> Option<wholeseek_core::status::SearchInfo> {
        None
    }
    /// Add a wishlist query (`Settings::wishlist`); false if empty or already there.
    /// Persist the settings afterwards.
    fn wishlist_add(&mut self, _query: &str) -> bool {
        false
    }
    /// Remove a wishlist query; whether it was there. Persist the settings afterwards.
    fn wishlist_remove(&mut self, _query: &str) -> bool {
        false
    }
    /// Seconds until `drain` has more to say without an outside wake (demo timers).
    fn next_tick(&self) -> Option<f64> {
        None
    }
    /// Called once with the window's waker: events arriving later wake the UI.
    fn set_waker(&mut self, _wake: rune::Waker) {}
    /// The album cover cache (`wholeseek_core::cover`), if this backend has one.
    fn covers(&self) -> Option<&wholeseek_core::Covers> {
        None
    }
    /// Fetch a folder's cover: only for the selected or playing folder (`view::art`).
    fn fetch_cover(
        &mut self,
        _username: &str,
        _folder: &str,
        _listing: Option<Vec<(String, u64)>>,
    ) {
    }
    /// Commands received so far (demo only; tests read it).
    fn calls(&self) -> &[Call] {
        &[]
    }
}

/// The real session. Events come from the core's channel; once a waker is known a forwarding
/// thread blocks on that channel and wakes the window for each event, so an idle UI draws nothing.
pub struct RealBackend {
    core: Core,
    events: Receiver<CoreEvent>,
}

impl RealBackend {
    /// # Errors
    /// If the core's worker thread cannot be spawned.
    pub fn start(settings: Settings) -> std::io::Result<Self> {
        let (core, events) = Core::start(settings)?;
        Ok(Self { core, events })
    }
}

impl Backend for RealBackend {
    fn settings(&self) -> &Settings {
        self.core.settings()
    }
    fn login(&mut self, credentials: Credentials) {
        self.core.login(credentials);
    }
    fn logout(&mut self) {
        self.core.logout();
    }
    fn search(&mut self, query: &str) -> SearchId {
        self.core.search(query)
    }
    fn cancel_search(&mut self, id: SearchId) {
        self.core.cancel_search(id);
    }
    fn browse(&mut self, username: &str) {
        self.core.browse(username);
    }
    fn play_fetch(&mut self, file: &RemoteFile) -> PlayTicket {
        self.core.play_fetch(file)
    }
    fn download(&mut self, file: &RemoteFile) -> TransferId {
        self.core.download(file)
    }
    fn download_folder(&mut self, username: &str, folder: &str) {
        self.core.download_folder(username, folder);
    }
    fn download_to(&mut self, file: &RemoteFile, dir: &Path) -> TransferId {
        self.core.download_to(file, dir)
    }
    fn download_folder_to(&mut self, username: &str, folder: &str, dir: &Path) {
        self.core.download_folder_to(username, folder, dir);
    }
    fn download_album_file(&mut self, file: &RemoteFile, root: Option<&Path>) -> TransferId {
        self.core.download_album_file(file, root)
    }
    fn cancel(&mut self, id: TransferId) {
        self.core.cancel(id);
    }
    fn retry(&mut self, id: TransferId) {
        self.core.retry(id);
    }
    fn set_shares(&mut self, dirs: Vec<PathBuf>) {
        self.core.set_shares(dirs);
    }
    fn rescan_shares(&mut self) {
        self.core.rescan_shares();
    }
    fn apply_settings(&mut self, settings: Settings) {
        self.core.apply_settings(settings);
    }
    fn cancel_upload(&mut self, username: &str, path: &str) {
        self.core.cancel_upload(username, path);
    }
    fn user_profile(&mut self, username: &str) {
        self.core.user_profile(username);
    }
    fn set_audio_only(&mut self, on: bool) {
        self.core.set_audio_only(on);
    }
    fn status(&self) -> CoreStatus {
        self.core.status()
    }
    fn clear_cache(&mut self) {
        self.core.clear_cache();
    }
    fn covers(&self) -> Option<&wholeseek_core::Covers> {
        Some(self.core.covers())
    }
    fn fetch_cover(&mut self, username: &str, folder: &str, listing: Option<Vec<(String, u64)>>) {
        self.core.fetch_cover(username, folder, listing);
    }
    fn downloaded_keys(&self) -> DownloadedKeys {
        self.core.downloaded_keys()
    }
    fn history(&self, query: &HistoryQuery) -> Vec<HistoryEntry> {
        self.core.history(query)
    }
    fn history_remove(&mut self, id: u64) -> bool {
        self.core.history_remove(id)
    }
    fn history_clear(&mut self) -> Result<(), String> {
        self.core.history_clear().map_err(|e| e.to_string())
    }
    fn search_info(&self, id: SearchId) -> Option<wholeseek_core::status::SearchInfo> {
        self.core.search_info(id)
    }
    fn wishlist_add(&mut self, query: &str) -> bool {
        self.core.wishlist_add(query)
    }
    fn wishlist_remove(&mut self, query: &str) -> bool {
        self.core.wishlist_remove(query)
    }
    fn drain(&mut self, _now: f64) -> Vec<CoreEvent> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }
    fn set_waker(&mut self, wake: rune::Waker) {
        // Swap in a forwarded channel: the thread owns the core's receiver and blocks on it.
        let (tx, rx) = mpsc::channel();
        let from_core = std::mem::replace(&mut self.events, rx);
        let spawned = std::thread::Builder::new()
            .name("wholeseek-ui-events".into())
            .spawn(move || {
                while let Ok(e) = from_core.recv() {
                    if tx.send(e).is_err() {
                        return;
                    }
                    wake();
                }
            });
        if let Err(e) = spawned {
            eprintln!(
                "wholeseek: event forwarder failed to start ({e}); events show on the next frame"
            );
        }
    }
}
