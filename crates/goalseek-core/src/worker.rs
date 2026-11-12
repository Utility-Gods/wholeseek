//! The core worker thread: owns the session, turns UI commands into library
//! calls, polls the library on a tick and emits [`CoreEvent`]s.
//!
//! Everything here runs on one thread. Calls that can block (connect + login,
//! a search window, share scans, keyring writes) run on short-lived helper
//! threads that report back through [`Internal`] messages.

use crate::cache;
use crate::core::Command;
use crate::event::{CoreEvent, DisconnectReason, PortMapOutcome};
use crate::model::{
    AudioAttrs, BrowseDir, Credentials, RemoteFile, SearchId, Settings, TransferId, TransferKind,
    UploadRow, UploadState,
};
use crate::net::{self, ConnectParams, Connector, LoginError, Session};
use crate::port_mapping::{self, PortMapper};
use crate::{paths, rank};
use soulseek_rs::{DownloadStatus, SessionLoss, SharedDirectory, UploadStatus};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// Timing knobs; tests shrink them.
#[derive(Debug, Clone)]
pub(crate) struct Tunables {
    pub tick: Duration,
    pub search_window: Duration,
    pub browse_timeout: Duration,
    pub folder_timeout: Duration,
    pub play_start_timeout: Duration,
    pub queue_place_after: Duration,
    pub uploads_every: Duration,
    pub backoff: fn(u32) -> Duration,
}

impl Default for Tunables {
    fn default() -> Self {
        Self {
            tick: crate::core::POLL_TICK,
            search_window: Duration::from_secs(15),
            browse_timeout: Duration::from_secs(60),
            folder_timeout: Duration::from_secs(30),
            play_start_timeout: Duration::from_secs(30),
            queue_place_after: Duration::from_secs(5),
            uploads_every: Duration::from_secs(1),
            backoff: net::backoff,
        }
    }
}

pub(crate) enum Msg {
    Cmd(Command),
    Internal(Internal),
}

pub(crate) enum Internal {
    LoginDone {
        generation: u64,
        result: Result<Arc<dyn Session>, LoginError>,
    },
    SearchWindowClosed(SearchId),
    SharesScanned {
        seq: u64,
        result: Result<(u32, u32), String>,
    },
    PortMap {
        generation: u64,
        port: u16,
        outcome: port_mapping::Outcome,
    },
}

enum Conn {
    Offline,
    Connecting,
    Online(Arc<dyn Session>),
    Waiting(Instant),
}

type Key = (String, String);

struct LiveSearch {
    id: SearchId,
    query: String,
    seen: usize,
    delivered: usize,
    cancel: Arc<AtomicBool>,
    session: Arc<dyn Session>,
}

struct BrowseReq {
    deadline: Instant,
    user_requested: bool,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum FolderStage {
    Contents,
    Browse,
}

struct FolderReq {
    username: String,
    folder: String,
    deadline: Instant,
    stage: FolderStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TState {
    Active,
    Done,
    Failed,
    Cancelled,
}

struct Transfer {
    kind: TransferKind,
    file: RemoteFile,
    /// Play: `cache/t<id>`; download: `downloads/<folder>`.
    dest_dir: PathBuf,
    /// What the UI was told in `TransferQueued` / `TransferProgress`.
    part_path: PathBuf,
    /// Play: the ticket's final path.
    final_path: PathBuf,
    state: TState,
    /// A play mirroring someone else's fetch has hard-linked the growing file.
    linked: bool,
}

/// One library download. Several transfers can subscribe to it (▶ and ↓ of
/// the same file), so the library only ever has one per (user, path) (G11).
struct Fetch {
    owner: TransferId,
    owner_kind: TransferKind,
    lib_dir: PathBuf,
    part: PathBuf,
    final_path: PathBuf,
    rx: Receiver<DownloadStatus>,
    started: Instant,
    got_bytes: bool,
    bytes: u64,
    total: u64,
    speed: f64,
    place_requested: bool,
    last_place: Option<u32>,
    last_place_poll: Instant,
    subs: Vec<TransferId>,
}

pub(crate) struct Worker {
    events: Sender<CoreEvent>,
    me: Sender<Msg>,
    connector: Arc<dyn Connector>,
    t: Tunables,
    settings: Settings,
    ids: Arc<AtomicU64>,
    audio_only: bool,

    creds: Option<Credentials>,
    conn: Conn,
    generation: u64,
    attempt: u32,
    mapper: Option<PortMapper>,

    searches: Vec<LiveSearch>,
    orphan_searches: HashMap<SearchId, (String, Arc<dyn Session>)>,
    browses: HashMap<String, BrowseReq>,
    browse_cache: Vec<(String, Vec<BrowseDir>)>,
    folders: Vec<FolderReq>,
    transfers: HashMap<TransferId, Transfer>,
    fetches: HashMap<Key, Fetch>,
    cache_index: HashMap<Key, PathBuf>,
    current_play: Option<TransferId>,
    share_seq: u64,
    shares_dirty: bool,
    last_uploads: Option<Vec<UploadRow>>,
    last_uploads_poll: Instant,
}

const BROWSE_CACHE_USERS: usize = 16;

impl Worker {
    pub(crate) fn new(
        events: Sender<CoreEvent>,
        me: Sender<Msg>,
        connector: Arc<dyn Connector>,
        t: Tunables,
        settings: Settings,
        ids: Arc<AtomicU64>,
    ) -> Self {
        Self {
            events,
            me,
            connector,
            t,
            settings,
            ids,
            audio_only: true,
            creds: None,
            conn: Conn::Offline,
            generation: 0,
            attempt: 0,
            mapper: None,
            searches: Vec::new(),
            orphan_searches: HashMap::new(),
            browses: HashMap::new(),
            browse_cache: Vec::new(),
            folders: Vec::new(),
            transfers: HashMap::new(),
            fetches: HashMap::new(),
            cache_index: HashMap::new(),
            current_play: None,
            share_seq: 0,
            shares_dirty: false,
            last_uploads: None,
            last_uploads_poll: Instant::now(),
        }
    }

