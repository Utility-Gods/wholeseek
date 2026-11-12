//! [`GrowingFile`]: a `Read + Seek` view of a file that is still being downloaded.
//!
//! The downloader (soulseek-rs-lib) appends to `<name>.part` and renames it to `<name>` when the
//! transfer completes. Reads block (polling the file length, woken early by
//! cancel/interrupt/notify) until the requested bytes exist or the download is complete. The open
//! file descriptor survives the `.part` → final rename on Linux; if the final file turns out to be
//! a different inode (copy-style finalisation) it is reopened.
//!
//! Size handling: the total size is normally known up front. It may also be unknown
//! ([`GrowingFile::new_unknown_size`]) or wrong. Once the final file exists and the `.part` is
//! gone, the final file's length is the truth (a mismatch is recorded in
//! [`GrowingHandle::notice`]). A `.part` growing beyond the announced size switches to
//! unknown-size mode (completion = rename).

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;

/// Suffix soulseek-rs-lib appends while a transfer is in progress.
pub const PART_SUFFIX: &str = ".part";

/// Sentinel for "total size unknown" in [`GrowingHandle::total_size`].
pub const UNKNOWN_SIZE: u64 = u64::MAX;

/// Errors surfaced (wrapped in `io::Error`) by [`GrowingFile`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrowingError {
    /// [`GrowingHandle::cancel`] was called.
    Cancelled,
    /// [`GrowingHandle::interrupt`] was called while a read was waiting for bytes.
    Interrupted,
    /// Neither `<name>` nor `<name>.part` exists (never appeared, or the download was discarded).
    Missing(PathBuf),
}

impl fmt::Display for GrowingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrowingError::Cancelled => write!(f, "playback cancelled"),
            GrowingError::Interrupted => write!(f, "read interrupted"),
            GrowingError::Missing(p) => write!(f, "download file missing: {}", p.display()),
        }
    }
}

impl std::error::Error for GrowingError {}

impl GrowingError {
    fn into_io(self) -> io::Error {
        io::Error::other(self)
    }

    /// Extract a `GrowingError` from an `io::Error` produced by a [`GrowingFile`].
    pub fn from_io(err: &io::Error) -> Option<&GrowingError> {
        err.get_ref().and_then(|e| e.downcast_ref::<GrowingError>())
    }
}

/// The download was reported failed via [`GrowingHandle::fail`]: reads that would have to wait
/// for more bytes return this (wrapped in `io::Error`) instead. Already-downloaded bytes stay
/// readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFailed(pub String);

impl fmt::Display for SourceFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "download failed: {}", self.0)
    }
}

impl std::error::Error for SourceFailed {}

impl SourceFailed {
    /// Extract a `SourceFailed` from an `io::Error` produced by a [`GrowingFile`].
    pub fn from_io(err: &io::Error) -> Option<&SourceFailed> {
        err.get_ref().and_then(|e| e.downcast_ref::<SourceFailed>())
    }
}

