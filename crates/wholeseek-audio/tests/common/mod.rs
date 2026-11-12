#![allow(dead_code)]

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const SECS: u32 = 20;
pub const RATE: u32 = 44_100;

/// Generate a 20 s, 44.1 kHz stereo test signal (two chirps + pink noise, so it compresses like
/// music rather than a pure tone) with ffmpeg.
pub fn make_fixture(dir: &Path, ext: &str) -> PathBuf {
    make_fixture_secs(dir, ext, SECS)
}

/// Like [`make_fixture`] with a custom length (and a different tone per length).
pub fn make_fixture_secs(dir: &Path, ext: &str, secs: u32) -> PathBuf {
    let out = dir.join(format!("fixture-{secs}s.{ext}"));
    let codec: &[&str] = match ext {
        "flac" => &["-c:a", "flac", "-sample_fmt", "s16"],
        "mp3" => &["-c:a", "libmp3lame", "-b:a", "320k"],
        other => panic!("no fixture recipe for {other}"),
    };
    // Different base tone per length so two fixtures are distinguishable.
    let f = if secs == SECS { 220 } else { 220 + 7 * secs };
    let tone =
        format!("aevalsrc=0.4*sin(2*PI*({f}+20*t)*t)|0.4*sin(2*PI*(330+15*t)*t):s={RATE}:d={secs}");
    let noise = format!("anoisesrc=d={secs}:c=pink:r={RATE}:a=0.15:seed=7");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", &tone, "-f", "lavfi", "-i", &noise])
        .args([
            "-filter_complex",
            "[1]aformat=channel_layouts=stereo[n];[0][n]amix=inputs=2:normalize=0",
        ])
        .args(["-ar", &RATE.to_string(), "-ac", "2"])
        .args(codec)
        .arg(&out)
        .status()
        .expect("ffmpeg must be installed to generate the test fixture");
    assert!(status.success(), "ffmpeg failed for {ext}");
    out
}

/// Full, independent decode of a complete file: (interleaved samples, rate, channels).
pub fn reference_decode(path: &Path) -> (Vec<f32>, u32, usize) {
    let mss = MediaSourceStream::new(Box::new(File::open(path).unwrap()), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension() {
        hint.with_extension(&e.to_string_lossy());
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .unwrap();
    let track = format.default_track(TrackType::Audio).unwrap();
    let id = track.id;
    let mut dec = symphonia::default::get_codecs()
        .make_audio_decoder(
            track.codec_params.as_ref().unwrap().audio().unwrap(),
            &AudioDecoderOptions::default(),
        )
        .unwrap();
    let (mut all, mut tmp, mut rate, mut ch) = (Vec::new(), Vec::<f32>::new(), 0, 0);
    loop {
        let p = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => panic!("reference decode: {e}"),
        };
        if p.track_id != id {
            continue;
        }
        match dec.decode(&p) {
            Ok(b) => {
                rate = b.spec().rate();
                ch = b.spec().channels().count();
                b.copy_to_vec_interleaved(&mut tmp);
                all.extend_from_slice(&tmp);
            }
            Err(Error::DecodeError(_)) => {}
            Err(e) => panic!("reference decode: {e}"),
        }
    }
    (all, rate, ch)
}

/// Simulated download: copies `src` into `<dest>.part` at `bytes_per_sec`, then renames it to
/// `dest` (like soulseek-rs-lib).
pub struct Writer {
    pub written: Arc<AtomicU64>,
    pub done: Arc<AtomicBool>,
    pub started: Instant,
    pub total: u64,
    handle: Option<JoinHandle<()>>,
}

/// How the simulated download misbehaves.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Pause for this long once this many bytes are written.
    pub stall: Option<(u64, Duration)>,
    /// Stop writing at this many bytes (no rename).
    pub stop_at: Option<u64>,
    /// After stopping, delete the `.part` (what a cancelled transfer does).
    pub delete_after_stop: bool,
}

