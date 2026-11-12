//! [`Player`]: one-track player over a [`GrowingFile`], decoding on its own thread.
//!
//! All `Player` methods return immediately; decoding, waiting for bytes and output happen on a
//! per-track decode thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::growing::{GrowingError, GrowingFile, GrowingHandle, PART_SUFFIX, SourceFailed};
use crate::sink::{AudioSink, CaptureSink, NullSink};

/// Where decoded audio goes.
#[derive(Debug, Clone)]
pub enum Output {
    /// The default system output device (cpal).
    #[cfg(feature = "output")]
    Device,
    /// Record samples (tests, headless proof).
    Capture(CaptureSink),
    /// Discard samples.
    Null,
}

impl Output {
    fn make_sink(&self) -> Box<dyn AudioSink> {
        match self {
            #[cfg(feature = "output")]
            Output::Device => Box::new(crate::output::CpalSink::new()),
            Output::Capture(c) => Box::new(c.clone()),
            Output::Null => Box::new(NullSink),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayState {
    /// Nothing loaded.
    Idle,
    /// Waiting for the download to provide bytes (startup or underrun).
    Buffering,
    Playing,
    Paused,
    Ended,
    Error(String),
}

/// Point-in-time view of the player, cheap to take from the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSnapshot {
    pub state: PlayState,
    pub path: Option<PathBuf>,
    /// Audible position in seconds.
    pub position_secs: f64,
    /// Track duration, if known (from the container, or estimated from bitrate).
    pub duration_secs: Option<f64>,
    /// True if `duration_secs` is a bitrate estimate.
    pub duration_estimated: bool,
    /// Downloaded bytes / total bytes.
    pub buffered_fraction: f64,
    /// Seconds of audio covered by downloaded bytes, if the duration is known.
    pub buffered_secs: Option<f64>,
    pub volume: f32,
    /// Last non-fatal problem (e.g. a failed seek or a skipped corrupt packet).
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SeekError {
    /// No track loaded, or the track failed / was stopped.
    NothingPlaying,
    /// The duration is not known yet, so the buffered range cannot be mapped to time.
    DurationUnknown,
    /// Target lies beyond what has been downloaded.
    NotBuffered { requested: f64, buffered_secs: f64 },
}

impl std::fmt::Display for SeekError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeekError::NothingPlaying => write!(f, "nothing is playing"),
            SeekError::DurationUnknown => write!(f, "duration not known yet"),
            SeekError::NotBuffered {
                requested,
                buffered_secs,
            } => write!(
                f,
                "{requested:.1}s is not downloaded yet (buffered up to {buffered_secs:.1}s)"
            ),
        }
    }
}

impl std::error::Error for SeekError {}

/// Seconds kept between a seek target and the buffered edge (container overhead, read-ahead).
const SEEK_MARGIN_SECS: f64 = 0.5;

/// Pushed to subscribers (see [`Player::subscribe`]) so the UI can redraw without polling.
/// Events of a replaced or stopped track are never delivered after its `Stopped`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlayerEvent {
    /// First decoded audio reached the output.
    Started {
        path: PathBuf,
        duration_secs: Option<f64>,
    },
    /// Waiting for the download (startup, underrun or stall).
    Buffering,
    Playing,
    Paused,
    /// A seek was carried out; playback continues from `position_secs`.
    Seeked {
        position_secs: f64,
    },
    /// About 4 times a second while playing.
    PositionTick {
        position_secs: f64,
        buffered_fraction: f64,
    },
    /// [`PlayerSnapshot::notice`] changed (e.g. a stalled download; `None` once it recovers).
    Notice(Option<String>),
    Ended,
    Error(String),
    /// The track was stopped or replaced by [`Player::stop`] / [`Player::play`].
    Stopped,
}

/// Builds a sink on the decode thread (the sink itself need not be `Send`).
pub type SinkFactory = Arc<dyn Fn() -> Box<dyn AudioSink> + Send + Sync>;

/// Default time without new bytes before a waiting player reports the download as stalled.
pub const DEFAULT_STALL_THRESHOLD: Duration = Duration::from_secs(5);

const TICK_EVERY: Duration = Duration::from_millis(250);

type Waker = Arc<dyn Fn() + Send + Sync>;

