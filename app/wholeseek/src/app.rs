//! App state and everything that changes it: core events, player snapshots and user actions.
//! Views (`view/`) only read it and attach handlers that call the methods here.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use rune::{TextEdit, Tx};
use wholeseek_audio::{PlayState, PlayerSnapshot, SeekError};
use wholeseek_core::{
    CoreEvent, Credentials, DisconnectReason, PlayTicket, PortMapOutcome, RemoteFile, Settings,
    TransferId, TransferKind, UploadRow, rank,
};

use crate::actions::DrawerUi;
use crate::backend::{Backend, RealBackend};
use crate::demo::{DemoBackend, FakePlayer, World};
use crate::desktop::{DemoDesktop, Desktop, DesktopCall, RealDesktop};
use crate::menu::Menu;
use crate::playback::{Playback, RealPlayer};
use crate::profile::Profiles;
use crate::queue::PlayQueue;
use crate::store::{MemoryStore, RealStore, Store, StoreNote};
use crate::tabs::{BrowseView, SearchTab, SearchView, Tab, TabId, TabKind, UserTab};

/// Results drawn at most; the rest are counted ("showing 300 of 4 120").
pub const MAX_ROWS: usize = 300;

/// Seconds between equalizer frames while a track plays.
pub const EQ_FRAME: f64 = 1.0 / 12.0;

/// How long settings' "Saved" tick shows.
pub const SAVED_FOR: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Reading the saved account (off the UI thread); the window is already up.
    Starting,
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
    /// Typed fallbacks, used only when no folder dialog can show (`manual`).
    pub download_dir: TextEdit,
    pub listen_port: TextEdit,
    pub share_input: TextEdit,
    /// Why folder dialogs are unavailable (then paths are typed).
    pub manual: Option<String>,
    pub path_error: Option<String>,
    /// Clock time of the last applied change, for the "Saved" tick.
    pub saved_at: Option<f64>,
    /// A new port typed, waiting for "Reconnect".
    pub port_confirm: Option<u16>,
    pub port_invalid: bool,
    /// The port field is open (after "Change").
    pub port_editing: bool,
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
    /// Which source this is, from 1 (the one clicked); >1 shows "Trying another source…".
    pub attempt: usize,
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
    /// Open tabs, in strip order; never empty.
    pub tabs: Vec<Tab>,
    /// Index of the shown tab.
    pub active: usize,
    next_tab: TabId,
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
    /// Router port mapping: (port, mapped).
    pub port_mapping: Option<(u16, bool)>,
    /// Open files and folders, measure the disk (never launches anything in demo/tests).
    pub desktop: Box<dyn Desktop>,
    /// The demo desktop's record of calls (empty with the real one).
    pub desktop_log: Rc<RefCell<Vec<DesktopCall>>>,
    pub menu: Option<Menu>,
    pub drawer: DrawerUi,
    /// What plays in order; the player strip shows its current item.
    pub queue: Option<PlayQueue>,
    /// Scroll the highlighted file into view once its folder is listed.
    pub reveal: bool,
    /// The core's polled snapshot, refreshed each frame.
    pub status: wholeseek_core::CoreStatus,
    pub store: Box<dyn Store>,
    /// False when the system keyring can't hold the password: sign-in is per session.
    pub keyring_available: bool,
    /// Credentials typed in onboarding, saved once the server accepts them.
    pending_credentials: Option<Credentials>,
    /// Peer profiles by username (one per open user view).
    pub profiles: Profiles,
    /// The account signed in with this run, for Reconnect (memory only).
    credentials: Option<Credentials>,
    /// Waiting for the startup load; `Screen::Starting` until it lands.
    pub boot: Option<Boot>,
    /// The window's waker (from setup), handed to a backend started later.
    waker: Option<rune::Waker>,
}

/// What the startup load found, and how to start the session with it.
pub struct Booted {
    pub settings: Settings,
    pub startup: Startup,
}

