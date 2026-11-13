//! Worker logic against a fake session: no network, temp dirs only.

use crate::core::Core;
use crate::event::{CoreEvent, DisconnectReason};
use crate::model::{
    AudioAttrs, Credentials, RemoteFile, Settings, TransferId, TransferKind, UploadState,
    UserProfile,
};
use crate::net::{ConnectParams, Connector, LoginError, Session};
use crate::worker::Tunables;
use soulseek_rs::message::peer::PeerInfo;
use soulseek_rs::{
    DownloadStatus, File, SearchResult, SessionLoss, SharedDirectory, SharedFileEntry, UploadInfo,
    UploadStatus,
};
use soulseek_rs::{UserInfo, UserPresence, UserStats, UserStatus};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------- fakes

struct FakeDl {
    path: String,
    dir: PathBuf,
    tx: Sender<DownloadStatus>,
}

#[derive(Default)]
struct FakeState {
    loss: Option<SessionLoss>,
    searches: HashMap<String, Vec<SearchResult>>,
    forgotten: Vec<String>,
    browses: HashMap<String, Vec<SharedDirectory>>,
    browse_requests: Vec<String>,
    folders: HashMap<(String, String), Vec<SharedDirectory>>,
    folder_requests: Vec<(String, String)>,
    downloads: Vec<FakeDl>,
    cancels: Vec<(String, String)>,
    place_requests: usize,
    uploads: Vec<UploadInfo>,
    upload_cancels: Vec<(String, String)>,
    share_sets: Vec<Vec<String>>,
    /// `listen_port()` answer; `None` = the requested 2234.
    bound: Option<Option<u16>>,
    search_linger: Duration,
    /// Wishlist searches sent (code 103), in order.
    wishes: Vec<String>,
    /// `wishlist_interval()` answer; zero = use the core's minimum.
    wish_interval: Duration,
    share_delay: Duration,
    user_infos: HashMap<String, UserInfo>,
    peer_infos: HashMap<String, PeerInfo>,
    profile_requests: Vec<String>,
    slots: usize,
}

#[derive(Default)]
struct FakeSession {
    s: Mutex<FakeState>,
}

impl FakeSession {
    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.s.lock().unwrap()
    }
    /// Simulate the library: write the `.part`, report progress.
    fn progress(&self, i: usize, data: &[u8], total: u64) -> PathBuf {
        let st = self.st();
        let d = &st.downloads[i];
        let part = crate::paths::part_path(&d.dir, &d.path);
        std::fs::create_dir_all(&d.dir).unwrap();
        std::fs::write(&part, data).unwrap();
        d.tx.send(DownloadStatus::InProgress {
            bytes_downloaded: data.len() as u64,
            total_bytes: total,
            speed_bytes_per_sec: 1000.0,
        })
        .unwrap();
        part
    }
    /// Simulate the library finishing: rename `.part` → final, report Completed.
    fn complete(&self, i: usize) -> PathBuf {
        let st = self.st();
        let d = &st.downloads[i];
        let part = crate::paths::part_path(&d.dir, &d.path);
        let fin = crate::paths::final_path(&d.dir, &d.path);
        std::fs::rename(&part, &fin).unwrap();
        d.tx.send(DownloadStatus::Completed).unwrap();
        fin
    }
}

impl Session for FakeSession {
    fn listen_port(&self) -> Option<u16> {
        self.st().bound.unwrap_or(Some(2234))
    }
    fn session_loss(&self) -> Option<SessionLoss> {
        self.st().loss
    }
    fn search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>) {
        self.st().searches.insert(query.to_string(), Vec::new());
        let start = Instant::now();
        while start.elapsed() < window && !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Like the real helper, it may hold the session a while after cancel.
        let linger = self.st().search_linger;
        std::thread::sleep(linger);
    }
    fn wishlist_search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>) {
        self.st().wishes.push(query.to_string());
        self.search_blocking(query, window, cancel);
    }
    fn wishlist_interval(&self) -> Duration {
        self.st().wish_interval
    }
    fn search_count(&self, q: &str) -> usize {
        self.st().searches.get(q).map_or(0, Vec::len)
    }
    fn search_results(&self, q: &str) -> Vec<SearchResult> {
        self.st().searches.get(q).cloned().unwrap_or_default()
    }
    fn forget_search(&self, q: &str) {
        let mut st = self.st();
        st.searches.remove(q);
        st.forgotten.push(q.to_string());
    }
    fn browse(&self, u: &str) -> Result<(), String> {
        self.st().browse_requests.push(u.to_string());
        Ok(())
    }
    fn take_browse(&self, u: &str) -> Option<Vec<SharedDirectory>> {
        self.st().browses.remove(u)
    }
    fn request_folder(&self, u: &str, f: &str) -> Result<(), String> {
        self.st().folder_requests.push((u.into(), f.into()));
        Ok(())
    }
    fn take_folder(&self, u: &str, f: &str) -> Option<Vec<SharedDirectory>> {
        self.st().folders.remove(&(u.to_string(), f.to_string()))
    }
    fn download(
        &self,
        _user: &str,
        path: &str,
        _size: u64,
        dir: &str,
    ) -> Result<Receiver<DownloadStatus>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.st().downloads.push(FakeDl {
            path: path.into(),
            dir: dir.into(),
            tx,
        });
        Ok(rx)
    }
    fn cancel_download(&self, u: &str, p: &str) -> bool {
        self.st().cancels.push((u.into(), p.into()));
        true
    }
    fn remove_download(&self, _u: &str, _p: &str) {}
    fn request_place_in_queue(&self, _u: &str, _p: &str) {
        self.st().place_requests += 1;
    }
    fn queue_place(&self, _u: &str, _p: &str) -> Option<u32> {
        Some(7)
    }
    fn set_shares(&self, dirs: Vec<String>) -> Result<(u32, u32), String> {
        let delay = self.st().share_delay;
        std::thread::sleep(delay);
        self.st().share_sets.push(dirs);
        Ok((1, 42))
    }
    fn shared_counts(&self) -> (u32, u32) {
        (0, 0)
    }
    fn uploads(&self) -> Vec<UploadInfo> {
        self.st().uploads.clone()
    }
    fn cancel_upload(&self, u: &str, p: &str) -> bool {
        self.st().upload_cancels.push((u.into(), p.into()));
        true
    }
    fn request_user_info(&self, u: &str) -> Result<(), String> {
        let mut st = self.st();
        st.user_infos.remove(u);
        st.profile_requests.push(u.to_string());
        Ok(())
    }
    fn user_info(&self, u: &str) -> Option<UserInfo> {
        self.st().user_infos.get(u).cloned()
    }
    fn request_peer_info(&self, u: &str) -> Result<(), String> {
        self.st().peer_infos.remove(u);
        Ok(())
    }
    fn peer_info(&self, u: &str) -> Option<PeerInfo> {
        self.st().peer_infos.get(u).cloned()
    }
    fn set_upload_slots(&self, n: usize) {
        self.st().slots = n;
    }
}

#[derive(Default)]
struct FakeConnector {
    replies: Mutex<VecDeque<Result<Arc<FakeSession>, LoginError>>>,
    params: Mutex<Vec<ConnectParams>>,
    /// Sessions handed out, to check none is still alive at the next connect.
    handed_out: Mutex<Vec<std::sync::Weak<FakeSession>>>,
    /// Per connect: was any earlier session still referenced?
    prev_alive: Mutex<Vec<bool>>,
    connected_at: Mutex<Vec<Instant>>,
}

impl Connector for FakeConnector {
    fn connect(&self, p: &ConnectParams) -> Result<Arc<dyn Session>, LoginError> {
        self.params.lock().unwrap().push(p.clone());
        self.connected_at.lock().unwrap().push(Instant::now());
        let alive = self
            .handed_out
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.strong_count() > 0);
        self.prev_alive.lock().unwrap().push(alive);
        let r = self.replies.lock().unwrap().pop_front();
        match r {
            Some(Ok(s)) => {
                self.handed_out.lock().unwrap().push(Arc::downgrade(&s));
                Ok(s)
            }
            Some(Err(e)) => Err(e),
            None => Err(LoginError::Other("no scripted reply".into())),
        }
    }
}

// ----------------------------------------------------------------- harness

struct H {
    core: Core,
    ev: Receiver<CoreEvent>,
    conn: Arc<FakeConnector>,
    tmp: tempfile::TempDir,
}

fn tunables() -> Tunables {
    Tunables {
        tick: Duration::from_millis(10),
        search_window: Duration::from_millis(300),
        browse_timeout: Duration::from_millis(200),
        folder_timeout: Duration::from_millis(100),
        play_start_timeout: Some(Duration::from_millis(400)),
        queue_place_after: Duration::from_millis(50),
        uploads_every: Duration::from_millis(20),
        profile_timeout: Duration::from_millis(300),
        release_wait: Duration::from_secs(2),
        cache_rescan: Duration::from_millis(50),
        wishlist_first: Duration::from_millis(50),
        wishlist_min: Duration::from_millis(250),
        backoff: |_| Duration::from_millis(30),
    }
}

fn harness_with(f: impl FnOnce(&mut Settings, &Path)) -> H {
    harness_tuned(tunables(), f)
}

fn harness_tuned(t: Tunables, f: impl FnOnce(&mut Settings, &Path)) -> H {
    let tmp = tempfile::tempdir().unwrap();
    let mut settings = Settings {
        download_dir: tmp.path().join("downloads"),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
        port_mapping: false,
        ..Settings::default()
    };
    f(&mut settings, tmp.path());
    let conn = Arc::new(FakeConnector::default());
    let (core, ev) = Core::start_with(settings, conn.clone(), t).unwrap();
    H {
        core,
        ev,
        conn,
        tmp,
    }
}

fn harness() -> H {
    harness_with(|_, _| {})
}

