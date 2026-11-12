//! Real audio output through cpal (feature `output`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::sink::AudioSink;

/// Ring buffer length in seconds of device audio (bounds seek/volume latency).
const RING_SECS: f32 = 0.2;

/// If the device consumes nothing for this long while we have audio queued (and are not
/// paused), the output is considered lost.
const STALL_LIMIT: Duration = Duration::from_secs(3);

/// State shared with the device callback.
#[derive(Default)]
struct DeviceState {
    paused: AtomicBool,
    /// Samples consumed by the callback (progress watchdog).
    consumed: AtomicU64,
    error: Mutex<Option<String>>,
}

impl DeviceState {
    fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn set_error(&self, e: String) {
        let mut g = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(e);
        }
    }
}

/// Errors after which the stream will not produce sound again.
fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
    )
}

struct Ring {
    buf: VecDeque<f32>,
    cap: usize,
}

/// Plays through the default cpal output device. Resamples (linear) and remaps channels if the
/// device cannot run at the source format.
pub struct CpalSink {
    stream: Option<cpal::Stream>,
    ring: Arc<Mutex<Ring>>,
    dev: Arc<DeviceState>,
    src_channels: usize,
    out_channels: usize,
    resampler: Option<Resampler>,
    scratch: Vec<f32>,
    pending: Vec<f32>,
}

