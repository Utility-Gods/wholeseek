//! The player strip's engine: the real [`wholeseek_audio::Player`] or the demo's simulated one.

use std::path::Path;

use std::sync::mpsc::Receiver;

use wholeseek_audio::{Output, Player, PlayerEvent, PlayerSnapshot, QueueItem, SeekError};

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
    /// Replace the queue and play `items[start]` (see `wholeseek_audio::Player::set_queue`).
    fn set_queue(&mut self, items: Vec<QueueItem>, start: usize);
    /// Where queue item `index` is being fetched; the player starts it if it was waiting.
    fn set_queue_path(&mut self, index: usize, path: &Path, size: u64) -> bool;
    fn next(&mut self);
    /// Restart the track after 3 s, else the previous queue item.
    fn previous(&mut self);
    fn queue_index(&self) -> Option<usize>;
    /// Events since the last call (track changes, prefetch requests, queue end).
    fn events(&mut self) -> Vec<PlayerEvent>;
    fn set_waker(&mut self, _wake: rune::Waker) {}
}

/// The system output device.
pub struct RealPlayer(Player, Receiver<PlayerEvent>);

impl RealPlayer {
    pub fn new() -> Self {
        let player = Player::new(Output::Device);
        let events = player.subscribe();
        Self(player, events)
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
    fn set_queue(&mut self, items: Vec<QueueItem>, start: usize) {
        self.0.set_queue(items, start);
    }
    fn set_queue_path(&mut self, index: usize, path: &Path, size: u64) -> bool {
        self.0.set_queue_path(index, path, size)
    }
    fn next(&mut self) {
        self.0.next();
    }
    fn previous(&mut self) {
        self.0.previous();
    }
    fn queue_index(&self) -> Option<usize> {
        self.0.queue_index()
    }
    fn events(&mut self) -> Vec<PlayerEvent> {
        self.1.try_iter().collect()
    }
    fn set_waker(&mut self, wake: rune::Waker) {
        self.0.set_waker(move || wake());
    }
}
