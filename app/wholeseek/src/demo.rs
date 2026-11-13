//! `--demo`: a fake Soulseek session and a simulated player, on the frame clock (virtual in
//! headless runs, so snapshots and tests are deterministic). No network, no files, no audio.
//!
//! The fake speaks the frozen `CoreEvent` contract: login → `Connected` + `ShareCounts` +
//! `Uploads`; search → ranked `SearchResults` batches then `SearchFinished`; browse →
//! `BrowseResult`; transfers → `TransferQueued`, `TransferProgress` every 0.25 s, `TransferDone`.
//! Scripted quirks: password `wrong` is rejected, user `ghost` can't be browsed, `nightbus`
//! queues you remotely for 1.5 s, and `marten_k`'s first transfer fails at 40 %.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use wholeseek_audio::{PlayState, PlayerEvent, PlayerSnapshot, QueueItem, SeekError};
use wholeseek_core::{
    AudioAttrs, BrowseDir, CoreEvent, Credentials, DisconnectReason, PlayTicket, RemoteFile,
    SearchHit, SearchId, Settings, TransferId, TransferKind, UploadRow, UploadState, UserProfile,
    paths, rank,
};

use crate::backend::Backend;
use crate::playback::Playback;
use wholeseek_core::status::{
    CacheUsage, CoreStatus, ListenStatus, ReconnectStatus, SearchProgress,
};

/// A command the demo backend received (tests assert on these).
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Login(String),
    Logout,
    Search(String),
    CancelSearch(SearchId),
    Browse(String),
    PlayFetch(RemoteFile),
    Download(RemoteFile),
    DownloadFolder(String, String),
    DownloadTo(RemoteFile, PathBuf),
    DownloadFolderTo(String, String, PathBuf),
    DownloadAlbumFile(RemoteFile, Option<PathBuf>),
    Cancel(TransferId),
    Retry(TransferId),
    SetShares(Vec<PathBuf>),
    RescanShares,
    ApplySettings(Settings),
    CancelUpload(String, String),
    UserProfile(String),
    SetAudioOnly(bool),
    ClearCache,
    WishlistAdd(String),
    WishlistRemove(String),
    HistoryRemove(u64),
    HistoryClear,
}

struct Peer {
    name: &'static str,
    free_slot: bool,
    /// Bytes per second.
    speed: u32,
}

const PEERS: &[Peer] = &[
    Peer {
        name: "alice",
        free_slot: true,
        speed: 3_100_000,
    },
    Peer {
        name: "vinyl_rips",
        free_slot: true,
        speed: 2_200_000,
    },
    Peer {
        name: "marten_k",
        free_slot: false,
        speed: 640_000,
    },
    Peer {
        name: "dj_ivo",
        free_slot: true,
        speed: 1_400_000,
    },
    Peer {
        name: "nightbus",
        free_slot: true,
        speed: 220_000,
    },
    Peer {
        name: "kassi",
        free_slot: true,
        speed: 900_000,
    },
    Peer {
        name: "tomasz",
        free_slot: false,
        speed: 1_800_000,
    },
];

#[derive(Clone, Copy)]
enum Q {
    Flac16,
    Flac24,
    Mp3,
    V0,
    Ogg,
    Wav,
}

fn file(user: &str, folder: &str, name: &str, q: Q, secs: u32) -> RemoteFile {
    let (bytes_per_sec, attrs) = match q {
        Q::Flac16 => (
            105_000,
            AudioAttrs {
                sample_rate_hz: Some(44_100),
                bit_depth: Some(16),
                ..AudioAttrs::default()
            },
        ),
        Q::Flac24 => (
            400_000,
            AudioAttrs {
                sample_rate_hz: Some(96_000),
                bit_depth: Some(24),
                ..AudioAttrs::default()
            },
        ),
        Q::Mp3 => (
            40_000,
            AudioAttrs {
                bitrate_kbps: Some(320),
                vbr: Some(false),
                ..AudioAttrs::default()
            },
        ),
        Q::V0 => (
            31_000,
            AudioAttrs {
                bitrate_kbps: Some(245),
                vbr: Some(true),
                ..AudioAttrs::default()
            },
        ),
        Q::Wav => (
            176_400,
            AudioAttrs {
                sample_rate_hz: Some(44_100),
                bit_depth: Some(16),
                ..AudioAttrs::default()
            },
        ),
        Q::Ogg => (
            24_000,
            AudioAttrs {
                bitrate_kbps: Some(192),
                ..AudioAttrs::default()
            },
        ),
    };
    RemoteFile {
        username: user.into(),
        path: format!("{folder}\\{name}"),
        size: u64::from(secs) * bytes_per_sec + u64::from(secs % 7) * 13_007,
        attrs: AudioAttrs {
            duration_secs: Some(secs),
            ..attrs
        },
    }
}

/// Every file every fake peer shares.
/// A user with a big, deep share (browse only): the folder tree scrolls.
pub const DEEP_USER: &str = "crate_digger";
/// A folder far down [`DEEP_USER`]'s tree.
pub const DEEP_FOLDER: &str = r"Music\Jungle\Iration Steppas\Iration Steppas - 1994 - Scud Missile";

/// [`DEEP_USER`]'s share: 12 genres × 48 artists × 3 albums of 2 tracks, plus
/// [`DEEP_FOLDER`].
fn deep_share() -> Vec<BrowseDir> {
    const GENRES: [&str; 12] = [
        "Ambient",
        "Breakbeat",
        "Dub",
        "Electro",
        "Footwork",
        "Garage",
        "House",
        "Jungle",
        "Reggae",
        "Soul",
        "Techno",
        "Trip Hop",
    ];
    const FIRST: [&str; 12] = [
        "Acid", "Bass", "Blue", "Cold", "Dark", "Echo", "Free", "Ghost", "Hazy", "Low", "Night",
        "Silver",
    ];
    const SECOND: [&str; 4] = ["Signals", "Harbour", "Collective", "Static"];
    let user = DEEP_USER;
    let mut dirs = Vec::new();
    let mut album = |folder: String| {
        let files = (1..=2)
            .map(|n| {
                file(
                    user,
                    &folder,
                    &format!("0{n} Track {n}.flac"),
                    Q::Flac16,
                    240 + n,
                )
            })
            .collect();
        dirs.push(BrowseDir {
            path: folder,
            files,
        });
    };
    for g in GENRES {
        for (i, a) in FIRST
            .iter()
            .flat_map(|f| SECOND.map(|s| format!("{f} {s}")))
            .enumerate()
        {
            for y in 0..3 {
                let year = 1990 + (i + y * 7) % 30;
                album(format!(r"Music\{g}\{a}\{a} - {year} - Volume {}", y + 1));
            }
        }
    }
    album(DEEP_FOLDER.to_string());
    dirs
}