/// Startup still reading the saved account off the UI thread (real mode).
pub struct Boot {
    pub rx: Receiver<Booted>,
    /// Builds the session backend from the loaded settings.
    pub make: fn(Settings) -> Box<dyn Backend>,
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
        let desktop_log = Rc::new(RefCell::new(Vec::new()));
        let desktop: Box<dyn Desktop> = if demo {
            Box::new(DemoDesktop::new(desktop_log.clone()))
        } else {
            Box::new(RealDesktop)
        };
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
            tabs: Vec::new(),
            active: 0,
            next_tab: 1,
            transfers: Vec::new(),
            uploads: Vec::new(),
            share_counts: None,
            panel: Panel::None,
            playing: None,
            volume: 0.8,
            notice: None,
            error: None,
            port_mapping: None,
            desktop,
            desktop_log,
            menu: None,
            drawer: DrawerUi::default(),
            queue: None,
            reveal: false,
            status: wholeseek_core::CoreStatus::default(),
            store,
            keyring_available: startup.keyring_available,
            pending_credentials: None,
            profiles: Profiles::default(),
            credentials: None,
            boot: None,
            waker: None,
        };
        app.new_search_tab();
        app.begin(startup);
        app
    }

    /// A remembered account skips onboarding and signs straight in.
    fn begin(&mut self, startup: Startup) {
        self.keyring_available = startup.keyring_available;
        self.screen = Screen::Onboarding;
        if let Some(c) = startup.credentials {
            self.onboarding.username.replace_text(c.username.clone());
            self.onboarding.password.replace_text(c.password.clone());
            self.screen = Screen::Main;
            self.conn = Conn::Connecting;
            self.credentials = Some(c.clone());
            self.backend.login(c);
        }
    }

    /// The window is up before the saved account is read: `Screen::Starting` ("Signing
    /// in…") until `boot` delivers, then onboarding or the signed-in main view.
    pub fn booting(
        player: Box<dyn Playback>,
        store: Box<dyn Store>,
        boot: Boot,
        demo: bool,
    ) -> Self {
        let mut app = Self::with(
            Box::new(crate::backend::IdleBackend::default()),
            player,
            store,
            Startup {
                credentials: None,
                keyring_available: true,
            },
            demo,
        );
        app.screen = Screen::Starting;
        app.boot = Some(boot);
        app
    }

    pub fn set_waker(&mut self, wake: rune::Waker) {
        self.store.set_waker(wake.clone());
        self.profiles.set_waker(wake.clone());
        self.backend.set_waker(wake.clone());
        self.player.set_waker(wake.clone());
        self.waker = Some(wake);
    }

    /// The startup load landed: start the session backend with its settings and begin.
    fn finish_boot(&mut self, booted: Booted) {
        let Some(boot) = self.boot.take() else { return };
        let mut backend = (boot.make)(booted.settings);
        if let Some(w) = &self.waker {
            backend.set_waker(w.clone());
        }
        let settings = backend.settings().clone();
        self.backend = backend;
        self.onboarding.shares.clone_from(&settings.share_dirs);
        self.settings_form = SettingsForm::from_settings(&settings);
        self.begin(booted.startup);
    }

    /// Signed in elsewhere (or dropped for good): sign in again with this run's account.
    /// Never automatic — the other client would be pushed off in turn.
    pub fn reconnect(&mut self) {
        match self.credentials.clone() {
            Some(c) => {
                self.conn = Conn::Connecting;
                self.backend.login(c);
            }
            None => self.screen = Screen::Onboarding,
        }
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

    /// The real core and the system audio device. The window shows at once; the saved
    /// settings and account (config file + keyring, which can take seconds) load on a
    /// background thread started in `setup`.
    pub fn real() -> (Self, std::sync::mpsc::Sender<Booted>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let boot = Boot {
            rx,
            make: |settings| {
                Box::new(RealBackend::start(settings).unwrap_or_else(|e| {
                    eprintln!("wholeseek: could not start the core: {e}");
                    std::process::exit(1);
                }))
            },
        };
        let app = Self::booting(
            Box::new(RealPlayer::new()),
            Box::new(RealStore::new()),
            boot,
            false,
        );
        (app, tx)
    }

    // ---- frame tick ----------------------------------------------------------------------

    /// Drain core events, advance the player, expire notices. Returns true if anything changed.
    /// Also returns how long until the next tick is wanted.
    pub fn tick(&mut self, now: f64) -> (bool, Option<f64>) {
        self.now = now;
        let booted = self.boot.as_ref().and_then(|b| b.rx.try_recv().ok());
        if let Some(b) = booted {
            self.finish_boot(b);
            return (true, Some(0.0));
        }
        let events = self.backend.drain(now);
        let notes = self.store.drain();
        let mut changed = !events.is_empty() || !notes.is_empty();
        changed |= self.profiles.settle();
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
        let player_events = self.player.events();
        changed |= !player_events.is_empty();
        for e in player_events {
            self.apply_player(e);
        }
        let mut wake = self.backend.next_tick();
        let status = self.backend.status();
        if status != self.status {
            self.status = status;
            changed = true;
        }
        if self.status.reconnect.is_some() {
            // "Reconnecting in 5 s" counts down.
            wake = Some(wake.map_or(0.5, |w: f64| w.min(0.5)));
        }
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
                // The playing row's equalizer moves with the clock: ~12 frames a second.
                wake = Some(wake.map_or(EQ_FRAME, |w: f64| w.min(EQ_FRAME)));
                changed = true;
            }
        }
        if let Some(at) = self.settings_form.saved_at {
            let until = at + SAVED_FOR;
            if now >= until {
                self.settings_form.saved_at = None;
                changed = true;
            } else {
                let d = until - now;
                wake = Some(wake.map_or(d, |w| w.min(d)));
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
                if let Some(s) = self.search_by_id(search) {
                    s.hits.extend(hits);
                    rank::rank(&mut s.hits, true);
                }
            }
            CoreEvent::SearchFinished { search, .. } => {
                if let Some(s) = self.search_by_id(search) {
                    s.finished = true;
                }
            }
            CoreEvent::BrowseResult { username, dirs } => {
                if let Some(b) = self.browse_of(&username) {
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
                if let Some(b) = self.browse_of(&username) {
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
                if let Some(b) = self.browse_of(&username) {
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
                self.queue_progress(id, &part_path, total);
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
                // ▶ never started (stalled, queued, refused): other sources, then skip.
                if !self.queue_failed(id, &reason)
                    && let Some(p) = self.playing.as_mut().filter(|p| p.ticket.id == id)
                {
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
                let mapped = matches!(outcome, PortMapOutcome::Mapped { .. });
                self.port_mapping = Some((port, mapped));
            }
            // Settings shows the port in effect; no banner for a busy port.
            CoreEvent::Error { context, .. } if context == "listen_port" => {}
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
                self.backend.set_shares(shares);
                self.pending_credentials = Some(credentials.clone());
                self.credentials = Some(credentials.clone());
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

    /// Run the query typed in the shown search tab (replacing its previous search).
    pub fn submit_search(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active).and_then(Tab::search_mut) else {
            return;
        };
        let query = tab.edit.text.trim().to_string();
        if query.is_empty() {
            return;
        }
        let old = tab.search.take();
        if let Some(old) = old
            && !old.finished
        {
            self.backend.cancel_search(old.id);
        }
        let id = self.backend.search(&query);
        if let Some(tab) = self.tabs.get_mut(self.active).and_then(Tab::search_mut) {
            tab.search = Some(SearchView {
                id,
                query,
                hits: Vec::new(),
                finished: false,
            });
        }
        if let Some(t) = self.tabs.get(self.active) {
            t.scroll.set(0.0);
        }
        self.error = None;
    }

    // ---- tabs ----------------------------------------------------------------------------

    fn push_tab(&mut self, kind: TabKind) {
        let id = self.next_tab;
        self.next_tab += 1;
        let at = if self.tabs.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.tabs.insert(at, Tab::new(id, kind));
        self.active = at;
    }

    /// ＋ / Ctrl+T: an empty search tab, shown.
    pub fn new_search_tab(&mut self) {
        self.push_tab(TabKind::Search(SearchTab {
            edit: edit(""),
            search: None,
            filters: crate::tabs::Filters::default(),
        }));
    }

    /// Show the user's tab, opening it (browse + profile requests) if there is none.
    pub fn open_user(&mut self, username: &str) {
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.username() == Some(username))
        {
            self.active = i;
            return;
        }
        self.backend.browse(username);
        self.backend.user_profile(username);
        self.profiles.requested(username);
        self.push_tab(TabKind::User(UserTab {
            browse: BrowseView::new(username),
        }));
    }

    /// The user's tab, at the folder of a search hit.
    pub fn open_folder_of(&mut self, file: &RemoteFile) {
        self.open_user(&file.username);
        if let Some(b) = self.tabs.get_mut(self.active).and_then(Tab::browse_mut) {
            b.select(file.folder());
        }
    }

    /// Type `query` into the shown search tab (a new one if a user tab is shown) and run it.
    pub fn search_for(&mut self, query: &str) {
        self.show_search_tab();
        if let Some(st) = self.tab_mut().search_mut() {
            st.edit.replace_text(query);
        }
        self.submit_search();
    }

    /// Ctrl+L / Ctrl+F: stay on a search tab, else go to the nearest one on the left (or the
    /// first), else open one.
    pub fn show_search_tab(&mut self) {
        if self.tab().search().is_some() {
            return;
        }
        let left = (0..self.active)
            .rev()
            .find(|&i| self.tabs[i].search().is_some());
        match left.or_else(|| self.tabs.iter().position(|t| t.search().is_some())) {
            Some(i) => self.active = i,
            None => self.new_search_tab(),
        }
    }

    pub fn activate(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
        }
    }

    /// Ctrl+Tab (+1) / Ctrl+Shift+Tab (-1), wrapping.
    pub fn cycle_tabs(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        self.active = (self.active as isize + step).rem_euclid(n) as usize;
    }

    /// × / middle-click / Ctrl+W. Stops a running search; the last tab leaves an empty
    /// search tab in its place.
    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(i);
        match tab.kind {
            TabKind::Search(s) => {
                if let Some(v) = s.search.filter(|v| !v.finished) {
                    self.backend.cancel_search(v.id);
                }
            }
            TabKind::User(u) => self.profiles.forget(&u.browse.username),
        }
        if self.tabs.is_empty() {
            self.new_search_tab();
            return;
        }
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    pub fn close_active(&mut self) {
        self.close_tab(self.active);
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// The shown search tab's search, if the shown tab is a search.
    pub fn active_search(&self) -> Option<&SearchView> {
        self.tab().search()?.search.as_ref()
    }

    /// The shown tab's browse, if it is a user tab.
    pub fn active_browse(&self) -> Option<&BrowseView> {
        self.tab().browse()
    }

    pub fn active_browse_mut(&mut self) -> Option<&mut BrowseView> {
        self.tab_mut().browse_mut()
    }

    fn search_by_id(&mut self, id: wholeseek_core::SearchId) -> Option<&mut SearchView> {
        self.tabs
            .iter_mut()
            .filter_map(|t| t.search_mut()?.search.as_mut())
            .find(|s| s.id == id)
    }

    fn browse_of(&mut self, username: &str) -> Option<&mut BrowseView> {
        self.tabs
            .iter_mut()
            .filter_map(Tab::browse_mut)
            .find(|b| b.username == username)
    }

    /// After a tab switch: put the shown tab's lists back where they were scrolled to.
    pub fn restore_scroll(&self, tx: &mut Tx) {
        let t = self.tab();
        tx.scroll_to_instant(t.list_key(), 0.0, t.scroll.get());
        if t.browse().is_some() {
            tx.scroll_to_instant(t.tree_key(), 0.0, t.tree_scroll.get());
        }
    }

    // ---- play & download -----------------------------------------------------------------

    /// ▶ on a file: fetch into the cache and play once bytes arrive; ▶ on the playing file
    /// toggles pause.
    pub(crate) fn upsert_play(&mut self, id: TransferId, file: &RemoteFile) {
        self.upsert(id, TransferKind::Play, file);
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
            .filter(|u| matches!(u.state, wholeseek_core::UploadState::InProgress))
            .count();
        down + up
    }

    // ---- panels & settings ---------------------------------------------------------------

    pub fn toggle_panel(&mut self, p: Panel) {
        self.panel = if self.panel == p { Panel::None } else { p };
        self.drawer.confirm_cancel = None;
        if self.panel == Panel::Settings {
            self.settings_form = SettingsForm::from_settings(self.backend.settings());
        }
    }

    /// A typed folder (no folder dialog on this desktop): add it to the shares.
    pub fn add_typed_share(&mut self) {
        match parse_dir(&self.settings_form.share_input.text) {
            Ok(dir) => {
                self.settings_form.share_input.clear();
                self.settings_form.path_error = None;
                self.add_share(dir);
            }
            Err(e) => self.settings_form.path_error = Some(e),
        }
    }

    /// A typed download folder (no folder dialog): applied if it exists.
    pub fn set_typed_download_dir(&mut self) {
        match parse_dir(&self.settings_form.download_dir.text) {
            Ok(dir) => {
                self.settings_form.path_error = None;
                self.set_download_dir(dir);
            }
            Err(e) => self.settings_form.path_error = Some(e),
        }
    }

    pub fn rescan(&mut self) {
        self.backend.rescan_shares();
        self.share_counts = None;
    }
}

impl SettingsForm {
    fn from_settings(s: &Settings) -> Self {
        Self {
            download_dir: edit(&s.download_dir.display().to_string()),
            listen_port: edit(&s.listen_port.to_string()),
            share_input: edit(""),
            manual: None,
            path_error: None,
            saved_at: None,
            port_confirm: None,
            port_invalid: false,
            port_editing: false,
        }
    }

    /// After a change was applied: fields show what is in effect.
    pub fn sync(&mut self, s: &Settings) {
        self.download_dir
            .replace_text(s.download_dir.display().to_string());
        if self.port_confirm.is_none() {
            self.listen_port.replace_text(s.listen_port.to_string());
            self.port_invalid = false;
        }
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
