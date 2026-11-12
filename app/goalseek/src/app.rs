//! App state and everything that changes it: core events, player snapshots and user actions.
//! Views (`view/`) only read it and attach handlers that call the methods here.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use goalseek_audio::{PlayState, PlayerSnapshot, SeekError};
use goalseek_core::{
    BrowseDir, CoreEvent, Credentials, DisconnectReason, PlayTicket, PortMapOutcome, RemoteFile,
    SearchHit, SearchId, Settings, TransferId, TransferKind, UploadRow, rank,
};
use rune::TextEdit;

use crate::backend::{Backend, RealBackend};
use crate::demo::{DemoBackend, FakePlayer, World};
use crate::playback::{Playback, RealPlayer};
use crate::profile::Profiles;
use crate::store::{MemoryStore, RealStore, Store, StoreNote};

/// Results drawn at most; the rest are counted ("showing 300 of 4 120").
pub const MAX_ROWS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Onboarding,
    Main,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Offline,
    Connecting,
    Connected {
        username: String,
        listen_port: Option<u16>,
    },
    Reconnecting(String),
    LoginRejected,
    Displaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    None,
    Transfers,
    Settings,
}

pub struct Onboarding {
    pub username: TextEdit,
    pub password: TextEdit,
    pub share_input: TextEdit,
    pub shares: Vec<PathBuf>,
    pub error: Option<String>,
    pub connecting: bool,
}

pub struct SettingsForm {
    pub download_dir: TextEdit,
    pub listen_port: TextEdit,
    pub upload_slots: TextEdit,
    pub share_input: TextEdit,
    pub shares: Vec<PathBuf>,
    pub message: Option<(bool, String)>,
}

pub struct SearchView {
    pub id: SearchId,
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub finished: bool,
}

pub struct BrowseView {
    pub username: String,
    pub dirs: Option<Vec<BrowseDir>>,
    pub error: Option<String>,
    pub selected: Option<String>,
    pub expanded: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TState {
    Queued,
    RemoteQueued(Option<u32>),
    Active,
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Transfer {
    pub id: TransferId,
    pub kind: TransferKind,
    pub file: RemoteFile,
    pub part_path: PathBuf,
    pub bytes: u64,
    pub total: u64,
    pub speed: f64,
    pub state: TState,
}

impl Transfer {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.bytes as f64 / self.total as f64) as f32
        }
    }
    pub fn is_live(&self) -> bool {
        matches!(
            self.state,
            TState::Queued | TState::RemoteQueued(_) | TState::Active
        )
    }
}

pub struct NowPlaying {
    pub ticket: PlayTicket,
    pub file: RemoteFile,
    /// The player has the `.part` open (after the first `TransferProgress`).
    pub opened: bool,
    /// ↓ was pressed on this track: switching tracks must not cancel its fetch.
    pub keep: bool,
    pub snapshot: Option<PlayerSnapshot>,
    pub failed: Option<String>,
}

pub struct App {
    pub backend: Box<dyn Backend>,
    pub player: Box<dyn Playback>,
    pub demo: bool,
    pub now: f64,
    pub screen: Screen,
    pub conn: Conn,
    pub onboarding: Onboarding,
    pub settings_form: SettingsForm,
    pub search_edit: TextEdit,
    pub search: Option<SearchView>,
    pub browse: Option<BrowseView>,
    pub transfers: Vec<Transfer>,
    pub uploads: Vec<UploadRow>,
    pub share_counts: Option<(u32, u32)>,
    pub panel: Panel,
    pub playing: Option<NowPlaying>,
    pub volume: f32,
    /// Transient line in the player strip (e.g. "not downloaded yet"), until the clock time.
    pub notice: Option<(String, f64)>,
    /// Last core error, shown under the search bar until dismissed.
    pub error: Option<String>,
    /// Router port-mapping result, as settings shows it.
    pub port_mapping: Option<String>,
    pub store: Box<dyn Store>,
    /// False when the system keyring can't hold the password: sign-in is per session.
    pub keyring_available: bool,
    /// Credentials typed in onboarding, saved once the server accepts them.
    pending_credentials: Option<Credentials>,
    pub audio_only: bool,
    /// Peer profiles by username (one per open user view).
    pub profiles: Profiles,
}