pub fn catalog() -> Vec<RemoteFile> {
    let coast = r"Music\Hollow Lights\Coastline (2019)";
    let ferry = r"Music\Hollow Lights\Night Ferry EP (2021)";
    let pale = r"Music\Pale Harbour\Signals (2017)";
    let rips = r"rips\Hollow Lights - Coastline (2019) [24-96]";
    let mut v = Vec::new();
    for (i, (name, secs)) in [
        ("Northern Glass", 250),
        ("Salt Roads", 224),
        ("Low Tide Radio", 262),
        ("Breakwater", 198),
        ("A Lamp in the Dunes", 301),
        ("Coastline", 277),
    ]
    .into_iter()
    .enumerate()
    {
        v.push(file(
            "alice",
            coast,
            &format!("{:02} {name}.flac", i + 1),
            Q::Flac16,
            secs,
        ));
    }
    // Not audio: hidden by the default audio-only filter.
    for (name, size) in [
        ("cover.jpg", 1_204_331),
        ("Hollow Lights - Coastline.log", 6_120),
    ] {
        v.push(RemoteFile {
            username: "alice".into(),
            path: format!("{coast}\\{name}"),
            size,
            attrs: AudioAttrs::default(),
        });
    }
    for (i, (name, secs)) in [
        ("Night Ferry", 233),
        ("Harbour Lights", 205),
        ("Undertow", 246),
        ("Morning Crossing", 270),
    ]
    .into_iter()
    .enumerate()
    {
        v.push(file(
            "alice",
            ferry,
            &format!("{:02} {name}.flac", i + 1),
            Q::Flac24,
            secs,
        ));
    }
    // Folder junk "Download folder" skips (transfers::JUNK_*).
    for (name, size) in [
        ("Thumbs.db", 18_432),
        ("desktop.ini", 182),
        ("Night Ferry EP.url", 96),
    ] {
        v.push(RemoteFile {
            username: "alice".into(),
            path: format!("{ferry}\\{name}"),
            size,
            attrs: AudioAttrs::default(),
        });
    }
    for (i, (name, secs)) in [
        ("Signals", 214),
        ("Static Bloom", 187),
        ("Lighthouse Keeper", 239),
    ]
    .into_iter()
    .enumerate()
    {
        v.push(file(
            "alice",
            pale,
            &format!("{:02} {name}.mp3", i + 1),
            Q::Mp3,
            secs,
        ));
    }
    v.push(file(
        "vinyl_rips",
        rips,
        "A1 Northern Glass.flac",
        Q::Flac24,
        251,
    ));
    v.push(file(
        "vinyl_rips",
        rips,
        "A2 Salt Roads.flac",
        Q::Flac24,
        226,
    ));
    v.push(file(
        "marten_k",
        r"shared\lossless\HL",
        "Hollow Lights - Salt Roads.flac",
        Q::Flac16,
        224,
    ));
    v.push(file(
        "dj_ivo",
        r"mp3\H\Hollow Lights",
        "Hollow Lights - Salt Roads.mp3",
        Q::Mp3,
        224,
    ));
    v.push(file(
        "dj_ivo",
        r"mp3\H\Hollow Lights",
        "Hollow Lights - Low Tide Radio.mp3",
        Q::Mp3,
        262,
    ));
    v.push(file(
        "nightbus",
        r"bootlegs\2021",
        "hollow_lights_salt_roads_live.mp3",
        Q::V0,
        241,
    ));
    v.push(file(
        "kassi",
        r"music\hollow lights",
        "salt roads (demo).ogg",
        Q::Ogg,
        219,
    ));
    v.push(file(
        "kassi",
        r"music\hollow lights",
        "low tide radio (live).opus",
        Q::Ogg,
        258,
    ));
    // Uncompressed rips: what the WAV chip finds.
    for (name, secs) in [("Salt Roads", 224), ("Northern Glass", 250)] {
        v.push(file(
            "tomasz",
            r"wav\Hollow Lights - Coastline",
            &format!("Hollow Lights - {name}.wav"),
            Q::Wav,
            secs,
        ));
    }
    v
}

fn peer(name: &str) -> &'static Peer {
    PEERS.iter().find(|p| p.name == name).unwrap_or(&PEERS[0])
}

fn hit(f: &RemoteFile) -> SearchHit {
    let p = peer(&f.username);
    SearchHit {
        file: f.clone(),
        free_slot: p.free_slot,
        speed: p.speed,
    }
}

/// Bytes on "disk" per `.part` path, shared by the fake backend (writer) and fake player (reader).
#[derive(Default)]
pub struct World {
    buffers: HashMap<PathBuf, Buffer>,
}

#[derive(Clone, Copy)]
struct Buffer {
    bytes: u64,
    total: u64,
    duration: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Waiting,
    RemoteQueued,
    Active,
    Done,
    Stopped,
}

struct FakeTransfer {
    id: TransferId,
    file: RemoteFile,
    part_path: PathBuf,
    final_path: PathBuf,
    phase: Phase,
    /// Clock time the next phase starts (bytes start flowing / remote queue clears).
    starts_at: f64,
    bytes: u64,
    last_tick: f64,
    last_emit: f64,
    fail_at: Option<u64>,
    kind: TransferKind,
}