#[derive(Debug)]
struct Shared {
    total: AtomicU64,
    complete: AtomicBool,
    cancel: AtomicBool,
    interrupt: AtomicBool,
    waiting: AtomicBool,
    buffered: AtomicU64,
    read_pos: AtomicU64,
    hide_len: AtomicBool,
    end_at_buffered: AtomicBool,
    last_growth: Mutex<Instant>,
    waiting_since: Mutex<Option<Instant>>,
    failed: Mutex<Option<String>>,
    notice: Mutex<Option<String>>,
    lock: Mutex<()>,
    wake: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Cheap cloneable control/observation handle for a [`GrowingFile`] that has been moved into a
/// decoder.
#[derive(Debug, Clone)]
pub struct GrowingHandle(Arc<Shared>);

impl GrowingHandle {
    /// Abort all current and future reads with [`GrowingError::Cancelled`].
    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::SeqCst);
        self.notify();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::SeqCst)
    }

    /// Make a read that is currently waiting for bytes return [`GrowingError::Interrupted`]
    /// (e.g. so the player can service a seek). Sticky until [`clear_interrupt`](Self::clear_interrupt).
    pub fn interrupt(&self) {
        self.0.interrupt.store(true, Ordering::SeqCst);
        self.notify();
    }

    pub fn clear_interrupt(&self) {
        self.0.interrupt.store(false, Ordering::SeqCst);
    }

    /// Wake waiting readers early (e.g. from a download progress event).
    pub fn notify(&self) {
        let _g = lock(&self.0.lock);
        self.0.wake.notify_all();
    }

    /// Bytes known to be on disk (as of the last check), capped at the total size.
    pub fn buffered_bytes(&self) -> u64 {
        self.0.buffered.load(Ordering::SeqCst)
    }

    /// Announced (or corrected) total size; [`UNKNOWN_SIZE`] if unknown.
    pub fn total_size(&self) -> u64 {
        self.0.total.load(Ordering::SeqCst)
    }

    /// Total size if known.
    pub fn known_total_size(&self) -> Option<u64> {
        Some(self.total_size()).filter(|&t| t != UNKNOWN_SIZE)
    }

    /// Set or correct the total size (e.g. from a transfer progress event). Ignored once the
    /// download is complete (the final file's length wins).
    pub fn set_total_size(&self, total: u64) {
        if !self.is_complete() {
            self.0.total.store(total, Ordering::SeqCst);
            self.notify();
        }
    }

    /// Downloaded fraction; 0.0 while the total is unknown (1.0 once complete).
    pub fn buffered_fraction(&self) -> f64 {
        if self.is_complete() {
            return 1.0;
        }
        match self.known_total_size() {
            Some(0) => 1.0,
            Some(t) => (self.buffered_bytes() as f64 / t as f64).min(1.0),
            None => 0.0,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::SeqCst)
    }

    /// True while a read is blocked waiting for the download to catch up.
    pub fn is_waiting(&self) -> bool {
        self.0.waiting.load(Ordering::SeqCst)
    }

    /// While a read is waiting: how long it has been waiting. `None` when not waiting.
    pub fn waiting_for(&self) -> Option<Duration> {
        lock(&self.0.waiting_since).map(|t| t.elapsed())
    }

    /// While a read is waiting: how long since the file last grew. `None` when not waiting.
    pub fn stalled_for(&self) -> Option<Duration> {
        self.is_waiting()
            .then(|| lock(&self.0.last_growth).elapsed())
    }

    /// Report that the download failed. Reads of already-downloaded bytes still succeed; a read
    /// that would wait for more returns [`SourceFailed`].
    pub fn fail(&self, reason: impl Into<String>) {
        *lock(&self.0.failed) = Some(reason.into());
        self.notify();
    }

    /// The reason passed to [`fail`](Self::fail), if any.
    pub fn failure(&self) -> Option<String> {
        lock(&self.0.failed).clone()
    }

    /// Non-fatal observation about the source (e.g. a size mismatch).
    pub fn notice(&self) -> Option<String> {
        lock(&self.0.notice).clone()
    }

    /// Current read position of the underlying source (includes decoder read-ahead).
    pub fn read_pos(&self) -> u64 {
        self.0.read_pos.load(Ordering::SeqCst)
    }

    /// While set (and the file is incomplete), `byte_len()` reports `None`. Used during format
    /// probing so symphonia does not scan the (not yet downloaded) end of the file for trailing
    /// tags, which would block until the download finishes.
    pub fn set_hide_len(&self, on: bool) {
        self.0.hide_len.store(on, Ordering::SeqCst);
    }

    /// While set (and the file is incomplete), `SeekFrom::End` is relative to the buffered end
    /// instead of the total size. Used during a format seek so binary searches (FLAC) stay
    /// within the already-downloaded range.
    pub fn set_end_at_buffered(&self, on: bool) {
        self.0.end_at_buffered.store(on, Ordering::SeqCst);
    }
}

/// A `Read + Seek + MediaSource` over a file being written by a downloader.
#[derive(Debug)]
pub struct GrowingFile {
    final_path: PathBuf,
    part_path: PathBuf,
    file: Option<File>,
    file_cursor: Option<u64>,
    last_len: u64,
    pos: u64,
    shared: Arc<Shared>,
    poll: Duration,
    missing_timeout: Duration,
}

