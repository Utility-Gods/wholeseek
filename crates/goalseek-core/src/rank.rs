//! Audio-only filter and "FLAC first" ordering for search results, plus the
//! conversion from the library's result type.

use crate::model::{AudioAttrs, RemoteFile, SearchHit};

/// Extensions treated as audio. Order is irrelevant; ranking is [`format_rank`].
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

/// Lower is better: FLAC, then MP3, then other audio, then anything else.
#[must_use]
pub fn format_rank(path: &str) -> u8 {
    match extension(path).as_str() {
        "flac" => 0,
        "mp3" => 1,
        e if AUDIO_EXTENSIONS.contains(&e) => 2,
        _ => 3,
    }
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

/// Apply the MVP's default view: optionally keep only audio, then sort
/// FLAC first, then MP3, then other audio; within a format, free slot first,
/// then faster peers, then path for a stable order.
pub fn rank(hits: &mut Vec<SearchHit>, audio_only: bool) {
    if audio_only {
        hits.retain(|h| is_audio(&h.file.path));
    }
    hits.sort_by(|a, b| {
        format_rank(&a.file.path)
            .cmp(&format_rank(&b.file.path))
            .then(b.free_slot.cmp(&a.free_slot))
            .then(b.speed.cmp(&a.speed))
            .then_with(|| a.file.username.cmp(&b.file.username))
            .then_with(|| a.file.path.cmp(&b.file.path))
    });
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

    #[test]
    fn flac_first_then_mp3_then_other_audio_and_filters_non_audio() {
        let mut hits = vec![
            hit(r"x\a.mp3", true, 9),
            hit(r"x\cover.jpg", true, 9),
            hit(r"x\b.ogg", true, 9),
            hit(r"x\c.flac", false, 1),
            hit(r"x\d.FLAC", true, 1),
        ];
        rank(&mut hits, true);
        let order: Vec<_> = hits.iter().map(|h| h.file.path.as_str()).collect();
        assert_eq!(order, [r"x\d.FLAC", r"x\c.flac", r"x\a.mp3", r"x\b.ogg"]);
    }

    #[test]
    fn audio_only_off_keeps_everything_last() {
        let mut hits = vec![hit(r"x\cover.jpg", true, 9), hit(r"x\a.mp3", true, 9)];
        rank(&mut hits, false);
        assert_eq!(hits[1].file.path, r"x\cover.jpg");
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
}