/// What `config::load()` found at startup (real mode).
pub struct Startup {
    pub credentials: Option<Credentials>,
    pub keyring_available: bool,
}

fn edit(s: &str) -> TextEdit {
    TextEdit::new(s)
}

impl App {
    pub fn with(
        backend: Box<dyn Backend>,
        player: Box<dyn Playback>,
        store: Box<dyn Store>,
        startup: Startup,
        demo: bool,
    ) -> Self {
        let settings = backend.settings().clone();
        let mut app = Self {
            onboarding: Onboarding {
                username: edit(""),
                password: TextEdit::new("").secure(),
                share_input: edit(""),
                shares: settings.share_dirs.clone(),
                error: None,
                connecting: false,
            },
            settings_form: SettingsForm::from_settings(&settings),
            backend,
            player,
            demo,
            now: 0.0,
            screen: Screen::Onboarding,
            conn: Conn::Offline,
            search_edit: edit(""),
            search: None,
            browse: None,
            transfers: Vec::new(),
            uploads: Vec::new(),
            share_counts: None,
            panel: Panel::None,
            playing: None,
            volume: 0.8,
            notice: None,
            error: None,
            port_mapping: None,
            store,
            keyring_available: startup.keyring_available,
            pending_credentials: None,
            audio_only: true,
            profiles: Profiles::default(),
        };
        // A remembered account skips onboarding and signs straight in.
        if let Some(c) = startup.credentials {
            app.onboarding.username.replace_text(c.username.clone());
            app.onboarding.password.replace_text(c.password.clone());
            app.screen = Screen::Main;
            app.conn = Conn::Connecting;
            app.backend.login(c);
        }
        app
    }

    /// Fake session and player on the frame clock.
    pub fn demo() -> Self {
        Self::demo_with(Startup {
            credentials: None,
            keyring_available: true,
        })
    }

    pub fn demo_with(startup: Startup) -> Self {
        let world = Rc::new(RefCell::new(World::default()));
        let keyring = startup.keyring_available;
        Self::with(
            Box::new(DemoBackend::new(world.clone())),
            Box::new(FakePlayer::new(world)),
            Box::new(MemoryStore::new(keyring)),
            startup,
            true,
        )
    }

    /// The real core and the system audio device.
    pub fn real(settings: Settings, startup: Startup) -> Self {
        let backend = RealBackend::start(settings).unwrap_or_else(|e| {
            eprintln!("goalseek: could not start the core: {e}");
            std::process::exit(1);
        });
        Self::with(
            Box::new(backend),
            Box::new(RealPlayer::new()),
            Box::new(RealStore::new()),
            startup,
            false,
        )
    }

    // ---- frame tick ----------------------------------------------------------------------

    /// Drain core events, advance the player, expire notices. Returns true if anything changed.
    /// Also returns how long until the next tick is wanted.
    pub fn tick(&mut self, now: f64) -> (bool, Option<f64>) {
        self.now = now;
        let events = self.backend.drain(now);
        let notes = self.store.drain();
        let mut changed = !events.is_empty() || !notes.is_empty();
        for e in events {
            self.apply(e);
        }
        for n in notes {
            match n {
                StoreNote::KeyringUnavailable(_) => self.keyring_available = false,
                StoreNote::Failed(m) => self.error = Some(m),
            }
        }
        self.player.tick(now);
        let mut wake = self.backend.next_tick();
        if let Some(p) = &mut self.playing
            && p.opened
        {
            let snap = self.player.snapshot();
            let running = matches!(snap.state, PlayState::Playing | PlayState::Buffering);
            if p.snapshot.as_ref() != Some(&snap) {
                p.snapshot = Some(snap);
                changed = true;
            }
            if running {
                wake = Some(wake.map_or(0.25, |w: f64| w.min(0.25)));
            }
        }
        if let Some((_, until)) = &self.notice {
            if now >= *until {
                self.notice = None;
                changed = true;
            } else {
                let d = until - now;
                wake = Some(wake.map_or(d, |w| w.min(d)));
            }
        }
        (changed, wake)
    }