impl Writer {
    pub fn start(src: &Path, dest: &Path, bytes_per_sec: u64) -> Self {
        Self::start_with(src, dest, bytes_per_sec, Plan::default())
    }

    pub fn start_with(src: &Path, dest: &Path, bytes_per_sec: u64, plan: Plan) -> Self {
        let data = std::fs::read(src).unwrap();
        let total = data.len() as u64;
        let part = PathBuf::from(format!("{}.part", dest.display()));
        let dest = dest.to_path_buf();
        let written = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let (w, d) = (written.clone(), done.clone());
        let started = Instant::now();
        let handle = std::thread::spawn(move || {
            const CHUNK: usize = 4096;
            let mut f = File::create(&part).unwrap();
            let mut paused = Duration::ZERO;
            let mut stalled = false;
            for (i, chunk) in data.chunks(CHUNK).enumerate() {
                let off = (i * CHUNK) as u64;
                if plan.stop_at.is_some_and(|s| off >= s) {
                    drop(f);
                    if plan.delete_after_stop {
                        std::fs::remove_file(&part).unwrap();
                    }
                    d.store(true, Ordering::SeqCst);
                    return;
                }
                if let Some((at, dur)) = plan.stall
                    && !stalled
                    && off >= at
                {
                    stalled = true;
                    std::thread::sleep(dur);
                    paused += dur;
                }
                // Pace against wall clock so the average rate is exact.
                let due = paused + Duration::from_secs_f64(off as f64 / bytes_per_sec as f64);
                if let Some(wait) = due.checked_sub(started.elapsed()) {
                    std::thread::sleep(wait);
                }
                f.write_all(chunk).unwrap();
                f.flush().unwrap();
                w.fetch_add(chunk.len() as u64, Ordering::SeqCst);
            }
            drop(f);
            std::fs::rename(&part, &dest).unwrap();
            d.store(true, Ordering::SeqCst);
        });
        Self {
            written,
            done,
            started,
            total,
            handle: Some(handle),
        }
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::SeqCst)
    }

    pub fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
    }
}

/// 16-bit PCM WAV.
pub fn write_wav(path: &Path, samples: &[f32], rate: u32, channels: usize) {
    let mut b = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    let ch = channels as u16;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&ch.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(ch) * 2).to_le_bytes());
    b.extend_from_slice(&(ch * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b).unwrap();
}

pub fn proof_dir() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proof/cp0/audio");
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

pub fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// Collect events until `stop` returns true for one (inclusive) or the timeout passes.
pub fn collect_until(
    rx: &std::sync::mpsc::Receiver<wholeseek_audio::PlayerEvent>,
    timeout: Duration,
    mut stop: impl FnMut(&wholeseek_audio::PlayerEvent) -> bool,
) -> (Vec<wholeseek_audio::PlayerEvent>, bool) {
    let end = Instant::now() + timeout;
    let mut out = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) => {
                let done = stop(&ev);
                out.push(ev);
                if done {
                    return (out, true);
                }
            }
            Err(_) => break,
        }
    }
    (out, false)
}

/// Compact event trace for logs (ticks summarised).
pub fn trace(events: &[wholeseek_audio::PlayerEvent]) -> String {
    use wholeseek_audio::PlayerEvent as E;
    let mut parts: Vec<String> = Vec::new();
    let mut ticks = 0;
    for e in events {
        if let E::PositionTick { .. } = e {
            ticks += 1;
            continue;
        }
        if ticks > 0 {
            parts.push(format!("Tick×{ticks}"));
            ticks = 0;
        }
        parts.push(match e {
            E::Started { duration_secs, .. } => format!("Started(dur={duration_secs:?})"),
            E::Seeked { position_secs } => format!("Seeked({position_secs:.2})"),
            E::Notice(n) => format!("Notice({n:?})"),
            E::Error(m) => format!("Error({m:?})"),
            other => format!("{other:?}"),
        });
    }
    if ticks > 0 {
        parts.push(format!("Tick×{ticks}"));
    }
    parts.join(" → ")
}