/// Fan-out of events to subscribers. `generation` identifies the current track; events from older
/// tracks are dropped. Checking `generation` and sending happen under the `subs` lock, so nothing from
/// a stopped track can follow its `Stopped`.
#[derive(Default)]
struct Hub {
    subs: Mutex<Vec<Sender<PlayerEvent>>>,
    waker: Mutex<Option<Waker>>,
    generation: AtomicU64,
}

impl Hub {
    fn emit(&self, generation: u64, ev: PlayerEvent) {
        {
            let mut subs = lock(&self.subs);
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            subs.retain(|s| s.send(ev.clone()).is_ok());
        }
        self.wake();
    }

    /// Start a new generation, emitting `Stopped` for the old one if `had_track`.
    fn bump(&self, had_track: bool) -> u64 {
        let generation = {
            let mut subs = lock(&self.subs);
            if had_track {
                subs.retain(|s| s.send(PlayerEvent::Stopped).is_ok());
            }
            self.generation.fetch_add(1, Ordering::SeqCst) + 1
        };
        if had_track {
            self.wake();
        }
        generation
    }

    fn wake(&self) {
        let w = lock(&self.waker).clone();
        if let Some(w) = w {
            w();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Starting,
    Playing,
    Paused,
    Ended,
    Error(String),
}

enum Cmd {
    Pause,
    Resume,
    Seek(f64),
}

struct Ctl {
    generation: u64,
    hub: Arc<Hub>,
    path: PathBuf,
    growing: GrowingHandle,
    status: Mutex<Status>,
    position: AtomicU64,
    duration: Mutex<Option<(f64, bool)>>,
    notice: Mutex<Option<String>>,
    cmd_pending: AtomicBool,
    volume: Arc<AtomicU32>,
    stall_ms: Arc<AtomicU64>,
    /// Audio queued in the sink after the last write, in microseconds.
    ahead_us: AtomicU64,
    /// Set by `Player::stop`/`play` (as opposed to the source being cancelled by someone else).
    stopped: AtomicBool,
    /// Set when the decode thread has exited (and dropped its sink).
    finished: Arc<AtomicBool>,
    kick: (Mutex<bool>, Condvar),
}

impl Ctl {
    fn set_status(&self, s: Status) {
        *lock(&self.status) = s;
        self.kick();
    }
    fn status(&self) -> Status {
        lock(&self.status).clone()
    }
    fn set_position(&self, secs: f64) {
        self.position
            .store(secs.max(0.0).to_bits(), Ordering::SeqCst);
    }
    fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::SeqCst))
    }
    fn duration(&self) -> Option<(f64, bool)> {
        *lock(&self.duration)
    }
    fn set_duration(&self, d: Option<(f64, bool)>) {
        *lock(&self.duration) = d;
    }
    fn set_notice(&self, n: impl Into<String>) {
        *lock(&self.notice) = Some(n.into());
        self.kick();
    }
    fn notice(&self) -> Option<String> {
        lock(&self.notice).clone()
    }
    fn emit(&self, ev: PlayerEvent) {
        self.hub.emit(self.generation, ev);
    }
    /// Wake the ticker so it reports a change immediately.
    fn kick(&self) {
        *lock(&self.kick.0) = true;
        self.kick.1.notify_all();
    }
    fn public_state(&self) -> PlayState {
        match self.status() {
            Status::Starting => PlayState::Buffering,
            // Only an actual underrun counts: the decoder waiting for bytes while the output
            // still has queued audio is not audible.
            Status::Playing
                if self.growing.waiting_for().is_some_and(|w| {
                    w.as_micros() as u64 >= self.ahead_us.load(Ordering::SeqCst)
                }) =>
            {
                PlayState::Buffering
            }
            Status::Playing => PlayState::Playing,
            Status::Paused => PlayState::Paused,
            Status::Ended => PlayState::Ended,
            Status::Error(e) => PlayState::Error(e),
        }
    }
}

struct Session {
    ctl: Arc<Ctl>,
    tx: Sender<Cmd>,
}

impl Session {
    fn send(&self, cmd: Cmd) {
        self.ctl.cmd_pending.store(true, Ordering::SeqCst);
        let _ = self.tx.send(cmd);
    }
}