pub struct DemoBackend {
    settings: Settings,
    next_id: u64,
    now: f64,
    calls: Vec<Call>,
    /// Scheduled events, in due order.
    queue: Vec<(f64, CoreEvent)>,
    transfers: Vec<FakeTransfer>,
    world: Rc<RefCell<World>>,
    marten_failed: bool,
    shares: Vec<PathBuf>,
    /// Who signed in (reconnects come back as them).
    user: String,
    /// Wall-clock origin of the virtual clock, so `status()` deadlines are stable.
    epoch: Instant,
    /// A (re)scan runs until this time; files found per share, +3 per rescan.
    scan_until: f64,
    rescans: u32,
    audio_only: bool,
    uploads: Vec<UploadRow>,
    /// Open search windows: id, query, clock end; files and users delivered so far.
    searches: Vec<(
        SearchId,
        String,
        f64,
        usize,
        std::collections::HashSet<String>,
    )>,
    connected: bool,
    /// A scripted reconnect wait: attempt and its clock window.
    reconnect: Option<(u32, f64, f64)>,
    /// Cached plays from earlier runs, until "Clear cache".
    cache_base: u64,
    /// Download history: dj_ivo's "Low Tide Radio" mp3 from an earlier run, plus every
    /// download finished since.
    downloaded: wholeseek_core::DownloadedKeys,
    /// What each search was: typed, and what it was broadened to when nothing matched.
    infos: HashMap<SearchId, wholeseek_core::status::SearchInfo>,
    /// The drawer's History: seeded, then every ↓ that ends.
    history: crate::demo_history::DemoHistory,
}

/// A port the demo pretends another program holds: the session falls back to 45543.
pub const DEMO_BUSY_PORT: u16 = 2240;
const DEMO_FALLBACK_PORT: u16 = 45543;

impl DemoBackend {
    pub fn new(world: Rc<RefCell<World>>) -> Self {
        let settings = Settings {
            download_dir: PathBuf::from("/home/demo/Downloads/wholeseek"),
            cache_dir: PathBuf::from("/home/demo/.cache/wholeseek"),
            ..Settings::default()
        };
        let history = crate::demo_history::DemoHistory::seeded(&settings.download_dir);
        Self {
            settings,
            history,
            next_id: 1,
            now: 0.0,
            calls: Vec::new(),
            queue: Vec::new(),
            transfers: Vec::new(),
            world,
            marten_failed: false,
            shares: Vec::new(),
            user: String::new(),
            epoch: Instant::now(),
            scan_until: 0.0,
            rescans: 0,
            audio_only: true,
            uploads: Vec::new(),
            searches: Vec::new(),
            connected: false,
            reconnect: None,
            cache_base: 412 * 1024 * 1024,
            infos: HashMap::new(),
            downloaded: std::sync::Arc::new(
                [(
                    "dj_ivo".to_string(),
                    r"mp3\H\Hollow Lights\Hollow Lights - Low Tide Radio.mp3".to_string(),
                )]
                .into_iter()
                .collect(),
            ),
        }
    }