    pub fn apply(&mut self, e: CoreEvent) {
        match e {
            CoreEvent::Connected {
                username,
                listen_port,
            } => {
                self.conn = Conn::Connected {
                    username,
                    listen_port,
                };
                self.screen = Screen::Main;
                self.onboarding.connecting = false;
                self.onboarding.error = None;
                // First successful sign-in from onboarding: remember the account and shares.
                if let Some(c) = self.pending_credentials.take() {
                    self.store.save_credentials(&c);
                    let mut settings = self.backend.settings().clone();
                    settings.share_dirs.clone_from(&self.onboarding.shares);
                    self.store.save_settings(&settings);
                }
            }
            CoreEvent::Disconnected { reason, will_retry } => {
                self.onboarding.connecting = false;
                match reason {
                    DisconnectReason::LoginRejected => {
                        self.pending_credentials = None;
                        self.conn = Conn::LoginRejected;
                        self.screen = Screen::Onboarding;
                        self.onboarding.error =
                            Some("Login rejected — check the username and password.".into());
                    }
                    DisconnectReason::Displaced => self.conn = Conn::Displaced,
                    DisconnectReason::Network(m) if will_retry => self.conn = Conn::Reconnecting(m),
                    DisconnectReason::Network(_) | DisconnectReason::User => {
                        self.conn = Conn::Offline
                    }
                }
            }
            CoreEvent::SearchResults { search, hits } => {
                let audio_only = self.audio_only;
                if let Some(s) = self.search.as_mut().filter(|s| s.id == search) {
                    s.hits.extend(hits);
                    rank::rank(&mut s.hits, audio_only);
                }
            }
            CoreEvent::SearchFinished { search, .. } => {
                if let Some(s) = self.search.as_mut().filter(|s| s.id == search) {
                    s.finished = true;
                }
            }
            CoreEvent::BrowseResult { username, dirs } => {
                if let Some(b) = self.browse.as_mut().filter(|b| b.username == username) {
                    b.dirs = Some(dirs);
                    b.error = None;
                    let first = b.selected.clone().or_else(|| {
                        let d = b.dirs.as_ref()?;
                        d.iter()
                            .find(|d| !d.files.is_empty())
                            .map(|d| d.path.clone())
                    });
                    if let Some(f) = first {
                        b.select(&f);
                    }
                }
            }
            CoreEvent::FolderContents { username, dirs, .. } => {
                if let Some(b) = self.browse.as_mut().filter(|b| b.username == username) {
                    let have = b.dirs.get_or_insert_with(Vec::new);
                    for d in dirs {
                        match have.iter_mut().find(|h| h.path == d.path) {
                            Some(h) => *h = d,
                            None => have.push(d),
                        }
                    }
                }
            }
            CoreEvent::UserProfile { username, profile } => {
                self.profiles.loaded(&username, profile);
            }
            CoreEvent::UserProfileFailed { username, reason } => {
                self.profiles.failed(&username, reason);
            }
            CoreEvent::BrowseFailed { username, reason } => {
                if let Some(b) = self.browse.as_mut().filter(|b| b.username == username) {
                    b.error = Some(reason);
                }
            }
            CoreEvent::TransferQueued {
                id,
                kind,
                file,
                part_path,
            } => {
                let t = self.upsert(id, kind, &file);
                t.part_path = part_path;
                t.total = file.size;
                t.state = TState::Queued;
            }
            CoreEvent::TransferRemoteQueued { id, place } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.state = TState::RemoteQueued(place);
                }
            }
            CoreEvent::TransferProgress {
                id,
                bytes,
                total,
                speed,
                part_path,
            } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.bytes = bytes;
                    t.total = total;
                    t.speed = speed;
                    t.state = TState::Active;
                    t.part_path.clone_from(&part_path);
                }
                if let Some(p) = &mut self.playing
                    && p.ticket.id == id
                    && !p.opened
                {
                    p.opened = true;
                    self.player.play(&part_path, total);
                    self.player.set_volume(self.volume);
                }
            }
            CoreEvent::TransferDone { id, path } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.bytes = t.total;
                    t.state = TState::Done(path);
                }
            }
            CoreEvent::TransferFailed { id, reason } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.state = TState::Failed(reason.clone());
                }
                if let Some(p) = self.playing.as_mut().filter(|p| p.ticket.id == id) {
                    p.failed = Some(reason);
                }
            }
            CoreEvent::TransferCancelled { id } => {
                if let Some(t) = self.transfer_mut(id) {
                    t.state = TState::Cancelled;
                }
            }
            CoreEvent::ShareCounts { folders, files } => self.share_counts = Some((folders, files)),
            CoreEvent::Uploads(rows) => self.uploads = rows,
            CoreEvent::PortMapping { port, outcome } => {
                self.port_mapping = Some(match outcome {
                    PortMapOutcome::Mapped { method, external } => {
                        let via = match method.as_str() {
                            "natpmp" => "NAT-PMP".to_string(),
                            "upnp" => "UPnP".to_string(),
                            m => m.to_string(),
                        };
                        let ext = external.map(|e| format!(" ({e})")).unwrap_or_default();
                        format!("Port {port} mapped via {via}{ext}")
                    }
                    PortMapOutcome::Unavailable(advice) => {
                        format!("Port {port} not mapped — forward it manually. {advice}")
                    }
                });
            }
            CoreEvent::Error { context, message } => {
                let text = format!("{context}: {message}");
                if self.onboarding.connecting && context == "login" {
                    self.onboarding.connecting = false;
                    self.onboarding.error = Some(text);
                } else {
                    self.error = Some(text);
                }
            }
        }
    }

    fn transfer_mut(&mut self, id: TransferId) -> Option<&mut Transfer> {
        self.transfers.iter_mut().find(|t| t.id == id)
    }

    fn upsert(&mut self, id: TransferId, kind: TransferKind, file: &RemoteFile) -> &mut Transfer {
        if let Some(i) = self.transfers.iter().position(|t| t.id == id) {
            return &mut self.transfers[i];
        }
        self.transfers.push(Transfer {
            id,
            kind,
            file: file.clone(),
            part_path: PathBuf::new(),
            bytes: 0,
            total: file.size,
            speed: 0.0,
            state: TState::Queued,
        });
        self.transfers.last_mut().expect("just pushed")
    }

    // ---- onboarding ----------------------------------------------------------------------

    /// # Errors
    /// What the user has to fix, in words.
    pub fn validate_onboarding(&self) -> Result<Credentials, String> {
        let username = self.onboarding.username.text.trim();
        let password = &self.onboarding.password.text;
        if username.is_empty() {
            return Err("Enter your Soulseek username.".into());
        }
        if username.chars().any(char::is_whitespace) {
            return Err("Usernames can't contain spaces.".into());
        }
        if password.is_empty() {
            return Err("Enter your password.".into());
        }
        for dir in &self.onboarding.shares {
            if !dir.is_dir() {
                return Err(format!("Share folder not found: {}", dir.display()));
            }
        }
        Ok(Credentials {
            username: username.to_string(),
            password: password.clone(),
        })
    }

    pub fn add_onboarding_share(&mut self) {
        match parse_dir(&self.onboarding.share_input.text) {
            Ok(dir) => {
                if !self.onboarding.shares.contains(&dir) {
                    self.onboarding.shares.push(dir);
                }
                self.onboarding.share_input.clear();
                self.onboarding.error = None;
            }
            Err(e) => self.onboarding.error = Some(e),
        }
    }

    pub fn connect(&mut self) {
        if self.onboarding.connecting {
            return;
        }
        // A path typed but not added still counts.
        if !self.onboarding.share_input.text.trim().is_empty() {
            self.add_onboarding_share();
            if self.onboarding.error.is_some() {
                return;
            }
        }
        match self.validate_onboarding() {
            Ok(credentials) => {
                self.onboarding.error = None;
                self.onboarding.connecting = true;
                self.conn = Conn::Connecting;
                let shares = self.onboarding.shares.clone();
                self.settings_form.shares.clone_from(&shares);
                self.backend.set_shares(shares);
                self.pending_credentials = Some(credentials.clone());
                self.backend.login(credentials);
            }
            Err(e) => self.onboarding.error = Some(e),
        }
    }

    /// Log out; `forget` also deletes the remembered account (keyring + username).
    pub fn logout(&mut self, forget: bool) {
        if forget {
            self.store.forget_credentials();
            self.onboarding.username.clear();
        }
        self.backend.logout();
        self.player.stop();
        self.playing = None;
        self.conn = Conn::Offline;
        self.screen = Screen::Onboarding;
        self.panel = Panel::None;
        self.onboarding.password.clear();
    }

    // ---- search & browse -----------------------------------------------------------------

    pub fn submit_search(&mut self) {
        let query = self.search_edit.text.trim().to_string();
        if query.is_empty() {
            return;
        }
        if let Some(old) = self.search.take()
            && !old.finished
        {
            self.backend.cancel_search(old.id);
        }
        let id = self.backend.search(&query);
        self.search = Some(SearchView {
            id,
            query,
            hits: Vec::new(),
            finished: false,
        });
        self.browse = None;
        self.error = None;
    }

    pub fn open_user(&mut self, username: &str) {
        if let Some(old) = self.browse.as_ref().map(|b| b.username.clone()) {
            self.profiles.forget(&old);
        }
        self.backend.browse(username);
        self.backend.user_profile(username);
        self.profiles.requested(username);
        self.browse = Some(BrowseView {
            username: username.to_string(),
            dirs: None,
            error: None,
            selected: None,
            expanded: BTreeSet::new(),
        });
    }

    /// Browse straight into the folder of a search hit.
    pub fn open_folder_of(&mut self, file: &RemoteFile) {
        self.open_user(&file.username);
        if let Some(b) = &mut self.browse {
            b.select(file.folder());
        }
    }

    pub fn back(&mut self) {
        if let Some(b) = self.browse.take() {
            self.profiles.forget(&b.username);
        }
    }

    // ---- play & download -----------------------------------------------------------------

    /// ▶ on a file: fetch into the cache and play once bytes arrive; ▶ on the playing file
    /// toggles pause.
    pub fn play(&mut self, file: &RemoteFile) {
        if self
            .playing
            .as_ref()
            .is_some_and(|p| &p.file == file && p.failed.is_none())
        {
            self.toggle_pause();
            return;
        }
        self.player.stop();
        if let Some(prev) = self.playing.take() {
            let live = self
                .transfers
                .iter()
                .any(|t| t.id == prev.ticket.id && t.is_live());
            if live && !prev.keep {
                self.backend.cancel(prev.ticket.id);
            }
        }
        let ticket = self.backend.play_fetch(file);
        self.upsert(ticket.id, TransferKind::Play, file);
        self.playing = Some(NowPlaying {
            ticket,
            file: file.clone(),
            opened: false,
            keep: false,
            snapshot: None,
            failed: None,
        });
        self.notice = None;
    }

    pub fn toggle_pause(&mut self) {
        let Some(p) = &self.playing else { return };
        match p.snapshot.as_ref().map(|s| &s.state) {
            Some(PlayState::Paused | PlayState::Ended) => self.player.resume(),
            Some(PlayState::Playing | PlayState::Buffering) => self.player.pause(),
            _ => {}
        }
        self.refresh_snapshot();
    }

    fn refresh_snapshot(&mut self) {
        if let Some(p) = &mut self.playing
            && p.opened
        {
            p.snapshot = Some(self.player.snapshot());
        }
    }

    /// Seek to a fraction of the track; outside the downloaded range shows a notice instead.
    pub fn seek_fraction(&mut self, f: f32) {
        let Some(dur) = self
            .playing
            .as_ref()
            .and_then(|p| p.snapshot.as_ref())
            .and_then(|s| s.duration_secs)
        else {
            self.show_notice("Not downloaded yet — wait for the track to start.".into());
            return;
        };
        let secs = f64::from(f.clamp(0.0, 1.0)) * dur;
        match self.player.seek(secs) {
            Ok(()) => self.refresh_snapshot(),
            Err(SeekError::NotBuffered { buffered_secs, .. }) => {
                self.show_notice(format!(
                    "Not downloaded yet — buffered up to {}",
                    crate::fmt::mmss(buffered_secs)
                ));
            }
            Err(e) => self.show_notice(format!("Can't seek: {e}")),
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.player.set_volume(self.volume);
        self.refresh_snapshot();
    }

    fn show_notice(&mut self, text: String) {
        self.notice = Some((text, self.now + 3.0));
    }

    /// ↓ on a file. A file already downloading (or done) is left alone.
    pub fn download(&mut self, file: &RemoteFile) {
        if self
            .download_of(file)
            .is_some_and(|t| !matches!(t.state, TState::Failed(_) | TState::Cancelled))
        {
            return;
        }
        if let Some(p) = self.playing.as_mut().filter(|p| &p.file == file) {
            p.keep = true; // the core promotes the in-flight play fetch
        }
        let id = self.backend.download(file);
        self.upsert(id, TransferKind::Download, file);
    }

    pub fn download_folder(&mut self, username: &str, folder: &str) {
        self.backend.download_folder(username, folder);
    }

    /// Latest download (not play fetch) of this file.
    pub fn download_of(&self, file: &RemoteFile) -> Option<&Transfer> {
        self.transfers.iter().rev().find(|t| {
            t.kind == TransferKind::Download
                && t.file.username == file.username
                && t.file.path == file.path
        })
    }

    pub fn cancel_upload(&mut self, username: &str, path: &str) {
        self.backend.cancel_upload(username, path);
    }

    /// Flip the audio-only filter and run the current search again under it.
    pub fn toggle_audio_only(&mut self) {
        self.audio_only = !self.audio_only;
        self.backend.set_audio_only(self.audio_only);
        if let Some(q) = self.search.as_ref().map(|s| s.query.clone()) {
            self.search_edit.replace_text(q);
            self.submit_search();
        }
    }

    pub fn cancel(&mut self, id: TransferId) {
        self.backend.cancel(id);
    }

    pub fn retry(&mut self, id: TransferId) {
        if let Some(t) = self.transfer_mut(id) {
            t.state = TState::Queued;
        }
        self.backend.retry(id);
    }

    pub fn active_transfers(&self) -> usize {
        let down = self
            .transfers
            .iter()
            .filter(|t| t.kind == TransferKind::Download && t.is_live())
            .count();
        let up = self
            .uploads
            .iter()
            .filter(|u| matches!(u.state, goalseek_core::UploadState::InProgress))
            .count();
        down + up
    }

    // ---- panels & settings ---------------------------------------------------------------

    pub fn toggle_panel(&mut self, p: Panel) {
        self.panel = if self.panel == p { Panel::None } else { p };
        if self.panel == Panel::Settings {
            let mut form = SettingsForm::from_settings(self.backend.settings());
            form.shares.clone_from(&self.onboarding.shares);
            self.settings_form = form;
        }
    }

    pub fn add_settings_share(&mut self) {
        match parse_dir(&self.settings_form.share_input.text) {
            Ok(dir) => {
                if !self.settings_form.shares.contains(&dir) {
                    self.settings_form.shares.push(dir);
                }
                self.settings_form.share_input.clear();
                self.settings_form.message = None;
            }
            Err(e) => self.settings_form.message = Some((false, e)),
        }
    }

    pub fn rescan(&mut self) {
        self.backend.rescan_shares();
        self.settings_form.message = Some((true, "Rescanning shares…".into()));
    }

    pub fn apply_settings(&mut self) {
        let form = &self.settings_form;
        let port = match form.listen_port.text.trim().parse::<u16>() {
            Ok(p) if p >= 1024 => p,
            _ => {
                self.settings_form.message = Some((
                    false,
                    "Listen port must be a number from 1024 to 65535.".into(),
                ));
                return;
            }
        };
        let slots = match form.upload_slots.text.trim().parse::<usize>() {
            Ok(n) if (1..=100).contains(&n) => n,
            _ => {
                self.settings_form.message = Some((false, "Upload slots must be 1–100.".into()));
                return;
            }
        };
        let dl = form.download_dir.text.trim();
        if dl.is_empty() {
            self.settings_form.message = Some((false, "Choose a download folder.".into()));
            return;
        }
        let shares = form.shares.clone();
        let mut settings = self.backend.settings().clone();
        settings.download_dir = expand_tilde(dl);
        settings.listen_port = port;
        settings.upload_slots = slots;
        let shares_changed = shares != self.onboarding.shares;
        settings.share_dirs.clone_from(&shares);
        self.store.save_settings(&settings);
        self.backend.apply_settings(settings);
        if shares_changed {
            self.backend.set_shares(shares.clone());
            self.onboarding.shares = shares;
        }
        self.settings_form.message = Some((true, "Saved.".into()));
    }
}