/// Plays one track at a time from a (possibly still downloading) file.
pub struct Player {
    sinks: SinkFactory,
    volume: Arc<AtomicU32>,
    stall_ms: Arc<AtomicU64>,
    hub: Arc<Hub>,
    current: Mutex<Option<Session>>,
    /// Exit flag of the most recent decode thread; a new track waits (on its own thread) for
    /// the old one to release the output before opening it.
    last_finished: Mutex<Option<Arc<AtomicBool>>>,
}

impl Player {
    pub fn new(output: Output) -> Self {
        Self::with_sink_factory(Arc::new(move || output.make_sink()))
    }

    /// Use a custom sink (built on the decode thread for every track).
    pub fn with_sink_factory(sinks: SinkFactory) -> Self {
        Self {
            sinks,
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            stall_ms: Arc::new(AtomicU64::new(DEFAULT_STALL_THRESHOLD.as_millis() as u64)),
            hub: Arc::new(Hub::default()),
            current: Mutex::new(None),
            last_finished: Mutex::new(None),
        }
    }

    fn session(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        lock(&self.current)
    }

    /// Receive [`PlayerEvent`]s. Any number of subscribers; dropped receivers are pruned.
    pub fn subscribe(&self) -> Receiver<PlayerEvent> {
        let (tx, rx) = mpsc::channel();
        lock(&self.hub.subs).push(tx);
        rx
    }