    fn bound_port(&self) -> u16 {
        if self.settings.listen_port == DEMO_BUSY_PORT {
            DEMO_FALLBACK_PORT
        } else {
            self.settings.listen_port
        }
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn at(&mut self, delay: f64, e: CoreEvent) {
        let due = self.now + delay;
        let i = self.queue.partition_point(|(t, _)| *t <= due);
        self.queue.insert(i, (due, e));
    }

    /// One file into `root`, flat or in its album folder, as `Core::download` decides
    /// (`Settings::single_file_subfolder`).
    fn fetch_single(&mut self, f: &RemoteFile, root: &Path) -> TransferId {
        let id = TransferId(self.id());
        let dir = if self.settings.single_file_subfolder {
            paths::download_dir_for(root, &f.path)
        } else {
            root.to_path_buf()
        };
        self.start_transfer(id, f, &dir, TransferKind::Download);
        id
    }

    /// A folder's files into `<root>/<remote folder name>/`.
    fn fetch_folder(&mut self, username: &str, folder: &str, root: &Path) {
        let files: Vec<RemoteFile> = catalog()
            .into_iter()
            .filter(|f| f.username == username && f.folder() == folder)
            .filter(|f| !wholeseek_core::is_download_junk(&f.path))
            .collect();
        for f in files {
            let id = TransferId(self.id());
            let dir = paths::download_dir_for(root, &f.path);
            self.start_transfer(id, &f, &dir, TransferKind::Download);
        }
    }

    fn start_transfer(
        &mut self,
        id: TransferId,
        file: &RemoteFile,
        dir: &Path,
        kind: TransferKind,
    ) {
        let part_path = paths::part_path(dir, &file.path);
        let final_path = paths::final_path(dir, &file.path);
        let fail_at = (file.username == "marten_k" && !self.marten_failed).then(|| {
            self.marten_failed = true;
            file.size * 2 / 5
        });
        let remote_queue = file.username == "nightbus";
        self.at(
            0.05,
            CoreEvent::TransferQueued {
                id,
                kind,
                file: file.clone(),
                part_path: part_path.clone(),
            },
        );
        if remote_queue {
            self.at(0.15, CoreEvent::TransferRemoteQueued { id, place: Some(3) });
        }
        let duration = f64::from(file.attrs.duration_secs.unwrap_or(240));
        self.world.borrow_mut().buffers.insert(
            part_path.clone(),
            Buffer {
                bytes: 0,
                total: file.size,
                duration,
            },
        );
        self.transfers.push(FakeTransfer {
            id,
            file: file.clone(),
            part_path,
            final_path,
            phase: if remote_queue {
                Phase::RemoteQueued
            } else {
                Phase::Waiting
            },
            starts_at: self.now + if remote_queue { 1.5 } else { 0.3 },
            bytes: 0,
            last_tick: self.now,
            last_emit: f64::NEG_INFINITY,
            fail_at,
            kind,
        });
    }

    fn step_transfers(&mut self, out: &mut Vec<CoreEvent>) {
        let now = self.now;
        for t in &mut self.transfers {
            if matches!(t.phase, Phase::Waiting | Phase::RemoteQueued) && now >= t.starts_at {
                t.phase = Phase::Active;
                t.last_tick = t.starts_at;
            }
            if t.phase != Phase::Active {
                continue;
            }
            let speed = f64::from(peer(&t.file.username).speed);
            let gained = ((now - t.last_tick).max(0.0) * speed) as u64;
            t.last_tick = now;
            let mut bytes = (t.bytes + gained).min(t.file.size);
            if let Some(fail) = t.fail_at
                && bytes >= fail
            {
                bytes = fail;
                t.bytes = bytes;
                t.phase = Phase::Stopped;
                out.push(CoreEvent::TransferFailed {
                    id: t.id,
                    reason: "peer closed the connection".into(),
                });
                continue;
            }
            // The library reports right after opening the .part, then every 128 KiB; the demo
            // throttles to 4 a second.
            let first = t.bytes == 0 && t.last_emit == f64::NEG_INFINITY;
            t.bytes = bytes;
            if let Some(b) = self.world.borrow_mut().buffers.get_mut(&t.part_path) {
                b.bytes = bytes;
            }
            if first || now - t.last_emit >= 0.25 || bytes == t.file.size {
                t.last_emit = now;
                out.push(CoreEvent::TransferProgress {
                    id: t.id,
                    bytes,
                    total: t.file.size,
                    speed,
                    part_path: t.part_path.clone(),
                });
            }
            if bytes == t.file.size {
                t.phase = Phase::Done;
                // The finished file plays from disk too.
                let mut w = self.world.borrow_mut();
                if let Some(b) = w.buffers.get(&t.part_path).copied() {
                    w.buffers.insert(t.final_path.clone(), b);
                }
                drop(w);
                if t.kind == TransferKind::Download {
                    std::sync::Arc::make_mut(&mut self.downloaded)
                        .insert((t.file.username.clone(), t.file.path.clone()));
                }
                out.push(CoreEvent::TransferDone {
                    id: t.id,
                    path: t.final_path.clone(),
                });
            }
        }
    }

    /// Every ↓ that ended in `events` goes in the history (▶ plays don't), as the core's
    /// worker does before sending the event.
    fn record_history(&mut self, events: &[CoreEvent]) {
        use wholeseek_core::Outcome;
        for e in events {
            let (id, outcome, path, reason) = match e {
                CoreEvent::TransferDone { id, path } => {
                    (*id, Outcome::Done, Some(path.clone()), None)
                }
                CoreEvent::TransferFailed { id, reason } => {
                    (*id, Outcome::Failed, None, Some(reason.clone()))
                }
                CoreEvent::TransferCancelled { id } => (*id, Outcome::Cancelled, None, None),
                _ => continue,
            };
            let Some(t) = self
                .transfers
                .iter()
                .find(|t| t.id == id && t.kind == TransferKind::Download)
            else {
                continue;
            };
            let file = t.file.clone();
            self.history.record(id, &file, outcome, path, reason);
        }
    }

    fn browse_dirs(username: &str) -> Vec<BrowseDir> {
        let mut dirs: Vec<BrowseDir> = Vec::new();
        for f in catalog().into_iter().filter(|f| f.username == username) {
            let folder = f.folder().to_string();
            match dirs.iter_mut().find(|d| d.path == folder) {
                Some(d) => d.files.push(f),
                None => dirs.push(BrowseDir {
                    path: folder,
                    files: vec![f],
                }),
            }
        }
        if username == DEEP_USER {
            return deep_share();
        }
        if username == "alice" {
            dirs.push(BrowseDir {
                path: r"Music\Unsorted".into(),
                files: Vec::new(),
            });
        }
        dirs
    }
}

impl Backend for DemoBackend {
    fn settings(&self) -> &Settings {
        &self.settings
    }

    fn login(&mut self, c: Credentials) {
        self.calls.push(Call::Login(c.username.clone()));
        self.user.clone_from(&c.username);
        if c.password == "wrong" {
            self.at(
                0.4,
                CoreEvent::Disconnected {
                    reason: DisconnectReason::LoginRejected,
                    will_retry: false,
                },
            );
            return;
        }
        // "flaky" loses the connection after a second and is back 5 s later.
        if c.username == "flaky" {
            self.at(
                1.0,
                CoreEvent::Disconnected {
                    reason: DisconnectReason::Network(
                        "the connection to the server dropped".into(),
                    ),
                    will_retry: true,
                },
            );
            self.reconnect = Some((1, self.now + 1.0, self.now + 6.0));
            self.at(
                6.0,
                CoreEvent::Connected {
                    username: c.username.clone(),
                    listen_port: Some(self.bound_port()),
                },
            );
        }
        // "twice" is signed in elsewhere a moment later (the server ends this session).
        if c.username == "twice" {
            self.at(
                1.5,
                CoreEvent::Disconnected {
                    reason: DisconnectReason::Displaced,
                    will_retry: false,
                },
            );
        }
        self.at(
            0.3,
            CoreEvent::Connected {
                username: c.username,
                listen_port: Some(self.bound_port()),
            },
        );
        let n = u32::try_from(self.shares.len()).unwrap_or(0);
        self.at(
            0.6,
            CoreEvent::ShareCounts {
                folders: 42 * n,
                files: 518 * n,
            },
        );
        let up = |user: &str, path: &str, size: u64, sent: u64, state| UploadRow {
            username: user.into(),
            path: path.into(),
            size,
            bytes_sent: sent,
            speed: if state == UploadState::InProgress {
                480_000.0
            } else {
                0.0
            },
            state,
        };
        self.uploads = vec![
            up(
                "reed_s",
                r"Music\Low Roar\0 (2014)\03 Easy Way Out.flac",
                31_200_000,
                12_900_000,
                UploadState::InProgress,
            ),
            up(
                "kassi",
                r"Music\Low Roar\0 (2014)\04 Breathe In.flac",
                27_400_000,
                27_400_000,
                UploadState::Completed,
            ),
            up(
                "tomasz",
                r"Music\Low Roar\0 (2014)\05 Phantoms.flac",
                29_100_000,
                0,
                UploadState::Queued { place: 1 },
            ),
        ];
        self.at(0.8, CoreEvent::Uploads(self.uploads.clone()));
        let port = self.settings.listen_port;
        let outcome = wholeseek_core::PortMapOutcome::Mapped {
            method: "upnp".into(),
            external: Some(format!("203.0.113.7:{port}")),
        };
        self.at(0.9, CoreEvent::PortMapping { port, outcome });
    }

