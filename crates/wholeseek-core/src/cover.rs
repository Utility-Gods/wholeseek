//! Album covers: an on-disk cache of small JPEGs keyed by (user, remote folder), and the
//! worker-side fetcher that downloads a folder's cover image from the peer on demand.
//!
//! - [`Covers`] (cheap to clone, shared with the UI): `cached` is an in-memory lookup that never
//!   touches the disk or the network, so list rows may call it every frame. `state` adds
//!   pending/missing. `store_embedded` takes art extracted from an audio file (it wins over the
//!   folder's image).
//! - [`crate::Core::fetch_cover`] asks the worker to fetch one folder's image: the folder listing
//!   (given, from a cached browse, or a single-folder contents request), [`pick_cover`], then a
//!   library download into `covers/tmp/` — never the downloads folder, never a transfer event,
//!   never history. At most one fetch per peer at a time.
//! - Images are downscaled to at most [`MAX_SIDE`] px and re-encoded as JPEG; the cache is
//!   trimmed to [`DEFAULT_CAP_BYTES`] by least recent use. A folder without an image is
//!   remembered for [`MISS_TTL`]; a failed fetch (peer offline, queued, timed out) for
//!   [`FAIL_TTL`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use soulseek_rs::DownloadStatus;

use crate::paths;

/// Longest side of a cached cover, in pixels.
pub const MAX_SIDE: u32 = 512;
/// Cover images larger than this on the peer are not fetched.
pub const MAX_REMOTE_BYTES: u64 = 5 * 1024 * 1024;
/// Total size of the cover cache before the least recently used covers go.
pub const DEFAULT_CAP_BYTES: u64 = 100 * 1024 * 1024;
/// A folder that has no image is not asked again for this long.
pub const MISS_TTL: Duration = Duration::from_secs(24 * 3600);
/// A fetch that failed (peer offline, remote queue, timeout) is not retried for this long.
pub const FAIL_TTL: Duration = Duration::from_secs(3600);
/// Name of the cover cache directory inside `Settings::cache_dir`.
pub const COVERS_DIR_NAME: &str = "covers";
const TMP_DIR_NAME: &str = "tmp";
const JPEG_QUALITY: u8 = 85;

/// What is known about one folder's cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverState {
    /// Never asked (or the last miss expired).
    Unknown,
    /// Queued or being fetched.
    Pending,
    /// The folder has no usable image, or fetching failed recently.
    Missing,
    /// A JPEG of at most [`MAX_SIDE`] px.
    Ready(PathBuf),
}

/// Cache key of (user, folder): 16 hex digits. The folder's trailing separators, slash
/// direction and case do not matter.
#[must_use]
pub fn cover_key(username: &str, folder: &str) -> String {
    let folder = folder
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in username
        .as_bytes()
        .iter()
        .chain(&[0u8])
        .chain(folder.as_bytes())
    {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Which of a folder's files is its cover: `(basename or path, size)` → index. JPEG/PNG only,
/// non-empty and ≤ [`MAX_REMOTE_BYTES`]. Names with "cover", "front" or "folder" first, then
/// "album*", then any other image that is not obviously a back/disc/inlay scan; the largest
/// within a tier.
#[must_use]
pub fn pick_cover(files: &[(String, u64)]) -> Option<usize> {
    const OTHER_SIDES: [&str; 8] = [
        "back", "inlay", "inside", "tray", "booklet", "cd", "disc", "rear",
    ];
    files
        .iter()
        .enumerate()
        .filter_map(|(i, (name, size))| {
            let base = paths::remote_basename(name).to_lowercase();
            let (stem, ext) = base.rsplit_once('.')?;
            if !matches!(ext, "jpg" | "jpeg" | "png") || *size == 0 || *size > MAX_REMOTE_BYTES {
                return None;
            }
            let words: Vec<&str> = stem
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let tier = if ["cover", "front", "folder"]
                .iter()
                .any(|k| stem.contains(k))
            {
                0
            } else if stem.starts_with("album") {
                1
            } else if words.iter().any(|w| OTHER_SIDES.contains(w)) {
                return None;
            } else {
                2
            };
            Some((tier, std::cmp::Reverse(*size), i))
        })
        .min()
        .map(|(_, _, i)| i)
}

/// Decode `bytes` (JPEG, PNG, …), shrink to fit [`MAX_SIDE`] (never enlarge) and encode as JPEG.
///
/// # Errors
/// Undecodable image data.
pub fn downscale(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("not an image: {e}"))?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| format!("encode: {e}"))?;
    Ok(out)
}

/// A `w`×`h` orange PNG (tests).
#[doc(hidden)]
#[must_use]
pub fn test_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([251, 146, 60]));
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// A `size`² two-tone gradient PNG picked by `seed` (demo covers; no real artwork).
#[doc(hidden)]
#[must_use]
pub fn demo_png(seed: u64, size: u32) -> Vec<u8> {
    const PALETTE: [[u8; 3]; 6] = [
        [30, 58, 95],
        [71, 85, 105],
        [154, 52, 18],
        [21, 94, 117],
        [88, 28, 135],
        [20, 83, 45],
    ];
    let a = PALETTE[(seed % 6) as usize];
    let b = PALETTE[((seed / 6 + 3) % 6) as usize];
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let t = (x + y) as f32 / (2 * size.max(1)) as f32;
        let mix = |i: usize| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t) as u8;
        image::Rgb([mix(0), mix(1), mix(2)])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// Called after any cover changes state (the app passes its window waker).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