    /// Called (from player threads) after events are sent, e.g. to wake the UI event loop.
    /// Must be cheap and must not call back into the `Player`.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *lock(&self.hub.waker) = Some(Arc::new(waker));
    }

    /// How long without new bytes, while waiting, before the stall notice (default 5 s).
    pub fn set_stall_threshold(&self, t: Duration) {
        self.stall_ms.store(t.as_millis() as u64, Ordering::SeqCst);
    }

    /// Start playing `path` (the final name or the `.part` name) whose complete size is
    /// `total_size`. Replaces any current track. Returns immediately.
    pub fn play(&self, path: impl AsRef<Path>, total_size: u64) {
        self.play_source(path.as_ref(), GrowingFile::new(path.as_ref(), total_size));
    }

    /// Like [`play`](Self::play) when the size is not known; the download counts as complete
    /// when `<name>.part` is renamed to `<name>`.
    pub fn play_unknown_size(&self, path: impl AsRef<Path>) {
        self.play_source(path.as_ref(), GrowingFile::new_unknown_size(path.as_ref()));
    }

    /// Like [`play`](Self::play) with a pre-configured source (poll interval, timeouts).
    pub fn play_source(&self, path: &Path, src: GrowingFile) {
        let had = self.stop_session();
        let generation = self.hub.bump(had);
        let (tx, rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let ctl = Arc::new(Ctl {
            generation,
            hub: self.hub.clone(),
            path: path.to_path_buf(),
            growing: src.handle(),
            status: Mutex::new(Status::Starting),
            position: AtomicU64::new(0f64.to_bits()),
            duration: Mutex::new(None),
            notice: Mutex::new(None),
            cmd_pending: AtomicBool::new(false),
            volume: self.volume.clone(),
            stall_ms: self.stall_ms.clone(),
            ahead_us: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            finished: finished.clone(),
            kick: (Mutex::new(false), Condvar::new()),
        });
        let prev = lock(&self.last_finished).replace(finished);
        let sinks = self.sinks.clone();
        let thread_ctl = ctl.clone();
        let hint_ext = extension_hint(path);
        let spawned = std::thread::Builder::new()
            .name("goalseek-decode".into())
            .spawn(move || decode_thread(src, thread_ctl, rx, sinks, hint_ext, prev));
        match spawned {
            Ok(_) => {
                let tick_ctl = ctl.clone();
                if let Err(e) = std::thread::Builder::new()
                    .name("goalseek-player-tick".into())
                    .spawn(move || ticker(&tick_ctl))
                {
                    ctl.set_notice(format!("no player events: {e}"));
                }
            }
            Err(e) => {
                ctl.finished.store(true, Ordering::SeqCst);
                ctl.set_status(Status::Error(format!("cannot start decoder: {e}")));
                ctl.emit(PlayerEvent::Error(format!("cannot start decoder: {e}")));
            }
        }
        *self.session() = Some(Session { ctl, tx });
    }

    pub fn pause(&self) {
        if let Some(s) = &*self.session() {
            s.send(Cmd::Pause);
        }
    }

    pub fn resume(&self) {
        if let Some(s) = &*self.session() {
            s.send(Cmd::Resume);
        }
    }

    fn stop_session(&self) -> bool {
        match self.session().take() {
            Some(s) => {
                s.ctl.stopped.store(true, Ordering::SeqCst);
                s.ctl.growing.cancel();
                s.ctl.kick();
                true
            }
            None => false,
        }
    }

    /// Stop and unload. The decode thread exits on its own (never joined here).
    pub fn stop(&self) {
        if self.stop_session() {
            self.hub.bump(true);
        }
    }

    /// The current track's source handle (e.g. to forward download progress with
    /// [`GrowingHandle::notify`] or [`GrowingHandle::set_total_size`]).
    pub fn source_handle(&self) -> Option<GrowingHandle> {
        self.session().as_ref().map(|s| s.ctl.growing.clone())
    }

    /// The download behind the current track failed or was cancelled: play what is already
    /// downloaded, then stop with `PlayState::Error(reason)` instead of waiting forever.
    pub fn fail_source(&self, reason: impl Into<String>) {
        if let Some(s) = &*self.session() {
            s.ctl.growing.fail(reason);
        }
    }

    /// Seek within the downloaded range. Targets beyond it are rejected immediately with
    /// [`SeekError::NotBuffered`] so the UI can show why. Seeking backwards is always allowed.
    pub fn seek(&self, secs: f64) -> Result<(), SeekError> {
        let guard = self.session();
        let s = guard.as_ref().ok_or(SeekError::NothingPlaying)?;
        if matches!(s.ctl.status(), Status::Error(_)) {
            return Err(SeekError::NothingPlaying);
        }
        let secs = secs.max(0.0);
        if !s.ctl.growing.is_complete() && secs > s.ctl.position() {
            let (dur, _) = s.ctl.duration().ok_or(SeekError::DurationUnknown)?;
            if s.ctl.growing.known_total_size().is_none() {
                return Err(SeekError::NotBuffered {
                    requested: secs,
                    buffered_secs: s.ctl.position(),
                });
            }
            let buffered = dur * s.ctl.growing.buffered_fraction();
            if secs > buffered - SEEK_MARGIN_SECS {
                return Err(SeekError::NotBuffered {
                    requested: secs,
                    buffered_secs: buffered,
                });
            }
        }
        // Wake a read that is blocked at the download edge so the seek is serviced now.
        s.ctl.growing.interrupt();
        s.send(Cmd::Seek(secs));
        Ok(())
    }

    /// Volume 0.0..=1.0 (clamped), applied on the decode thread.
    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.volume.store(v.to_bits(), Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        let volume = f32::from_bits(self.volume.load(Ordering::SeqCst));
        let guard = self.session();
        let Some(s) = guard.as_ref() else {
            return PlayerSnapshot {
                state: PlayState::Idle,
                path: None,
                position_secs: 0.0,
                duration_secs: None,
                duration_estimated: false,
                buffered_fraction: 0.0,
                buffered_secs: None,
                volume,
                notice: None,
            };
        };
        let g = &s.ctl.growing;
        let dur = s.ctl.duration();
        let frac = g.buffered_fraction();
        PlayerSnapshot {
            state: s.ctl.public_state(),
            path: Some(s.ctl.path.clone()),
            position_secs: s.ctl.position(),
            duration_secs: dur.map(|d| d.0),
            duration_estimated: dur.is_some_and(|d| d.1),
            buffered_fraction: frac,
            buffered_secs: dur.and_then(|d| {
                if g.is_complete() {
                    Some(d.0)
                } else {
                    g.known_total_size().map(|_| d.0 * frac)
                }
            }),
            volume,
            notice: s.ctl.notice(),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Body of the decode thread: never panics outward, always marks `finished`.
fn decode_thread(
    src: GrowingFile,
    ctl: Arc<Ctl>,
    rx: Receiver<Cmd>,
    sinks: SinkFactory,
    hint_ext: Option<String>,
    prev: Option<Arc<AtomicBool>>,
) {
    struct Finished<'a>(&'a Ctl);
    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.finished.store(true, Ordering::SeqCst);
            self.0.kick();
        }
    }
    let _finished = Finished(&ctl);

    // Let the previous track release the output first (bounded; it exits within ms).
    if let Some(prev) = prev {
        let until = Instant::now() + Duration::from_secs(1);
        while !prev.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sink = sinks();
        run(src, &ctl, rx, sink, hint_ext)
    }))
    .unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".into());
        Err(format!("internal player error: {msg}"))
    });
    if ctl.stopped.load(Ordering::SeqCst) {
        return;
    }
    let err = match result {
        Err(e) => Some(e),
        // Someone else cancelled the source.
        Ok(()) if ctl.growing.is_cancelled() => Some("playback source cancelled".to_string()),
        Ok(()) => None,
    };
    if let Some(e) = err {
        ctl.set_status(Status::Error(e));
    }
}

