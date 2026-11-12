//! Embedded cover extraction for FLAC (PICTURE), MP3 (ID3v2 APIC) and M4A (covr). Fixtures are
//! tiny files generated with ffmpeg: a short tone plus an 8×8 PNG attached as the cover.

use std::path::{Path, PathBuf};
use std::process::Command;

use wholeseek_audio::{ArtLookup, embedded_art};

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg must be installed");
    assert!(status.success(), "ffmpeg {args:?} failed");
}

fn cover_png(dir: &Path) -> PathBuf {
    let png = dir.join("cover.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=0xfb923c:s=8x8",
        "-frames:v",
        "1",
        png.to_str().unwrap(),
    ]);
    png
}

/// A 1 s tone in `name`, with `cover` attached as its front cover when given.
fn fixture(dir: &Path, name: &str, codec: &[&str], cover: Option<&Path>) -> PathBuf {
    let out = dir.join(name);
    let mut args: Vec<&str> = vec!["-f", "lavfi", "-i", "sine=frequency=440:duration=1"];
    if let Some(c) = cover {
        args.extend(["-i", c.to_str().unwrap(), "-map", "0:a", "-map", "1:v"]);
        args.extend(["-c:v", "png", "-disposition:v", "attached_pic"]);
        args.extend(["-metadata:s:v", "comment=Cover (front)"]);
    }
    args.extend(codec);
    args.push(out.to_str().unwrap());
    ffmpeg(&args);
    out
}

const CASES: &[(&str, &[&str])] = &[
    ("a.flac", &["-c:a", "flac"]),
    ("a.mp3", &["-c:a", "libmp3lame", "-id3v2_version", "3"]),
    ("a.m4a", &["-c:a", "aac", "-movflags", "+faststart"]),
];

#[test]
fn finds_embedded_cover_in_flac_mp3_m4a() {
    let tmp = tempfile::tempdir().unwrap();
    let png = cover_png(tmp.path());
    let want = std::fs::read(&png).unwrap();
    for (name, codec) in CASES {
        let f = fixture(tmp.path(), name, codec, Some(&png));
        match embedded_art(&f) {
            ArtLookup::Found(a) => {
                assert_eq!(a.data, want, "{name}: picture bytes");
                assert!(
                    a.media_type.as_deref().is_none_or(|m| m.contains("png")),
                    "{name}: media type {:?}",
                    a.media_type
                );
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn file_without_cover_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, codec) in CASES {
        let f = fixture(tmp.path(), name, codec, None);
        assert_eq!(embedded_art(&f), ArtLookup::None, "{name}");
    }
}

#[test]
fn truncated_or_missing_file_is_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        embedded_art(&tmp.path().join("nope.flac")),
        ArtLookup::Incomplete
    );
    let png = cover_png(tmp.path());
    let f = fixture(tmp.path(), "b.flac", &["-c:a", "flac"], Some(&png));
    // Cut inside the PICTURE block (fLaC + 38-byte STREAMINFO + a few bytes).
    let bytes = std::fs::read(&f).unwrap();
    let part = tmp.path().join("b.flac.part");
    std::fs::write(&part, &bytes[..60]).unwrap();
    assert_eq!(embedded_art(&part), ArtLookup::Incomplete);
    std::fs::write(&part, &bytes).unwrap();
    assert!(matches!(embedded_art(&part), ArtLookup::Found(_)));
}