impl H {
    fn script(&self, r: Result<Arc<FakeSession>, LoginError>) {
        self.conn.replies.lock().unwrap().push_back(r);
    }
    /// Wait for the first event matching `pred`, discarding others.
    fn want(&self, what: &str, pred: impl Fn(&CoreEvent) -> bool) -> CoreEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            if let Ok(e) = self.ev.recv_timeout(Duration::from_millis(20)) {
                if pred(&e) {
                    return e;
                }
                seen.push(e);
            }
        }
        panic!("timed out waiting for {what}; saw {seen:#?}");
    }
    /// Assert no matching event arrives within `d`.
    fn none_within(&self, d: Duration, pred: impl Fn(&CoreEvent) -> bool) {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            if let Ok(e) = self.ev.recv_timeout(Duration::from_millis(10)) {
                assert!(!pred(&e), "unexpected {e:?}");
            }
        }
    }
    fn online(&self) -> Arc<FakeSession> {
        let s = Arc::new(FakeSession::default());
        self.script(Ok(s.clone()));
        self.core.login(creds());
        self.want("Connected", |e| matches!(e, CoreEvent::Connected { .. }));
        s
    }
    fn downloads_dir(&self) -> PathBuf {
        self.tmp.path().join("downloads")
    }
    fn cache_dir(&self) -> PathBuf {
        self.tmp.path().join("cache")
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out: {what}");
}

fn creds() -> Credentials {
    Credentials {
        username: "me".into(),
        password: "pw".into(),
    }
}

fn rfile(path: &str, size: u64) -> RemoteFile {
    RemoteFile {
        username: "alice".into(),
        path: path.into(),
        size,
        attrs: AudioAttrs::default(),
    }
}

fn result(user: &str, names: &[&str]) -> SearchResult {
    SearchResult {
        token: 1,
        files: names
            .iter()
            .map(|n| File {
                username: user.into(),
                name: (*n).into(),
                size: 10,
                attribs: HashMap::new(),
            })
            .collect(),
        slots: 1,
        speed: 100,
        username: user.into(),
    }
}

fn dir(name: &str, files: &[&str]) -> SharedDirectory {
    SharedDirectory {
        name: name.into(),
        files: files
            .iter()
            .map(|f| SharedFileEntry {
                name: (*f).into(),
                size: 4,
                attributes: vec![(1, 200)],
            })
            .collect(),
    }
}

/// Every regular file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn is_done(id: TransferId) -> impl Fn(&CoreEvent) -> bool {
    move |e| matches!(e, CoreEvent::TransferDone { id: i, .. } if *i == id)
}

fn done_path(e: CoreEvent) -> PathBuf {
    match e {
        CoreEvent::TransferDone { path, .. } => path,
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------- session

#[test]
fn login_connects_and_reports_bound_port_and_slots() {
    let h = harness();
    let s = Arc::new(FakeSession::default());
    h.script(Ok(s.clone()));
    h.core.login(creds());
    match h.want("Connected", |e| matches!(e, CoreEvent::Connected { .. })) {
        CoreEvent::Connected {
            username,
            listen_port,
        } => {
            assert_eq!(username, "me");
            assert_eq!(listen_port, Some(2234));
        }
        _ => unreachable!(),
    }
    assert_eq!(s.st().slots, 10);
    let p = h.conn.params.lock().unwrap()[0].clone();
    assert_eq!(p.listen_port, 2234);
}

#[test]
fn rejected_login_does_not_retry() {
    let h = harness();
    h.script(Err(LoginError::Rejected));
    h.core.login(creds());
    let e = h.want("Disconnected", |e| {
        matches!(e, CoreEvent::Disconnected { .. })
    });
    assert_eq!(
        e,
        CoreEvent::Disconnected {
            reason: DisconnectReason::LoginRejected,
            will_retry: false
        }
    );
    h.none_within(Duration::from_millis(150), |e| {
        matches!(e, CoreEvent::Connected { .. })
    });
    assert_eq!(h.conn.params.lock().unwrap().len(), 1);
}

#[test]
fn network_failure_retries_with_backoff_then_connects() {
    let h = harness();
    h.script(Err(LoginError::Other("connection refused".into())));
    h.script(Ok(Arc::new(FakeSession::default())));
    h.core.login(creds());
    let e = h.want("Disconnected", |e| {
        matches!(e, CoreEvent::Disconnected { .. })
    });
    assert!(matches!(
        e,
        CoreEvent::Disconnected {
            reason: DisconnectReason::Network(_),
            will_retry: true
        }
    ));
    h.want("Connected", |e| matches!(e, CoreEvent::Connected { .. }));
    assert_eq!(h.conn.params.lock().unwrap().len(), 2);
}

#[test]
fn dropped_session_reconnects_but_displaced_does_not() {
    let h = harness();
    let s1 = h.online();
    let s2 = Arc::new(FakeSession::default());
    h.script(Ok(s2.clone()));
    s1.st().loss = Some(SessionLoss::Disconnected);
    let e = h.want("Disconnected", |e| {
        matches!(e, CoreEvent::Disconnected { .. })
    });
    assert!(matches!(
        e,
        CoreEvent::Disconnected {
            will_retry: true,
            ..
        }
    ));
    h.want("reconnected", |e| matches!(e, CoreEvent::Connected { .. }));

    s2.st().loss = Some(SessionLoss::Displaced);
    let e = h.want("Disconnected", |e| {
        matches!(e, CoreEvent::Disconnected { .. })
    });
    assert_eq!(
        e,
        CoreEvent::Disconnected {
            reason: DisconnectReason::Displaced,
            will_retry: false
        }
    );
    h.none_within(Duration::from_millis(150), |e| {
        matches!(e, CoreEvent::Connected { .. })
    });
}

#[test]
fn logout_stops_and_listen_port_change_reconnects() {
    let mut h = harness();
    let _s = h.online();
    h.script(Ok(Arc::new(FakeSession::default())));
    let mut new = h.core.settings().clone();
    new.listen_port = 40_000;
    h.core.apply_settings(new);
    h.want("reconnected", |e| matches!(e, CoreEvent::Connected { .. }));
    assert_eq!(
        h.conn.params.lock().unwrap().last().unwrap().listen_port,
        40_000
    );
    h.core.logout();
    let e = h.want("Disconnected", |e| {
        matches!(
            e,
            CoreEvent::Disconnected {
                reason: DisconnectReason::User,
                ..
            }
        )
    });
    assert!(matches!(
        e,
        CoreEvent::Disconnected {
            will_retry: false,
            ..
        }
    ));
}

// -------------------------------------------------------------------- search

#[test]
fn search_streams_ranked_deltas_then_finishes_and_forgets() {
    let h = harness();
    let s = h.online();
    let id = h.core.search("  daft punk ");
    wait_until("search registered", || {
        s.st().searches.contains_key("daft punk")
    });
    s.st()
        .searches
        .get_mut("daft punk")
        .unwrap()
        .push(result("a", &[r"x\1.mp3"]));
    let e = h.want("batch 1", |e| matches!(e, CoreEvent::SearchResults { .. }));
    let CoreEvent::SearchResults { search, hits } = e else {
        unreachable!()
    };
    assert_eq!(search, id);
    assert_eq!(hits.len(), 1);
    s.st()
        .searches
        .get_mut("daft punk")
        .unwrap()
        .push(result("b", &[r"y\cover.jpg", r"y\2.ogg", r"y\1.flac"]));
    let CoreEvent::SearchResults { hits, .. } =
        h.want("batch 2", |e| matches!(e, CoreEvent::SearchResults { .. }))
    else {
        unreachable!()
    };
    let paths: Vec<_> = hits.iter().map(|h| h.file.path.as_str()).collect();
    assert_eq!(
        paths,
        [r"y\2.ogg", r"y\1.flac"],
        "only the delta, audio-only, arrival order (ranking is format-neutral)"
    );
    let e = h.want("finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: id,
            total_files: 3
        }
    );
    assert!(s.st().forgotten.contains(&"daft punk".to_string()));
}

#[test]
fn audio_only_can_be_turned_off() {
    let h = harness();
    let s = h.online();
    h.core.set_audio_only(false);
    let _ = h.core.search("q");
    wait_until("registered", || s.st().searches.contains_key("q"));
    s.st()
        .searches
        .get_mut("q")
        .unwrap()
        .push(result("a", &[r"x\cover.jpg"]));
    let CoreEvent::SearchResults { hits, .. } =
        h.want("batch", |e| matches!(e, CoreEvent::SearchResults { .. }))
    else {
        unreachable!()
    };
    assert_eq!(hits.len(), 1);
}

#[test]
fn same_query_twice_ends_the_first_and_cancel_finishes() {
    let h = harness();
    let _s = h.online();
    let a = h.core.search("q");
    let b = h.core.search("q");
    let e = h.want("first finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: a,
            total_files: 0
        }
    );
    h.core.cancel_search(b);
    let e = h.want("second finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: b,
            total_files: 0
        }
    );
}

#[test]
fn search_offline_errors_and_finishes() {
    let h = harness();
    let id = h.core.search("q");
    h.want(
        "error",
        |e| matches!(e, CoreEvent::Error { context, .. } if context == "search"),
    );
    h.want("finished", |e| {
        *e == CoreEvent::SearchFinished {
            search: id,
            total_files: 0,
        }
    });
}

// -------------------------------------------------------------------- browse

#[test]
fn browse_result_and_timeout() {
    let h = harness();
    let s = h.online();
    s.st()
        .browses
        .insert("bob".into(), vec![dir(r"Music\Album", &["01.flac"])]);
    h.core.browse("bob");
    let CoreEvent::BrowseResult { username, dirs } =
        h.want("browse", |e| matches!(e, CoreEvent::BrowseResult { .. }))
    else {
        unreachable!()
    };
    assert_eq!(username, "bob");
    assert_eq!(dirs[0].path, r"Music\Album");
    assert_eq!(dirs[0].files[0].path, r"Music\Album\01.flac");
    assert_eq!(dirs[0].files[0].username, "bob");
    assert_eq!(dirs[0].files[0].attrs.duration_secs, Some(200));

    h.core.browse("ghost");
    let e = h.want("BrowseFailed", |e| {
        matches!(e, CoreEvent::BrowseFailed { .. })
    });
    assert!(matches!(e, CoreEvent::BrowseFailed { username, .. } if username == "ghost"));
}

// ----------------------------------------------------------- folder download