struct Entry {
    path: PathBuf,
    bytes: u64,
    used: SystemTime,
    embedded: bool,
}

#[derive(Default)]
struct Index {
    ready: HashMap<String, Entry>,
    /// key → until when it counts as missing.
    missing: HashMap<String, SystemTime>,
    pending: HashSet<String>,
    /// Keys whose file mtime was refreshed this run (LRU across restarts).
    touched: HashSet<String>,
}

struct Inner {
    dir: PathBuf,
    cap: u64,
    index: Mutex<Index>,
    notify: Mutex<Option<Notify>>,
    generation: std::sync::atomic::AtomicU64,
}

/// The cover cache. Cheap to clone; all clones share one index.
#[derive(Clone)]
pub struct Covers(Arc<Inner>);

impl std::fmt::Debug for Covers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Covers").field("dir", &self.0.dir).finish()
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Covers {
    /// The cache in `dir` (created on first write), capped at [`DEFAULT_CAP_BYTES`].
    #[must_use]
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self::open_with_cap(dir, DEFAULT_CAP_BYTES)
    }

    /// Reads what is in `dir`: covers, unexpired misses. Leftover partial fetches are deleted.
    #[must_use]
    pub fn open_with_cap(dir: impl Into<PathBuf>, cap: u64) -> Self {
        let dir = dir.into();
        let mut index = Index::default();
        let now = SystemTime::now();
        let _ = std::fs::remove_dir_all(dir.join(TMP_DIR_NAME));
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            let Ok(meta) = e.metadata() else { continue };
            if let Some(key) = name.strip_suffix(".miss") {
                let until = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map(|s| UNIX_EPOCH + Duration::from_secs(s));
                match until {
                    Some(u) if u > now => {
                        index.missing.insert(key.to_string(), u);
                    }
                    _ => {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                continue;
            }
            let (key, embedded) = if let Some(k) = name.strip_suffix("-e.jpg") {
                (k, true)
            } else if let Some(k) = name.strip_suffix("-f.jpg") {
                (k, false)
            } else {
                continue;
            };
            // An embedded cover beats the folder's image.
            if index.ready.get(key).is_some_and(|x| x.embedded) {
                continue;
            }
            index.ready.insert(
                key.to_string(),
                Entry {
                    path: path.clone(),
                    bytes: meta.len(),
                    used: meta.modified().unwrap_or(UNIX_EPOCH),
                    embedded,
                },
            );
        }
        Self(Arc::new(Inner {
            dir,
            cap,
            index: Mutex::new(index),
            notify: Mutex::new(None),
            generation: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.0.dir
    }

    /// Called (from any thread) whenever a cover becomes ready, missing or stops pending.
    pub fn set_notify(&self, notify: Notify) {
        if let Ok(mut n) = self.0.notify.lock() {
            *n = Some(notify);
        }
    }

    /// Bumped on every change of any cover's state; compare to notice changes.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.0.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn notify(&self) {
        self.0
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let n = self.0.notify.lock().ok().and_then(|n| n.clone());
        if let Some(n) = n {
            n();
        }
    }

    fn index(&self) -> std::sync::MutexGuard<'_, Index> {
        self.0
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The cached cover of (user, folder), if any. Memory only: no disk, no network. Safe to
    /// call while rendering lists.
    #[must_use]
    pub fn cached(&self, username: &str, folder: &str) -> Option<PathBuf> {
        let key = cover_key(username, folder);
        let mut ix = self.index();
        let path = ix.ready.get(&key)?.path.clone();
        if let Some(e) = ix.ready.get_mut(&key) {
            e.used = SystemTime::now();
        }
        Some(path)
    }

    /// [`Covers::cached`] plus pending and missing.
    #[must_use]
    pub fn state(&self, username: &str, folder: &str) -> CoverState {
        self.state_of(&cover_key(username, folder))
    }

    pub(crate) fn state_of(&self, key: &str) -> CoverState {
        let mut ix = self.index();
        if let Some(e) = ix.ready.get_mut(key) {
            e.used = SystemTime::now();
            return CoverState::Ready(e.path.clone());
        }
        if ix.pending.contains(key) {
            return CoverState::Pending;
        }
        match ix.missing.get(key) {
            Some(until) if *until > SystemTime::now() => CoverState::Missing,
            Some(_) => {
                ix.missing.remove(key);
                CoverState::Unknown
            }
            None => CoverState::Unknown,
        }
    }

    /// Mark `key` pending if nothing is known about it. Returns whether it was marked.
    pub(crate) fn begin(&self, key: &str) -> bool {
        if self.state_of(key) != CoverState::Unknown {
            return false;
        }
        self.index().pending.insert(key.to_string());
        self.notify();
        true
    }

    /// Mark (user, folder) as being fetched if nothing is known about it (tests, previews).
    #[doc(hidden)]
    pub fn mark_pending(&self, username: &str, folder: &str) -> bool {
        self.begin(&cover_key(username, folder))
    }

    /// A pending fetch was dropped without an answer (session ended): back to unknown.
    pub(crate) fn abandon(&self, key: &str) {
        self.index().pending.remove(key);
        self.notify();
    }

    /// Remember that (key) has no cover for `ttl`.
    pub(crate) fn miss(&self, key: &str, ttl: Duration) {
        let until = SystemTime::now() + ttl;
        {
            let mut ix = self.index();
            ix.pending.remove(key);
            if ix.ready.contains_key(key) {
                drop(ix);
                self.notify();
                return;
            }
            ix.missing.insert(key.to_string(), until);
        }
        if std::fs::create_dir_all(&self.0.dir).is_ok() {
            let _ = std::fs::write(
                self.0.dir.join(format!("{key}.miss")),
                unix(until).to_string(),
            );
        }
        self.notify();
    }

    /// Art extracted from an audio file of (user, folder): downscaled and stored, replacing the
    /// folder's image if one was fetched. Blocking (decode + write): call off the UI thread.
    ///
    /// # Errors
    /// Undecodable image or I/O error.
    pub fn store_embedded(
        &self,
        username: &str,
        folder: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, String> {
        if let Some(e) = self.index().ready.get(&cover_key(username, folder))
            && e.embedded
        {
            return Ok(e.path.clone());
        }
        self.store(&cover_key(username, folder), bytes, true)
    }

    /// Store a fetched folder image under `key` (kept out if embedded art is already there).
    pub(crate) fn store_folder(&self, key: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        if let Some(e) = self.index().ready.get(key)
            && e.embedded
        {
            return Ok(e.path.clone());
        }
        self.store(key, bytes, false)
    }

    fn store(&self, key: &str, bytes: &[u8], embedded: bool) -> Result<PathBuf, String> {
        let result = (|| {
            let jpeg = downscale(bytes)?;
            std::fs::create_dir_all(&self.0.dir).map_err(|e| e.to_string())?;
            let path = self
                .0
                .dir
                .join(format!("{key}-{}.jpg", if embedded { "e" } else { "f" }));
            let tmp = path.with_extension("jpg.tmp");
            std::fs::write(&tmp, &jpeg).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
            Ok((path, jpeg.len() as u64))
        })();
        let out = match result {
            Ok((path, len)) => {
                let mut ix = self.index();
                ix.pending.remove(key);
                ix.missing.remove(key);
                let _ = std::fs::remove_file(self.0.dir.join(format!("{key}.miss")));
                if let Some(old) = ix.ready.insert(
                    key.to_string(),
                    Entry {
                        path: path.clone(),
                        bytes: len,
                        used: SystemTime::now(),
                        embedded,
                    },
                ) && old.path != path
                {
                    let _ = std::fs::remove_file(&old.path);
                }
                ix.touched.insert(key.to_string());
                self.evict(&mut ix, key);
                Ok(path)
            }
            Err(e) => {
                self.index().pending.remove(key);
                Err(e)
            }
        };
        self.notify();
        out
    }

    /// Drop least recently used covers (never `keep`) until under the cap.
    fn evict(&self, ix: &mut Index, keep: &str) {
        let mut total: u64 = ix.ready.values().map(|e| e.bytes).sum();
        if total <= self.0.cap {
            return;
        }
        let mut order: Vec<(SystemTime, String)> = ix
            .ready
            .iter()
            .filter(|(k, _)| k.as_str() != keep)
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        order.sort();
        for (_, k) in order {
            if total <= self.0.cap {
                break;
            }
            if let Some(e) = ix.ready.remove(&k) {
                let _ = std::fs::remove_file(&e.path);
                total = total.saturating_sub(e.bytes);
            }
        }
    }

    /// Persist recent use as file mtimes (once per cover per run) so LRU order survives a
    /// restart. Cheap; the worker calls it now and then.
    pub(crate) fn flush_use(&self) {
        let mut ix = self.index();
        let Index { ready, touched, .. } = &mut *ix;
        for (k, e) in ready.iter() {
            if e.used.elapsed().is_ok_and(|d| d < Duration::from_secs(600))
                && touched.insert(k.clone())
                && let Ok(f) = std::fs::File::options().append(true).open(&e.path)
            {
                let _ = f.set_modified(e.used);
            }
        }
    }

    /// Total bytes of cached covers.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.index().ready.values().map(|e| e.bytes).sum()
    }

    pub(crate) fn tmp_dir(&self, key: &str) -> PathBuf {
        self.0.dir.join(TMP_DIR_NAME).join(key)
    }
}

// ---------------------------------------------------------------------- worker-side fetching

/// The library calls a cover fetch needs; implemented by every [`crate::net::Session`].
pub(crate) trait CoverNet {
    fn request_folder(&self, username: &str, folder: &str) -> Result<(), String>;
    fn download(
        &self,
        username: &str,
        path: &str,
        size: u64,
        dir: &str,
    ) -> Result<Receiver<DownloadStatus>, String>;
    fn cancel_download(&self, username: &str, path: &str) -> bool;
    fn remove_download(&self, username: &str, path: &str);
}

impl<T: crate::net::Session + ?Sized> CoverNet for T {
    fn request_folder(&self, username: &str, folder: &str) -> Result<(), String> {
        crate::net::Session::request_folder(self, username, folder)
    }
    fn download(
        &self,
        username: &str,
        path: &str,
        size: u64,
        dir: &str,
    ) -> Result<Receiver<DownloadStatus>, String> {
        crate::net::Session::download(self, username, path, size, dir)
    }
    fn cancel_download(&self, username: &str, path: &str) -> bool {
        crate::net::Session::cancel_download(self, username, path)
    }
    fn remove_download(&self, username: &str, path: &str) {
        crate::net::Session::remove_download(self, username, path);
    }
}

/// Fetch deadlines (shortened in tests).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CoverTimeouts {
    /// Wait for a single-folder listing.
    pub listing: Duration,
    /// Wait for the first byte (a peer that queues us is not waited for).
    pub first_byte: Duration,
    /// Whole download.
    pub total: Duration,
}

impl Default for CoverTimeouts {
    fn default() -> Self {
        Self {
            listing: Duration::from_secs(30),
            first_byte: Duration::from_secs(20),
            total: Duration::from_secs(90),
        }
    }
}

/// Whether the worker is already transferring (user, remote path) — e.g. a folder download
/// that includes the cover image. The library allows one transfer per (user, path).
pub(crate) type Busy<'a> = &'a dyn Fn(&str, &str) -> bool;

/// Queued requests kept per peer; older ones are dropped (the user moved on).
const QUEUE_PER_PEER: usize = 4;

struct Job {
    store: Covers,
    key: String,
    username: String,
    folder: String,
    listing: Option<Vec<(String, u64)>>,
}

enum Stage {
    Listing {
        deadline: Instant,
    },
    Downloading {
        rx: Receiver<DownloadStatus>,
        remote: String,
        file: PathBuf,
        started: Instant,
        got_bytes: bool,
    },
}

struct Active {
    job: Job,
    stage: Stage,
}

/// Owned by the worker; polled every tick with the current session.
#[derive(Default)]
pub(crate) struct Fetcher {
    queue: VecDeque<Job>,
    /// By username: one fetch per peer.
    active: HashMap<String, Active>,
    timeouts: CoverTimeouts,
    last_flush: Option<Instant>,
}

/// The files of `folder` in a listing: the matching directory, else everything (peers may echo
/// another spelling, see G14).
fn files_of(username: &str, folder: &str, dirs: &[crate::model::BrowseDir]) -> Vec<(String, u64)> {
    let norm = |s: &str| {
        s.trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    let want = norm(folder);
    let pick: Vec<&crate::model::BrowseDir> = match dirs.iter().find(|d| norm(&d.path) == want) {
        Some(d) => vec![d],
        None => dirs.iter().collect(),
    };
    pick.iter()
        .flat_map(|d| d.files.iter())
        .filter(|f| f.username == username || f.username.is_empty())
        .map(|f| (f.path.clone(), f.size))
        .collect()
}

impl Fetcher {
    #[cfg(test)]
    pub(crate) fn with_timeouts(timeouts: CoverTimeouts) -> Self {
        Self {
            timeouts,
            ..Self::default()
        }
    }

    /// Queue a fetch unless the cover is known, pending or recently missing. `listing`: the
    /// folder's files (full remote paths or basenames, with sizes) if the caller has them;
    /// `browse` is the worker's cached browse of the user, if any.
    pub(crate) fn request(
        &mut self,
        store: Covers,
        username: String,
        folder: String,
        listing: Option<Vec<(String, u64)>>,
        browse: Option<&[crate::model::BrowseDir]>,
    ) {
        let folder = folder.trim_end_matches(['\\', '/']).to_string();
        let key = cover_key(&username, &folder);
        if self.active.values().any(|a| a.job.key == key) || self.queue.iter().any(|j| j.key == key)
        {
            return;
        }
        if !store.begin(&key) {
            return;
        }
        let listing = listing.or_else(|| browse.map(|d| files_of(&username, &folder, d)));
        let mine = self.queue.iter().filter(|j| j.username == username).count();
        if mine >= QUEUE_PER_PEER
            && let Some(i) = self.queue.iter().position(|j| j.username == username)
            && let Some(old) = self.queue.remove(i)
        {
            old.store.abandon(&old.key);
        }
        self.queue.push_back(Job {
            store,
            key,
            username,
            folder,
            listing,
        });
    }

    /// Requests waiting or running.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.queue.len() + self.active.len()
    }

    /// The session ended: forget everything in flight (not cached as missing).
    pub(crate) fn session_ended(&mut self) {
        for j in self.queue.drain(..) {
            j.store.abandon(&j.key);
        }
        for (_, a) in self.active.drain() {
            let _ = std::fs::remove_dir_all(a.job.store.tmp_dir(&a.job.key));
            a.job.store.abandon(&a.job.key);
        }
    }

    pub(crate) fn poll<N: CoverNet + ?Sized>(&mut self, net: Option<&N>, busy: Busy) {
        let Some(net) = net else {
            if !self.queue.is_empty() || !self.active.is_empty() {
                self.session_ended();
            }
            return;
        };
        let users: Vec<String> = self.active.keys().cloned().collect();
        for u in users {
            self.advance(net, &u);
        }
        // Start the oldest request of every idle peer.
        // One pass over a snapshot: a job deferred during it (its image is busy) is pushed back
        // onto `self.queue` and waits for the next poll — never retried in this loop.
        let mut started: HashSet<String> = HashSet::new();
        for job in std::mem::take(&mut self.queue) {
            if self.active.contains_key(&job.username) || started.contains(&job.username) {
                self.queue.push_back(job);
                continue;
            }
            started.insert(job.username.clone());
            self.start(net, job, busy);
        }
        if self
            .last_flush
            .is_none_or(|t| t.elapsed() > Duration::from_secs(60))
        {
            self.last_flush = Some(Instant::now());
            if let Some(a) = self.active.values().next() {
                a.job.store.flush_use();
            }
        }
    }

    /// Folders whose contents a fetch is waiting for. The worker takes each reply once and
    /// hands it to every waiter ([`Fetcher::offer_folder`] and folder downloads).
    pub(crate) fn listing_waits(&self) -> Vec<(String, String)> {
        self.active
            .values()
            .filter(|a| matches!(a.stage, Stage::Listing { .. }))
            .map(|a| (a.job.username.clone(), a.job.folder.clone()))
            .collect()
    }

    /// A folder-contents reply arrived: the fetch waiting for it picks its image.
    pub(crate) fn offer_folder<N: CoverNet + ?Sized>(
        &mut self,
        net: &N,
        username: &str,
        folder: &str,
        dirs: &[crate::model::BrowseDir],
        busy: Busy,
    ) {
        let waiting = self.active.get(username).is_some_and(|a| {
            matches!(a.stage, Stage::Listing { .. })
                && cover_key(username, &a.job.folder) == cover_key(username, folder)
        });
        if !waiting {
            return;
        }
        if let Some(mut a) = self.active.remove(username) {
            a.job.listing = Some(files_of(username, &a.job.folder, dirs));
            self.choose(net, a.job, busy);
        }
    }

    fn start<N: CoverNet + ?Sized>(&mut self, net: &N, job: Job, busy: Busy) {
        if job.listing.is_some() {
            self.choose(net, job, busy);
            return;
        }
        if let Err(e) = net.request_folder(&job.username, &job.folder) {
            log::fail(&job, &e);
            job.store.miss(&job.key, FAIL_TTL);
            return;
        }
        let username = job.username.clone();
        self.active.insert(
            username,
            Active {
                job,
                stage: Stage::Listing {
                    deadline: Instant::now() + self.timeouts.listing,
                },
            },
        );
    }

    /// Pick the image from the listing and start its download, or remember the miss.
    fn choose<N: CoverNet + ?Sized>(&mut self, net: &N, job: Job, busy: Busy) {
        let files = job.listing.clone().unwrap_or_default();
        let Some(i) = pick_cover(&files) else {
            job.store.miss(&job.key, MISS_TTL);
            return;
        };
        let (name, size) = &files[i];
        let remote = if name.contains('\\') {
            name.clone()
        } else {
            format!("{}\\{name}", job.folder)
        };
        if busy(&job.username, &remote) {
            // Being downloaded with its folder: try again once that transfer is over.
            self.queue.push_back(job);
            return;
        }
        let dir = job.store.tmp_dir(&job.key);
        let _ = std::fs::remove_dir_all(&dir);
        let file = paths::final_path(&dir, &remote);
        match net.download(&job.username, &remote, *size, &dir.to_string_lossy()) {
            Ok(rx) => {
                let username = job.username.clone();
                self.active.insert(
                    username,
                    Active {
                        job,
                        stage: Stage::Downloading {
                            rx,
                            remote,
                            file,
                            started: Instant::now(),
                            got_bytes: false,
                        },
                    },
                );
            }
            Err(e) => {
                log::fail(&job, &e);
                job.store.miss(&job.key, FAIL_TTL);
            }
        }
    }

    fn advance<N: CoverNet + ?Sized>(&mut self, net: &N, username: &str) {
        let Some(mut a) = self.active.remove(username) else {
            return;
        };
        match &mut a.stage {
            Stage::Listing { deadline } => {
                if Instant::now() >= *deadline {
                    log::fail(&a.job, "no folder listing");
                    a.job.store.miss(&a.job.key, FAIL_TTL);
                } else {
                    self.active.insert(username.to_string(), a);
                }
            }
            Stage::Downloading {
                rx,
                remote,
                file,
                started,
                got_bytes,
            } => {
                let mut outcome: Option<Result<(), String>> = None;
                while let Ok(s) = rx.try_recv() {
                    match s {
                        DownloadStatus::InProgress {
                            bytes_downloaded, ..
                        } if bytes_downloaded > 0 => *got_bytes = true,
                        DownloadStatus::Completed => outcome = Some(Ok(())),
                        DownloadStatus::Failed(r) => {
                            outcome = Some(Err(r.unwrap_or_else(|| "failed".into())));
                        }
                        DownloadStatus::Cancelled => outcome = Some(Err("cancelled".into())),
                        DownloadStatus::TimedOut => outcome = Some(Err("timed out".into())),
                        _ => {}
                    }
                }
                let waited = started.elapsed();
                if outcome.is_none() {
                    if !*got_bytes && waited >= self.timeouts.first_byte {
                        outcome = Some(Err("no data (queued?)".into()));
                    } else if waited >= self.timeouts.total {
                        outcome = Some(Err("too slow".into()));
                    }
                    if outcome.is_some() {
                        net.cancel_download(&a.job.username, remote);
                    }
                }
                let Some(outcome) = outcome else {
                    self.active.insert(username.to_string(), a);
                    return;
                };
                net.remove_download(&a.job.username, remote);
                let (job, file) = (a.job, file.clone());
                match outcome {
                    // Decode + downscale off the worker thread.
                    Ok(()) => {
                        let spawned = std::thread::Builder::new()
                            .name("wholeseek-cover".into())
                            .spawn({
                                let (store, key) = (job.store.clone(), job.key.clone());
                                move || finish(&store, &key, &file)
                            });
                        if spawned.is_err() {
                            job.store.miss(&job.key, FAIL_TTL);
                        }
                    }
                    Err(e) => {
                        log::fail(&job, &e);
                        let _ = std::fs::remove_dir_all(job.store.tmp_dir(&job.key));
                        job.store.miss(&job.key, FAIL_TTL);
                    }
                }
            }
        }
    }
}

/// The downloaded image → cache; the temporary copy is deleted either way.
fn finish(store: &Covers, key: &str, file: &Path) {
    // The temporary copy goes before the cover is published, so "ready" implies it is gone.
    let bytes = std::fs::read(file).map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(store.tmp_dir(key));
    let stored = bytes.and_then(|b| store.store_folder(key, &b));
    if stored.is_err() {
        store.miss(key, MISS_TTL);
    }
}

mod log {
    pub(super) fn fail(job: &super::Job, why: &str) {
        eprintln!(
            "wholeseek: cover of {}'s folder {} not fetched: {why}",
            job.username, job.folder
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Sender};

    fn f(name: &str, size: u64) -> (String, u64) {
        (name.to_string(), size)
    }

    #[test]
    fn picks_named_cover_then_largest() {
        let files = vec![
            f("01 intro.flac", 30_000_000),
            f("scan.jpg", 900_000),
            f("back.jpg", 2_000_000),
            f("Folder.jpg", 40_000),
            f("cover.png", 300_000),
        ];
        assert_eq!(pick_cover(&files), Some(4), "cover.png: tier 0, largest");
        let files = vec![f("albumart_small.jpg", 5_000), f("scan.jpg", 900_000)];
        assert_eq!(pick_cover(&files), Some(0), "album* before others");
        let files = vec![f("back.jpg", 2_000_000), f("Artist - Album.JPG", 100)];
        assert_eq!(pick_cover(&files), Some(1), "back is never picked");
        let files = vec![f(r"Music\A\Front.jpeg", 10), f("cd.jpg", 10)];
        assert_eq!(pick_cover(&files), Some(0), "full paths work");
    }

    #[test]
    fn pick_skips_huge_empty_and_non_images() {
        assert_eq!(pick_cover(&[f("cover.jpg", 6 * 1024 * 1024)]), None);
        assert_eq!(pick_cover(&[f("cover.jpg", 0)]), None);
        assert_eq!(pick_cover(&[f("cover.gif", 10), f("cover.txt", 10)]), None);
        assert_eq!(pick_cover(&[]), None);
        assert_eq!(
            pick_cover(&[
                f("cover.jpg", 6 * 1024 * 1024),
                f("folder.jpg", 5 * 1024 * 1024)
            ]),
            Some(1)
        );
    }

    #[test]
    fn key_ignores_separator_style_and_case() {
        assert_eq!(
            cover_key("u", r"Music\Album\"),
            cover_key("u", "music/album")
        );
        assert_ne!(
            cover_key("u", r"Music\Album"),
            cover_key("v", r"Music\Album")
        );
        assert_eq!(cover_key("u", "x").len(), 16);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([251, 146, 60]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn downscale_fits_512_and_never_enlarges() {
        let big = image::load_from_memory(&downscale(&png(1000, 500)).unwrap()).unwrap();
        assert_eq!((big.width(), big.height()), (512, 256));
        let small = image::load_from_memory(&downscale(&png(40, 30)).unwrap()).unwrap();
        assert_eq!((small.width(), small.height()), (40, 30));
        assert!(downscale(b"not an image").is_err());
    }

    #[test]
    fn embedded_wins_and_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Covers::open(tmp.path());
        let key = cover_key("u", "A");
        assert_eq!(c.state("u", "A"), CoverState::Unknown);
        assert!(c.begin(&key));
        assert_eq!(c.state("u", "A"), CoverState::Pending);
        let folder = c.store_folder(&key, &png(600, 600)).unwrap();
        assert_eq!(c.cached("u", "A"), Some(folder.clone()));
        let emb = c.store_embedded("u", "A", &png(10, 10)).unwrap();
        assert_ne!(emb, folder);
        assert!(!folder.exists(), "folder image replaced");
        // A later folder fetch does not override embedded art.
        assert_eq!(c.store_folder(&key, &png(20, 20)).unwrap(), emb);
        let again = Covers::open(tmp.path());
        assert_eq!(again.cached("u", "A"), Some(emb));
    }

    #[test]
    fn lru_cap_drops_least_recently_used() {
        let tmp = tempfile::tempdir().unwrap();
        let probe = Covers::open(tmp.path().join("probe"));
        probe.store_embedded("p", "p", &png(300, 300)).unwrap();
        let one = probe.used_bytes();
        // Room for two covers.
        let c = Covers::open_with_cap(tmp.path().join("c"), one * 2 + one / 2);
        c.store_embedded("u", "a", &png(300, 300)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        c.store_embedded("u", "b", &png(300, 300)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(c.cached("u", "a").is_some()); // a used after b
        std::thread::sleep(Duration::from_millis(20));
        c.store_embedded("u", "c", &png(300, 300)).unwrap();
        assert!(c.cached("u", "a").is_some());
        assert!(c.cached("u", "b").is_none(), "b was least recently used");
        assert!(c.cached("u", "c").is_some());
        assert!(c.used_bytes() <= one * 2 + one / 2);
        let files = std::fs::read_dir(tmp.path().join("c")).unwrap().count();
        assert_eq!(files, 2);
    }

    #[test]
    fn negative_cache_persists_and_expires() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Covers::open(tmp.path());
        let k = cover_key("u", "A");
        c.miss(&k, MISS_TTL);
        assert_eq!(c.state("u", "A"), CoverState::Missing);
        assert!(!c.begin(&k), "a miss is not asked again");
        assert_eq!(
            Covers::open(tmp.path()).state("u", "A"),
            CoverState::Missing
        );
        // Expired.
        let k2 = cover_key("u", "B");
        c.miss(&k2, Duration::ZERO);
        assert_eq!(c.state("u", "B"), CoverState::Unknown);
        assert_eq!(
            Covers::open(tmp.path()).state("u", "B"),
            CoverState::Unknown
        );
        assert!(!tmp.path().join(format!("{k2}.miss")).exists());
        // Embedded art found later replaces a miss.
        c.store_embedded("u", "A", &png(4, 4)).unwrap();
        assert!(matches!(c.state("u", "A"), CoverState::Ready(_)));
    }

    type Started = (String, String, String, Sender<DownloadStatus>);

    /// Records library calls; downloads answer through senders the test drives.
    #[derive(Default)]
    struct FakeNet {
        folder_requests: Mutex<Vec<(String, String)>>,
        downloads: Mutex<Vec<Started>>,
        cancelled: Mutex<Vec<String>>,
        removed: Mutex<Vec<String>>,
    }

    impl CoverNet for FakeNet {
        fn request_folder(&self, u: &str, f: &str) -> Result<(), String> {
            self.folder_requests
                .lock()
                .unwrap()
                .push((u.into(), f.into()));
            Ok(())
        }
        fn download(
            &self,
            u: &str,
            p: &str,
            _size: u64,
            dir: &str,
        ) -> Result<Receiver<DownloadStatus>, String> {
            let (tx, rx) = mpsc::channel();
            self.downloads
                .lock()
                .unwrap()
                .push((u.into(), p.into(), dir.into(), tx));
            Ok(rx)
        }
        fn cancel_download(&self, _u: &str, p: &str) -> bool {
            self.cancelled.lock().unwrap().push(p.into());
            true
        }
        fn remove_download(&self, _u: &str, p: &str) {
            self.removed.lock().unwrap().push(p.into());
        }
    }

    fn idle(_: &str, _: &str) -> bool {
        false
    }

    #[test]
    fn image_in_a_running_folder_download_waits_for_it() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path());
        let net = FakeNet::default();
        let mut fx = Fetcher::default();
        fx.request(
            store.clone(),
            "a".into(),
            "F".into(),
            listing(&[("cover.jpg", 5)]),
            None,
        );
        let downloading = std::cell::Cell::new(true);
        let busy = |_: &str, p: &str| downloading.get() && p == r"F\cover.jpg";
        fx.poll(Some(&net), &busy);
        fx.poll(Some(&net), &busy);
        assert!(net.downloads.lock().unwrap().is_empty());
        assert_eq!(store.state("a", "F"), CoverState::Pending);
        downloading.set(false);
        fx.poll(Some(&net), &busy);
        assert_eq!(net.downloads.lock().unwrap().len(), 1);
    }

    fn listing(names: &[(&str, u64)]) -> Option<Vec<(String, u64)>> {
        Some(names.iter().map(|(n, s)| ((*n).to_string(), *s)).collect())
    }

    /// Block until (u, folder) is no longer pending, woken by the store's notify (no polling).
    fn wait_ready(c: &Covers, u: &str, folder: &str, changed: &Receiver<()>) -> CoverState {
        loop {
            let s = c.state(u, folder);
            if s != CoverState::Pending {
                return s;
            }
            changed
                .recv_timeout(Duration::from_secs(10))
                .expect("cover never finished");
        }
    }

    #[test]
    fn one_fetch_per_peer_and_full_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path().join("covers"));
        let (ping, changed) = mpsc::channel();
        let ping = Mutex::new(ping);
        store.set_notify(Arc::new(move || {
            let _ = ping.lock().unwrap().send(());
        }));
        let net = FakeNet::default();
        let mut fx = Fetcher::default();
        for album in ["A1", "A2", "A3"] {
            fx.request(
                store.clone(),
                "alice".into(),
                format!(r"Music\{album}"),
                listing(&[("01.flac", 9), ("cover.jpg", 100)]),
                None,
            );
        }
        fx.request(store.clone(), "bob".into(), r"Mp3\B".into(), None, None);
        // Duplicate request: ignored.
        fx.request(
            store.clone(),
            "alice".into(),
            r"Music\A1\".into(),
            None,
            None,
        );
        assert_eq!(fx.len(), 4);
        fx.poll(Some(&net), &idle);
        {
            let d = net.downloads.lock().unwrap();
            assert_eq!(d.len(), 1, "one alice download at a time");
            assert_eq!(d[0].1, r"Music\A1\cover.jpg");
            assert!(
                Path::new(&d[0].2).starts_with(store.dir()),
                "into the cover dir"
            );
        }
        assert_eq!(
            *net.folder_requests.lock().unwrap(),
            vec![("bob".to_string(), r"Mp3\B".to_string())]
        );
        fx.poll(Some(&net), &idle);
        assert_eq!(net.downloads.lock().unwrap().len(), 1, "still one");
        // alice's first finishes: the library wrote the file into the temp dir.
        let (tx, dir) = {
            let d = net.downloads.lock().unwrap();
            (d[0].3.clone(), PathBuf::from(&d[0].2))
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cover.jpg"), png(800, 800)).unwrap();
        tx.send(DownloadStatus::InProgress {
            bytes_downloaded: 10,
            total_bytes: 100,
            speed_bytes_per_sec: 1.0,
        })
        .unwrap();
        tx.send(DownloadStatus::Completed).unwrap();
        fx.poll(Some(&net), &idle);
        assert!(matches!(
            wait_ready(&store, "alice", r"Music\A1", &changed),
            CoverState::Ready(_)
        ));
        assert!(!dir.exists(), "temp copy removed");
        assert_eq!(*net.removed.lock().unwrap(), vec![r"Music\A1\cover.jpg"]);
        let d = net.downloads.lock().unwrap();
        assert_eq!(d.len(), 2, "next alice fetch starts after the first");
        assert_eq!(d[1].1, r"Music\A2\cover.jpg");
    }

    #[test]
    fn folder_listing_without_image_is_a_day_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path());
        let net = FakeNet::default();
        let mut fx = Fetcher::default();
        fx.request(store.clone(), "bob".into(), r"M\B".into(), None, None);
        fx.poll(Some(&net), &idle);
        assert_eq!(fx.listing_waits(), vec![("bob".into(), r"M\B".into())]);
        let empty = crate::model::BrowseDir {
            path: r"M\B".into(),
            files: vec![],
        };
        fx.offer_folder(&net, "bob", r"m\b\", &[empty], &idle);
        assert_eq!(store.state("bob", r"M\B"), CoverState::Missing);
        assert_eq!(fx.len(), 0);
        assert!(net.downloads.lock().unwrap().is_empty());
        // Asking again does nothing for a day.
        fx.request(store.clone(), "bob".into(), r"M\B".into(), None, None);
        assert_eq!(fx.len(), 0);
    }

    #[test]
    fn queued_peer_is_cancelled_after_first_byte_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path());
        let net = FakeNet::default();
        let mut fx = Fetcher::with_timeouts(CoverTimeouts {
            listing: Duration::from_millis(1),
            first_byte: Duration::from_millis(1),
            total: Duration::from_secs(9),
        });
        fx.request(
            store.clone(),
            "q".into(),
            "F".into(),
            listing(&[("folder.jpg", 5)]),
            None,
        );
        fx.request(store.clone(), "slow".into(), "G".into(), None, None);
        fx.poll(Some(&net), &idle);
        std::thread::sleep(Duration::from_millis(5));
        fx.poll(Some(&net), &idle);
        assert_eq!(*net.cancelled.lock().unwrap(), vec![r"F\folder.jpg"]);
        assert_eq!(store.state("q", "F"), CoverState::Missing);
        assert_eq!(
            store.state("slow", "G"),
            CoverState::Missing,
            "listing timeout"
        );
        assert_eq!(fx.len(), 0);
    }

    #[test]
    fn session_end_forgets_without_negative_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path());
        let net = FakeNet::default();
        let mut fx = Fetcher::default();
        fx.request(store.clone(), "a".into(), "F".into(), None, None);
        fx.request(store.clone(), "a".into(), "G".into(), None, None);
        fx.poll(Some(&net), &idle);
        fx.poll::<FakeNet>(None, &idle);
        assert_eq!(fx.len(), 0);
        assert_eq!(store.state("a", "F"), CoverState::Unknown);
        assert_eq!(store.state("a", "G"), CoverState::Unknown);
    }

    #[test]
    fn browse_listing_skips_folder_request() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Covers::open(tmp.path());
        let net = FakeNet::default();
        let mut fx = Fetcher::default();
        let file = |p: &str| crate::model::RemoteFile {
            username: "a".into(),
            path: p.into(),
            size: 50,
            attrs: crate::model::AudioAttrs::default(),
        };
        let browse = vec![
            crate::model::BrowseDir {
                path: r"M\X".into(),
                files: vec![file(r"M\X\front.jpg")],
            },
            crate::model::BrowseDir {
                path: r"M\Y".into(),
                files: vec![file(r"M\Y\cover.jpg")],
            },
        ];
        fx.request(store, "a".into(), r"M\Y".into(), None, Some(&browse));
        fx.poll(Some(&net), &idle);
        assert!(net.folder_requests.lock().unwrap().is_empty());
        assert_eq!(net.downloads.lock().unwrap()[0].1, r"M\Y\cover.jpg");
    }
}