impl CpalSink {
    pub fn new() -> Self {
        Self {
            stream: None,
            ring: Arc::new(Mutex::new(Ring {
                buf: VecDeque::new(),
                cap: 0,
            })),
            dev: Arc::new(DeviceState::default()),
            src_channels: 0,
            out_channels: 0,
            resampler: None,
            scratch: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue device-format samples with back-pressure. On abort the unqueued tail is kept in
    /// `pending` (so no audio is lost or duplicated) and `false` is returned.
    fn push(&mut self, data: &[f32], abort: &dyn Fn() -> bool) -> bool {
        let mut off = 0;
        let mut watch = Watchdog::new(&self.dev);
        while off < data.len() {
            if self.dev.error().is_some() || watch.stalled(&self.dev) {
                return false;
            }
            {
                let mut r = self.lock();
                let room = r.cap.saturating_sub(r.buf.len());
                let n = room.min(data.len() - off);
                r.buf.extend(&data[off..off + n]);
                off += n;
            }
            if off < data.len() {
                if abort() {
                    self.pending.extend_from_slice(&data[off..]);
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        true
    }
}

impl Default for CpalSink {
    fn default() -> Self {
        Self::new()
    }
}

fn pick_config(
    device: &cpal::Device,
    rate: u32,
    channels: usize,
) -> Result<(StreamConfig, SampleFormat), String> {
    let supported = [
        SampleFormat::F32,
        SampleFormat::I16,
        SampleFormat::I32,
        SampleFormat::U16,
    ];
    if let Ok(configs) = device.supported_output_configs() {
        let mut best = None;
        for c in configs {
            if usize::from(c.channels()) != channels
                || !supported.contains(&c.sample_format())
                || rate < c.min_sample_rate()
                || rate > c.max_sample_rate()
            {
                continue;
            }
            let is_f32 = c.sample_format() == SampleFormat::F32;
            if best.is_none() || is_f32 {
                best = Some(c.with_sample_rate(rate));
            }
        }
        if let Some(c) = best {
            return Ok((c.config(), c.sample_format()));
        }
    }
    let c = device
        .default_output_config()
        .map_err(|e| format!("no output config: {e}"))?;
    if !supported.contains(&c.sample_format()) {
        return Err(format!("unsupported device format {:?}", c.sample_format()));
    }
    Ok((c.config(), c.sample_format()))
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    ring: Arc<Mutex<Ring>>,
    dev: Arc<DeviceState>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream::<T, _, _>(
            config,
            {
                let dev = dev.clone();
                move |data: &mut [T], _| {
                    let silent = T::from_sample(0.0f32);
                    // Count callback progress even when paused/silent: it proves the device runs.
                    dev.consumed.fetch_add(data.len() as u64, Ordering::Relaxed);
                    if dev.paused.load(Ordering::Relaxed) {
                        data.fill(silent);
                        return;
                    }
                    let mut r = ring.lock().unwrap_or_else(|e| e.into_inner());
                    for out in data.iter_mut() {
                        *out = match r.buf.pop_front() {
                            Some(s) => T::from_sample(s),
                            None => silent,
                        };
                    }
                }
            },
            move |e: cpal::Error| {
                if is_fatal(e.kind()) {
                    dev.set_error(format!("audio output lost: {e}"));
                }
            },
            None,
        )
        .map_err(|e| format!("cannot open output stream: {e}"))
}

impl AudioSink for CpalSink {
    fn start(&mut self, sample_rate: u32, channels: usize) -> Result<(), String> {
        // Not `cpal::default_host()`: it panics if the host cannot be initialised.
        #[cfg(target_os = "macos")]
        let host_id = cpal::HostId::CoreAudio;
        #[cfg(not(target_os = "macos"))]
        let host_id = cpal::HostId::Alsa;
        let host =
            cpal::host_from_id(host_id).map_err(|e| format!("audio system unavailable: {e}"))?;
        let device = host
            .default_output_device()
            .ok_or("no audio output device found")?;
        let (config, format) = pick_config(&device, sample_rate, channels)?;
        let out_rate = config.sample_rate;
        self.out_channels = usize::from(config.channels);
        self.src_channels = channels;
        self.resampler =
            (out_rate != sample_rate).then(|| Resampler::new(sample_rate, out_rate, channels));
        {
            let mut r = self.lock();
            r.cap = ((out_rate as f32 * RING_SECS) as usize).max(1024) * self.out_channels;
            r.buf = VecDeque::with_capacity(r.cap);
        }
        let ring = self.ring.clone();
        let dev = self.dev.clone();
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, ring, dev),
            SampleFormat::I16 => build::<i16>(&device, config, ring, dev),
            SampleFormat::I32 => build::<i32>(&device, config, ring, dev),
            SampleFormat::U16 => build::<u16>(&device, config, ring, dev),
            other => Err(format!("unsupported device format {other:?}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("cannot start output: {e}"))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn write(&mut self, samples: &[f32], abort: &dyn Fn() -> bool) -> usize {
        // Device-format samples left over from an aborted write go first.
        let pending = std::mem::take(&mut self.pending);
        if !self.push(&pending, abort) {
            return 0;
        }
        let src_ch = self.src_channels.max(1);
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut accepted = 0;
        for chunk in samples.chunks(src_ch * 1024) {
            scratch.clear();
            let src = match &mut self.resampler {
                Some(rs) => rs.process(chunk),
                None => chunk.to_vec(),
            };
            map_channels(&src, src_ch, self.out_channels, &mut scratch);
            accepted += chunk.len();
            if !self.push(&scratch, abort) {
                break;
            }
        }
        self.scratch = scratch;
        accepted
    }

    fn pause(&mut self) {
        self.dev.paused.store(true, Ordering::SeqCst);
    }

    fn resume(&mut self) {
        self.dev.paused.store(false, Ordering::SeqCst);
    }

    fn flush(&mut self) {
        self.lock().buf.clear();
        self.pending.clear();
        if let Some(rs) = &mut self.resampler {
            rs.reset();
        }
    }

    fn drain(&mut self, abort: &dyn Fn() -> bool) {
        let mut watch = Watchdog::new(&self.dev);
        while !self.lock().buf.is_empty()
            && !abort()
            && self.dev.error().is_none()
            && !watch.stalled(&self.dev)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn error(&self) -> Option<String> {
        self.dev.error()
    }

    fn queued_frames(&self) -> usize {
        let frames_out = (self.lock().buf.len() + self.pending.len()) / self.out_channels.max(1);
        match &self.resampler {
            Some(rs) => (frames_out as f64 * rs.in_rate as f64 / rs.out_rate as f64) as usize,
            None => frames_out,
        }
    }
}

/// Detects a device that stopped calling back (e.g. unplugged without an error callback).
struct Watchdog {
    consumed: u64,
    since: Instant,
}

impl Watchdog {
    fn new(dev: &DeviceState) -> Self {
        Self {
            consumed: dev.consumed.load(Ordering::Relaxed),
            since: Instant::now(),
        }
    }

    fn stalled(&mut self, dev: &DeviceState) -> bool {
        let now = dev.consumed.load(Ordering::Relaxed);
        if now != self.consumed || dev.paused.load(Ordering::Relaxed) {
            self.consumed = now;
            self.since = Instant::now();
            return false;
        }
        if self.since.elapsed() >= STALL_LIMIT {
            dev.set_error(format!(
                "audio output stopped responding (no progress for {} s)",
                STALL_LIMIT.as_secs()
            ));
            return true;
        }
        false
    }
}

fn map_channels(src: &[f32], in_ch: usize, out_ch: usize, dst: &mut Vec<f32>) {
    if in_ch == out_ch {
        dst.extend_from_slice(src);
        return;
    }
    for frame in src.chunks_exact(in_ch) {
        if out_ch == 1 {
            dst.push(frame.iter().sum::<f32>() / in_ch as f32);
        } else {
            for c in 0..out_ch {
                dst.push(if in_ch == 1 {
                    frame[0]
                } else {
                    frame.get(c).copied().unwrap_or(0.0)
                });
            }
        }
    }
}

/// Streaming linear-interpolation resampler (adequate fallback when the device rate differs).
struct Resampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Fractional read position relative to `prev` (in input frames).
    t: f64,
    prev: Vec<f32>,
    has_prev: bool,
}

impl Resampler {
    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        Self {
            in_rate,
            out_rate,
            channels,
            t: 0.0,
            prev: vec![0.0; channels],
            has_prev: false,
        }
    }

    fn reset(&mut self) {
        self.t = 0.0;
        self.has_prev = false;
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + ch);
        for frame in input.chunks_exact(ch) {
            if !self.has_prev {
                self.prev.copy_from_slice(frame);
                self.has_prev = true;
                continue;
            }
            // Emit output samples falling between prev (t=0) and frame (t=1).
            while self.t < 1.0 {
                let a = self.t as f32;
                out.extend(self.prev.iter().zip(frame).map(|(p, f)| p + (f - p) * a));
                self.t += step;
            }
            self.t -= 1.0;
            self.prev.copy_from_slice(frame);
        }
        out
    }
}