#[test]
fn folder_download_from_contents_keeps_folder_name() {
    let h = harness();
    let s = h.online();
    s.st().folders.insert(
        ("alice".into(), r"Music\Album (2019)".into()),
        vec![dir(r"Music\Album (2019)", &["01.flac", "02.flac"])],
    );
    h.core.download_folder("alice", r"Music\Album (2019)\");
    h.want("contents", |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    for _ in 0..2 {
        h.want("queued", |e| {
            matches!(
                e,
                CoreEvent::TransferQueued {
                    kind: TransferKind::Download,
                    ..
                }
            )
        });
    }
    wait_until("2 downloads", || s.st().downloads.len() == 2);
    let staging = h.downloads_dir().join(crate::paths::STAGING_DIR_NAME);
    assert!(s.st().downloads.iter().all(|d| d.dir.starts_with(&staging)));
    let id_done = {
        s.progress(0, b"abcd", 4);
        s.complete(0);
        h.want("done", |e| matches!(e, CoreEvent::TransferDone { .. }))
    };
    assert_eq!(
        done_path(id_done),
        h.downloads_dir().join("Album (2019)").join("01.flac")
    );
}

#[test]
fn folder_download_falls_back_to_full_browse_g14() {
    let h = harness();
    let s = h.online();
    // The peer never answers the folder request under our spelling…
    h.core.download_folder("alice", r"music\album");
    wait_until("fallback browse", || {
        s.st().browse_requests.contains(&"alice".to_string())
    });
    // …but the full listing has it (different case).
    s.st().browses.insert(
        "alice".into(),
        vec![
            dir(r"Music\Album", &["01.flac"]),
            dir(r"Music\Other", &["x.flac"]),
        ],
    );
    let e = h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. }));
    let CoreEvent::TransferQueued { file, .. } = e else {
        unreachable!()
    };
    assert_eq!(file.path, r"Music\Album\01.flac");
    h.none_within(Duration::from_millis(100), |e| {
        matches!(e, CoreEvent::BrowseResult { .. })
    });
    assert_eq!(s.st().downloads.len(), 1);
}

// ----------------------------------------------------------------- transfers

#[test]
fn play_fetch_goes_to_cache_only_and_reports_part_path() {
    let h = harness();
    let s = h.online();
    let t = h.core.play_fetch(&rfile(r"Music\Album\01.flac", 4));
    let CoreEvent::TransferQueued {
        id,
        kind,
        part_path,
        ..
    } = h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. }))
    else {
        unreachable!()
    };
    assert_eq!(
        (id, kind, &part_path),
        (t.id, TransferKind::Play, &t.part_path)
    );
    wait_until("lib download", || s.st().downloads.len() == 1);
    assert_eq!(
        s.st().downloads[0].dir,
        crate::paths::cache_transfer_dir(&h.cache_dir(), t.id)
    );
    let part = s.progress(0, b"ab", 4);
    assert_eq!(part, t.part_path);
    let e = h.want("progress", |e| {
        matches!(e, CoreEvent::TransferProgress { .. })
    });
    assert_eq!(
        e,
        CoreEvent::TransferProgress {
            id: t.id,
            bytes: 2,
            total: 4,
            speed: 1000.0,
            part_path: t.part_path.clone()
        }
    );
    std::fs::write(&part, b"abcd").unwrap();
    s.complete(0);
    assert_eq!(done_path(h.want("done", is_done(t.id))), t.final_path);
    assert!(
        files_under(&h.downloads_dir()).is_empty(),
        "▶ never writes to downloads"
    );
}

#[test]
fn download_during_play_is_promoted_not_refetched() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let t = h.core.play_fetch(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.progress(0, b"ab", 4);
    let d = h.core.download(&f);
    let CoreEvent::TransferQueued { part_path, .. } = h.want(
        "dl queued",
        |e| matches!(e, CoreEvent::TransferQueued { id, .. } if *id == d),
    ) else {
        unreachable!()
    };
    assert_eq!(part_path, t.part_path, "↓ reports the in-flight ▶ bytes");
    std::fs::write(&t.part_path, b"abcd").unwrap();
    s.complete(0);
    let p = done_path(h.want("play done", is_done(t.id)));
    assert_eq!(p, t.final_path);
    let dl = done_path(h.want("dl done", is_done(d)));
    assert_eq!(dl, h.downloads_dir().join("01.flac"), "flat by default");
    assert_eq!(std::fs::read(&dl).unwrap(), b"abcd");
    assert!(t.final_path.exists(), "cache copy stays for the player");
    assert_eq!(s.st().downloads.len(), 1, "only one library transfer");
}

#[test]
fn completed_cache_is_reused_for_download_and_replay() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let t = h.core.play_fetch(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.progress(0, b"abcd", 4);
    s.complete(0);
    h.want("done", is_done(t.id));

    let d = h.core.download(&f);
    let dl = done_path(h.want("dl done", is_done(d)));
    assert_eq!(std::fs::read(&dl).unwrap(), b"abcd");

    let t2 = h.core.play_fetch(&f);
    h.want(
        "replay progress",
        |e| matches!(e, CoreEvent::TransferProgress { id, .. } if *id == t2.id),
    );
    assert!(
        t2.part_path.exists(),
        "progress means part_path is openable"
    );
    assert_eq!(
        done_path(h.want("replay done", is_done(t2.id))),
        t2.final_path
    );
    assert_eq!(std::fs::read(&t2.final_path).unwrap(), b"abcd");
    assert_eq!(s.st().downloads.len(), 1, "no refetch");
}

#[test]
fn cache_index_survives_restart() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let t = h.core.play_fetch(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.progress(0, b"abcd", 4);
    s.complete(0);
    h.want("done", is_done(t.id));
    let H { core, tmp, .. } = h;
    drop(core);

    let conn = Arc::new(FakeConnector::default());
    let settings = Settings {
        download_dir: tmp.path().join("downloads"),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
        port_mapping: false,
        ..Settings::default()
    };
    let (core, ev) = Core::start_with(settings, conn.clone(), tunables()).unwrap();
    let h = H {
        core,
        ev,
        conn,
        tmp,
    };
    let d = h.core.download(&f); // offline: must come from cache
    assert!(d.0 > t.id.0, "ids never reuse an existing t<id>");
    let dl = done_path(h.want("dl done", is_done(d)));
    assert_eq!(std::fs::read(dl).unwrap(), b"abcd");
}

#[test]
fn cancelling_the_play_keeps_a_promoted_download_running() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let t = h.core.play_fetch(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    let d = h.core.download(&f);
    h.want(
        "dl queued",
        |e| matches!(e, CoreEvent::TransferQueued { id, .. } if *id == d),
    );
    h.core.cancel(t.id);
    h.want("cancelled", |e| {
        *e == CoreEvent::TransferCancelled { id: t.id }
    });
    assert!(s.st().cancels.is_empty(), "library transfer must survive");
    s.progress(0, b"abcd", 4);
    s.complete(0);
    let dl = done_path(h.want("dl done", is_done(d)));
    assert!(dl.starts_with(h.downloads_dir()));

    // Cancelling the last subscriber cancels the library transfer.
    let g = rfile(r"Music\Album\02.flac", 4);
    let t2 = h.core.play_fetch(&g);
    wait_until("2nd lib download", || s.st().downloads.len() == 2);
    h.core.cancel(t2.id);
    h.want("cancelled", |e| {
        *e == CoreEvent::TransferCancelled { id: t2.id }
    });
    wait_until("lib cancel", || {
        s.st().cancels == vec![("alice".into(), r"Music\Album\02.flac".into())]
    });
}

#[test]
fn play_attached_to_running_download_gets_its_own_growing_link() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let d = h.core.download(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    let t = h.core.play_fetch(&f);
    h.want(
        "play queued",
        |e| matches!(e, CoreEvent::TransferQueued { id, .. } if *id == t.id),
    );
    s.progress(0, b"ab", 4);
    h.want(
        "play progress",
        |e| matches!(e, CoreEvent::TransferProgress { id, .. } if *id == t.id),
    );
    assert_eq!(
        std::fs::read(&t.part_path).unwrap(),
        b"ab",
        "ticket path shows the growing bytes"
    );
    s.complete(0);
    assert_eq!(done_path(h.want("play done", is_done(t.id))), t.final_path);
    let dl = done_path(h.want("dl done", is_done(d)));
    assert_eq!(dl.parent().unwrap(), h.downloads_dir());
    wait_until("staging cleaned up", || {
        !h.downloads_dir()
            .join(crate::paths::STAGING_DIR_NAME)
            .exists()
    });
}

#[test]
fn duplicate_download_is_refused() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let _a = h.core.download(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    let b = h.core.download(&f);
    h.want(
        "refused",
        |e| matches!(e, CoreEvent::TransferFailed { id, .. } if *id == b),
    );
    assert_eq!(s.st().downloads.len(), 1);
}

#[test]
fn play_that_never_starts_times_out_and_is_cancelled_then_retry_resumes() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"Music\Album\01.flac", 4);
    let t = h.core.play_fetch(&f);
    h.want(
        "remote queued",
        |e| matches!(e, CoreEvent::TransferRemoteQueued { id, .. } if *id == t.id),
    );
    let e = h.want("failed", |e| matches!(e, CoreEvent::TransferFailed { .. }));
    assert!(matches!(e, CoreEvent::TransferFailed { id, .. } if id == t.id));
    assert_eq!(s.st().cancels.len(), 1);
    assert!(s.st().place_requests >= 1);

    h.core.retry(t.id);
    wait_until("retried", || s.st().downloads.len() == 2);
    {
        let st = s.st();
        assert_eq!(
            st.downloads[1].dir, st.downloads[0].dir,
            "same dir → library resumes the .part"
        );
    }
    s.progress(1, b"abcd", 4);
    s.complete(1);
    h.want("done", is_done(t.id));
}

#[test]
fn library_failure_and_session_loss_fail_transfers() {
    let h = harness();
    let s = h.online();
    let t = h.core.play_fetch(&rfile(r"a\1.flac", 4));
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.st().downloads[0]
        .tx
        .send(DownloadStatus::Failed(Some("peer said no".into())))
        .unwrap();
    let e = h.want("failed", |e| matches!(e, CoreEvent::TransferFailed { .. }));
    assert_eq!(
        e,
        CoreEvent::TransferFailed {
            id: t.id,
            reason: "peer said no".into()
        }
    );

    let d = h.core.download(&rfile(r"a\2.flac", 4));
    wait_until("lib download 2", || s.st().downloads.len() == 2);
    h.script(Ok(Arc::new(FakeSession::default())));
    s.st().loss = Some(SessionLoss::Disconnected);
    h.want(
        "dl failed",
        |e| matches!(e, CoreEvent::TransferFailed { id, .. } if *id == d),
    );
}

