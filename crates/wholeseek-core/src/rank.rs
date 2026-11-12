//! Audio-only filter and availability ordering for search results (no format
//! preference: users pick formats with filters), plus the conversion from the
//! library's result type.

use crate::model::{AudioAttrs, RemoteFile, SearchHit};

/// Extensions treated as audio. Order is irrelevant; ranking never looks at the format.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "opus", "m4a", "aac", "wav", "aif", "aiff", "alac", "ape", "wv",
];

/// Lower-case extension of a remote path without the dot, empty if none.
#[must_use]
pub fn extension(path: &str) -> String {
    let base = crate::paths::remote_basename(path);
    base.rfind('.')
        .filter(|&i| i > 0)
        .map(|i| base[i + 1..].to_ascii_lowercase())
        .unwrap_or_default()
}

#[must_use]
pub fn is_audio(path: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(path).as_str())
}

/// Flatten library search responses into hits (one per file).
#[must_use]
pub fn hits_from_results(results: &[soulseek_rs::SearchResult]) -> Vec<SearchHit> {
    results
        .iter()
        .flat_map(|r| {
            r.files.iter().map(move |f| SearchHit {
                file: RemoteFile {
                    username: f.username.clone(),
                    path: f.name.clone(),
                    size: f.size,
                    attrs: AudioAttrs::from_pairs(f.attribs.iter().map(|(k, v)| (*k, *v))),
                },
                free_slot: r.slots > 0,
                speed: r.speed,
            })
        })
        .collect()
}

/// Optionally keep only audio, then order by availability: free slot first,
/// then faster peer; ties keep their original (arrival) order. The format is
/// never preferred. With `audio_only` off, non-audio files sort after audio.
///
/// Peers' queue lengths are not part of a search response (the library
/// reports only `slots` and `speed`), so "shorter queue" can't rank here.
pub fn rank(hits: &mut Vec<SearchHit>, audio_only: bool) {
    if audio_only {
        hits.retain(|h| is_audio(&h.file.path));
    }
    // Stable sort: equal keys keep arrival order.
    hits.sort_by(|a, b| {
        is_audio(&b.file.path)
            .cmp(&is_audio(&a.file.path))
            .then(b.free_slot.cmp(&a.free_slot))
            .then(b.speed.cmp(&a.speed))
    });
}

