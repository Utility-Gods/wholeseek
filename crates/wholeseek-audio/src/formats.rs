//! Which files the player can decode (decides where the UI offers ▶).

/// Lower-case extensions (no dot) of files the enabled symphonia features decode:
///
/// | extension | container → codecs |
/// |---|---|
/// | `flac` | FLAC |
/// | `mp3`, `mp2` | MPEG audio layer III / II |
/// | `ogg`, `oga` | Ogg → Vorbis, FLAC (not Opus) |
/// | `m4a`, `m4b`, `mp4` | MP4 → AAC-LC, ALAC |
/// | `aac` | ADTS → AAC-LC |
/// | `wav` | RIFF WAVE → PCM, IEEE float, MS/IMA ADPCM |
/// | `aif`, `aiff` | AIFF → PCM |
/// | `caf` | Core Audio Format → PCM, ALAC |
/// | `mka` | Matroska audio → FLAC, Vorbis, AAC, PCM (not Opus) |
///
/// Not playable: Opus (`.opus`; symphonia 0.6 has no Opus decoder), WMA, APE (Monkey's
/// Audio), WavPack, DSD. HE-AAC (SBR) plays at its core rate only.
pub const PLAYABLE_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "mp2", "ogg", "oga", "m4a", "m4b", "mp4", "aac", "wav", "aif", "aiff", "caf",
    "mka",
];

/// Whether `path` has a [playable extension](PLAYABLE_EXTENSIONS). Case-insensitive; accepts
/// Soulseek paths (`\` separators) as well as local ones, and the `.part` name of a download.
pub fn is_playable(path: &str) -> bool {
    let path = path.strip_suffix(crate::PART_SUFFIX).unwrap_or(path);
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => PLAYABLE_EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playable_paths() {
        for p in [
            "x\\a.FLAC",
            "@@abcde\\Music\\Artist\\01 - Song.mp3",
            "/home/u/.cache/wholeseek/t1/01.flac.part",
            "Album (2001)\\02. Track.M4A",
            "a.ogg",
            "b.oga",
            "c.aiff",
            "c.AIF",
            "d.caf",
            "e.mka",
            "f.aac",
            "g.wav",
            "h.mp2",
            "i.m4b",
            "j.mp4",
        ] {
            assert!(is_playable(p), "{p}");
        }
        for p in [
            "cover.JPG",
            "a.opus",
            "x\\folder.flac\\notes.txt",
            "x\\flac",
            "x\\.flac",
            "",
            "track.flac.cue",
            "track.wma",
            "track.ape",
            "track.wv",
            "track.dsf",
            "playlist.m3u",
            "x.part",
        ] {
            assert!(!is_playable(p), "{p}");
        }
    }

    #[test]
    fn extensions_are_lower_case_without_dot() {
        for e in PLAYABLE_EXTENSIONS {
            assert!(!e.contains('.') && *e == e.to_ascii_lowercase(), "{e}");
        }
    }
}