#[test]
fn cache_is_evicted_oldest_first_but_not_the_current_play() {
    let h = harness_with(|s, _| s.cache_cap_bytes = 6);
    let s = h.online();
    let mut tickets = Vec::new();
    for (i, name) in ["1.flac", "2.flac", "3.flac"].iter().enumerate() {
        let t = h.core.play_fetch(&rfile(&format!(r"a\{name}"), 4));
        wait_until("lib download", || s.st().downloads.len() == i + 1);
        s.progress(i, b"abcd", 4);
        s.complete(i);
        h.want("done", is_done(t.id));
        std::thread::sleep(Duration::from_millis(20)); // distinct mtimes
        tickets.push(t);
    }
    wait_until("evicted", || {
        !tickets[0].final_path.exists() && !tickets[1].final_path.exists()
    });
    assert!(tickets[2].final_path.exists(), "current play is protected");
}

// ------------------------------------------------------------ shares/uploads

#[test]
fn shares_offline_scan_and_online_rescan() {
    let mut h = harness();
    let music = h.tmp.path().join("music");
    std::fs::create_dir_all(music.join("album")).unwrap();
    std::fs::write(music.join("album").join("a.flac"), b"x").unwrap();
    h.core.set_shares(vec![music.clone()]);
    let e = h.want("counts", |e| matches!(e, CoreEvent::ShareCounts { .. }));
    assert_eq!(
        e,
        CoreEvent::ShareCounts {
            folders: 1,
            files: 1
        }
    );

    let s = h.online();
    assert_eq!(
        h.conn.params.lock().unwrap()[0].share_dirs,
        vec![music.to_string_lossy().into_owned()]
    );
    h.core.rescan_shares();
    wait_until("rescan", || s.st().share_sets.len() == 1);
    h.want("online counts", |e| {
        *e == CoreEvent::ShareCounts {
            folders: 1,
            files: 42,
        }
    });
}

#[test]
fn uploads_snapshot_and_cancel() {
    let h = harness();
    let s = h.online();
    s.st().uploads.push(UploadInfo {
        username: "peer".into(),
        filename: r"music\a.flac".into(),
        size: 10,
        bytes_sent: 5,
        status: UploadStatus::InProgress,
        speed_bytes_per_sec: 2.0,
    });
    let CoreEvent::Uploads(rows) = h.want(
        "uploads",
        |e| matches!(e, CoreEvent::Uploads(r) if !r.is_empty()),
    ) else {
        unreachable!()
    };
    assert_eq!(rows[0].state, UploadState::InProgress);
    assert_eq!(rows[0].bytes_sent, 5);
    h.core.cancel_upload("peer", r"music\a.flac");
    wait_until("cancelled", || s.st().upload_cancels.len() == 1);
}

// ------------------------------------------------------------------- profile

fn server_info(u: &str, status: Option<UserStatus>, stats: Option<(u32, u32, u32)>) -> UserInfo {
    let mut i = UserInfo::pending(u.to_string());
    i.presence = status.map(|status| UserPresence {
        status,
        privileged: false,
    });
    i.stats = stats.map(|(average_speed, shared_files, shared_folders)| UserStats {
        average_speed,
        shared_files,
        shared_folders,
    });
    i
}

fn is_profile(e: &CoreEvent) -> bool {
    matches!(
        e,
        CoreEvent::UserProfile { .. } | CoreEvent::UserProfileFailed { .. }
    )
}

#[test]
fn profile_merges_server_and_peer_parts_as_they_arrive() {
    let h = harness();
    let s = h.online();
    h.core.user_profile("bob");
    wait_until("requested", || s.st().profile_requests == ["bob"]);
    s.st().user_infos.insert(
        "bob".into(),
        server_info("bob", Some(UserStatus::Away), None),
    );
    let e = h.want("presence", is_profile);
    let expect1 = UserProfile {
        online: Some(true),
        ..UserProfile::default()
    };
    assert_eq!(
        e,
        CoreEvent::UserProfile {
            username: "bob".into(),
            profile: expect1
        }
    );

    s.st().user_infos.insert(
        "bob".into(),
        server_info("bob", Some(UserStatus::Away), Some((1000, 50, 4))),
    );
    s.st().peer_infos.insert(
        "bob".into(),
        PeerInfo {
            description: "flac only".into(),
            picture_bytes: Some(10),
            total_uploads: 3,
            queue_size: 9,
            slots_free: false,
            upload_allowed: Some(1),
        },
    );
    let CoreEvent::UserProfile { profile, .. } = h.want(
        "full",
        |e| matches!(e, CoreEvent::UserProfile { profile, .. } if profile.description.is_some()),
    ) else {
        unreachable!()
    };
    assert_eq!(
        profile,
        UserProfile {
            online: Some(true),
            avg_speed: Some(1000),
            shared_files: Some(50),
            shared_dirs: Some(4),
            description: Some("flac only".into()),
            picture: None,
            upload_slots: Some(3),
            free_slot: Some(false),
            queue_size: Some(9),
        }
    );
    // Complete → no further events, no failure at the deadline.
    h.none_within(Duration::from_millis(400), is_profile);
}

#[test]
fn profile_with_server_stats_only_is_partial_not_failed() {
    let h = harness();
    let s = h.online();
    h.core.user_profile("carol");
    wait_until("requested", || !s.st().profile_requests.is_empty());
    s.st().user_infos.insert(
        "carol".into(),
        server_info("carol", Some(UserStatus::Online), Some((5, 6, 7))),
    );
    let CoreEvent::UserProfile { profile, .. } = h.want(
        "stats",
        |e| matches!(e, CoreEvent::UserProfile { profile, .. } if profile.shared_files.is_some()),
    ) else {
        unreachable!()
    };
    assert_eq!(profile.online, Some(true));
    assert_eq!(
        (profile.avg_speed, profile.shared_files, profile.shared_dirs),
        (Some(5), Some(6), Some(7))
    );
    assert_eq!(
        (profile.description, profile.upload_slots, profile.free_slot),
        (None, None, None)
    );
    // The peer never answers: the timeout passes without UserProfileFailed.
    h.none_within(Duration::from_millis(400), |e| {
        matches!(e, CoreEvent::UserProfileFailed { .. })
    });
}

#[test]
fn offline_user_finishes_without_waiting_for_the_peer() {
    let h = harness();
    let s = h.online();
    h.core.user_profile("ghost");
    wait_until("requested", || !s.st().profile_requests.is_empty());
    s.st().user_infos.insert(
        "ghost".into(),
        server_info("ghost", Some(UserStatus::Offline), Some((0, 0, 0))),
    );
    let CoreEvent::UserProfile { profile, .. } = h.want("profile", is_profile) else {
        unreachable!()
    };
    assert_eq!(profile.online, Some(false));
    h.none_within(Duration::from_millis(400), is_profile);
}

#[test]
fn profile_fails_on_timeout_and_when_offline() {
    let h = harness();
    h.core.user_profile("dave");
    assert_eq!(
        h.want("not connected", is_profile),
        CoreEvent::UserProfileFailed {
            username: "dave".into(),
            reason: "not connected".into()
        }
    );
    let _s = h.online();
    h.core.user_profile("dave");
    let e = h.want("timeout", is_profile);
    assert!(
        matches!(e, CoreEvent::UserProfileFailed { ref username, .. } if username == "dave"),
        "{e:?}"
    );
}

#[test]
fn profile_pending_at_session_loss_fails() {
    let h = harness();
    let s = h.online();
    h.core.user_profile("erin");
    wait_until("requested", || !s.st().profile_requests.is_empty());
    s.st().loss = Some(SessionLoss::Displaced);
    let e = h.want("failed", is_profile);
    assert!(matches!(e, CoreEvent::UserProfileFailed { .. }), "{e:?}");
}

// ------------------------------------------------------- ▶ start deadline

fn is_failed(id: TransferId) -> impl Fn(&CoreEvent) -> bool {
    move |e| matches!(e, CoreEvent::TransferFailed { id: i, .. } if *i == id)
}

#[test]
fn play_start_deadline_comes_from_settings_and_ignores_zero_byte_progress() {
    let t = Tunables {
        play_start_timeout: None,
        ..tunables()
    };
    let h = harness_tuned(t, |s, _| s.play_start_timeout_secs = 1);
    let s = h.online();
    let started = Instant::now();
    let p = h.core.play_fetch(&rfile(r"a\1.flac", 4));
    let d = h.core.download(&rfile(r"a\2.flac", 4));
    wait_until("lib downloads", || s.st().downloads.len() == 2);
    // The library's report right after opening the .part carries 0 bytes: not data.
    s.progress(0, b"", 4);
    let e = h.want("stall", is_failed(p.id));
    let waited = started.elapsed();
    let CoreEvent::TransferFailed { reason, .. } = e else {
        unreachable!()
    };
    assert!(reason.starts_with(crate::PLAY_STALLED), "{reason}");
    assert!(
        waited >= Duration::from_millis(950),
        "fired after {waited:?}"
    );
    assert!(waited < Duration::from_secs(3), "fired after {waited:?}");
    wait_until("lib cancel", || {
        s.st().cancels == vec![("alice".into(), r"a\1.flac".into())]
    });
    // The ↓ that is just as silent is not failed by the ▶ deadline.
    h.none_within(Duration::from_millis(300), is_failed(d));
}

#[test]
fn play_with_data_before_the_deadline_is_not_failed() {
    let h = harness();
    let s = h.online();
    let p = h.core.play_fetch(&rfile(r"a\1.flac", 4));
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.progress(0, b"ab", 4);
    h.none_within(Duration::from_millis(700), is_failed(p.id));
    assert!(s.st().cancels.is_empty());
}

#[test]
fn play_joining_a_long_queued_download_gets_its_own_deadline() {
    let h = harness();
    let s = h.online();
    let f = rfile(r"a\1.flac", 4);
    let d = h.core.download(&f);
    wait_until("lib download", || s.st().downloads.len() == 1);
    std::thread::sleep(Duration::from_millis(500)); // longer than the 400 ms ▶ deadline
    let joined = Instant::now();
    let p = h.core.play_fetch(&f);
    h.want("stall", is_failed(p.id));
    assert!(
        joined.elapsed() >= Duration::from_millis(380),
        "{:?}",
        joined.elapsed()
    );
    assert!(
        s.st().cancels.is_empty(),
        "the ↓ keeps the library transfer"
    );
    // The ↓ still completes.
    s.progress(0, b"abcd", 4);
    s.complete(0);
    h.want("dl done", is_done(d));
}

// ------------------------------------------- session loss, reconnect status

