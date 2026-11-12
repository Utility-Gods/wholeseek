//! Every extension in `PLAYABLE_EXTENSIONS` decodes while the file is still growing, and the
//! result is identical to a decode of the complete file. Samples are generated with ffmpeg.
//! Log lines are prefixed with `[formats]`.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;
use wholeseek_audio::{CaptureSink, Output, PLAYABLE_EXTENSIONS, PlayState, Player, is_playable};

const SAMPLE_SECS: u32 = 4;
/// Fast enough to keep the suite short, slow enough that playback starts mid-download.
const RATE_BPS: u64 = 400 * 1024;

/// (extension, label, ffmpeg output args).
const CASES: &[(&str, &str, &[&str])] = &[
    ("flac", "flac", &["-c:a", "flac"]),
    ("mp3", "mp3", &["-c:a", "libmp3lame", "-b:a", "192k"]),
    ("mp2", "mp2", &["-c:a", "mp2", "-b:a", "192k"]),
    ("ogg", "vorbis", &["-c:a", "libvorbis", "-q:a", "5"]),
    ("ogg", "flac", &["-c:a", "flac", "-f", "ogg"]),
    ("oga", "flac", &["-c:a", "flac", "-f", "ogg"]),
    ("m4a", "aac (moov at end)", &["-c:a", "aac", "-b:a", "192k"]),
    (
        "m4a",
        "aac faststart",
        &["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"],
    ),
    ("m4a", "alac", &["-c:a", "alac"]),
    ("m4b", "aac", &["-c:a", "aac", "-b:a", "128k", "-f", "mp4"]),
    ("mp4", "aac", &["-c:a", "aac", "-b:a", "192k"]),
    (
        "aac",
        "adts",
        &["-c:a", "aac", "-b:a", "192k", "-f", "adts"],
    ),
    ("wav", "pcm_s16le", &["-c:a", "pcm_s16le"]),
    ("wav", "pcm_f32le", &["-c:a", "pcm_f32le"]),
    ("wav", "adpcm_ms", &["-c:a", "adpcm_ms"]),
    ("wav", "adpcm_ima_wav", &["-c:a", "adpcm_ima_wav"]),
    ("aif", "pcm_s16be", &["-c:a", "pcm_s16be", "-f", "aiff"]),
    ("aiff", "pcm_s24be", &["-c:a", "pcm_s24be"]),
    ("caf", "alac", &["-c:a", "alac"]),
    ("caf", "pcm_s16le", &["-c:a", "pcm_s16le"]),
    ("mka", "flac", &["-c:a", "flac"]),
    ("mka", "vorbis", &["-c:a", "libvorbis"]),
];

fn ffmpeg_sample(dir: &Path, name: &str, args: &[&str]) -> PathBuf {
    let out = dir.join(name);
    let secs = SAMPLE_SECS;
    let tone =
        format!("aevalsrc=0.4*sin(2*PI*(300+40*t)*t)|0.4*sin(2*PI*(450+20*t)*t):s={RATE}:d={secs}");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", &tone, "-ar", &RATE.to_string(), "-ac", "2"])
        .args(args)
        .arg(&out)
        .status()
        .expect("ffmpeg must be installed");
    assert!(status.success(), "ffmpeg failed for {name}");
    out
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// Play `fixture` as a growing download; returns (first-audio % of file written, decoded
/// samples, reference samples).
fn growing_run(fixture: &Path, ext: &str) -> Result<(f64, usize, usize), String> {
    let (reference, ref_rate, ref_ch) = reference_decode(fixture);
    let dir = fixture.parent().unwrap();
    let dest = dir.join(format!("Download Track.{ext}"));
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let total = std::fs::metadata(fixture).unwrap().len();
    player.play(&dest, total);
    let mut writer = Writer::start(fixture, &dest, RATE_BPS);
    let mut at_first = None;
    let ended = wait_until(Duration::from_secs(60), || {
        if at_first.is_none() && capture.sample_count() > 0 {
            at_first = Some(writer.written());
        }
        matches!(
            player.snapshot().state,
            PlayState::Ended | PlayState::Error(_)
        )
    });
    let snap = player.snapshot();
    writer.join();
    let _ = std::fs::remove_file(&dest);
    if !ended || snap.state != PlayState::Ended {
        return Err(format!("did not end cleanly: {:?}", snap.state));
    }
    let data = capture.data();
    if (data.sample_rate, data.channels) != (ref_rate, ref_ch) {
        return Err(format!(
            "format {} Hz/{} ch, reference {ref_rate} Hz/{ref_ch} ch",
            data.sample_rate, data.channels
        ));
    }
    if data.samples.len() != reference.len() {
        return Err(format!(
            "{} samples, reference {}",
            data.samples.len(),
            reference.len()
        ));
    }
    let diff = max_abs_diff(&data.samples, &reference);
    if diff != 0.0 {
        return Err(format!("max |captured - reference| = {diff}"));
    }
    let pct = at_first.unwrap_or(total) as f64 * 100.0 / total as f64;
    Ok((pct, data.samples.len(), reference.len()))
}

#[test]
fn every_playable_extension_decodes_while_growing() {
    let dir = tempfile::tempdir().unwrap();
    let mut covered = BTreeSet::new();
    let mut failures = Vec::new();
    for (i, (ext, label, args)) in CASES.iter().enumerate() {
        let name = format!("sample{i}.{ext}");
        let fixture = ffmpeg_sample(dir.path(), &name, args);
        let size = std::fs::metadata(&fixture).unwrap().len();
        let t = Instant::now();
        match growing_run(&fixture, ext) {
            Ok((pct, n, _)) => {
                println!(
                    "[formats] .{ext:<4} {label:<18} {size:>8} B  ok: first audio at {pct:5.1}% \
                     written, {n} samples bit-identical to full decode ({:.1} s)",
                    t.elapsed().as_secs_f64()
                );
                covered.insert(*ext);
            }
            Err(e) => {
                println!("[formats] .{ext:<4} {label:<18} {size:>8} B  FAILED: {e}");
                failures.push(format!(".{ext} {label}: {e}"));
            }
        }
    }
    let missing: Vec<_> = PLAYABLE_EXTENSIONS
        .iter()
        .filter(|e| !covered.contains(*e))
        .collect();
    println!("[formats] PLAYABLE_EXTENSIONS = {PLAYABLE_EXTENSIONS:?}");
    assert!(failures.is_empty(), "failures: {failures:#?}");
    assert!(missing.is_empty(), "no passing sample for {missing:?}");
    for (ext, _, _) in CASES {
        assert!(is_playable(&format!("x\\y.{}", ext.to_uppercase())));
    }
}

/// `.opus` is not in the list because symphonia 0.6 cannot decode it: the player reports an
/// error instead of hanging.
#[test]
fn opus_is_not_playable_and_fails_fast() {
    assert!(!is_playable("a.opus"));
    let dir = tempfile::tempdir().unwrap();
    let fixture = ffmpeg_sample(
        dir.path(),
        "sample.opus",
        &["-c:a", "libopus", "-ar", "48000"],
    );
    let player = Player::new(Output::Null);
    player.play(&fixture, std::fs::metadata(&fixture).unwrap().len());
    let failed = wait_until(Duration::from_secs(5), || {
        matches!(player.snapshot().state, PlayState::Error(_))
    });
    println!("[formats] .opus → {:?}", player.snapshot().state);
    assert!(failed);
}