impl GrowingFile {
    /// `path` may be either the final name or the `.part` name; both are watched. Does not touch
    /// the filesystem (never blocks); the file is opened on first read.
    pub fn new(path: impl AsRef<Path>, total_size: u64) -> Self {
        let path = path.as_ref();
        let s = path.as_os_str().to_string_lossy();
        let (final_path, part_path) = match s.strip_suffix(PART_SUFFIX) {
            Some(stripped) => (PathBuf::from(stripped), path.to_path_buf()),
            None => (
                path.to_path_buf(),
                PathBuf::from(format!("{s}{PART_SUFFIX}")),
            ),
        };
        Self {
            final_path,
            part_path,
            file: None,
            file_cursor: None,
            last_len: 0,
            pos: 0,
            shared: Arc::new(Shared {
                total: AtomicU64::new(total_size),
                complete: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                interrupt: AtomicBool::new(false),
                waiting: AtomicBool::new(false),
                buffered: AtomicU64::new(0),
                read_pos: AtomicU64::new(0),
                hide_len: AtomicBool::new(false),
                end_at_buffered: AtomicBool::new(false),
                last_growth: Mutex::new(Instant::now()),
                waiting_since: Mutex::new(None),
                failed: Mutex::new(None),
                notice: Mutex::new(None),
                lock: Mutex::new(()),
                wake: Condvar::new(),
            }),
            poll: Duration::from_millis(20),
            missing_timeout: Duration::from_secs(30),
        }
    }

    /// Size not known: the download counts as complete once `<name>.part` has been renamed to
    /// `<name>` (or [`GrowingHandle::set_total_size`] is called and reached).
    pub fn new_unknown_size(path: impl AsRef<Path>) -> Self {
        Self::new(path, UNKNOWN_SIZE)
    }