#[test]
fn displaced_is_distinct_and_never_reconnects() {
    let h = harness();
    let s = h.online();
    h.script(Ok(Arc::new(FakeSession::default()))); // would succeed if tried
    s.st().loss = Some(SessionLoss::Displaced);
    let e = h.want("Disconnected", |e| {
        matches!(e, CoreEvent::Disconnected { .. })
    });
    assert_eq!(
        e,
        CoreEvent::Disconnected {
            reason: DisconnectReason::Displaced,
            will_retry: false
        }
    );
    h.none_within(Duration::from_millis(400), |e| {
        matches!(e, CoreEvent::Connected { .. })
    });
    assert_eq!(
        h.conn.params.lock().unwrap().len(),
        1,
        "no reconnect attempt"
    );
    assert_eq!(h.core.status().reconnect, None);
}

#[test]
fn network_drop_reconnects_with_growing_backoff_status_and_reannounces_shares() {
    let backoff: fn(u32) -> Duration = |a| Duration::from_millis(100 * (u64::from(a) + 1));
    let t = Tunables {
        backoff,
        ..tunables()
    };
    let mut h = harness_tuned(t, |_, _| {});
    let music = h.tmp.path().join("music");
    std::fs::create_dir_all(&music).unwrap();
    h.core.set_shares(vec![music.clone()]);
    let s1 = h.online();
    s1.st().loss = Some(SessionLoss::Disconnected);
    h.script(Err(LoginError::Other("connection refused".into())));
    let s2 = Arc::new(FakeSession::default());
    h.script(Ok(s2.clone()));

    // Drop → attempt 1 after 100 ms; it fails → attempt 2 after 200 ms.
    for attempt in 1..=2u32 {
        let e = h.want("Disconnected", |e| {
            matches!(e, CoreEvent::Disconnected { .. })
        });
        let CoreEvent::Disconnected {
            reason: DisconnectReason::Network(m),
            will_retry: true,
        } = e
        else {
            panic!("{e:?}")
        };
        assert!(m.contains(&format!("(attempt {attempt})")), "{m}");
        assert!(m.contains("reconnecting in"), "{m}");
        let r = h.core.status().reconnect.expect("reconnect status");
        assert_eq!(r.attempt, attempt);
        assert_eq!(r.delay, Duration::from_millis(100 * u64::from(attempt)));
        assert!(r.in_secs() <= 1);
    }
    h.want("reconnected", |e| matches!(e, CoreEvent::Connected { .. }));
    h.want("counts re-sent", |e| {
        matches!(e, CoreEvent::ShareCounts { .. })
    });
    let params = h.conn.params.lock().unwrap().clone();
    assert_eq!(params.len(), 3);
    let shared = vec![music.to_string_lossy().into_owned()];
    assert!(
        params.iter().all(|p| p.share_dirs == shared),
        "every session announces the shares"
    );
    wait_until("status cleared", || h.core.status().reconnect.is_none());
}

#[test]
fn reconnect_waits_for_the_old_session_to_release_the_port() {
    let h = harness();
    let s1 = h.online();
    s1.st().search_linger = Duration::from_millis(400);
    let _ = h.core.search("q");
    wait_until("search started", || s1.st().searches.contains_key("q"));
    h.script(Ok(Arc::new(FakeSession::default())));
    s1.st().loss = Some(SessionLoss::Disconnected);
    drop(s1);
    let dropped = Instant::now();
    h.want("reconnected", |e| matches!(e, CoreEvent::Connected { .. }));
    assert_eq!(*h.conn.prev_alive.lock().unwrap(), [false, false]);
    let at = h.conn.connected_at.lock().unwrap()[1];
    assert!(
        at.duration_since(dropped) >= Duration::from_millis(300),
        "waited for the helper"
    );
}

// ------------------------------------------------------------ listen port

#[test]
fn bound_port_is_reported_with_the_reason_when_it_differs() {
    let h = harness();
    let s = Arc::new(FakeSession::default());
    s.st().bound = Some(Some(45_543));
    h.script(Ok(s));
    h.core.login(creds());
    let e = h.want("Connected", |e| matches!(e, CoreEvent::Connected { .. }));
    assert!(
        matches!(
            e,
            CoreEvent::Connected {
                listen_port: Some(45_543),
                ..
            }
        ),
        "{e:?}"
    );
    let e = h.want(
        "note",
        |e| matches!(e, CoreEvent::Error { context, .. } if context == "listen_port"),
    );
    let CoreEvent::Error { message, .. } = e else {
        unreachable!()
    };
    assert!(
        message.contains("2234 is in use") && message.contains("45543"),
        "{message}"
    );
    let l = h.core.status().listen;
    assert_eq!((l.requested, l.bound), (2234, Some(45_543)));
    assert_eq!(l.note.as_deref(), Some(message.as_str()));
}

#[test]
fn matching_bound_port_has_no_note() {
    let h = harness();
    let _s = h.online();
    h.none_within(Duration::from_millis(100), |e| {
        matches!(e, CoreEvent::Error { .. })
    });
    let l = h.core.status().listen;
    assert_eq!((l.requested, l.bound, l.note), (2234, Some(2234), None));
}

// ------------------------------------------------------------ search progress

#[test]
fn search_keeps_streaming_with_progress_until_the_window_closes() {
    let t = Tunables {
        search_window: Duration::from_millis(600),
        ..tunables()
    };
    let h = harness_tuned(t, |_, _| {});
    let s = h.online();
    let started = Instant::now();
    let id = h.core.search("q");
    wait_until("registered", || s.st().searches.contains_key("q"));
    let p = h.core.status().searches;
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].id, p[0].files, p[0].users), (id, 0, 0));
    assert!(p[0].remaining() > Duration::from_millis(300));

    let batches = [
        result("u1", &[r"a\1.flac", r"a\2.flac"]),
        result("u2", &[r"b\1.mp3"]),
        result("u1", &[r"c\3.flac"]),
    ];
    let mut files = 0;
    for (i, r) in batches.into_iter().enumerate() {
        let n = r.files.len();
        s.st().searches.get_mut("q").unwrap().push(r);
        let CoreEvent::SearchResults { hits, .. } =
            h.want("batch", |e| matches!(e, CoreEvent::SearchResults { .. }))
        else {
            unreachable!()
        };
        assert_eq!(hits.len(), n, "batch {i} is only the new files");
        files += n;
        wait_until("progress", || {
            h.core
                .status()
                .searches
                .first()
                .is_some_and(|p| p.files == files)
        });
        std::thread::sleep(Duration::from_millis(60));
    }
    let p = &h.core.status().searches[0];
    assert_eq!((p.files, p.users), (4, 2));
    let e = h.want("finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: id,
            total_files: 4
        }
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(550) && took < Duration::from_millis(1500),
        "{took:?}"
    );
    wait_until("gone from status", || h.core.status().searches.is_empty());
}

// ------------------------------------------------------------------- cache

#[test]
fn cache_usage_and_clear_cache_keeps_plays_in_use() {
    let h = harness();
    let s = h.online();
    let done_play = |name: &str, i: usize, data: &[u8]| {
        let t = h
            .core
            .play_fetch(&rfile(&format!(r"a\{name}"), data.len() as u64));
        wait_until("lib download", || s.st().downloads.len() == i + 1);
        s.progress(i, data, data.len() as u64);
        s.complete(i);
        h.want("done", is_done(t.id));
        t
    };
    let old = done_play("1.flac", 0, b"0123456789");
    // In flight (being fetched), not the current play once the next starts.
    let fetching = h.core.play_fetch(&rfile(r"a\2.flac", 8));
    wait_until("lib download", || s.st().downloads.len() == 2);
    s.progress(1, b"abc", 8);
    let current = done_play("3.flac", 2, b"xyz12");
    // The snapshot is rescanned every 50 ms here; wait for one taken after
    // the last play finished.
    wait_until("usage with one finished, unprotected play", || {
        let c = h.core.status().cache;
        c.in_use_bytes >= 3 + 5 && c.in_use_bytes < c.used_bytes
    });
    let c = h.core.status().cache;
    assert_eq!(c.cap_bytes, Settings::default().cache_cap_bytes);
    assert!(
        c.in_use_bytes >= 3 + 5 && c.in_use_bytes < c.used_bytes,
        "{c:?}"
    );

    h.core.clear_cache();
    wait_until("cleared", || !old.final_path.exists());
    assert!(fetching.part_path.exists(), "the fetching play is kept");
    assert!(current.final_path.exists(), "the current play is kept");
    wait_until("usage after clear", || {
        let c = h.core.status().cache;
        c.used_bytes == c.in_use_bytes && c.used_bytes > 0
    });
    // The cleared file is fetched again rather than served from a deleted path.
    let again = h.core.play_fetch(&rfile(r"a\1.flac", 10));
    wait_until("refetch", || s.st().downloads.len() == 4);
    h.core.cancel(again.id);
}

// -------------------------------------------------- folder junk, share status

#[test]
fn download_folder_skips_os_junk_but_keeps_music_extras() {
    let h = harness();
    let s = h.online();
    let listing = vec![dir(
        r"Music\Album",
        &[
            "01 Track.flac",
            "Thumbs.db",
            "desktop.ini",
            ".DS_Store",
            "Discogs.URL",
            "._01 Track.flac",
            "album.cue",
            "rip.log",
            "cover.jpg",
            "back.png",
            "info.nfo",
        ],
    )];
    s.st()
        .folders
        .insert(("alice".into(), r"Music\Album".into()), listing);
    h.core.download_folder("alice", r"Music\Album");
    wait_until("downloads queued", || s.st().downloads.len() >= 6);
    std::thread::sleep(Duration::from_millis(100));
    let mut names: Vec<String> = s
        .st()
        .downloads
        .iter()
        .map(|d| crate::paths::remote_basename(&d.path).to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "01 Track.flac",
            "album.cue",
            "back.png",
            "cover.jpg",
            "info.nfo",
            "rip.log"
        ]
    );
}

#[test]
fn folder_of_only_junk_reports_an_error() {
    let h = harness();
    let s = h.online();
    s.st().folders.insert(
        ("alice".into(), r"Pics".into()),
        vec![dir("Pics", &["Thumbs.db", "desktop.ini"])],
    );
    h.core.download_folder("alice", "Pics");
    let e = h.want(
        "error",
        |e| matches!(e, CoreEvent::Error { context, .. } if context == "download_folder"),
    );
    assert!(
        matches!(e, CoreEvent::Error { ref message, .. } if message.contains("only system files")),
        "{e:?}"
    );
    assert!(s.st().downloads.is_empty());
}