/// Per-track observer: turns state changes into events, reports stalls, ticks the position.
fn ticker(ctl: &Ctl) {
    let mut last_state: Option<PlayState> = None;
    let mut last_notice: Option<String> = None;
    let mut last_source_notice: Option<String> = None;
    let mut stall_notice: Option<String> = None;
    let mut last_tick = Instant::now();
    loop {
        if ctl.stopped.load(Ordering::SeqCst) {
            return;
        }
        let g = &ctl.growing;
        g.refresh();

        // Stall detection (only while actually waiting for bytes).
        let threshold = Duration::from_millis(ctl.stall_ms.load(Ordering::SeqCst));
        match g.stalled_for() {
            Some(d) if d >= threshold => {
                if stall_notice.is_none() {
                    let n = format!(
                        "download stalled: no data for {} s, waiting",
                        d.as_secs().max(1)
                    );
                    *lock(&ctl.notice) = Some(n.clone());
                    stall_notice = Some(n);
                }
            }
            Some(_) => {}
            None => {
                if let Some(n) = stall_notice.take() {
                    let mut cur = lock(&ctl.notice);
                    if cur.as_ref() == Some(&n) {
                        *cur = None;
                    }
                }
            }
        }
        // Source observations (size mismatch).
        let src_notice = g.notice();
        if src_notice.is_some() && src_notice != last_source_notice {
            *lock(&ctl.notice) = src_notice.clone();
            last_source_notice = src_notice;
        }

        let notice = ctl.notice();
        if notice != last_notice {
            ctl.emit(PlayerEvent::Notice(notice.clone()));
            last_notice = notice;
        }
        let state = ctl.public_state();
        if last_state.as_ref() != Some(&state) {
            let ev = match &state {
                PlayState::Idle => None,
                PlayState::Buffering => Some(PlayerEvent::Buffering),
                PlayState::Playing => Some(PlayerEvent::Playing),
                PlayState::Paused => Some(PlayerEvent::Paused),
                PlayState::Ended => Some(PlayerEvent::Ended),
                PlayState::Error(e) => Some(PlayerEvent::Error(e.clone())),
            };
            if let Some(ev) = ev {
                ctl.emit(ev);
            }
            last_state = Some(state.clone());
            last_tick = Instant::now();
        }
        if state == PlayState::Playing && last_tick.elapsed() >= TICK_EVERY {
            ctl.emit(PlayerEvent::PositionTick {
                position_secs: ctl.position(),
                buffered_fraction: g.buffered_fraction(),
            });
            last_tick = Instant::now();
        }
        if matches!(state, PlayState::Error(_)) && ctl.finished.load(Ordering::SeqCst) {
            return;
        }
        let mut k = lock(&ctl.kick.0);
        if !*k {
            k = ctl
                .kick
                .1
                .wait_timeout(k, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *k = false;
    }
}

fn extension_hint(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    let base = s.strip_suffix(PART_SUFFIX).unwrap_or(&s);
    Path::new(base)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
}

fn sym_err(e: &SymError) -> Option<GrowingError> {
    match e {
        SymError::IoError(io) => GrowingError::from_io(io).cloned(),
        _ => None,
    }
}

/// User-facing text for a read/probe failure (source problems are passed through verbatim).
fn source_err_text(e: &SymError) -> Option<String> {
    match e {
        SymError::IoError(io) => GrowingError::from_io(io)
            .map(ToString::to_string)
            .or_else(|| SourceFailed::from_io(io).map(ToString::to_string)),
        _ => None,
    }
}

/// Decoded audio not yet fully accepted by the sink.
struct Pending {
    samples: Vec<f32>,
    off: usize,
    start_secs: f64,
}

struct Decoding {
    format: Box<dyn FormatReader + 'static>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    sample_rate: u32,
    channels: usize,
    sink_started: bool,
    pending: Option<Pending>,
    skip_to: Option<Timestamp>,
    duration_known: bool,
    decoded_secs: f64,
}

impl Decoding {
    fn ts_secs(&self, ts: Timestamp) -> f64 {
        match self.time_base {
            Some(tb) => tb.calc_time_saturating(ts).as_secs_f64(),
            None if self.sample_rate > 0 => ts.get() as f64 / f64::from(self.sample_rate),
            None => 0.0,
        }
    }
}

fn open(src: GrowingFile, ctl: &Ctl, hint_ext: Option<String>) -> Result<Decoding, String> {
    let g = ctl.growing.clone();
    // Don't let the probe scan the (undownloaded) tail for trailing tags.
    g.set_hide_len(true);
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = &hint_ext {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    );
    g.set_hide_len(false);
    let format = probed.map_err(|e| {
        source_err_text(&e).unwrap_or_else(|| format!("unsupported or unreadable file: {e}"))
    })?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or("no audio track")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio codec parameters")?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;
    let sample_rate = params.sample_rate.unwrap_or(0);
    let duration = match (track.duration, track.time_base, track.num_frames) {
        (Some(d), Some(tb), _) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
        (_, _, Some(n)) if sample_rate > 0 => Some(n as f64 / f64::from(sample_rate)),
        _ => None,
    };
    ctl.set_duration(duration.map(|d| (d, false)));
    Ok(Decoding {
        track_id: track.id,
        time_base: track.time_base,
        sample_rate,
        channels: params.channels.as_ref().map(|c| c.count()).unwrap_or(0),
        decoder,
        format,
        sink_started: false,
        pending: None,
        skip_to: None,
        duration_known: duration.is_some(),
        decoded_secs: 0.0,
    })
}

fn do_seek(d: &mut Decoding, ctl: &Ctl, sink: &mut dyn AudioSink, secs: f64) {
    let g = &ctl.growing;
    g.clear_interrupt();
    let Some(time) = Time::try_from_secs_f64(secs) else {
        return;
    };
    g.set_end_at_buffered(true);
    let res = d.format.seek(
        SeekMode::Accurate,
        SeekTo::Time {
            time,
            track_id: Some(d.track_id),
        },
    );
    g.set_end_at_buffered(false);
    match res {
        Ok(seeked) => {
            d.decoder.reset();
            d.pending = None;
            sink.flush();
            d.skip_to = Some(seeked.required_ts);
            ctl.set_position(d.ts_secs(seeked.required_ts));
            ctl.emit(PlayerEvent::Seeked {
                position_secs: ctl.position(),
            });
            if ctl.status() == Status::Ended {
                ctl.set_status(Status::Playing);
            }
        }
        Err(e) if sym_err(&e) == Some(GrowingError::Cancelled) => {}
        Err(e) => ctl.set_notice(format!("seek failed: {e}")),
    }
}

/// Process one command. Returns the new paused flag.
fn handle(cmd: Cmd, d: &mut Decoding, ctl: &Ctl, sink: &mut dyn AudioSink, paused: bool) -> bool {
    match cmd {
        Cmd::Pause => {
            sink.pause();
            if matches!(ctl.status(), Status::Playing | Status::Starting) {
                ctl.set_status(Status::Paused);
            }
            true
        }
        Cmd::Resume => {
            sink.resume();
            if ctl.status() == Status::Paused {
                ctl.set_status(Status::Playing);
            }
            false
        }
        Cmd::Seek(secs) => {
            do_seek(d, ctl, sink, secs);
            paused
        }
    }
}

fn run(
    src: GrowingFile,
    ctl: &Ctl,
    rx: Receiver<Cmd>,
    mut sink: Box<dyn AudioSink>,
    hint_ext: Option<String>,
) -> Result<(), String> {
    let g = ctl.growing.clone();
    let mut d = open(src, ctl, hint_ext)?;
    let mut paused = false;
    let mut announced = false;
    let mut buf: Vec<f32> = Vec::new();
    let abort = || ctl.cmd_pending.load(Ordering::SeqCst) || g.is_cancelled();

    loop {
        if g.is_cancelled() {
            return Ok(());
        }
        ctl.cmd_pending.store(false, Ordering::SeqCst);
        loop {
            match rx.try_recv() {
                Ok(cmd) => paused = handle(cmd, &mut d, ctl, sink.as_mut(), paused),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if paused || ctl.status() == Status::Ended {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(cmd) => paused = handle(cmd, &mut d, ctl, sink.as_mut(), paused),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
            continue;
        }

        // Feed the sink whatever is left from the previous packet.
        if let Some(p) = &mut d.pending {
            let n = sink.write(&p.samples[p.off..], &abort);
            if let Some(e) = sink.error() {
                return Err(e);
            }
            if n > 0 && !announced {
                announced = true;
                ctl.emit(PlayerEvent::Started {
                    path: ctl.path.clone(),
                    duration_secs: ctl.duration().map(|d| d.0),
                });
                if ctl.status() == Status::Starting {
                    ctl.set_status(Status::Playing);
                }
            }
            p.off += n;
            let ch = d.channels.max(1);
            let rate = f64::from(d.sample_rate.max(1));
            let queued = sink.queued_frames() as f64;
            ctl.ahead_us
                .store((queued / rate * 1e6) as u64, Ordering::SeqCst);
            let played = (p.off / ch) as f64 - queued;
            ctl.set_position(p.start_secs + played / rate);
            if p.off < p.samples.len() {
                continue;
            }
            d.pending = None;
        }

        let packet = match d.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => {
                sink.drain(&abort);
                if let Some(e) = sink.error() {
                    return Err(e);
                }
                if !abort() {
                    ctl.set_status(Status::Ended);
                }
                continue;
            }
            Err(e) => match sym_err(&e) {
                Some(GrowingError::Cancelled) => return Ok(()),
                Some(GrowingError::Interrupted) => {
                    g.clear_interrupt();
                    continue;
                }
                _ => match e {
                    _ if source_err_text(&e).is_some() => {
                        return Err(source_err_text(&e).unwrap_or_default());
                    }
                    SymError::IoError(io)
                        if io.kind() == std::io::ErrorKind::UnexpectedEof && g.is_complete() =>
                    {
                        // Truncated tail (e.g. the file ended up shorter than announced).
                        sink.drain(&abort);
                        if !abort() {
                            ctl.set_status(Status::Ended);
                        }
                        continue;
                    }
                    SymError::DecodeError(msg) => {
                        ctl.set_notice(format!("skipped corrupt data: {msg}"));
                        continue;
                    }
                    other => return Err(format!("read failed: {other}")),
                },
            },
        };
        if packet.track_id != d.track_id {
            continue;
        }
        let pts = packet.pts;
        let decoded = match d.decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(msg)) => {
                ctl.set_notice(format!("skipped corrupt packet: {msg}"));
                continue;
            }
            Err(e) => return Err(format!("decode failed: {e}")),
        };
        if !d.sink_started {
            let spec = decoded.spec();
            d.sample_rate = spec.rate();
            d.channels = spec.channels().count();
            sink.start(d.sample_rate, d.channels)?;
            d.sink_started = true;
            if paused {
                sink.pause();
            }
        }
        decoded.copy_to_vec_interleaved(&mut buf);
        let ch = d.channels.max(1);
        let rate = f64::from(d.sample_rate.max(1));
        let mut start_secs = d.ts_secs(pts);
        let mut skip_frames = 0usize;
        if let Some(target) = d.skip_to {
            // Accurate seek: drop frames before the requested timestamp.
            let frames = (buf.len() / ch) as i64;
            let behind = target.get() - pts.get();
            if behind >= frames {
                continue;
            }
            skip_frames = behind.max(0) as usize;
            start_secs = d.ts_secs(target);
            d.skip_to = None;
        }
        let vol = f32::from_bits(ctl.volume.load(Ordering::SeqCst));
        let mut out: Vec<f32> = buf[skip_frames * ch..].to_vec();
        if vol != 1.0 {
            out.iter_mut().for_each(|s| *s *= vol);
        }
        // Bitrate-based duration estimate for containers that don't declare one (e.g. CBR MP3
        // while the tail is not downloaded yet).
        d.decoded_secs = start_secs + (out.len() / ch) as f64 / rate;
        if !d.duration_known && d.decoded_secs >= 2.0 {
            let bytes = g.read_pos().max(1) as f64;
            ctl.set_duration(Some((d.decoded_secs * g.total_size() as f64 / bytes, true)));
        }
        d.pending = Some(Pending {
            samples: out,
            off: 0,
            start_secs,
        });
    }
}