    /// Maximum poll interval while waiting for bytes (default 20 ms).
    pub fn with_poll_interval(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// How long to wait for the file to appear at all (default 30 s).
    pub fn with_missing_timeout(mut self, t: Duration) -> Self {
        self.missing_timeout = t;
        self
    }

    pub fn handle(&self) -> GrowingHandle {
        GrowingHandle(self.shared.clone())
    }

    pub fn buffered_bytes(&self) -> u64 {
        self.shared.buffered.load(Ordering::SeqCst)
    }

    /// See [`GrowingHandle::total_size`].
    pub fn total_size(&self) -> u64 {
        self.shared.total.load(Ordering::SeqCst)
    }

    fn complete(&self) -> bool {
        self.shared.complete.load(Ordering::SeqCst)
    }

    /// End of data for reads: the total if known, else the buffered end once complete.
    fn read_limit(&self) -> u64 {
        let total = self.total_size();
        if self.complete() {
            self.buffered_bytes()
        } else {
            total
        }
    }

    fn set_notice(&self, n: String) {
        *lock(&self.shared.notice) = Some(n);
    }

    /// Re-check the file on disk and return the number of available bytes (capped at total).
    fn refresh(&mut self) -> io::Result<Option<u64>> {
        if self.file.is_none() {
            // Prefer the final file (download complete), then the in-progress part.
            for p in [&self.final_path, &self.part_path] {
                if let Ok(f) = File::open(p) {
                    self.file = Some(f);
                    self.file_cursor = None;
                    break;
                }
            }
        }
        let Some(file) = &self.file else {
            return Ok(None);
        };
        let mut len = file.metadata()?.len();
        let mut finished = false;
        if !self.part_path.exists() {
            // Part is gone. Normally it was renamed (same inode, our fd already sees it). If the
            // final file is a different, longer file, switch to it; if nothing exists, the
            // download was discarded.
            match std::fs::metadata(&self.final_path) {
                Ok(m) => {
                    if m.len() > len {
                        self.file = Some(File::open(&self.final_path)?);
                        self.file_cursor = None;
                        len = m.len();
                    }
                    finished = true;
                }
                Err(_) => {
                    if !self.complete() && len < self.total_size() {
                        return Err(GrowingError::Missing(self.final_path.clone()).into_io());
                    }
                }
            }
        }
        if len > self.last_len {
            *lock(&self.shared.last_growth) = Instant::now();
        }
        self.last_len = len;

        let total = self.total_size();
        if finished && !self.complete() {
            // The final file is authoritative.
            if total != UNKNOWN_SIZE && total != len {
                self.set_notice(format!(
                    "download size mismatch: expected {total} B, file has {len} B"
                ));
            }
            self.shared.total.store(len, Ordering::SeqCst);
            self.shared.complete.store(true, Ordering::SeqCst);
        } else if !finished && total != UNKNOWN_SIZE && len > total {
            // More bytes than announced: stop trusting the size until the rename.
            self.set_notice(format!(
                "download larger than announced ({total} B); waiting for completion"
            ));
            self.shared.total.store(UNKNOWN_SIZE, Ordering::SeqCst);
        }
        let total = self.total_size();
        let avail = len.min(total);
        if total != UNKNOWN_SIZE && avail >= total {
            self.shared.complete.store(true, Ordering::SeqCst);
        }
        self.shared.buffered.store(avail, Ordering::SeqCst);
        Ok(Some(avail))
    }

    /// Block until at least `needed` bytes are available, or the data is known to end earlier
    /// (returns the available length then), or cancelled/interrupted/failed.
    fn wait_for(&mut self, needed: u64) -> io::Result<u64> {
        let started = Instant::now();
        let result = loop {
            if self.shared.cancel.load(Ordering::SeqCst) {
                break Err(GrowingError::Cancelled.into_io());
            }
            match self.refresh() {
                Ok(Some(avail)) if avail >= needed || self.complete() => break Ok(avail),
                Ok(Some(_)) => {}
                Ok(None) if started.elapsed() >= self.missing_timeout => {
                    break Err(GrowingError::Missing(self.part_path.clone()).into_io());
                }
                Ok(None) => {}
                Err(e) => break Err(e),
            }
            if let Some(reason) = lock(&self.shared.failed).clone() {
                break Err(io::Error::other(SourceFailed(reason)));
            }
            if self.shared.interrupt.load(Ordering::SeqCst) {
                break Err(GrowingError::Interrupted.into_io());
            }
            if !self.shared.waiting.swap(true, Ordering::SeqCst) {
                *lock(&self.shared.waiting_since) = Some(Instant::now());
            }
            let g = lock(&self.shared.lock);
            if !self.shared.cancel.load(Ordering::SeqCst)
                && !self.shared.interrupt.load(Ordering::SeqCst)
            {
                let _ = self.shared.wake.wait_timeout(g, self.poll);
            }
        };
        self.shared.waiting.store(false, Ordering::SeqCst);
        *lock(&self.shared.waiting_since) = None;
        result
    }
}

impl Read for GrowingFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.read_limit() {
            return Ok(0);
        }
        if self.shared.cancel.load(Ordering::SeqCst) {
            return Err(GrowingError::Cancelled.into_io());
        }
        let mut avail = self.buffered_bytes();
        if avail <= self.pos || self.file.is_none() {
            avail = self.wait_for(self.pos + 1)?;
            if avail <= self.pos {
                return Ok(0); // complete, and shorter than our position
            }
        }
        let want = (avail - self.pos).min(buf.len() as u64) as usize;
        let file = self.file.as_mut().expect("file opened by wait_for");
        if self.file_cursor != Some(self.pos) {
            file.seek(SeekFrom::Start(self.pos))?;
        }
        let n = file.read(&mut buf[..want])?;
        self.pos += n as u64;
        self.file_cursor = Some(self.pos);
        self.shared.read_pos.store(self.pos, Ordering::SeqCst);
        Ok(n)
    }
}

impl Seek for GrowingFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let mut end = self.read_limit();
                let incomplete = !self.complete();
                if (end == UNKNOWN_SIZE || self.shared.end_at_buffered.load(Ordering::SeqCst))
                    && incomplete
                    && let Ok(Some(avail)) = self.refresh()
                {
                    end = avail;
                }
                if end == UNKNOWN_SIZE {
                    end = self.buffered_bytes();
                }
                end.checked_add_signed(d)
            }
        };
        let new = new.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "seek before start of file")
        })?;
        // Lazy: a seek past the buffered range only blocks when the next read happens.
        self.pos = new;
        self.shared.read_pos.store(new, Ordering::SeqCst);
        Ok(new)
    }
}

impl MediaSource for GrowingFile {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        let complete = self.complete();
        let total = self.total_size();
        if total == UNKNOWN_SIZE || (self.shared.hide_len.load(Ordering::SeqCst) && !complete) {
            None
        } else {
            Some(total)
        }
    }
}