    fn logout(&mut self) {
        self.calls.push(Call::Logout);
        self.at(
            0.05,
            CoreEvent::Disconnected {
                reason: DisconnectReason::User,
                will_retry: false,
            },
        );
    }

    fn search(&mut self, query: &str) -> SearchId {
        let query = query.trim().to_string();
        self.calls.push(Call::Search(query.clone()));
        let id = SearchId(self.id());
        let find = |q: &str| -> Vec<SearchHit> {
            let words: Vec<String> = q
                .to_lowercase()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            catalog()
                .iter()
                .filter(|f| {
                    let p = f.path.to_lowercase().replace(['_', '-'], " ");
                    !words.is_empty() && words.iter().all(|w| p.contains(w.as_str()))
                })
                .map(hit)
                .collect()
        };
        let sent = wholeseek_core::query::normalize_query(&query);
        let mut hits = find(&query);
        // As the core does: nothing found, so once more with a broader query.
        let mut broadened_to = None;
        if hits.is_empty()
            && let Some(b) = wholeseek_core::query::broaden_query(&sent)
        {
            hits = find(&b);
            broadened_to = Some(b);
        }
        self.infos.insert(
            id,
            wholeseek_core::status::SearchInfo {
                id,
                typed: query.clone(),
                sent,
                broadened_to,
                wishlist: false,
                files: hits.len(),
                users: hits
                    .iter()
                    .map(|h| h.file.username.as_str())
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                finished: false,
            },
        );
        let total = hits.len();
        // Arrival order is peers answering, not rank order: the UI merges and re-sorts.
        let mut batches: [Vec<SearchHit>; 3] = Default::default();
        for (i, h) in hits.into_iter().enumerate() {
            batches[(i * 7 + 1) % 3].push(h);
        }
        for (i, mut batch) in batches.into_iter().enumerate() {
            if batch.is_empty() {
                continue;
            }
            rank::rank(&mut batch, self.audio_only);
            self.at(
                0.25 + 0.4 * i as f64,
                CoreEvent::SearchResults {
                    search: id,
                    hits: batch,
                },
            );
        }
        self.searches.retain(|s| s.1 != query);
        self.searches
            .push((id, query, self.now + 1.4, 0, Default::default()));
        self.at(
            1.4,
            CoreEvent::SearchFinished {
                search: id,
                total_files: total,
            },
        );
        id
    }

    fn cancel_search(&mut self, id: SearchId) {
        self.calls.push(Call::CancelSearch(id));
        self.queue.retain(|(_, e)| !matches!(e, CoreEvent::SearchResults { search, .. } | CoreEvent::SearchFinished { search, .. } if *search == id));
    }

    fn browse(&mut self, username: &str) {
        self.calls.push(Call::Browse(username.into()));
        let dirs = Self::browse_dirs(username);
        let e = if dirs.is_empty() {
            CoreEvent::BrowseFailed {
                username: username.into(),
                reason: "user is offline".into(),
            }
        } else {
            CoreEvent::BrowseResult {
                username: username.into(),
                dirs,
            }
        };
        self.at(0.4, e);
    }

    fn play_fetch(&mut self, f: &RemoteFile) -> PlayTicket {
        self.calls.push(Call::PlayFetch(f.clone()));
        let id = TransferId(self.id());
        let dir = paths::cache_transfer_dir(&self.settings.cache_dir, id);
        self.start_transfer(id, f, &dir, TransferKind::Play);
        PlayTicket {
            id,
            part_path: paths::part_path(&dir, &f.path),
            final_path: paths::final_path(&dir, &f.path),
        }
    }

    fn download(&mut self, f: &RemoteFile) -> TransferId {
        self.calls.push(Call::Download(f.clone()));
        let root = self.settings.download_dir.clone();
        self.fetch_single(f, &root)
    }

    fn download_to(&mut self, f: &RemoteFile, dir: &Path) -> TransferId {
        self.calls
            .push(Call::DownloadTo(f.clone(), dir.to_path_buf()));
        self.settings.remember_download_dir(dir);
        self.fetch_single(f, dir)
    }

    fn download_album_file(&mut self, f: &RemoteFile, root: Option<&Path>) -> TransferId {
        self.calls.push(Call::DownloadAlbumFile(
            f.clone(),
            root.map(Path::to_path_buf),
        ));
        let root = match root {
            Some(d) => {
                self.settings.remember_download_dir(d);
                d.to_path_buf()
            }
            None => self.settings.download_dir.clone(),
        };
        let id = TransferId(self.id());
        let dir = paths::download_dir_for(&root, &f.path);
        self.start_transfer(id, f, &dir, TransferKind::Download);
        id
    }

    fn download_folder(&mut self, username: &str, folder: &str) {
        self.calls
            .push(Call::DownloadFolder(username.into(), folder.into()));
        let root = self.settings.download_dir.clone();
        self.fetch_folder(username, folder, &root);
    }

    fn download_folder_to(&mut self, username: &str, folder: &str, dir: &Path) {
        self.calls.push(Call::DownloadFolderTo(
            username.into(),
            folder.into(),
            dir.to_path_buf(),
        ));
        self.settings.remember_download_dir(dir);
        self.fetch_folder(username, folder, dir);
    }

    fn cancel(&mut self, id: TransferId) {
        self.calls.push(Call::Cancel(id));
        if let Some(t) = self
            .transfers
            .iter_mut()
            .find(|t| t.id == id && t.phase != Phase::Done)
        {
            t.phase = Phase::Stopped;
            self.world.borrow_mut().buffers.remove(&t.part_path);
            self.at(0.0, CoreEvent::TransferCancelled { id });
        }
    }

