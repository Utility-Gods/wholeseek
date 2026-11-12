//! Audio sinks: where decoded interleaved `f32` samples go.

use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A destination for decoded audio. Created and used only on the decode thread.
pub trait AudioSink {
    /// Prepare for a stream with the given format. Called once before the first `write`.
    fn start(&mut self, sample_rate: u32, channels: usize) -> Result<(), String>;
    /// Queue interleaved samples. May block for back-pressure, but must return early (with the
    /// number of samples accepted so far, a multiple of the channel count) once `abort()` is true.
    fn write(&mut self, samples: &[f32], abort: &dyn Fn() -> bool) -> usize;
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    /// Drop any queued, not yet played audio (after a seek).
    fn flush(&mut self) {}
    /// Block until queued audio has played, or `abort()` is true (end of track).
    fn drain(&mut self, _abort: &dyn Fn() -> bool) {}
    /// Frames queued but not yet audible (for position reporting).
    fn queued_frames(&self) -> usize {
        0
    }
    /// A fatal output problem (device lost, output stopped consuming). The player stops with
    /// `PlayState::Error` carrying this message.
    fn error(&self) -> Option<String> {
        None
    }
}

/// Discards audio. Useful for headless runs.
#[derive(Debug, Default)]
pub struct NullSink;

impl AudioSink for NullSink {
    fn start(&mut self, _: u32, _: usize) -> Result<(), String> {
        Ok(())
    }
    fn write(&mut self, samples: &[f32], _: &dyn Fn() -> bool) -> usize {
        samples.len()
    }
}

/// What a [`CaptureSink`] has recorded.
#[derive(Debug, Default, Clone)]
pub struct CaptureData {
    pub sample_rate: u32,
    pub channels: usize,
    /// All interleaved samples written (volume already applied).
    pub samples: Vec<f32>,
    /// When the first non-empty write happened.
    pub first_write: Option<Instant>,
    /// Sample indices (into `samples`) at which a `flush` happened (i.e. a seek).
    pub flush_marks: Vec<usize>,
    /// Sample indices at which `start` was called (one per track played into this sink).
    pub start_marks: Vec<usize>,
}

/// Records every sample it receives; cloneable so a test can inspect it while the player owns it.
#[derive(Debug, Default, Clone)]
pub struct CaptureSink {
    data: Arc<Mutex<CaptureData>>,
}

impl CaptureSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of everything captured so far.
    pub fn data(&self) -> CaptureData {
        self.lock().clone()
    }

    pub fn sample_count(&self) -> usize {
        self.lock().samples.len()
    }

    pub fn first_write(&self) -> Option<Instant> {
        self.lock().first_write
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureData> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl AudioSink for CaptureSink {
    fn start(&mut self, sample_rate: u32, channels: usize) -> Result<(), String> {
        let mut d = self.lock();
        d.sample_rate = sample_rate;
        d.channels = channels;
        let n = d.samples.len();
        d.start_marks.push(n);
        Ok(())
    }

    fn write(&mut self, samples: &[f32], _: &dyn Fn() -> bool) -> usize {
        let mut d = self.lock();
        if d.first_write.is_none() && !samples.is_empty() {
            d.first_write = Some(Instant::now());
        }
        d.samples.extend_from_slice(samples);
        samples.len()
    }

    fn flush(&mut self) {
        let mut d = self.lock();
        let n = d.samples.len();
        d.flush_marks.push(n);
    }
}