fn make_share(root: &Path, name: &str, albums: &[(&str, usize)]) -> PathBuf {
    let d = root.join(name);
    for (album, n) in albums {
        std::fs::create_dir_all(d.join(album)).unwrap();
        for i in 0..*n {
            std::fs::write(d.join(album).join(format!("{i:02}.flac")), b"x").unwrap();
        }
    }
    d
}

#[test]
fn share_status_per_directory_offline_and_after_rescan() {
    let mut h = harness();
    let a = make_share(h.tmp.path(), "a", &[("x", 2), ("y", 1)]);
    let b = make_share(h.tmp.path(), "b", &[("z", 4)]);
    let missing = h.tmp.path().join("missing");
    h.core
        .set_shares(vec![a.clone(), b.clone(), missing.clone()]);
    h.want("counts", |e| matches!(e, CoreEvent::ShareCounts { .. }));
    let sh = h.core.status().shares;
    let got: Vec<_> = sh
        .iter()
        .map(|s| (s.dir.clone(), s.files, s.scanning))
        .collect();
    assert_eq!(
        got,
        [
            (a.clone(), 3, false),
            (b.clone(), 4, false),
            (missing.clone(), 0, false)
        ]
    );
    assert!(sh[0].folders >= 2 && sh[1].folders >= 1, "{sh:?}");

    // Online rescan: marked scanning while it runs, then updated per dir.
    let s = h.online();
    std::fs::write(b.join("z").join("new.flac"), b"x").unwrap();
    s.st().share_delay = Duration::from_millis(300);
    h.core.rescan_shares();
    wait_until("marked scanning", || {
        h.core.status().shares.iter().all(|s| s.scanning)
    });
    let mid = h.core.status().shares;
    assert_eq!(mid[1].files, 4, "previous counts kept while scanning");
    wait_until("rescanned", || {
        let sh = h.core.status().shares;
        sh[1].files == 5 && sh.iter().all(|s| !s.scanning)
    });
    assert_eq!(s.st().share_sets.len(), 1, "library rescan ran too");

    // Removing a share drops its entry; order follows Settings::share_dirs.
    h.core.set_shares(vec![b.clone()]);
    wait_until("one share", || {
        let sh = h.core.status().shares;
        sh.len() == 1 && sh[0].dir == b && !sh[0].scanning
    });
}

#[test]
fn share_status_is_filled_at_startup_without_login() {
    let tmp = tempfile::tempdir().unwrap();
    let a = make_share(tmp.path(), "a", &[("x", 2)]);
    let settings = Settings {
        download_dir: tmp.path().join("downloads"),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
        share_dirs: vec![a.clone()],
        port_mapping: false,
        ..Settings::default()
    };
    let conn = Arc::new(FakeConnector::default());
    let (core, _ev) = Core::start_with(settings, conn.clone(), tunables()).unwrap();
    wait_until("startup scan", || {
        core.status()
            .shares
            .first()
            .is_some_and(|s| s.files == 2 && !s.scanning)
    });
    assert!(conn.params.lock().unwrap().is_empty(), "no network");
}

// ------------------------------------------- query normalization, broaden, wishlist

#[test]
fn search_sends_the_normalized_query_and_keeps_what_was_typed() {
    let h = harness();
    let s = h.online();
    let id = h.core.search("  AC/DC  back in black ");
    wait_until("sent", || {
        s.st().searches.contains_key("AC DC back in black")
    });
    let p = h.core.status().searches;
    assert_eq!(
        p[0].query, "AC/DC  back in black",
        "status shows what was typed"
    );
    let info = h.core.search_info(id).unwrap();
    assert_eq!(
        (info.typed.as_str(), info.sent.as_str(), info.wishlist),
        ("AC/DC  back in black", "AC DC back in black", false)
    );
    s.st()
        .searches
        .get_mut("AC DC back in black")
        .unwrap()
        .push(result("u1", &[r"AC-DC\Back In Black\01 Hells Bells.flac"]));
    h.want(
        "results",
        |e| matches!(e, CoreEvent::SearchResults { search, .. } if *search == id),
    );
    h.want(
        "finished",
        |e| matches!(e, CoreEvent::SearchFinished { search, total_files: 1 } if *search == id),
    );
    let info = h.core.search_info(id).unwrap();
    assert!(info.finished && info.broadened_to.is_none(), "{info:?}");
    assert_eq!((info.files, info.users), (1, 1));
}

#[test]
fn zero_audio_broadens_once_under_the_same_id() {
    let h = harness();
    let s = h.online();
    let id = h.core.search("wasn't me shaggý");
    wait_until("first", || s.st().searches.contains_key("wasn t me shaggý"));
    // Only a non-audio hit: still "no audio", so it broadens.
    s.st()
        .searches
        .get_mut("wasn t me shaggý")
        .unwrap()
        .push(result("u0", &[r"x\cover.jpg"]));
    wait_until("broadened", || {
        s.st().searches.contains_key("wasn me shaggy")
    });
    let info = h.core.search_info(id).unwrap();
    assert_eq!(info.broadened_to.as_deref(), Some("wasn me shaggy"));
    assert!(!info.finished);
    assert!(s.st().forgotten.contains(&"wasn t me shaggý".to_string()));
    let p = h.core.status().searches;
    assert_eq!((p.len(), p[0].id), (1, id), "still one search, same id");
    s.st()
        .searches
        .get_mut("wasn me shaggy")
        .unwrap()
        .push(result("u1", &[r"Shaggy\Hot Shot\Wasn't Me.mp3"]));
    let e = h.want("results", |e| matches!(e, CoreEvent::SearchResults { .. }));
    assert!(matches!(e, CoreEvent::SearchResults { search, .. } if search == id));
    let e = h.want("finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: id,
            total_files: 1
        }
    );
    // Exactly one SearchFinished, and no third query.
    h.none_within(Duration::from_millis(500), |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    let sent: Vec<String> = s.st().forgotten.clone();
    assert_eq!(sent, ["wasn t me shaggý", "wasn me shaggy"]);
}