    fn retry(&mut self, id: TransferId) {
        self.calls.push(Call::Retry(id));
        let now = self.now;
        if let Some(t) = self
            .transfers
            .iter_mut()
            .find(|t| t.id == id && t.phase == Phase::Stopped)
        {
            t.phase = Phase::Waiting;
            t.starts_at = now + 0.3;
            t.fail_at = None;
            t.last_emit = f64::NEG_INFINITY;
            if !self.world.borrow().buffers.contains_key(&t.part_path) {
                t.bytes = 0; // cancelled: the .part was deleted
            }
            let duration = f64::from(t.file.attrs.duration_secs.unwrap_or(240));
            self.world.borrow_mut().buffers.insert(
                t.part_path.clone(),
                Buffer {
                    bytes: t.bytes,
                    total: t.file.size,
                    duration,
                },
            );
            let e = CoreEvent::TransferQueued {
                id,
                kind: TransferKind::Download,
                file: t.file.clone(),
                part_path: t.part_path.clone(),
            };
            self.at(0.05, e);
        }
    }

    fn set_shares(&mut self, dirs: Vec<PathBuf>) {
        self.calls.push(Call::SetShares(dirs.clone()));
        self.settings.share_dirs.clone_from(&dirs);
        self.shares = dirs;
        self.scan_until = self.now + 0.5;
        let n = u32::try_from(self.shares.len()).unwrap_or(0);
        self.at(
            0.5,
            CoreEvent::ShareCounts {
                folders: 42 * n,
                files: 518 * n,
            },
        );
    }

    fn rescan_shares(&mut self) {
        self.calls.push(Call::RescanShares);
        self.scan_until = self.now + 0.5;
        self.rescans += 1;
        let n = u32::try_from(self.shares.len()).unwrap_or(0);
        self.at(
            0.5,
            CoreEvent::ShareCounts {
                folders: 42 * n,
                files: 518 * n + 3,
            },
        );
    }

    fn apply_settings(&mut self, settings: Settings) {
        self.calls.push(Call::ApplySettings(settings.clone()));
        let port_changed = settings.listen_port != self.settings.listen_port;
        self.settings = settings;
        // As the core: a new listen port reconnects the session.
        if port_changed && self.connected {
            self.at(
                0.0,
                CoreEvent::Disconnected {
                    reason: DisconnectReason::Network("listen port changed; reconnecting".into()),
                    will_retry: true,
                },
            );
            self.at(
                0.5,
                CoreEvent::Connected {
                    username: self.user.clone(),
                    listen_port: Some(self.bound_port()),
                },
            );
        }
    }

    fn status(&self) -> CoreStatus {
        let ahead = |at: f64| self.epoch + Duration::from_secs_f64(at.max(0.0));
        let bound = self.connected.then(|| self.bound_port());
        let requested = self.settings.listen_port;
        let live_play = |t: &&FakeTransfer| {
            t.kind == TransferKind::Play
                && matches!(
                    t.phase,
                    Phase::Waiting | Phase::RemoteQueued | Phase::Active
                )
        };
        let plays: u64 = self
            .transfers
            .iter()
            .filter(|t| t.kind == TransferKind::Play && t.phase != Phase::Stopped)
            .map(|t| t.bytes)
            .sum();
        CoreStatus {
            reconnect: self
                .reconnect
                .filter(|(_, from, until)| (*from..*until).contains(&self.now))
                .map(|(attempt, _, until)| ReconnectStatus {
                    attempt,
                    delay: Duration::from_secs(5),
                    at: ahead(until),
                }),
            listen: ListenStatus {
                requested,
                bound,
                note: bound
                    .filter(|b| *b != requested)
                    .map(|b| format!("port {requested} is in use, so the system picked port {b}")),
            },
            searches: self
                .searches
                .iter()
                .filter(|s| s.2 > self.now)
                .map(|(id, query, end, files, users)| SearchProgress {
                    id: *id,
                    query: query.clone(),
                    files: *files,
                    users: users.len(),
                    ends_at: ahead(*end),
                })
                .collect(),
            cache: CacheUsage {
                used_bytes: self.cache_base + plays,
                cap_bytes: self.settings.cache_cap_bytes,
                in_use_bytes: self
                    .transfers
                    .iter()
                    .filter(live_play)
                    .map(|t| t.bytes)
                    .sum(),
            },
            shares: self
                .shares
                .iter()
                .map(|dir| wholeseek_core::status::ShareStatus {
                    dir: dir.clone(),
                    folders: 42,
                    files: 518 + 3 * self.rescans,
                    scanning: self.now < self.scan_until,
                })
                .collect(),
        }
    }

    fn downloaded_keys(&self) -> wholeseek_core::DownloadedKeys {
        self.downloaded.clone()
    }

    fn history(&self, query: &wholeseek_core::HistoryQuery) -> Vec<wholeseek_core::HistoryEntry> {
        self.history.query(query)
    }

    fn history_remove(&mut self, id: u64) -> bool {
        self.calls.push(Call::HistoryRemove(id));
        let removed = self.history.remove(id);
        self.downloaded = std::sync::Arc::new(self.history.keys());
        removed
    }

    fn history_clear(&mut self) -> Result<(), String> {
        self.calls.push(Call::HistoryClear);
        self.history.clear();
        self.downloaded = std::sync::Arc::new(self.history.keys());
        Ok(())
    }

    fn history_on_disk(&self, entry: &wholeseek_core::HistoryEntry) -> bool {
        crate::demo_history::DemoHistory::on_disk(entry)
    }

    fn search_info(&self, id: SearchId) -> Option<wholeseek_core::status::SearchInfo> {
        self.infos.get(&id).cloned()
    }

    fn wishlist_add(&mut self, query: &str) -> bool {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let have = self
            .settings
            .wishlist
            .iter()
            .any(|w| w.eq_ignore_ascii_case(&q));
        if q.is_empty() || have {
            return false;
        }
        self.calls.push(Call::WishlistAdd(q.clone()));
        self.settings.wishlist.push(q);
        true
    }

