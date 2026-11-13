//! Tags and stream facts of a local audio file, for the Library scanner
//! (`wholeseek_core::library`, which takes this as its tag reader through the app).
//!
//! Probes the container only: no packet is decoded, so a file costs its header read. Reads
//! with a plain `File`; never blocks on a growing download.

use std::fs::File;
use std::path::Path;

use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTag};

/// What [`read_tags`] found. Mirrors `wholeseek_core::library::Tags` field for field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: Option<u32>,
    pub codec: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
}

/// The tags and stream facts of the file at `path`; `None` when it can't be opened or
/// parsed as audio. Blocking file I/O: call off the UI thread.
#[must_use]
pub fn read_tags(path: &Path) -> Option<TrackTags> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    // A malformed file can trip a demuxer bug; that is "unreadable", never a crash.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .ok()?;
        let mut out = TrackTags::default();
        if let Some(track) = format.default_track(TrackType::Audio) {
            let params = track.codec_params.as_ref().and_then(|p| p.audio());
            let rate = params.and_then(|p| p.sample_rate);
            out.sample_rate_hz = rate;
            out.bit_depth = params.and_then(|p| p.bits_per_sample);
            out.codec = params.and_then(|p| {
                symphonia::default::get_codecs()
                    .get_audio_decoder(p.codec)
                    .map(|d| codec_name(d.codec.info.short_name))
            });
            let secs = match (track.duration, track.time_base, track.num_frames) {
                (Some(d), Some(tb), _) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
                (_, _, Some(n)) => rate.filter(|&r| r > 0).map(|r| n as f64 / f64::from(r)),
                _ => None,
            };
            out.duration_secs = secs
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| s.round() as u32);
        }
        let mut revisions: Vec<MetadataRevision> = Vec::new();
        let mut meta = format.metadata();
        while let Some(r) = meta.pop() {
            revisions.push(r);
        }
        if let Some(r) = meta.current() {
            revisions.push(r.clone());
        }
        for r in &revisions {
            let tags = r
                .media
                .tags
                .iter()
                .chain(r.per_track.iter().flat_map(|t| t.metadata.tags.iter()));
            for tag in tags {
                if let Some(std) = &tag.std {
                    apply(&mut out, std);
                }
            }
        }
        Some(out)
    }))
    .ok()
    .flatten()
}

/// The first value of each field wins (media-level before per-track, older revision first).
fn apply(out: &mut TrackTags, tag: &StandardTag) {
    fn set(slot: &mut Option<String>, v: &str) {
        let v = v.trim();
        if slot.is_none() && !v.is_empty() {
            *slot = Some(v.to_string());
        }
    }
    fn num(slot: &mut Option<u32>, v: u64) {
        if slot.is_none() && v > 0 {
            *slot = u32::try_from(v).ok();
        }
    }
    match tag {
        StandardTag::TrackTitle(v) => set(&mut out.title, v),
        StandardTag::Artist(v) => set(&mut out.artist, v),
        StandardTag::AlbumArtist(v) => set(&mut out.album_artist, v),
        StandardTag::Album(v) => set(&mut out.album, v),
        StandardTag::TrackNumber(n) => num(&mut out.track, *n),
        StandardTag::DiscNumber(n) => num(&mut out.disc, *n),
        StandardTag::ReleaseYear(y) | StandardTag::RecordingYear(y) => {
            num(&mut out.year, u64::from(*y));
        }
        StandardTag::ReleaseDate(v)
        | StandardTag::RecordingDate(v)
        | StandardTag::OriginalReleaseDate(v)
            if out.year.is_none() =>
        {
            out.year = year_in(v);
        }
        _ => {}
    }
}

/// The first 4-digit year (1000–2999) in a date string ("2019-05-01", "2019").
fn year_in(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &b[i..i + 4];
        (w.iter().all(u8::is_ascii_digit) && matches!(w[0], b'1' | b'2'))
            .then(|| s[i..i + 4].parse().ok())
            .flatten()
    })
}

/// Display name of a symphonia codec short name.
fn codec_name(short: &str) -> String {
    match short {
        "flac" => "FLAC".into(),
        "mp3" => "MP3".into(),
        "mp2" => "MP2".into(),
        "mp1" => "MP1".into(),
        "aac" => "AAC".into(),
        "alac" => "ALAC".into(),
        "vorbis" => "Vorbis".into(),
        s if s.starts_with("pcm") => "PCM".into(),
        s if s.contains("adpcm") => "ADPCM".into(),
        s => s.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_from_dates() {
        assert_eq!(year_in("2019-05-01"), Some(2019));
        assert_eq!(year_in("1999"), Some(1999));
        assert_eq!(year_in("May 1987"), Some(1987));
        assert_eq!(year_in("n/a"), None);
    }

    #[test]
    fn unreadable_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.flac");
        std::fs::write(&p, b"not audio at all").unwrap();
        assert_eq!(read_tags(&p), None);
        assert_eq!(read_tags(&tmp.path().join("missing.flac")), None);
    }
}