/// Track title of a remote path for matching copies across users: the
/// basename without extension, lower-cased, leading track/disc numbers
/// (`01 - `, `1-02.`, `03_`) removed, then only letters and digits kept.
/// A name that is nothing but a number keeps it (`1999.flac` → `1999`).
#[must_use]
pub fn normalized_title(path: &str) -> String {
    let base = crate::paths::remote_basename(path);
    let stem = base
        .rfind('.')
        .filter(|&i| i > 0)
        .map_or(base, |i| &base[..i]);
    let lower = stem.to_lowercase();
    let stripped = lower.trim_start_matches(|c: char| {
        c.is_ascii_digit()
            || c.is_whitespace()
            || matches!(c, '-' | '.' | '_' | '(' | ')' | '[' | ']')
    });
    let keep = if stripped.chars().any(char::is_alphanumeric) {
        stripped
    } else {
        lower.as_str()
    };
    keep.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn same_title(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    // "Artist - Title" vs "Title": the longer name ends with the shorter one,
    // which must be long enough not to match everything ("intro").
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    short.chars().count() >= 6 && long.ends_with(short)
}

fn size_within_1pct(a: u64, b: u64) -> bool {
    a.abs_diff(b).saturating_mul(100) <= a.max(b)
}

/// Other users' copies of `file` among `hits`, best first, for ▶ fallback.
///
/// A hit matches when its [`normalized_title`] equals `file`'s (or one ends
/// with the other, at least 6 characters) **and** either the size is within
/// 1 % or both durations are known and equal (a different encoding of the
/// same track). `file.username` is excluded; one entry per user (its best).
/// Order: free slot first, then same format as `file`, then faster peer.
#[must_use]
pub fn alternates(file: &RemoteFile, hits: &[SearchHit]) -> Vec<RemoteFile> {
    let title = normalized_title(&file.path);
    let ext = extension(&file.path);
    let mut found: Vec<&SearchHit> = hits
        .iter()
        .filter(|h| h.file.username != file.username)
        .filter(|h| {
            let same_duration = matches!(
                (file.attrs.duration_secs, h.file.attrs.duration_secs),
                (Some(a), Some(b)) if a == b
            );
            (size_within_1pct(file.size, h.file.size) || same_duration)
                && same_title(&title, &normalized_title(&h.file.path))
        })
        .collect();
    found.sort_by(|a, b| {
        b.free_slot
            .cmp(&a.free_slot)
            .then_with(|| (extension(&b.file.path) == ext).cmp(&(extension(&a.file.path) == ext)))
            .then(b.speed.cmp(&a.speed))
            .then_with(|| a.file.username.cmp(&b.file.username))
            .then_with(|| a.file.path.cmp(&b.file.path))
    });
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|h| seen.insert(h.file.username.clone()))
        .map(|h| h.file.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn hit(path: &str, free: bool, speed: u32) -> SearchHit {
        SearchHit {
            file: RemoteFile {
                username: "u".into(),
                path: path.into(),
                size: 1,
                attrs: AudioAttrs::default(),
            },
            free_slot: free,
            speed,
        }
    }

    #[test]
    fn extension_is_case_insensitive_and_ignores_dotfiles() {
        assert_eq!(extension(r"a\B.FLAC"), "flac");
        assert_eq!(extension(r"a\.hidden"), "");
        assert_eq!(extension(r"a.dir\noext"), "");
    }

    fn order(hits: &[SearchHit]) -> Vec<&str> {
        hits.iter().map(|h| h.file.path.as_str()).collect()
    }

    #[test]
    fn ranking_is_by_availability_not_format_and_filters_non_audio() {
        let mut hits = vec![
            hit(r"x\a.flac", false, 9_000),
            hit(r"x\cover.jpg", true, 9),
            hit(r"x\b.mp3", true, 100),
            hit(r"x\c.wav", true, 500),
            hit(r"x\d.ogg", false, 50),
        ];
        rank(&mut hits, true);
        // Free slot first, then speed; FLAC gets no bonus.
        assert_eq!(
            order(&hits),
            [r"x\c.wav", r"x\b.mp3", r"x\a.flac", r"x\d.ogg"]
        );
    }

    #[test]
    fn same_availability_keeps_arrival_order_whatever_the_format() {
        let paths = [
            r"x\1.mp3",
            r"x\2.flac",
            r"x\3.wav",
            r"x\4.opus",
            r"x\5.FLAC",
        ];
        let mut hits: Vec<_> = paths.iter().map(|p| hit(p, true, 100)).collect();
        rank(&mut hits, true);
        assert_eq!(order(&hits), paths);
        // Reversed arrival gives the reversed result: nothing reorders by format.
        let mut hits: Vec<_> = paths.iter().rev().map(|p| hit(p, true, 100)).collect();
        rank(&mut hits, true);
        let reversed: Vec<&str> = paths.iter().rev().copied().collect();
        assert_eq!(order(&hits), reversed);
    }

    #[test]
    fn audio_only_off_keeps_everything_with_non_audio_last() {
        let mut hits = vec![hit(r"x\cover.jpg", true, 9_999), hit(r"x\a.mp3", false, 1)];
        rank(&mut hits, false);
        assert_eq!(order(&hits), [r"x\a.mp3", r"x\cover.jpg"]);
    }

    #[test]
    fn converts_library_results_with_attributes() {
        let mut attribs = HashMap::new();
        attribs.insert(1, 250);
        attribs.insert(4, 44_100);
        attribs.insert(5, 16);
        let r = soulseek_rs::SearchResult {
            token: 1,
            files: vec![soulseek_rs::File {
                username: "alice".into(),
                name: r"Music\Album\01.flac".into(),
                size: 28_000_000,
                attribs,
            }],
            slots: 1,
            speed: 500_000,
            username: "alice".into(),
        };
        let hits = hits_from_results(&[r]);
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert!(h.free_slot);
        assert_eq!(h.file.attrs.duration_secs, Some(250));
        assert_eq!(h.file.attrs.sample_rate_hz, Some(44_100));
        assert_eq!(h.file.attrs.bit_depth, Some(16));
        assert_eq!(h.file.basename(), "01.flac");
        assert_eq!(h.file.folder(), r"Music\Album");
    }

    fn alt(
        user: &str,
        path: &str,
        size: u64,
        dur: Option<u32>,
        free: bool,
        speed: u32,
    ) -> SearchHit {
        SearchHit {
            file: RemoteFile {
                username: user.into(),
                path: path.into(),
                size,
                attrs: AudioAttrs {
                    duration_secs: dur,
                    ..AudioAttrs::default()
                },
            },
            free_slot: free,
            speed,
        }
    }

    #[test]
    fn normalized_title_strips_numbers_case_and_punctuation() {
        assert_eq!(normalized_title(r"a\01 - Xtal.flac"), "xtal");
        assert_eq!(normalized_title(r"a\1-01. Xtal.FLAC"), "xtal");
        assert_eq!(normalized_title("b/03_Xtal!.mp3"), "xtal");
        assert_eq!(
            normalized_title(r"a\(07) We Are the Music Makers.flac"),
            "wearethemusicmakers"
        );
        assert_eq!(normalized_title(r"a\1999.flac"), "1999");
        assert_eq!(normalized_title(r"a\Ágeispolis.flac"), "ágeispolis");
    }

    #[test]
    fn alternates_match_same_track_from_other_users_ranked() {
        let file = alt(
            "orig",
            r"Music\SAW\01 Xtal.flac",
            1_000_000,
            Some(294),
            true,
            9,
        )
        .file;
        let hits = vec![
            alt("orig", r"other\01 Xtal.flac", 1_000_000, None, true, 99), // same user
            alt("slow", r"x\01 - xtal.flac", 1_004_000, None, true, 10),   // size within 1%
            alt("fast", r"y\Xtal.flac", 1_000_000, None, true, 500),
            alt("busy", r"z\1-01 Xtal.flac", 1_000_000, None, false, 9_999), // no free slot
            alt("mp3", r"m\01 Xtal.mp3", 300_000, Some(294), true, 800),     // same duration
            alt("fast", r"y2\Xtal (copy).flac", 1_000_000, None, true, 900), // other title
            alt("fast", r"y3\01 Xtal.flac", 999_000, None, true, 400),       // 2nd copy, same user
        ];
        let users: Vec<_> = alternates(&file, &hits)
            .into_iter()
            .map(|f| f.username)
            .collect();
        assert_eq!(users, ["fast", "slow", "mp3", "busy"]);
    }

    #[test]
    fn alternates_reject_different_tracks() {
        let file = alt("orig", r"a\01 Intro.flac", 1_000_000, Some(60), true, 1).file;
        let hits = vec![
            alt("a", r"x\01 Intro.flac", 1_020_000, None, true, 1), // 2% off, no duration
            alt("b", r"x\01 Intro.flac", 1_020_000, Some(61), true, 1), // duration differs
            alt("c", r"x\02 Outro.flac", 1_000_000, Some(60), true, 1), // other title
            alt("d", r"x\Album Intro.flac", 1_000_000, Some(60), true, 1), // "intro" too short to suffix-match
            alt("e", r"x\cover.jpg", 1_000_000, None, true, 1),
        ];
        assert!(alternates(&file, &hits).is_empty());
    }

    #[test]
    fn alternates_accept_artist_prefixed_names_for_long_titles() {
        let file = alt("orig", r"a\04 Ageispolis.flac", 34_909_356, None, true, 1).file;
        let hits = vec![alt(
            "p",
            r"x\Aphex Twin - Ageispolis.flac",
            34_909_356,
            None,
            true,
            1,
        )];
        assert_eq!(alternates(&file, &hits).len(), 1);
    }
}