#[test]
fn broadened_search_that_still_finds_nothing_finishes_once() {
    let h = harness();
    let s = h.online();
    let id = h.core.search("sigur rós");
    wait_until("broadened", || s.st().searches.contains_key("sigur ros"));
    let e = h.want("finished", |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    assert_eq!(
        e,
        CoreEvent::SearchFinished {
            search: id,
            total_files: 0
        }
    );
    h.none_within(Duration::from_millis(500), |e| {
        matches!(e, CoreEvent::SearchFinished { .. })
    });
    let info = h.core.search_info(id).unwrap();
    assert_eq!(info.broadened_to.as_deref(), Some("sigur ros"));
    assert!(info.finished);
}

#[test]
fn operators_and_unbroadenable_queries_are_never_broadened() {
    let h = harness();
    let s = h.online();
    for q in [
        "wasn't me -remix",
        "*aggy wasn't",
        "\"don't stop\" me",
        "aphex twin",
    ] {
        let id = h.core.search(q);
        let e = h.want("finished", |e| {
            matches!(e, CoreEvent::SearchFinished { .. })
        });
        assert_eq!(
            e,
            CoreEvent::SearchFinished {
                search: id,
                total_files: 0
            },
            "{q}"
        );
        assert_eq!(h.core.search_info(id).unwrap().broadened_to, None, "{q}");
    }
    let forgotten = s.st().forgotten.clone();
    assert_eq!(
        forgotten,
        [
            "wasn t me -remix",
            "*aggy wasn t",
            "don t stop me",
            "aphex twin"
        ]
    );
}

#[test]
fn audio_results_are_not_broadened() {
    let h = harness();
    let s = h.online();
    let id = h.core.search("don't stop me now");
    wait_until("sent", || s.st().searches.contains_key("don t stop me now"));
    s.st()
        .searches
        .get_mut("don t stop me now")
        .unwrap()
        .push(result("u1", &[r"Queen\Jazz\Don't Stop Me Now.flac"]));
    h.want(
        "finished",
        |e| matches!(e, CoreEvent::SearchFinished { search, .. } if *search == id),
    );
    assert_eq!(h.core.search_info(id).unwrap().broadened_to, None);
}

#[test]
fn wishlist_runs_one_wish_per_interval_round_robin_while_online() {
    let mut h = harness();
    assert!(h.core.wishlist_add("Wasn't  Me"));
    assert!(
        !h.core.wishlist_add("wasn't me"),
        "duplicate (case-insensitive)"
    );
    assert!(!h.core.wishlist_add("   "));
    assert!(h.core.wishlist_add("sigur rós"));
    assert_eq!(h.core.wishlist(), ["Wasn't Me", "sigur rós"]);
    assert_eq!(h.core.settings().wishlist, ["Wasn't Me", "sigur rós"]);
    // Offline: nothing is sent.
    std::thread::sleep(Duration::from_millis(400));
    let s = h.online();
    let t0 = Instant::now();
    wait_until("first wish", || s.st().wishes.len() == 1);
    assert!(
        t0.elapsed() < Duration::from_millis(250),
        "first wish soon after login"
    );
    assert_eq!(
        s.st().wishes[0],
        "Wasn t Me",
        "normalized, sent as a wishlist search (103)"
    );
    // Results come as ordinary SearchResults under an id tagged as a wish.
    s.st()
        .searches
        .get_mut("Wasn t Me")
        .unwrap()
        .push(result("u1", &[r"Shaggy\Wasn't Me.flac"]));
    let CoreEvent::SearchResults { search, .. } = h.want("wish results", |e| {
        matches!(e, CoreEvent::SearchResults { .. })
    }) else {
        unreachable!()
    };
    let info = h.core.search_info(search).unwrap();
    assert!(info.wishlist && info.typed == "Wasn't Me", "{info:?}");
    h.want(
        "wish finished",
        |e| matches!(e, CoreEvent::SearchFinished { search: s, .. } if *s == search),
    );
    // Next wish only after the interval (min 250 ms in these tunables).
    let first = Instant::now();
    wait_until("second wish", || s.st().wishes.len() == 2);
    assert!(
        t0.elapsed() >= Duration::from_millis(250),
        "{:?}",
        first.elapsed()
    );
    assert_eq!(
        s.st().wishes[1],
        "sigur rós",
        "round-robin; wishes are never broadened"
    );
    wait_until("third wish wraps", || s.st().wishes.len() == 3);
    assert_eq!(s.st().wishes[2], "Wasn t Me");
    // Removing stops it.
    assert!(h.core.wishlist_remove("WASN'T ME"));
    assert!(!h.core.wishlist_remove("nope"));
    let n = s.st().wishes.len();
    wait_until("next after removal", || s.st().wishes.len() > n);
    assert!(s.st().wishes[n..].iter().all(|w| w == "sigur rós"));
}

#[test]
fn wishlist_respects_a_longer_server_interval() {
    let mut h = harness();
    h.core.wishlist_add("a b c");
    let s = h.online();
    s.st().wish_interval = Duration::from_millis(900);
    wait_until("first wish", || s.st().wishes.len() == 1);
    let t1 = Instant::now();
    wait_until("second wish", || s.st().wishes.len() == 2);
    assert!(
        t1.elapsed() >= Duration::from_millis(700),
        "{:?}",
        t1.elapsed()
    );
}

// ------------------------------------------------------------- download to…

#[test]
fn download_to_another_folder_keeps_layout_staging_and_collision_rules() {
    // With the album-subfolder setting on (flat is covered below).
    let mut h = harness_with(|s, _| s.single_file_subfolder = true);
    let s = h.online();
    let elsewhere = h.tmp.path().join("other disk");
    // An existing file with the same name: the new one gets a unique name, as
    // in the downloads folder.
    std::fs::create_dir_all(elsewhere.join("Album")).unwrap();
    std::fs::write(elsewhere.join("Album").join("01.flac"), b"old").unwrap();
    let id = h
        .core
        .download_to(&rfile(r"Music\Album\01.flac", 4), &elsewhere);
    let e = h.want(
        "queued",
        |e| matches!(e, CoreEvent::TransferQueued { id: i, .. } if *i == id),
    );
    let CoreEvent::TransferQueued { part_path, .. } = e else {
        unreachable!()
    };
    let staging = h.downloads_dir().join(crate::paths::STAGING_DIR_NAME);
    assert!(
        part_path.starts_with(&staging),
        "streams in the usual staging dir: {part_path:?}"
    );
    wait_until("lib download", || s.st().downloads.len() == 1);
    assert!(s.st().downloads[0].dir.starts_with(&staging));
    s.progress(0, b"abcd", 4);
    s.complete(0);
    let p = done_path(h.want("done", is_done(id)));
    assert_eq!(p.parent().unwrap(), elsewhere.join("Album"));
    assert_ne!(
        p,
        elsewhere.join("Album").join("01.flac"),
        "never overwrites"
    );
    assert_eq!(std::fs::read(&p).unwrap(), b"abcd");
    assert_eq!(
        std::fs::read(elsewhere.join("Album").join("01.flac")).unwrap(),
        b"old"
    );
    assert!(
        !h.downloads_dir().join("Album").exists(),
        "nothing in the default dir"
    );
    // Cleanup may land just after the event.
    wait_until("staging cleaned", || {
        std::fs::read_dir(&staging).map_or(true, |mut d| d.next().is_none())
    });
    assert_eq!(h.core.settings().recent_download_dirs, [elsewhere]);
}

#[test]
fn download_folder_to_puts_the_album_folder_under_the_chosen_dir() {
    let mut h = harness();
    let s = h.online();
    let elsewhere = h.tmp.path().join("usb");
    s.st().folders.insert(
        ("alice".into(), r"Music\Album (2019)".into()),
        vec![dir(
            r"Music\Album (2019)",
            &["01.flac", "02.flac", "Thumbs.db"],
        )],
    );
    h.core
        .download_folder_to("alice", r"Music\Album (2019)\", &elsewhere);
    wait_until("2 downloads", || s.st().downloads.len() == 2);
    let ids: Vec<TransferId> = (0..2)
        .map(
            |_| match h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. })) {
                CoreEvent::TransferQueued { id, .. } => id,
                _ => unreachable!(),
            },
        )
        .collect();
    for i in 0..2 {
        s.progress(i, b"xy", 2);
        s.complete(i);
    }
    let mut got: Vec<PathBuf> = (0..2)
        .map(|_| {
            done_path(h.want(
                "done",
                |e| matches!(e, CoreEvent::TransferDone { id, .. } if ids.contains(id)),
            ))
        })
        .collect();
    got.sort();
    let album = elsewhere.join("Album (2019)");
    assert_eq!(got, [album.join("01.flac"), album.join("02.flac")]);
    assert!(!h.downloads_dir().join("Album (2019)").exists());
    assert_eq!(h.core.settings().recent_download_dirs, [elsewhere]);
    // The plain download_folder still uses the downloads folder.
    s.st().folders.insert(
        ("alice".into(), r"Music\B".into()),
        vec![dir(r"Music\B", &["1.flac"])],
    );
    h.core.download_folder("alice", r"Music\B");
    wait_until("3rd download", || s.st().downloads.len() == 3);
    let id = match h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. })) {
        CoreEvent::TransferQueued { id, .. } => id,
        _ => unreachable!(),
    };
    s.progress(2, b"z", 1);
    s.complete(2);
    assert_eq!(
        done_path(h.want("done", is_done(id))),
        h.downloads_dir().join("B").join("1.flac")
    );
}

#[test]
fn recent_download_dirs_are_mru_capped_at_five() {
    let mut s = Settings::default();
    for d in ["a", "b", "c", "d", "e", "f"] {
        s.remember_download_dir(Path::new(d));
    }
    s.remember_download_dir(Path::new("c"));
    let got: Vec<&str> = s
        .recent_download_dirs
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();
    assert_eq!(got, ["c", "f", "e", "d", "b"]);
}

// ------------------------------------------------ single-file subfolder setting

#[test]
fn single_downloads_are_flat_by_default_and_never_overwrite() {
    let h = harness();
    let s = h.online();
    assert!(!h.core.settings().single_file_subfolder);
    let a = h.core.download(&rfile(r"Music\Album A\01.flac", 1));
    let b = h.core.download(&rfile(r"Music\Album B\01.flac", 1));
    wait_until("2 lib downloads", || s.st().downloads.len() == 2);
    for i in 0..2 {
        s.progress(i, b"x", 1);
        s.complete(i);
    }
    let pa = done_path(h.want(
        "a done",
        |e| matches!(e, CoreEvent::TransferDone { id, .. } if *id == a || *id == b),
    ));
    let pb = done_path(h.want(
        "b done",
        |e| matches!(e, CoreEvent::TransferDone { id, .. } if *id == a || *id == b),
    ));
    assert_eq!(pa.parent().unwrap(), h.downloads_dir());
    assert_eq!(pb.parent().unwrap(), h.downloads_dir());
    assert_ne!(
        pa, pb,
        "same name from two albums: unique name, no overwrite"
    );
    let mut names = vec![
        pa.file_name().unwrap().to_owned(),
        pb.file_name().unwrap().to_owned(),
    ];
    names.sort();
    assert_eq!(names, ["01 (2).flac", "01.flac"]);
    assert!(!h.downloads_dir().join("Album A").exists());
}

#[test]
fn single_download_uses_the_album_folder_when_the_setting_is_on() {
    let h = harness_with(|s, _| s.single_file_subfolder = true);
    let s = h.online();
    let id = h.core.download(&rfile(r"Music\Album A\01.flac", 1));
    wait_until("lib download", || s.st().downloads.len() == 1);
    s.progress(0, b"x", 1);
    s.complete(0);
    assert_eq!(
        done_path(h.want("done", is_done(id))),
        h.downloads_dir().join("Album A").join("01.flac")
    );
}

#[test]
fn keeping_a_cached_play_follows_the_setting() {
    for on in [false, true] {
        let h = harness_with(|s, _| s.single_file_subfolder = on);
        let s = h.online();
        let f = rfile(r"Music\Album\01.flac", 2);
        let t = h.core.play_fetch(&f);
        wait_until("lib download", || s.st().downloads.len() == 1);
        s.progress(0, b"ab", 2);
        s.complete(0);
        h.want("play done", is_done(t.id));
        let d = h.core.download(&f);
        let p = done_path(h.want("kept", is_done(d)));
        let want = if on {
            h.downloads_dir().join("Album").join("01.flac")
        } else {
            h.downloads_dir().join("01.flac")
        };
        assert_eq!(p, want, "single_file_subfolder = {on}");
        assert_eq!(s.st().downloads.len(), 1, "from the cache, not refetched");
    }
}

#[test]
fn folder_and_album_downloads_always_keep_the_folder() {
    let mut h = harness(); // single_file_subfolder = false
    let s = h.online();
    s.st().folders.insert(
        ("alice".into(), r"Music\Album".into()),
        vec![dir(r"Music\Album", &["01.flac"])],
    );
    h.core.download_folder("alice", r"Music\Album");
    wait_until("folder download", || s.st().downloads.len() == 1);
    let elsewhere = h.tmp.path().join("usb");
    let t = h
        .core
        .download_album_file(&rfile(r"Music\Other\02.flac", 1), None);
    let u = h
        .core
        .download_album_file(&rfile(r"Music\Other\03.flac", 1), Some(&elsewhere));
    wait_until("3 downloads", || s.st().downloads.len() == 3);
    for i in 0..3 {
        s.progress(i, b"x", 1);
        s.complete(i);
    }
    let mut got: Vec<PathBuf> = (0..3)
        .map(|_| done_path(h.want("done", |e| matches!(e, CoreEvent::TransferDone { .. }))))
        .collect();
    got.sort();
    let mut want = vec![
        h.downloads_dir().join("Album").join("01.flac"),
        h.downloads_dir().join("Other").join("02.flac"),
        elsewhere.join("Other").join("03.flac"),
    ];
    want.sort();
    assert_eq!(got, want);
    let _ = (t, u);
    assert_eq!(h.core.settings().recent_download_dirs, [elsewhere]);
}

// ------------------------------------------------------------ download history

fn history(h: &H) -> Vec<crate::history::HistoryEntry> {
    h.core.history(&crate::history::HistoryQuery::default())
}