    pub(crate) fn run(mut self, rx: &Receiver<Msg>) {
        self.cache_index = cache::index(&self.settings.cache_dir);
        self.evict();
        let mut last_poll = Instant::now();
        loop {
            let wait = self.t.tick.saturating_sub(last_poll.elapsed());
            match rx.recv_timeout(wait) {
                Ok(Msg::Cmd(Command::Shutdown)) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Cmd(c)) => self.command(c),
                Ok(Msg::Internal(i)) => self.internal(i),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if last_poll.elapsed() >= self.t.tick {
                self.poll();
                last_poll = Instant::now();
            }
        }
        self.end_session("shutting down");
        self.conn = Conn::Offline;
    }

    fn emit(&self, ev: CoreEvent) {
        let _ = self.events.send(ev);
    }

    fn error(&self, context: &str, message: impl Into<String>) {
        self.emit(CoreEvent::Error {
            context: context.to_string(),
            message: message.into(),
        });
    }

    fn session(&self) -> Option<Arc<dyn Session>> {
        match &self.conn {
            Conn::Online(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed)
    }

    fn command(&mut self, c: Command) {
        match c {
            Command::Login(creds) => self.login(creds),
            Command::Logout => self.logout(),
            Command::Search { id, query } => self.search(id, query),
            Command::CancelSearch(id) => self.finish_search(id, true),
            Command::Browse { username } => self.browse(username, true),
            Command::Fetch {
                id,
                file,
                dir,
                kind,
            } => self.fetch(id, file, dir, kind),
            Command::DownloadFolder { username, folder } => self.download_folder(username, folder),
            Command::Cancel(id) => self.cancel(id),
            Command::Retry(id) => self.retry(id),
            Command::SetShares(dirs) => {
                self.settings.share_dirs = dirs;
                self.rescan();
            }
            Command::RescanShares => self.rescan(),
            Command::ApplySettings(s) => self.apply_settings(s),
            Command::SetAudioOnly(on) => self.audio_only = on,
            Command::CancelUpload { username, path } => {
                match self.session() {
                    Some(s) if s.cancel_upload(&username, &path) => {}
                    Some(_) => self.error(
                        "cancel_upload",
                        format!("no active upload of {path} to {username}"),
                    ),
                    None => self.error("cancel_upload", "not connected"),
                }
                self.last_uploads_poll = Instant::now() - self.t.uploads_every;
                self.poll_uploads();
            }
            Command::Shutdown => {}
        }
    }

    fn internal(&mut self, i: Internal) {
        match i {
            Internal::LoginDone { generation, result } => self.on_login_done(generation, result),
            Internal::SearchWindowClosed(id) => {
                if self.searches.iter().any(|s| s.id == id) {
                    self.finish_search(id, false);
                } else if let Some((query, session)) = self.orphan_searches.remove(&id)
                    && !self.searches.iter().any(|s| s.query == query)
                {
                    session.forget_search(&query);
                }
            }
            Internal::SharesScanned { seq, result } => {
                if seq != self.share_seq {
                    return;
                }
                match result {
                    Ok((folders, files)) => self.emit(CoreEvent::ShareCounts { folders, files }),
                    Err(e) => self.error("shares", e),
                }
            }
            Internal::PortMap {
                generation,
                port,
                outcome,
            } => {
                if generation == self.generation {
                    let outcome = match outcome {
                        port_mapping::Outcome::Mapped { method, external } => {
                            PortMapOutcome::Mapped {
                                method: method.to_string(),
                                external,
                            }
                        }
                        port_mapping::Outcome::Unavailable(m) => PortMapOutcome::Unavailable(m),
                    };
                    self.emit(CoreEvent::PortMapping { port, outcome });
                }
            }
        }
    }

    fn poll(&mut self) {
        self.poll_session();
        let searches: Vec<SearchId> = self.searches.iter().map(|s| s.id).collect();
        for id in searches {
            self.poll_search(id);
        }
        self.poll_browses();
        self.poll_folders();
        self.poll_transfers();
        self.poll_uploads();
    }

    // ---------------------------------------------------------------- session

    fn login(&mut self, creds: Credentials) {
        if self.session().is_some() || matches!(self.conn, Conn::Connecting) {
            self.end_session("logging in again");
        }
        self.creds = Some(creds);
        self.attempt = 0;
        self.start_connect();
    }

    fn start_connect(&mut self) {
        let Some(c) = self.creds.clone() else {
            self.conn = Conn::Offline;
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.conn = Conn::Connecting;
        self.shares_dirty = false;
        let params = ConnectParams {
            username: c.username,
            password: c.password,
            listen_port: self.settings.listen_port,
            share_dirs: share_dir_strings(&self.settings.share_dirs),
        };
        let connector = self.connector.clone();
        let me = self.me.clone();
        let _ = std::thread::Builder::new()
            .name("goalseek-connect".into())
            .spawn(move || {
                let result = connector.connect(&params);
                let _ = me.send(Msg::Internal(Internal::LoginDone { generation, result }));
            });
    }

    fn on_login_done(&mut self, generation: u64, result: Result<Arc<dyn Session>, LoginError>) {
        if generation != self.generation || !matches!(self.conn, Conn::Connecting) {
            return; // stale; dropping the session disconnects it
        }
        let Some(username) = self.creds.as_ref().map(|c| c.username.clone()) else {
            self.conn = Conn::Offline;
            return;
        };
        match result {
            Ok(s) => {
                self.attempt = 0;
                s.set_upload_slots(self.settings.upload_slots);
                let port = s.listen_port();
                self.conn = Conn::Online(s.clone());
                self.emit(CoreEvent::Connected {
                    username,
                    listen_port: port,
                });
                let (folders, files) = s.shared_counts();
                self.emit(CoreEvent::ShareCounts { folders, files });
                if self.shares_dirty {
                    self.rescan();
                }
                self.start_port_mapping(port);
            }
            Err(LoginError::Rejected) => {
                self.creds = None;
                self.conn = Conn::Offline;
                self.emit(CoreEvent::Disconnected {
                    reason: DisconnectReason::LoginRejected,
                    will_retry: false,
                });
            }
            Err(LoginError::Other(m)) => self.schedule_retry(m),
        }
    }

    fn schedule_retry(&mut self, reason: String) {
        let delay = (self.t.backoff)(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        self.conn = Conn::Waiting(Instant::now() + delay);
        self.emit(CoreEvent::Disconnected {
            reason: DisconnectReason::Network(reason),
            will_retry: true,
        });
    }

    fn logout(&mut self) {
        self.generation += 1;
        self.creds = None;
        self.attempt = 0;
        self.end_session("logged out");
        self.conn = Conn::Offline;
        self.emit(CoreEvent::Disconnected {
            reason: DisconnectReason::User,
            will_retry: false,
        });
    }

    /// Tear down everything bound to the current session. The caller sets `conn`.
    fn end_session(&mut self, why: &str) {
        self.mapper = None;
        for s in std::mem::take(&mut self.searches) {
            s.cancel.store(true, Ordering::Relaxed);
            self.emit(CoreEvent::SearchFinished {
                search: s.id,
                total_files: s.delivered,
            });
        }
        for (username, b) in std::mem::take(&mut self.browses) {
            if b.user_requested {
                self.emit(CoreEvent::BrowseFailed {
                    username,
                    reason: why.to_string(),
                });
            }
        }
        for f in std::mem::take(&mut self.folders) {
            self.error(
                "download_folder",
                format!("could not list {} from {}: {why}", f.folder, f.username),
            );
        }
        let keys: Vec<Key> = self.fetches.keys().cloned().collect();
        for k in keys {
            self.fail_fetch(&k, why);
        }
        self.last_uploads = None;
        self.conn = Conn::Offline;
    }

    fn poll_session(&mut self) {
        match &self.conn {
            Conn::Online(s) => {
                if let Some(loss) = s.session_loss() {
                    match loss {
                        SessionLoss::Displaced => {
                            self.end_session("another client logged in with this account");
                            self.creds = None;
                            self.emit(CoreEvent::Disconnected {
                                reason: DisconnectReason::Displaced,
                                will_retry: false,
                            });
                        }
                        SessionLoss::Disconnected => {
                            self.end_session("the connection to the server dropped");
                            self.schedule_retry("the connection to the server dropped".into());
                        }
                    }
                }
            }
            Conn::Waiting(until) if Instant::now() >= *until => self.start_connect(),
            _ => {}
        }
    }

    fn start_port_mapping(&mut self, port: Option<u16>) {
        self.mapper = None;
        let (true, Some(port)) = (self.settings.port_mapping, port) else {
            return;
        };
        let generation = self.generation;
        let me = self.me.clone();
        self.mapper = Some(PortMapper::spawn(port, move |outcome| {
            let _ = me.send(Msg::Internal(Internal::PortMap {
                generation,
                port,
                outcome,
            }));
        }));
    }

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        if old.upload_slots != self.settings.upload_slots
            && let Some(s) = self.session()
        {
            s.set_upload_slots(self.settings.upload_slots);
        }
        if old.share_dirs != self.settings.share_dirs {
            self.rescan();
        }
        let live = !matches!(self.conn, Conn::Offline);
        if old.listen_port != self.settings.listen_port && live && self.creds.is_some() {
            self.end_session("reconnecting with the new listen port");
            self.emit(CoreEvent::Disconnected {
                reason: DisconnectReason::Network("listen port changed; reconnecting".into()),
                will_retry: true,
            });
            self.attempt = 0;
            self.start_connect();
        } else if old.port_mapping != self.settings.port_mapping
            && let Some(s) = self.session()
        {
            self.start_port_mapping(s.listen_port());
        }
        if old.cache_cap_bytes != self.settings.cache_cap_bytes
            || old.cache_dir != self.settings.cache_dir
        {
            self.cache_index = cache::index(&self.settings.cache_dir);
            self.evict();
        }
    }

    // ----------------------------------------------------------------- shares

    fn rescan(&mut self) {
        self.share_seq += 1;
        let seq = self.share_seq;
        let dirs = self.settings.share_dirs.clone();
        let me = self.me.clone();
        let session = match &self.conn {
            Conn::Connecting => {
                self.shares_dirty = true;
                return;
            }
            Conn::Online(s) => Some(s.clone()),
            _ => None,
        };
        let _ = std::thread::Builder::new()
            .name("goalseek-shares".into())
            .spawn(move || {
                let result = match session {
                    Some(s) => s.set_shares(share_dir_strings(&dirs)),
                    None => Ok(net::scan_counts(&dirs)),
                };
                let _ = me.send(Msg::Internal(Internal::SharesScanned { seq, result }));
            });
    }

    fn poll_uploads(&mut self) {
        if self.last_uploads_poll.elapsed() < self.t.uploads_every {
            return;
        }
        self.last_uploads_poll = Instant::now();
        let Some(s) = self.session() else { return };
        let rows: Vec<UploadRow> = s
            .uploads()
            .into_iter()
            .map(|u| UploadRow {
                username: u.username,
                path: u.filename,
                size: u.size,
                bytes_sent: u.bytes_sent,
                speed: u.speed_bytes_per_sec,
                state: match u.status {
                    UploadStatus::Queued(place) => UploadState::Queued { place },
                    UploadStatus::InProgress => UploadState::InProgress,
                    UploadStatus::Completed => UploadState::Completed,
                    UploadStatus::Cancelled => UploadState::Cancelled,
                    UploadStatus::Failed(e) => UploadState::Failed(e),
                },
            })
            .collect();
        if self.last_uploads.as_ref() != Some(&rows) {
            self.last_uploads = Some(rows.clone());
            self.emit(CoreEvent::Uploads(rows));
        }
    }

    // ----------------------------------------------------------------- search

    fn search(&mut self, id: SearchId, query: String) {
        if query.is_empty() {
            self.emit(CoreEvent::SearchFinished {
                search: id,
                total_files: 0,
            });
            return;
        }
        let Some(session) = self.session() else {
            self.error("search", "not connected");
            self.emit(CoreEvent::SearchFinished {
                search: id,
                total_files: 0,
            });
            return;
        };
        // One live search per query string (G4): the library files results by it.
        if let Some(old) = self
            .searches
            .iter()
            .find(|s| s.query == query)
            .map(|s| s.id)
        {
            self.finish_search(old, true);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (s2, q2, c2, me, window) = (
            session.clone(),
            query.clone(),
            cancel.clone(),
            self.me.clone(),
            self.t.search_window,
        );
        let _ = std::thread::Builder::new()
            .name("goalseek-search".into())
            .spawn(move || {
                s2.search_blocking(&q2, window, c2);
                let _ = me.send(Msg::Internal(Internal::SearchWindowClosed(id)));
            });
        self.searches.push(LiveSearch {
            id,
            query,
            seen: 0,
            delivered: 0,
            cancel,
            session,
        });
    }

    fn poll_search(&mut self, id: SearchId) {
        let audio_only = self.audio_only;
        let Some(s) = self.searches.iter_mut().find(|s| s.id == id) else {
            return;
        };
        let count = s.session.search_count(&s.query);
        if count < s.seen {
            s.seen = count;
        }
        if count == s.seen {
            return;
        }
        let results = s.session.search_results(&s.query);
        let new = results.get(s.seen..).unwrap_or(&[]);
        s.seen = results.len();
        let mut hits = rank::hits_from_results(new);
        rank::rank(&mut hits, audio_only);
        if !hits.is_empty() {
            s.delivered += hits.len();
            let ev = CoreEvent::SearchResults { search: id, hits };
            self.emit(ev);
        }
    }

    /// End a search. `cancelled`: the helper may still be running, so the
    /// library entry is forgotten when it reports back.
    fn finish_search(&mut self, id: SearchId, cancelled: bool) {
        let Some(pos) = self.searches.iter().position(|s| s.id == id) else {
            return;
        };
        if !cancelled {
            self.poll_search(id);
        }
        let s = self.searches.remove(pos);
        s.cancel.store(true, Ordering::Relaxed);
        if cancelled {
            self.orphan_searches
                .insert(id, (s.query.clone(), s.session.clone()));
        } else {
            s.session.forget_search(&s.query);
        }
        self.emit(CoreEvent::SearchFinished {
            search: id,
            total_files: s.delivered,
        });
    }

    // ----------------------------------------------------------------- browse

    fn browse(&mut self, username: String, user_requested: bool) {
        let started = self.session().map(|s| s.browse(&username));
        match started {
            Some(Ok(())) => {
                let deadline = Instant::now() + self.t.browse_timeout;
                let req = self.browses.entry(username).or_insert(BrowseReq {
                    deadline,
                    user_requested,
                });
                req.deadline = deadline;
                req.user_requested |= user_requested;
            }
            Some(Err(e)) => self.browse_failed(username, user_requested, &e),
            None => self.browse_failed(username, user_requested, "not connected"),
        }
    }

    fn browse_failed(&mut self, username: String, user_requested: bool, why: &str) {
        self.fail_folders_waiting_browse(&username, why);
        if user_requested {
            self.emit(CoreEvent::BrowseFailed {
                username,
                reason: why.to_string(),
            });
        }
    }

    fn poll_browses(&mut self) {
        let Some(s) = self.session() else { return };
        let users: Vec<String> = self.browses.keys().cloned().collect();
        for u in users {
            if let Some(listing) = s.take_browse(&u) {
                let Some(req) = self.browses.remove(&u) else {
                    continue;
                };
                let dirs = to_browse_dirs(&u, listing);
                self.browse_cache.retain(|(name, _)| name != &u);
                self.browse_cache.push((u.clone(), dirs.clone()));
                if self.browse_cache.len() > BROWSE_CACHE_USERS {
                    self.browse_cache.remove(0);
                }
                if req.user_requested {
                    self.emit(CoreEvent::BrowseResult {
                        username: u.clone(),
                        dirs: dirs.clone(),
                    });
                }
                self.resolve_folders_from_browse(&u, &dirs);
            } else if self
                .browses
                .get(&u)
                .is_some_and(|r| Instant::now() >= r.deadline)
            {
                let req = self.browses.remove(&u);
                let why = format!(
                    "{u} did not answer within {}s (offline or unreachable)",
                    self.t.browse_timeout.as_secs()
                );
                self.browse_failed(u, req.is_some_and(|r| r.user_requested), &why);
            }
        }
    }

    fn cached_listing(&self, username: &str) -> Option<&Vec<BrowseDir>> {
        self.browse_cache
            .iter()
            .find(|(u, _)| u == username)
            .map(|(_, d)| d)
    }

    // --------------------------------------------------------- folder download

    fn download_folder(&mut self, username: String, folder: String) {
        let folder = trim_folder(&folder).to_string();
        if let Some(dirs) = self.cached_listing(&username).cloned()
            && self.expand_folder(&username, &folder, &dirs, true)
        {
            return;
        }
        let Some(s) = self.session() else {
            self.error("download_folder", "not connected");
            return;
        };
        if let Err(e) = s.request_folder(&username, &folder) {
            self.error("download_folder", e);
            return;
        }
        self.folders.push(FolderReq {
            username,
            folder,
            deadline: Instant::now() + self.t.folder_timeout,
            stage: FolderStage::Contents,
        });
    }

    fn poll_folders(&mut self) {
        let Some(s) = self.session() else { return };
        let mut i = 0;
        while i < self.folders.len() {
            let (u, folder, stage, deadline) = {
                let f = &self.folders[i];
                (f.username.clone(), f.folder.clone(), f.stage, f.deadline)
            };
            if stage == FolderStage::Contents {
                if let Some(listing) = s.take_folder(&u, &folder) {
                    self.folders.remove(i);
                    let dirs = to_browse_dirs(&u, listing);
                    self.emit(CoreEvent::FolderContents {
                        username: u.clone(),
                        folder: folder.clone(),
                        dirs: dirs.clone(),
                    });
                    self.expand_folder(&u, &folder, &dirs, false);
                    continue;
                }
                if Instant::now() >= deadline {
                    // G14: no (matching) answer — fall back to the full listing.
                    self.folders[i].stage = FolderStage::Browse;
                    self.folders[i].deadline =
                        Instant::now() + self.t.browse_timeout + self.t.tick * 4;
                    self.browse(u, false);
                }
            } else if Instant::now() >= deadline {
                self.folders.remove(i);
                self.error(
                    "download_folder",
                    format!("could not list {folder} from {u}"),
                );
                continue;
            }
            i += 1;
        }
    }

    fn resolve_folders_from_browse(&mut self, username: &str, dirs: &[BrowseDir]) {
        let (ready, rest): (Vec<FolderReq>, Vec<FolderReq>) = std::mem::take(&mut self.folders)
            .into_iter()
            .partition(|f| f.username == username && f.stage == FolderStage::Browse);
        self.folders = rest;
        for f in ready {
            if !self.expand_folder(username, &f.folder, dirs, true) {
                self.error(
                    "download_folder",
                    format!("{} is not in {username}'s shares", f.folder),
                );
            }
        }
    }

    fn fail_folders_waiting_browse(&mut self, username: &str, why: &str) {
        let (failed, rest): (Vec<FolderReq>, Vec<FolderReq>) = std::mem::take(&mut self.folders)
            .into_iter()
            .partition(|f| f.username == username && f.stage == FolderStage::Browse);
        self.folders = rest;
        for f in failed {
            self.error(
                "download_folder",
                format!("could not list {} from {username}: {why}", f.folder),
            );
        }
    }

    /// Queue a download for every file directly in `folder`. With
    /// `exact_only` false (a folder-contents answer), a reply whose folder
    /// spelling differs from ours is taken whole (G14). Returns false when
    /// `exact_only` and the folder is not in `dirs`.
    fn expand_folder(
        &mut self,
        username: &str,
        folder: &str,
        dirs: &[BrowseDir],
        exact_only: bool,
    ) -> bool {
        let matched: Vec<&BrowseDir> = dirs
            .iter()
            .filter(|d| same_folder(&d.path, folder))
            .collect();
        let files: Vec<RemoteFile> = if !matched.is_empty() {
            matched
                .iter()
                .flat_map(|d| d.files.iter().cloned())
                .collect()
        } else if exact_only {
            return false;
        } else {
            dirs.iter().flat_map(|d| d.files.iter().cloned()).collect()
        };
        if files.is_empty() {
            self.error(
                "download_folder",
                format!("{folder} from {username} has no files"),
            );
            return true;
        }
        for file in files {
            let id = TransferId(self.next_id());
            let dir = paths::download_dir_for(&self.settings.download_dir, &file.path);
            self.fetch(id, file, dir, TransferKind::Download);
        }
        true
    }

    // -------------------------------------------------------------- transfers

    fn fetch(&mut self, id: TransferId, file: RemoteFile, dir: PathBuf, kind: TransferKind) {
        let final_path = paths::final_path(&dir, &file.path);
        let part_path = paths::part_path(&dir, &file.path);
        if kind == TransferKind::Play {
            self.current_play = Some(id);
            if let Err(e) = cache::write_meta(&dir, &file.username, &file.path) {
                self.transfers.insert(
                    id,
                    Transfer {
                        kind,
                        file: file.clone(),
                        dest_dir: dir,
                        part_path: part_path.clone(),
                        final_path,
                        state: TState::Failed,
                        linked: false,
                    },
                );
                self.emit(CoreEvent::TransferQueued {
                    id,
                    kind,
                    file,
                    part_path,
                });
                self.emit(CoreEvent::TransferFailed {
                    id,
                    reason: format!("cache dir: {e}"),
                });
                return;
            }
        }
        self.transfers.insert(
            id,
            Transfer {
                kind,
                file,
                dest_dir: dir,
                part_path,
                final_path,
                state: TState::Active,
                linked: false,
            },
        );
        self.begin(id);
    }

    fn queued(&self, id: TransferId) {
        if let Some(t) = self.transfers.get(&id) {
            self.emit(CoreEvent::TransferQueued {
                id,
                kind: t.kind,
                file: t.file.clone(),
                part_path: t.part_path.clone(),
            });
        }
    }

    fn done(&mut self, id: TransferId, path: PathBuf) {
        if let Some(t) = self.transfers.get_mut(&id) {
            t.state = TState::Done;
        }
        self.emit(CoreEvent::TransferDone { id, path });
    }

    fn fail(&mut self, id: TransferId, reason: impl Into<String>) {
        if let Some(t) = self.transfers.get_mut(&id) {
            t.state = TState::Failed;
        }
        self.emit(CoreEvent::TransferFailed {
            id,
            reason: reason.into(),
        });
    }

    /// Start (or restart, for retry) transfer `id`: attach to an in-flight
    /// fetch of the same file, reuse a complete cached copy, or ask the library.
    fn begin(&mut self, id: TransferId) {
        let Some(t) = self.transfers.get(&id) else {
            return;
        };
        let (kind, file, dest_dir) = (t.kind, t.file.clone(), t.dest_dir.clone());
        let key: Key = (file.username.clone(), file.path.clone());

        // 1. Same file already in flight: subscribe (play→keep promotion, G11).
        if let Some(f) = self.fetches.get_mut(&key) {
            let dup_download = kind == TransferKind::Download
                && f.subs.iter().any(|s| {
                    self.transfers
                        .get(s)
                        .is_some_and(|t| t.kind == TransferKind::Download)
                });
            if dup_download {
                let owner = f.owner;
                self.queued(id);
                self.fail(
                    id,
                    format!("already being downloaded (transfer {})", owner.0),
                );
                return;
            }
            f.subs.push(id);
            let fetch_part = f.part.clone();
            let got = f.got_bytes;
            if let Some(t) = self.transfers.get_mut(&id) {
                t.linked = false;
                if kind == TransferKind::Download || fetch_part == t.part_path {
                    t.part_path = fetch_part;
                    t.linked = true;
                }
            }
            self.queued(id);
            if got {
                self.mirror_progress(&key);
            }
            return;
        }

        // 2. A complete cached copy: no network.
        if let Some(src) = self.cache_index.get(&key).filter(|p| p.is_file()).cloned() {
            match kind {
                TransferKind::Play => {
                    let (part, fin) = {
                        let t = &self.transfers[&id];
                        (t.part_path.clone(), t.final_path.clone())
                    };
                    self.queued(id);
                    let res = cache::link_or_copy(&src, &part)
                        .and_then(|()| cache::link_or_copy(&src, &fin));
                    match res {
                        Ok(()) => {
                            self.emit(CoreEvent::TransferProgress {
                                id,
                                bytes: file.size,
                                total: file.size,
                                speed: 0.0,
                                part_path: part,
                            });
                            self.done(id, fin);
                        }
                        Err(e) => self.fail(id, format!("cache: {e}")),
                    }
                }
                TransferKind::Download => {
                    if let Some(t) = self.transfers.get_mut(&id) {
                        t.part_path = src.clone();
                    }
                    self.queued(id);
                    let dest = cache::unique_path(&dest_dir, file.basename());
                    match cache::link_or_copy(&src, &dest) {
                        Ok(()) => self.done(id, dest),
                        Err(e) => self.fail(id, format!("{}: {e}", dest.display())),
                    }
                }
            }
            return;
        }

        // 3. Ask the library.
        let lib_dir = match kind {
            TransferKind::Play => dest_dir,
            TransferKind::Download => paths::staging_transfer_dir(&self.settings.download_dir, id),
        };
        let part = paths::part_path(&lib_dir, &file.path);
        let final_path = paths::final_path(&lib_dir, &file.path);
        if let Some(t) = self.transfers.get_mut(&id) {
            t.part_path = part.clone();
            t.linked = true;
        }
        self.queued(id);
        let Some(s) = self.session() else {
            self.fail(id, "not connected");
            return;
        };
        let Some(dir_str) = lib_dir.to_str() else {
            self.fail(id, "target path is not valid UTF-8");
            return;
        };
        // No transfer of this file is active (step 1), so this only clears
        // dead entries of earlier attempts.
        s.remove_download(&file.username, &file.path);
        match s.download(&file.username, &file.path, file.size, dir_str) {
            Ok(rx) => {
                self.fetches.insert(
                    key,
                    Fetch {
                        owner: id,
                        owner_kind: kind,
                        lib_dir,
                        part,
                        final_path,
                        rx,
                        started: Instant::now(),
                        got_bytes: false,
                        bytes: 0,
                        total: file.size,
                        speed: 0.0,
                        place_requested: false,
                        last_place: None,
                        last_place_poll: Instant::now(),
                        subs: vec![id],
                    },
                );
            }
            Err(e) => self.fail(id, e),
        }
    }

    fn poll_transfers(&mut self) {
        let keys: Vec<Key> = self.fetches.keys().cloned().collect();
        for key in keys {
            while let Some(f) = self.fetches.get(&key) {
                match f.rx.try_recv() {
                    Ok(DownloadStatus::InProgress {
                        bytes_downloaded,
                        total_bytes,
                        speed_bytes_per_sec,
                    }) => {
                        if let Some(f) = self.fetches.get_mut(&key) {
                            f.got_bytes = true;
                            f.bytes = bytes_downloaded;
                            f.total = total_bytes;
                            f.speed = speed_bytes_per_sec;
                        }
                        self.mirror_progress(&key);
                    }
                    Ok(DownloadStatus::Completed) => {
                        self.complete_fetch(&key);
                        break;
                    }
                    Ok(DownloadStatus::Failed(reason)) => {
                        self.fail_fetch(&key, &reason.unwrap_or_else(|| "transfer failed".into()));
                        break;
                    }
                    Ok(DownloadStatus::TimedOut) => {
                        self.fail_fetch(&key, "transfer timed out");
                        break;
                    }
                    Ok(DownloadStatus::Cancelled) => {
                        self.fetches.remove(&key);
                        break;
                    }
                    Ok(DownloadStatus::Queued | DownloadStatus::Paused { .. }) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.fail_fetch(&key, "transfer ended unexpectedly");
                        break;
                    }
                }
            }
            self.fetch_timers(&key);
        }
    }

    /// Progress to every subscriber; a play mirroring another transfer's
    /// file gets a hard link to the growing `.part` at its own ticket path.
    fn mirror_progress(&mut self, key: &Key) {
        let Some(f) = self.fetches.get(key) else {
            return;
        };
        let (part, bytes, total, speed, subs) =
            (f.part.clone(), f.bytes, f.total, f.speed, f.subs.clone());
        for id in subs {
            let Some(t) = self.transfers.get_mut(&id) else {
                continue;
            };
            if !t.linked {
                let target = t.part_path.clone();
                let linked = target
                    .parent()
                    .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                    && (target.exists() || std::fs::hard_link(&part, &target).is_ok());
                if !linked {
                    continue; // .part may not exist yet; retried on the next progress
                }
                t.linked = true;
            }
            let part_path = t.part_path.clone();
            self.emit(CoreEvent::TransferProgress {
                id,
                bytes,
                total,
                speed,
                part_path,
            });
        }
    }

    fn fetch_timers(&mut self, key: &Key) {
        let session = self.session();
        let t = self.t.clone();
        let Some(f) = self.fetches.get_mut(key) else {
            return;
        };
        if f.got_bytes {
            return;
        }
        let elapsed = f.started.elapsed();
        let mut place_events = Vec::new();
        if elapsed >= t.queue_place_after
            && let Some(s) = &session
        {
            if !f.place_requested {
                f.place_requested = true;
                s.request_place_in_queue(&key.0, &key.1);
                place_events.extend(f.subs.iter().map(|id| (*id, None)));
            } else if f.last_place_poll.elapsed() >= Duration::from_secs(1) {
                f.last_place_poll = Instant::now();
                let place = s.queue_place(&key.0, &key.1);
                if place.is_some() && place != f.last_place {
                    f.last_place = place;
                    place_events.extend(f.subs.iter().map(|id| (*id, place)));
                }
            }
        }
        for (id, place) in place_events {
            self.emit(CoreEvent::TransferRemoteQueued { id, place });
        }

        // G8: a ▶ that has not started within the timeout fails, so the UI can
        // try another source. A ↓ on the same fetch keeps waiting in the queue.
        if elapsed < t.play_start_timeout {
            return;
        }
        let Some(f) = self.fetches.get_mut(key) else {
            return;
        };
        let plays: Vec<TransferId> = f
            .subs
            .iter()
            .copied()
            .filter(|id| {
                self.transfers
                    .get(id)
                    .is_some_and(|t| t.kind == TransferKind::Play)
            })
            .collect();
        if plays.is_empty() {
            return;
        }
        f.subs.retain(|id| !plays.contains(id));
        let empty = f.subs.is_empty();
        if empty {
            if let Some(s) = &session {
                s.cancel_download(&key.0, &key.1);
            }
            self.fetches.remove(key);
        }
        for id in plays {
            self.fail(
                id,
                format!(
                    "{} did not start sending within {}s (queued or unreachable); try another source",
                    key.0,
                    t.play_start_timeout.as_secs()
                ),
            );
        }
    }

    fn complete_fetch(&mut self, key: &Key) {
        let Some(f) = self.fetches.remove(key) else {
            return;
        };
        let src = f.final_path.clone();
        if !src.is_file() {
            for id in f.subs {
                self.fail(id, format!("completed but {} is missing", src.display()));
            }
            return;
        }
        let (plays, downloads): (Vec<TransferId>, Vec<TransferId>) =
            f.subs.iter().partition(|id| {
                self.transfers
                    .get(id)
                    .is_some_and(|t| t.kind == TransferKind::Play)
            });
        if f.owner_kind == TransferKind::Play {
            self.cache_index.insert(key.clone(), src.clone());
        }
        for id in plays {
            let fin = self.transfers[&id].final_path.clone();
            if fin != src
                && let Err(e) = cache::link_or_copy(&src, &fin)
            {
                self.fail(id, format!("cache: {e}"));
                continue;
            }
            self.cache_index.insert(key.clone(), fin.clone());
            self.done(id, fin);
        }
        for id in downloads {
            let (dest_dir, name) = {
                let t = &self.transfers[&id];
                (t.dest_dir.clone(), t.file.basename().to_string())
            };
            let dest = cache::unique_path(&dest_dir, &name);
            // A staged ↓ is moved; a ▶-owned file stays in the cache (linked/copied).
            let res = if f.owner_kind == TransferKind::Download {
                cache::move_file(&src, &dest)
            } else {
                cache::link_or_copy(&src, &dest)
            };
            match res {
                Ok(()) => self.done(id, dest),
                Err(e) => self.fail(id, format!("{}: {e}", dest.display())),
            }
        }
        if f.owner_kind == TransferKind::Download {
            remove_staging(&f.lib_dir);
        }
        self.evict();
    }

    fn fail_fetch(&mut self, key: &Key, reason: &str) {
        let Some(f) = self.fetches.remove(key) else {
            return;
        };
        for id in f.subs {
            self.fail(id, reason.to_string());
        }
    }

    fn cancel(&mut self, id: TransferId) {
        let Some(t) = self.transfers.get(&id) else {
            self.error("cancel", format!("unknown transfer {}", id.0));
            return;
        };
        match t.state {
            TState::Active => {}
            TState::Failed => {
                if let Some(t) = self.transfers.get_mut(&id) {
                    t.state = TState::Cancelled;
                }
                self.emit(CoreEvent::TransferCancelled { id });
                return;
            }
            TState::Done | TState::Cancelled => return,
        }
        let key: Key = (t.file.username.clone(), t.file.path.clone());
        if let Some(f) = self.fetches.get_mut(&key)
            && f.subs.contains(&id)
        {
            f.subs.retain(|s| *s != id);
            if f.subs.is_empty() {
                if let Some(s) = self.session() {
                    s.cancel_download(&key.0, &key.1);
                }
                if let Some(f) = self.fetches.remove(&key)
                    && f.owner_kind == TransferKind::Download
                {
                    remove_staging(&f.lib_dir);
                }
            }
        }
        if let Some(t) = self.transfers.get_mut(&id) {
            t.state = TState::Cancelled;
        }
        if self.current_play == Some(id) {
            self.current_play = None;
        }
        self.emit(CoreEvent::TransferCancelled { id });
    }

    fn retry(&mut self, id: TransferId) {
        let Some(t) = self.transfers.get_mut(&id) else {
            self.error("retry", format!("unknown transfer {}", id.0));
            return;
        };
        if !matches!(t.state, TState::Failed | TState::Cancelled) {
            self.error(
                "retry",
                format!("transfer {} is not failed or cancelled", id.0),
            );
            return;
        }
        t.state = TState::Active;
        if t.kind == TransferKind::Play {
            self.current_play = Some(id);
        }
        self.begin(id);
    }

    /// Keep the cache under its cap: delete the oldest `t<id>` dirs that are
    /// not playing and not being written.
    fn evict(&mut self) {
        let mut protected: HashSet<u64> = HashSet::new();
        protected.extend(self.current_play.map(|p| p.0));
        protected.extend(
            self.transfers
                .iter()
                .filter(|(_, t)| t.kind == TransferKind::Play && t.state == TState::Active)
                .map(|(id, _)| id.0),
        );
        protected.extend(
            self.fetches
                .values()
                .filter(|f| f.owner_kind == TransferKind::Play)
                .map(|f| f.owner.0),
        );
        let entries = cache::scan(&self.settings.cache_dir);
        for id in cache::plan_eviction(&entries, self.settings.cache_cap_bytes, &protected) {
            let dir = paths::cache_transfer_dir(&self.settings.cache_dir, TransferId(id));
            let _ = std::fs::remove_dir_all(&dir);
            self.cache_index.retain(|_, p| !p.starts_with(&dir));
        }
    }
}

fn remove_staging(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent); // only if empty
    }
}

fn share_dir_strings(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect()
}

fn trim_folder(folder: &str) -> &str {
    folder.trim_end_matches(['\\', '/'])
}

fn same_folder(a: &str, b: &str) -> bool {
    let norm = |s: &str| trim_folder(s).replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}\\{name}", trim_folder(dir))
    }
}

pub(crate) fn to_browse_dirs(username: &str, listing: Vec<SharedDirectory>) -> Vec<BrowseDir> {
    listing
        .into_iter()
        .map(|d| BrowseDir {
            files: d
                .files
                .into_iter()
                .map(|f| RemoteFile {
                    username: username.to_string(),
                    path: join_remote(&d.name, &f.name),
                    size: f.size,
                    attrs: AudioAttrs::from_pairs(f.attributes),
                })
                .collect(),
            path: d.name,
        })
        .collect()
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