impl SettingsForm {
    fn from_settings(s: &Settings) -> Self {
        Self {
            download_dir: edit(&s.download_dir.display().to_string()),
            listen_port: edit(&s.listen_port.to_string()),
            upload_slots: edit(&s.upload_slots.to_string()),
            share_input: edit(""),
            shares: s.share_dirs.clone(),
            message: None,
        }
    }
}

impl BrowseView {
    /// Select a folder and expand its ancestors so it is visible in the tree.
    pub fn select(&mut self, path: &str) {
        let mut prefix = String::new();
        for part in path.split('\\') {
            if !prefix.is_empty() {
                self.expanded.insert(prefix.clone());
                prefix.push('\\');
            }
            prefix.push_str(part);
        }
        self.selected = Some(path.to_string());
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    pub fn selected_dir(&self) -> Option<&BrowseDir> {
        let sel = self.selected.as_deref()?;
        self.dirs.as_ref()?.iter().find(|d| d.path == sel)
    }

    /// Folder tree rows to draw: (path, depth, has children, expanded), in order.
    pub fn visible_tree(&self) -> Vec<(String, usize, bool, bool)> {
        let mut all = BTreeSet::new();
        for d in self.dirs.iter().flatten() {
            let mut prefix = String::new();
            for part in d.path.split('\\') {
                if !prefix.is_empty() {
                    prefix.push('\\');
                }
                prefix.push_str(part);
                all.insert(prefix.clone());
            }
        }
        let mut out = Vec::new();
        for p in &all {
            let depth = p.matches('\\').count();
            let mut visible = true;
            let mut anc = p.as_str();
            while let Some(i) = anc.rfind('\\') {
                anc = &anc[..i];
                if !self.expanded.contains(anc) {
                    visible = false;
                    break;
                }
            }
            if !visible {
                continue;
            }
            let child_prefix = format!("{p}\\");
            let has_children = all
                .range(child_prefix.clone()..)
                .next()
                .is_some_and(|n| n.starts_with(&child_prefix));
            out.push((p.clone(), depth, has_children, self.expanded.contains(p)));
        }
        out
    }
}

fn expand_tilde(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(s),
    }
}

/// A folder path the user typed: must exist.
fn parse_dir(text: &str) -> Result<PathBuf, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("Type a folder path to share.".into());
    }
    let p = expand_tilde(t);
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!("Folder not found: {}", p.display()))
    }
}
