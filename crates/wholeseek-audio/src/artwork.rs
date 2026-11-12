//! Cover art embedded in an audio file: FLAC `PICTURE`, ID3v2 `APIC`, MP4 `covr` (and anything
//! else symphonia exposes as a [`Visual`](symphonia::core::meta::Visual)).
//!
//! Reads the file with a plain `File` (never a [`crate::GrowingFile`]), so it never blocks: a
//! `.part` whose header is not complete yet is [`ArtLookup::Incomplete`], to be retried later.

use std::fs::File;
use std::path::Path;

use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardVisualKey, Visual};

/// Pictures larger than this are ignored (a cover is a few hundred KB at most).
pub const MAX_ART_BYTES: usize = 16 * 1024 * 1024;

/// An embedded picture, still encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub data: Vec<u8>,
    /// MIME type as the tag states it (`image/jpeg`), if any.
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtLookup {
    /// The best picture: the front cover, else the first one.
    Found(Artwork),
    /// The file's metadata was read and holds no picture.
    None,
    /// The file could not be read or parsed (yet): a partial download, or not audio.
    Incomplete,
}

/// The cover embedded in the file at `path`. Blocking file I/O: call off the UI thread.
#[must_use]
pub fn embedded_art(path: &Path) -> ArtLookup {
    let Ok(file) = File::open(path) else {
        return ArtLookup::Incomplete;
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    let name = path.to_string_lossy();
    let name = name.strip_suffix(crate::PART_SUFFIX).unwrap_or(&name);
    if let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    // A malformed file can trip a demuxer bug; that is "no art", never a crash.
    let probed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .ok()?;
        let mut revisions = Vec::new();
        let mut meta = format.metadata();
        while let Some(r) = meta.pop() {
            revisions.push(r);
        }
        if let Some(r) = meta.current() {
            revisions.push(r.clone());
        }
        Some(pick(&revisions))
    }));
    match probed {
        Ok(Some(Some(art))) => ArtLookup::Found(art),
        Ok(Some(None)) => ArtLookup::None,
        Ok(None) | Err(_) => ArtLookup::Incomplete,
    }
}

/// Front cover first, then any picture; media-level before per-track.
fn pick(revisions: &[MetadataRevision]) -> Option<Artwork> {
    let visuals: Vec<&Visual> = revisions
        .iter()
        .flat_map(|r| {
            r.media
                .visuals
                .iter()
                .chain(r.per_track.iter().flat_map(|t| t.metadata.visuals.iter()))
        })
        .filter(|v| !v.data.is_empty() && v.data.len() <= MAX_ART_BYTES)
        .collect();
    let best = visuals
        .iter()
        .find(|v| v.usage == Some(StandardVisualKey::FrontCover))
        .or_else(|| visuals.first())?;
    Some(Artwork {
        data: best.data.to_vec(),
        media_type: best.media_type.clone(),
    })
}
