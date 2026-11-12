//! wholeseek-audio: play a file that is still being downloaded.
//!
//! - [`GrowingFile`]: blocking `Read + Seek + MediaSource` over a growing `.part` file.
//! - [`Player`]: symphonia decode thread → [`AudioSink`] (cpal device with feature `output`,
//!   or a capture/null sink), with an optional play queue ([`QueueItem`]) for albums.

pub mod growing;
#[cfg(feature = "output")]
pub mod output;
pub mod player;
pub mod sink;

pub use growing::{
    GrowingError, GrowingFile, GrowingHandle, PART_SUFFIX, SourceFailed, UNKNOWN_SIZE,
};
#[cfg(feature = "output")]
pub use output::CpalSink;
pub use player::{
    DEFAULT_STALL_THRESHOLD, Output, PREFETCH_REMAINING_SECS, PREVIOUS_RESTART_SECS, PlayState,
    Player, PlayerEvent, PlayerSnapshot, QueueItem, SeekError, SinkFactory,
};
pub use sink::{AudioSink, CaptureData, CaptureSink, NullSink};
