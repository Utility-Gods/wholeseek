//! What the UI talks to: the real [`goalseek_core::Core`], or the demo's fake event source.
//! Both are driven the same way: commands return at once, events are drained each frame.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use goalseek_core::{
    Core, CoreEvent, Credentials, PlayTicket, RemoteFile, SearchId, Settings, TransferId,
};

use crate::demo::Call;
use goalseek_core::status::CoreStatus;

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
        let dir = goalseek_core::paths::cache_transfer_dir(&self.settings.cache_dir, TransferId(0));
        PlayTicket {
            id: TransferId(0),
            part_path: goalseek_core::paths::part_path(&dir, &f.path),
            final_path: goalseek_core::paths::final_path(&dir, &f.path),
        }
    }
    fn download(&mut self, _: &RemoteFile) -> TransferId {
        TransferId(0)
    }
    fn download_folder(&mut self, _: &str, _: &str) {}
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
    /// Seconds until `drain` has more to say without an outside wake (demo timers).
    fn next_tick(&self) -> Option<f64> {
        None
    }
    /// Called once with the window's waker: events arriving later wake the UI.
    fn set_waker(&mut self, _wake: rune::Waker) {}
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
            .name("goalseek-ui-events".into())
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
                "goalseek: event forwarder failed to start ({e}); events show on the next frame"
            );
        }
    }
}
