//! `read_tags` on real files made by ffmpeg, in every format the Library lists, and the core
//! scanner using it: tags and stream facts come through, untagged files fall back to names.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use wholeseek_audio::read_tags;
use wholeseek_core::library::{self, LibraryIndex, TagReader, Tags};

/// A 2 s tone at `path` (format from its extension), with `meta` tags.
fn make(path: &Path, meta: &[(&str, &str)], extra: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut c = Command::new("ffmpeg");
    c.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
    ])
    .arg("sine=frequency=440:sample_rate=44100:duration=2")
    .args(extra);
    for (k, v) in meta {
        c.arg("-metadata").arg(format!("{k}={v}"));
    }
    let ok = c.arg(path).status().expect("ffmpeg must be installed");
    assert!(ok.success(), "ffmpeg failed for {}", path.display());
}

const META: &[(&str, &str)] = &[
    ("title", "Salt Roads"),
    ("artist", "Hollow Lights"),
    ("album_artist", "Hollow Lights"),
    ("album", "Coastline"),
    ("track", "3/10"),
    ("disc", "1/2"),
    ("date", "2019"),
];

#[test]
fn tags_in_every_common_format() {
    let tmp = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[&str], &str, bool)] = &[
        ("flac", &["-sample_fmt", "s16"], "FLAC", true),
        ("mp3", &["-b:a", "192k"], "MP3", false),
        ("ogg", &["-c:a", "libvorbis"], "Vorbis", false),
        ("m4a", &["-c:a", "aac"], "AAC", false),
        ("wav", &[], "PCM", true),
    ];
    for &(ext, extra, codec, lossless) in cases {
        let p = tmp.path().join(format!("t.{ext}"));
        make(&p, META, extra);
        let t = read_tags(&p).unwrap_or_else(|| panic!("{ext}: unreadable"));
        assert_eq!(t.codec.as_deref(), Some(codec), "{ext}");
        assert_eq!(t.sample_rate_hz, Some(44_100), "{ext}");
        assert!(
            matches!(t.duration_secs, Some(2)),
            "{ext}: {:?}",
            t.duration_secs
        );
        if lossless {
            assert_eq!(t.bit_depth, Some(16), "{ext}");
        }
        if ext == "wav" {
            continue; // RIFF INFO: ffmpeg writes few fields
        }
        assert_eq!(t.title.as_deref(), Some("Salt Roads"), "{ext}");
        assert_eq!(t.artist.as_deref(), Some("Hollow Lights"), "{ext}");
        assert_eq!(t.album.as_deref(), Some("Coastline"), "{ext}");
        assert_eq!(t.track, Some(3), "{ext}");
        assert_eq!(t.year, Some(2019), "{ext}");
    }
}

fn reader() -> TagReader {
    Arc::new(|p: &Path| {
        read_tags(p).map(|t| Tags {
            title: t.title,
            artist: t.artist,
            album_artist: t.album_artist,
            album: t.album,
            track: t.track,
            disc: t.disc,
            year: t.year,
            duration_secs: t.duration_secs,
            codec: t.codec,
            bit_depth: t.bit_depth,
            sample_rate_hz: t.sample_rate_hz,
        })
    })
}

#[test]
fn scanner_with_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("music");
    make(&root.join("Hollow Lights/Coastline/03.flac"), META, &[]);
    // No tags: names.
    make(
        &root.join("Northern Glass/Tideline (2021)/01 - Harbour.mp3"),
        &[],
        &["-b:a", "128k"],
    );
    let roots: Vec<PathBuf> = vec![root];
    let (entries, report) =
        library::scan(&roots, &[], &reader(), &mut |_| {}, &AtomicBool::new(false));
    assert_eq!((report.tracks, report.reread), (2, 2));
    let ix = LibraryIndex::build(&entries, &roots);
    let titles: Vec<(&str, &str, &str, Option<u32>)> = ix
        .tracks
        .iter()
        .map(|t| {
            (
                t.artist.as_str(),
                t.album.as_str(),
                t.title.as_str(),
                t.year,
            )
        })
        .collect();
    assert!(titles.contains(&("Hollow Lights", "Coastline", "Salt Roads", Some(2019))));
    assert!(titles.contains(&("Northern Glass", "Tideline", "Harbour", Some(2021))));
    let mp3 = ix.tracks.iter().find(|t| t.format == "MP3").unwrap();
    assert!(!mp3.tagged && !mp3.lossless());
    assert!(
        mp3.bitrate_kbps.is_some_and(|b| (100..=160).contains(&b)),
        "{:?}",
        mp3.bitrate_kbps
    );
}