    fn wishlist_remove(&mut self, query: &str) -> bool {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
        let before = self.settings.wishlist.len();
        self.settings
            .wishlist
            .retain(|w| !w.eq_ignore_ascii_case(&q));
        let removed = self.settings.wishlist.len() != before;
        if removed {
            self.calls.push(Call::WishlistRemove(q));
        }
        removed
    }

    fn clear_cache(&mut self) {
        self.calls.push(Call::ClearCache);
        self.cache_base = 0;
        let keep: Vec<TransferId> = self
            .transfers
            .iter()
            .filter(|t| {
                matches!(
                    t.phase,
                    Phase::Waiting | Phase::RemoteQueued | Phase::Active
                )
            })
            .map(|t| t.id)
            .collect();
        self.transfers
            .retain(|t| t.kind != TransferKind::Play || keep.contains(&t.id));
    }

    fn cancel_upload(&mut self, username: &str, path: &str) {
        self.calls
            .push(Call::CancelUpload(username.into(), path.into()));
        if let Some(u) = self
            .uploads
            .iter_mut()
            .find(|u| u.username == username && u.path == path)
        {
            u.state = UploadState::Cancelled;
            u.speed = 0.0;
            let rows = self.uploads.clone();
            self.at(0.05, CoreEvent::Uploads(rows));
        }
    }

    fn user_profile(&mut self, username: &str) {
        self.calls.push(Call::UserProfile(username.into()));
        for (delay, e) in demo_profile(username) {
            self.at(delay, e);
        }
    }

    fn set_audio_only(&mut self, on: bool) {
        self.calls.push(Call::SetAudioOnly(on));
        self.audio_only = on;
    }

    fn drain(&mut self, now: f64) -> Vec<CoreEvent> {
        self.now = now;
        let due = self.queue.partition_point(|(t, _)| *t <= now);
        let mut out: Vec<CoreEvent> = self.queue.drain(..due).map(|(_, e)| e).collect();
        // An uploads snapshot is taken when it is sent, like the core's.
        for e in &mut out {
            match e {
                CoreEvent::Uploads(rows) => rows.clone_from(&self.uploads),
                CoreEvent::Connected { .. } => self.connected = true,
                CoreEvent::Disconnected { .. } => self.connected = false,
                CoreEvent::SearchResults { search, hits } => {
                    if let Some(s) = self.searches.iter_mut().find(|s| s.0 == *search) {
                        s.3 += hits.len();
                        s.4.extend(hits.iter().map(|h| h.file.username.clone()));
                    }
                }
                _ => {}
            }
        }
        self.step_transfers(&mut out);
        self.record_history(&out);
        out
    }

    fn next_tick(&self) -> Option<f64> {
        let active = self.transfers.iter().any(|t| {
            matches!(
                t.phase,
                Phase::Waiting | Phase::RemoteQueued | Phase::Active
            )
        });
        let next_event = self.queue.first().map(|(t, _)| (t - self.now).max(0.0));
        match (active, next_event) {
            (true, Some(d)) => Some(d.min(0.1)),
            (true, None) => Some(0.1),
            (false, d) => d,
        }
    }

    fn calls(&self) -> &[Call] {
        &self.calls
    }
}

/// Simulates `wholeseek_audio::Player` against [`World`]'s byte counts: buffering until bytes
/// arrive, playing in clock time, refusing seeks past the buffered range.
pub struct FakePlayer {
    world: Rc<RefCell<World>>,
    path: Option<PathBuf>,
    state: PlayState,
    position: f64,
    last: Option<f64>,
    volume: f32,
    notice: Option<String>,
    queue: Vec<QueueItem>,
    index: Option<usize>,
    prefetched: bool,
    events: Vec<PlayerEvent>,
}

impl FakePlayer {
    pub fn new(world: Rc<RefCell<World>>) -> Self {
        Self {
            world,
            path: None,
            state: PlayState::Idle,
            position: 0.0,
            last: None,
            volume: 0.8,
            notice: None,
            queue: Vec::new(),
            index: None,
            prefetched: false,
            events: Vec::new(),
        }
    }

    fn start(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.state = PlayState::Buffering;
        self.position = 0.0;
        self.notice = None;
    }

    /// As the real player: `TrackChanged`, then play the item or wait for its path.
    fn goto(&mut self, i: usize) {
        self.index = Some(i);
        self.prefetched = false;
        let item = self.queue[i].clone();
        self.events.push(PlayerEvent::TrackChanged {
            index: i,
            id: item.id,
        });
        self.start(item.path);
    }

    fn buffer(&self) -> Option<Buffer> {
        self.world
            .borrow()
            .buffers
            .get(self.path.as_ref()?)
            .copied()
    }
}

impl Buffer {
    fn fraction(self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.bytes as f64 / self.total as f64
        }
    }
    fn secs(self) -> f64 {
        self.fraction() * self.duration
    }
}

