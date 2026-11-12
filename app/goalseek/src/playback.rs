//! The player strip's engine: the real [`goalseek_audio::Player`] or the demo's simulated one.

use std::path::Path;

use goalseek_audio::{Output, Player, PlayerSnapshot, SeekError};

pub trait Playback {
    fn play(&mut self, path: &Path, total_size: u64);
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(&mut self);
    /// # Errors
    /// The target is not downloaded yet, or nothing is playing.
    fn seek(&mut self, secs: f64) -> Result<(), SeekError>;
    fn set_volume(&mut self, volume: f32);
    fn snapshot(&self) -> PlayerSnapshot;
    /// Advance simulated time (demo); the real player runs on its own thread.
    fn tick(&mut self, _now: f64) {}
}

/// The system output device.
pub struct RealPlayer(Player);

impl RealPlayer {
    pub fn new() -> Self {
        Self(Player::new(Output::Device))
    }
}

impl Playback for RealPlayer {
    fn play(&mut self, path: &Path, total_size: u64) {
        self.0.play(path, total_size);
    }
    fn pause(&mut self) {
        self.0.pause();
    }
    fn resume(&mut self) {
        self.0.resume();
    }
    fn stop(&mut self) {
        self.0.stop();
    }
    fn seek(&mut self, secs: f64) -> Result<(), SeekError> {
        self.0.seek(secs)
    }
    fn set_volume(&mut self, volume: f32) {
        self.0.set_volume(volume);
    }
    fn snapshot(&self) -> PlayerSnapshot {
        self.0.snapshot()
    }
}