#[test]
fn history_records_downloads_and_kept_plays_but_not_plain_plays() {
    use crate::history::Outcome;
    let h = harness();
    let s = h.online();
    // ▶ only: cache, not history.
    let play = rfile(r"Music\Album\01.flac", 2);
    let t = h.core.play_fetch(&play);
    wait_until("play fetch", || s.st().downloads.len() == 1);
    s.progress(0, b"ab", 2);
    s.complete(0);
    h.want("play done", is_done(t.id));
    assert!(history(&h).is_empty(), "plain ▶ is not history");
    assert!(!h.core.was_downloaded("alice", r"Music\Album\01.flac"));
    // ▶ kept (↓ from the cache): history.
    let k = h.core.download(&play);
    let kept = done_path(h.want("kept", is_done(k)));
    // A fresh ↓.
    let f = RemoteFile {
        attrs: crate::model::AudioAttrs {
            bitrate_kbps: Some(320),
            ..Default::default()
        },
        ..rfile(r"Music\Other\02.mp3", 1)
    };
    let d = h.core.download(&f);
    wait_until("lib download", || s.st().downloads.len() == 2);
    s.progress(1, b"x", 1);
    s.complete(1);
    let got = done_path(h.want("done", is_done(d)));
    let all = history(&h);
    assert_eq!(all.len(), 2);
    let (new, old) = (&all[0], &all[1]);
    assert_eq!(
        new.local_path.as_deref(),
        Some(got.as_path()),
        "newest first"
    );
    assert_eq!((new.format.as_str(), new.album.as_str()), ("mp3", "Other"));
    assert_eq!(
        (new.size, new.bitrate_kbps, new.outcome),
        (1, Some(320), Outcome::Done)
    );
    assert_eq!(old.local_path.as_deref(), Some(kept.as_path()));
    assert!(new.exists_on_disk());
    std::fs::remove_file(&got).unwrap();
    assert!(!new.exists_on_disk(), "deleted file noticed lazily");
    assert!(h.core.was_downloaded(&f.username, &f.path));
    assert!(h.core.was_downloaded(&play.username, &play.path));
    assert_eq!(h.core.downloaded_keys().len(), 2);
}

#[test]
fn history_marks_failed_and_cancelled_downloads() {
    use crate::history::Outcome;
    let h = harness();
    let s = h.online();
    let a = h.core.download(&rfile(r"Music\A\01.flac", 4));
    wait_until("lib download", || s.st().downloads.len() == 1);
    let dup = h.core.download(&rfile(r"Music\A\01.flac", 4));
    h.want("dup refused", is_failed(dup));
    h.core.cancel(a);
    h.want(
        "cancelled",
        |e| matches!(e, CoreEvent::TransferCancelled { id } if *id == a),
    );
    // Dismissing the refused duplicate (cancel on a failed transfer) adds no row either.
    h.core.cancel(dup);
    h.want(
        "dup dismissed",
        |e| matches!(e, CoreEvent::TransferCancelled { id } if *id == dup),
    );
    let all = history(&h);
    assert_eq!(
        all.len(),
        1,
        "duplicate refusal is not a history row: {all:?}"
    );
    assert_eq!(all[0].outcome, Outcome::Cancelled);
    assert!(all[0].local_path.is_none());
    assert!(!h.core.was_downloaded("alice", r"Music\A\01.flac"));
    // Offline ↓ fails and is marked with the reason.
    let h = harness();
    let b = h.core.download(&rfile(r"Music\B\02.flac", 4));
    h.want("failed", is_failed(b));
    let all = history(&h);
    assert_eq!(all[0].outcome, Outcome::Failed);
    assert_eq!(all[0].reason.as_deref(), Some("not connected"));
}

#[test]
fn history_survives_restart_and_clear_remove_work() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    {
        let h = harness_with(|st, _| st.data_dir = data.clone());
        let s = h.online();
        for (i, p) in [r"Music\A\01.flac", r"Music\A\02.flac"].iter().enumerate() {
            let id = h.core.download(&rfile(p, 1));
            wait_until("lib download", || s.st().downloads.len() == i + 1);
            s.progress(i, b"x", 1);
            s.complete(i);
            h.want("done", is_done(id));
        }
    }
    let h = harness_with(|st, _| st.data_dir = data.clone());
    let all = history(&h);
    assert_eq!(all.len(), 2, "reloaded from {}", data.display());
    assert!(h.core.was_downloaded("alice", r"Music\A\01.flac"));
    assert!(h.core.history_remove(all[1].id));
    assert!(!h.core.history_remove(all[1].id));
    assert!(!h.core.was_downloaded("alice", r"Music\A\01.flac"));
    assert!(h.core.was_downloaded("alice", r"Music\A\02.flac"));
    h.core.history_clear().unwrap();
    assert!(history(&h).is_empty() && h.core.downloaded_keys().is_empty());
    drop(h);
    let h = harness_with(|st, _| st.data_dir = data.clone());
    assert!(history(&h).is_empty(), "clear persisted");
}

// ------------------------------------------- folder contents fan-out (covers + downloads)

fn tiny_png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([251, 146, 60]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Two "Download folder to…" of one folder, asked before the reply: one request each, one
/// reply, both waiters expanded into their own root, one `FolderContents` event.
#[test]
fn one_folder_reply_reaches_every_folder_download() {
    let mut h = harness();
    let s = h.online();
    let (a, b) = (h.tmp.path().join("a"), h.tmp.path().join("b"));
    h.core.download_folder_to("alice", r"Music\Album", &a);
    h.core.download_folder_to("alice", r"Music\Album", &b);
    wait_until("2 requests", || s.st().folder_requests.len() == 2);
    s.st().folders.insert(
        ("alice".into(), r"Music\Album".into()),
        vec![dir(r"Music\Album", &["01.flac", "02.flac"])],
    );
    h.want("contents", |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    // Every file of both waiters is queued (the second copy of each is then refused by the
    // one-transfer-per-file rule, G11 — that is the fetch's business, not the fan-out's).
    for _ in 0..4 {
        h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. }));
    }
    h.none_within(Duration::from_millis(300), |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    assert_eq!(s.st().downloads.len(), 2);
}

/// Download folder and a cover fetch of the same folder at once: both get the one reply; the
/// cover image is not fetched twice while the folder download has it in flight.
#[test]
fn folder_download_and_cover_fetch_share_the_reply() {
    let h = harness();
    let s = h.online();
    h.core.download_folder("alice", r"Music\Album");
    h.core.fetch_cover("alice", r"Music\Album", None);
    wait_until("2 requests", || s.st().folder_requests.len() == 2);
    s.st().folders.insert(
        ("alice".into(), r"Music\Album".into()),
        vec![dir(r"Music\Album", &["01.flac", "cover.jpg"])],
    );
    h.want("contents", |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    wait_until("2 folder downloads", || s.st().downloads.len() == 2);
    h.none_within(Duration::from_millis(300), |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    assert_eq!(
        s.st().downloads.len(),
        2,
        "cover waits for the folder's copy"
    );
    let i = s
        .st()
        .downloads
        .iter()
        .position(|d| d.path.ends_with("cover.jpg"))
        .unwrap();
    let png = tiny_png();
    s.progress(i, &png, png.len() as u64);
    s.complete(i);
    // Now the cover fetch gets its own copy, into the cover cache.
    wait_until("cover download", || s.st().downloads.len() == 3);
    let covers_dir = h.core.covers().dir().to_path_buf();
    assert!(s.st().downloads[2].dir.starts_with(&covers_dir));
    s.progress(2, &png, png.len() as u64);
    s.complete(2);
    wait_until("cover ready", || {
        h.core.covers().cached("alice", r"Music\Album").is_some()
    });
}

/// Two spellings of one folder (case, slashes): the peer answers one of them; both waiters
/// get that reply instead of one falling back to a full browse.
#[test]
fn folder_reply_reaches_waiters_with_another_spelling() {
    let mut h = harness();
    let s = h.online();
    let b = h.tmp.path().join("b");
    h.core.download_folder("alice", r"Music\Album");
    h.core.download_folder_to("alice", "music/album", &b);
    wait_until("2 requests", || s.st().folder_requests.len() == 2);
    s.st().folders.insert(
        ("alice".into(), "music/album".into()),
        vec![dir(r"Music\Album", &["01.flac"])],
    );
    h.want("contents", |e| {
        matches!(e, CoreEvent::FolderContents { .. })
    });
    for _ in 0..2 {
        h.want("queued", |e| matches!(e, CoreEvent::TransferQueued { .. }));
    }
    assert!(s.st().browse_requests.is_empty(), "no browse fallback");
}

// ------------------------------------------------------------------- library backstop

#[test]
fn library_files_never_reach_the_network() {
    let mut h = harness();
    let s = h.online();
    let local = RemoteFile {
        username: crate::LOCAL_USER.into(),
        path: "/music/Artist/Album/01 Song.flac".into(),
        size: 4,
        attrs: AudioAttrs::default(),
    };
    assert!(local.is_local());
    let play = h.core.play_fetch(&local);
    let down = h.core.download(&local);
    let dir = h.tmp.path().join("elsewhere");
    let to = h.core.download_to(&local, &dir);
    let album = h.core.download_album_file(&local, None);
    h.core.browse(crate::LOCAL_USER);
    h.core.user_profile(crate::LOCAL_USER);
    h.core
        .download_folder(crate::LOCAL_USER, "/music/Artist/Album");
    h.core
        .download_folder_to(crate::LOCAL_USER, "/music/Artist/Album", &dir);
    h.core.cancel_upload(crate::LOCAL_USER, &local.path);
    h.core
        .fetch_cover(crate::LOCAL_USER, "/music/Artist/Album", None);
    // Each refused transfer fails at once, so no row waits forever.
    for id in [play.id, down, to, album] {
        h.want(
            "refused",
            |e| matches!(e, CoreEvent::TransferFailed { id: i, .. } if *i == id),
        );
    }
    // A peer request after them goes out: the worker is fine and saw them all first.
    h.core.browse("alice");
    wait_until("alice browsed", || {
        s.st().browse_requests.contains(&"alice".to_string())
    });
    h.none_within(Duration::from_millis(100), |e| {
        matches!(e, CoreEvent::TransferQueued { .. })
    });
    let st = s.st();
    assert_eq!(st.browse_requests, ["alice"]);
    assert!(st.downloads.is_empty(), "no download sent");
    assert!(st.folder_requests.is_empty(), "no folder request sent");
    assert!(st.profile_requests.is_empty(), "no user info asked");
    assert!(st.upload_cancels.is_empty());
    drop(st);
    assert!(h.core.history(&Default::default()).is_empty());
    assert!(files_under(&h.downloads_dir()).is_empty());
    assert!(files_under(&h.cache_dir()).is_empty());
}