impl Playback for FakePlayer {
    fn play(&mut self, path: &Path, _total: u64) {
        self.queue.clear();
        self.index = None;
        self.start(Some(path.to_path_buf()));
    }
    fn pause(&mut self) {
        if matches!(self.state, PlayState::Playing | PlayState::Buffering) {
            self.state = PlayState::Paused;
        }
    }
    fn resume(&mut self) {
        if matches!(self.state, PlayState::Paused | PlayState::Ended) {
            if self.state == PlayState::Ended {
                self.position = 0.0;
            }
            self.state = PlayState::Buffering;
        }
    }
    fn stop(&mut self) {
        self.queue.clear();
        self.index = None;
        self.path = None;
        self.state = PlayState::Idle;
        self.position = 0.0;
    }
    fn seek(&mut self, secs: f64) -> Result<(), SeekError> {
        let b = self.buffer().ok_or(SeekError::NothingPlaying)?;
        let edge = (b.secs() - 0.5).max(0.0);
        if secs > edge && b.bytes < b.total {
            let e = SeekError::NotBuffered {
                requested: secs,
                buffered_secs: b.secs(),
            };
            self.notice = Some(e.to_string());
            return Err(e);
        }
        self.position = secs.clamp(0.0, b.duration);
        Ok(())
    }
    fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
    }
    fn snapshot(&self) -> PlayerSnapshot {
        let b = self.buffer();
        PlayerSnapshot {
            state: self.state.clone(),
            path: self.path.clone(),
            position_secs: self.position,
            duration_secs: b.map(|b| b.duration),
            duration_estimated: false,
            buffered_fraction: b.map_or(0.0, Buffer::fraction),
            buffered_secs: b.map(Buffer::secs),
            volume: self.volume,
            notice: self.notice.clone(),
        }
    }
    fn set_queue(&mut self, items: Vec<QueueItem>, start: usize) {
        self.queue.clear();
        self.index = None;
        if start >= items.len() {
            self.stop();
            return;
        }
        self.queue = items;
        self.goto(start);
    }
    fn set_queue_path(&mut self, index: usize, path: &Path, size: u64) -> bool {
        let Some(item) = self.queue.get_mut(index) else {
            return false;
        };
        item.path = Some(path.to_path_buf());
        item.size = size;
        if self.index == Some(index) && self.path.is_none() {
            self.start(Some(path.to_path_buf()));
        }
        true
    }
    fn next(&mut self) {
        let Some(i) = self.index else { return };
        if i + 1 < self.queue.len() {
            self.goto(i + 1);
        } else {
            self.events.push(PlayerEvent::QueueEnded);
            self.state = PlayState::Ended;
        }
    }
    fn previous(&mut self) {
        let Some(i) = self.index else { return };
        if self.position > wholeseek_audio::PREVIOUS_RESTART_SECS {
            self.position = 0.0;
        } else {
            self.goto(i.saturating_sub(1));
        }
    }
    fn queue_index(&self) -> Option<usize> {
        self.index
    }
    fn events(&mut self) -> Vec<PlayerEvent> {
        std::mem::take(&mut self.events)
    }
    fn tick(&mut self, now: f64) {
        let dt = self.last.map_or(0.0, |l| (now - l).max(0.0));
        self.last = Some(now);
        let Some(b) = self.buffer() else { return };
        let complete = b.bytes >= b.total;
        if let Some(i) = self.index
            && i + 1 < self.queue.len()
            && !self.prefetched
            && (b.duration - self.position <= wholeseek_audio::PREFETCH_REMAINING_SECS
                || (complete && self.position > b.duration / 2.0))
        {
            self.prefetched = true;
            let id = self.queue[i + 1].id.clone();
            self.events
                .push(PlayerEvent::PrefetchNext { index: i + 1, id });
        }
        match self.state {
            PlayState::Buffering if complete || b.secs() > self.position + 1.0 => {
                self.state = PlayState::Playing
            }
            PlayState::Playing => {
                self.position =
                    (self.position + dt).min(if complete { b.duration } else { b.secs() });
                if self.position >= b.duration {
                    self.state = PlayState::Ended;
                    if let Some(i) = self.index {
                        self.events.push(PlayerEvent::Ended);
                        self.events.push(PlayerEvent::TrackEnded { index: i });
                        if i + 1 < self.queue.len() {
                            self.goto(i + 1);
                        } else {
                            self.events.push(PlayerEvent::QueueEnded);
                        }
                    }
                } else if !complete && self.position >= b.secs() - 0.05 {
                    self.state = PlayState::Buffering;
                }
            }
            _ => {}
        }
    }
}

/// The demo's profile answers: server stats first, then the peer's description (and picture),
/// as the real core sends them.
///
/// - alice: everything, picture included (a PNG).
/// - vinyl_rips: stats only.
/// - marten_k: no free slot, queue 12, short description.
/// - dj_ivo: picture bytes no decoder accepts (the header skips the thumbnail).
/// - nightbus (folders browse fine), ghost and unknown users: failure.
pub fn demo_profile(username: &str) -> Vec<(f64, CoreEvent)> {
    // nightbus shares folders but answers nothing about itself.
    let peer = PEERS
        .iter()
        .find(|p| p.name == username && p.name != "nightbus");
    let Some(peer) = peer else {
        return vec![(
            0.6,
            CoreEvent::UserProfileFailed {
                username: username.into(),
                reason: "no answer from the server or the peer".into(),
            },
        )];
    };
    let files = catalog().iter().filter(|f| f.username == username).count() as u32;
    let stats = UserProfile {
        online: Some(true),
        avg_speed: Some(peer.speed),
        shared_files: Some(files * 1_637 + 3),
        shared_dirs: Some(files * 168 + 4),
        ..UserProfile::default()
    };
    let mut full = UserProfile {
        free_slot: Some(peer.free_slot),
        upload_slots: Some(3),
        queue_size: Some(if peer.free_slot { 0 } else { 12 }),
        ..stats.clone()
    };
    match username {
        "alice" => {
            full.description = Some(
                "Lossless only, mostly ambient, post-rock and old Warp records. Ripped from my own \
                 CDs with EAC, logs and cues included.\n\n\
                 Please don't queue whole discographies at once — take what you like and share \
                 back.\n\
                 Slow upload in the evenings: the house is on a long DSL line.\n\
                 Requests by message, I check on weekends."
                    .into(),
            );
            full.picture = Some(include_bytes!("../assets/demo-avatar.png").to_vec());
        }
        "vinyl_rips" => {
            return vec![(
                0.3,
                CoreEvent::UserProfile {
                    username: username.into(),
                    profile: stats,
                },
            )];
        }
        "marten_k" => {
            full.description = Some("24/96 needle drops. Be patient with the queue.".into())
        }
        "dj_ivo" => {
            full.description = Some("Mixes and edits.".into());
            full.picture = Some(b"not an image".to_vec());
        }
        _ => {}
    }
    vec![
        (
            0.3,
            CoreEvent::UserProfile {
                username: username.into(),
                profile: stats,
            },
        ),
        (
            0.7,
            CoreEvent::UserProfile {
                username: username.into(),
                profile: full,
            },
        ),
    ]
}
