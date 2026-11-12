//! Checkpoint 0 proof: play a FLAC (and MP3) while it is still being "downloaded" into a `.part`
//! file at ~200 KB/s. Runs offline; fixtures are generated with ffmpeg into a temp dir.
//!
//! Timing lines are prefixed with `[timing]`.

mod common;

use std::time::{Duration, Instant};

use common::*;
use wholeseek_audio::{CaptureSink, Output, PlayState, Player, SeekError};

const RATE_BPS: u64 = 200 * 1024;

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

/// Full playback of a growing file into the capture sink.
fn full_run(ext: &str) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), ext);
    let (reference, ref_rate, ref_ch) = reference_decode(&fixture);
    let dest = dir.path().join(format!("Track.{ext}"));

    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    // Start the player first: it must wait for the .part to appear.
    let t0 = Instant::now();
    player.play(&dest, std::fs::metadata(&fixture).unwrap().len());
    let mut writer = Writer::start(&fixture, &dest, RATE_BPS);
    let total = writer.total;

    let mut saw_buffering = false;
    let mut bytes_at_first_audio = 0;
    assert!(
        wait_until(Duration::from_secs(30), || {
            saw_buffering |= player.snapshot().state == PlayState::Buffering;
            if capture.sample_count() > 0 {
                bytes_at_first_audio = writer.written();
                true
            } else {
                false
            }
        }),
        "no audio within 30 s"
    );
    let first_audio = capture
        .first_write()
        .unwrap()
        .duration_since(writer.started);
    let pct = bytes_at_first_audio as f64 * 100.0 / total as f64;
    println!(
        "[timing] {ext}: file {total} B, writer {} KB/s; first audio at {:.1} ms after download \
         start, writer had written {bytes_at_first_audio} B ({pct:.2}% of file)",
        RATE_BPS / 1024,
        first_audio.as_secs_f64() * 1000.0,
    );
    assert!(
        pct < 10.0,
        "playback must start before 10% is written (was {pct:.2}%)"
    );
    let snap = player.snapshot();
    println!(
        "[timing] {ext}: snapshot after first audio: state={:?} pos={:.2}s dur={:?} est={} \
         buffered={:.1}%",
        snap.state,
        snap.position_secs,
        snap.duration_secs,
        snap.duration_estimated,
        snap.buffered_fraction * 100.0
    );

    // Mid-download state should be Buffering (decoder outruns the 200 KB/s writer).
    assert!(
        wait_until(Duration::from_secs(10), || {
            saw_buffering |= player.snapshot().state == PlayState::Buffering;
            saw_buffering
        }),
        "never observed Buffering while the download lagged"
    );

    assert!(
        wait_until(Duration::from_secs(90), || player.snapshot().state
            == PlayState::Ended),
        "playback did not end: {:?}",
        player.snapshot()
    );
    let ended = t0.elapsed();
    writer.join();
    assert!(dest.exists(), ".part should have been renamed");
    let data = capture.data();
    println!(
        "[timing] {ext}: ended {:.2} s after play(); download took {:.2} s; decoded {} samples \
         ({} frames, {} Hz, {} ch); reference {} samples",
        ended.as_secs_f64(),
        writer.started.elapsed().as_secs_f64(),
        data.samples.len(),
        data.samples.len() / data.channels.max(1),
        data.sample_rate,
        data.channels,
        reference.len()
    );
    assert_eq!(data.sample_rate, ref_rate);
    assert_eq!(data.channels, ref_ch);
    assert_eq!(
        data.samples.len(),
        reference.len(),
        "growing-file decode must match a full decode"
    );
    let diff = max_abs_diff(&data.samples, &reference);
    println!("[timing] {ext}: max |captured - reference| = {diff}");
    assert_eq!(diff, 0.0, "decoded audio must be bit-identical");
    if ext == "flac" {
        assert_eq!(data.samples.len(), (SECS * RATE * 2) as usize);
    }
    let snap = player.snapshot();
    assert!((snap.position_secs - SECS as f64).abs() < 0.1, "{snap:?}");
    assert_eq!(snap.buffered_fraction, 1.0);

    let wav = proof_dir().join(format!("captured-growing-{ext}.wav"));
    write_wav(&wav, &data.samples, data.sample_rate, data.channels);
    println!("[proof] wrote {}", wav.display());
}

/// Seek inside the buffered range while the download is still in progress.
fn seek_run(ext: &str) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), ext);
    let (reference, rate, ch) = reference_decode(&fixture);
    let dest = dir.path().join(format!("Track.{ext}"));

    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let mut writer = Writer::start(&fixture, &dest, RATE_BPS);
    player.play(&dest, writer.total);

    assert!(
        wait_until(Duration::from_secs(30), || {
            let s = player.snapshot();
            s.buffered_fraction >= 0.4 && s.duration_secs.is_some() && capture.sample_count() > 0
        }),
        "never reached 40% buffered: {:?}",
        player.snapshot()
    );
    let snap = player.snapshot();
    let buffered_secs = snap.buffered_secs.unwrap();
    println!(
        "[timing] {ext} seek: at {:.2} s download is {:.1}% ({} B), buffered≈{buffered_secs:.2} s, \
         pos={:.2} s, state={:?}",
        writer.started.elapsed().as_secs_f64(),
        snap.buffered_fraction * 100.0,
        writer.written(),
        snap.position_secs,
        snap.state
    );

    // Beyond the buffered range: rejected immediately with a displayable error.
    let far = buffered_secs + 5.0;
    let err = player.seek(far).unwrap_err();
    assert!(matches!(err, SeekError::NotBuffered { .. }), "{err:?}");
    println!("[timing] {ext} seek: seek({far:.1}) -> Err: {err}");

    // Within the buffered range (backwards, while the decoder is parked at the download edge).
    let target = 2.0;
    let t = Instant::now();
    player.seek(target).expect("seek within buffered range");
    let want = (0.5 * rate as f64) as usize * ch;
    assert!(
        wait_until(Duration::from_secs(5), || {
            let d = capture.data();
            d.flush_marks
                .last()
                .is_some_and(|&m| d.samples.len() >= m + want)
        }),
        "no audio after seek"
    );
    let latency = t.elapsed();
    assert!(
        writer.written() < writer.total,
        "seek should happen mid-download"
    );
    let d = capture.data();
    let mark = *d.flush_marks.last().unwrap();
    let after = &d.samples[mark..mark + want];
    let start = (target * rate as f64) as usize * ch;
    let expected = &reference[start..start + want];
    let diff = max_abs_diff(after, expected);
    let snap = player.snapshot();
    println!(
        "[timing] {ext} seek: seek({target}) serviced, 0.5 s of audio after {:.1} ms; \
         pos now {:.2} s; max |post-seek - reference@{target}s| = {diff}",
        latency.as_secs_f64() * 1000.0,
        snap.position_secs
    );
    assert!(
        snap.position_secs >= target && snap.position_secs < buffered_secs + 1.0,
        "{snap:?}"
    );
    // FLAC is lossless and frame-exact. MP3 frames overlap (MDCT), so the first frame after a
    // seek lacks its predecessor; compare after that warm-up with a small tolerance.
    match ext {
        "flac" => assert_eq!(diff, 0.0),
        _ => {
            let skip = 2304 * ch;
            let diff2 = max_abs_diff(&after[skip..], &expected[skip..]);
            println!("[timing] {ext} seek: after 2 MP3 frames warm-up max diff = {diff2}");
            assert!(diff2 < 1e-3, "post-seek audio does not match: {diff2}");
        }
    }

    // Pause / resume / volume are accepted and reflected.
    player.pause();
    assert!(wait_until(Duration::from_secs(2), || player
        .snapshot()
        .state
        == PlayState::Paused));
    player.set_volume(0.5);
    player.resume();
    assert!(wait_until(Duration::from_secs(2), || matches!(
        player.snapshot().state,
        PlayState::Playing | PlayState::Buffering
    )));
    assert_eq!(player.snapshot().volume, 0.5);

    // Stop must return immediately even though the decoder is blocked on the download.
    let t = Instant::now();
    player.stop();
    assert!(t.elapsed() < Duration::from_millis(50));
    assert_eq!(player.snapshot().state, PlayState::Idle);
    writer.join();
}

#[test]
fn flac_plays_while_downloading() {
    full_run("flac");
}

#[test]
fn flac_seek_within_buffered_range() {
    seek_run("flac");
}

#[test]
fn mp3_plays_while_downloading() {
    full_run("mp3");
}

#[test]
fn mp3_seek_within_buffered_range() {
    seek_run("mp3");
}

/// Manual check of the real output device (cpal): plays 3 s of the growing FLAC.
/// `cargo test -p wholeseek-audio -- --ignored device_smoke --nocapture`
#[cfg(feature = "output")]
#[test]
#[ignore = "needs an audio device"]
fn device_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), "flac");
    let dest = dir.path().join("Track.flac");
    let player = Player::new(Output::Device);
    let rx = player.subscribe();
    let mut writer = Writer::start(&fixture, &dest, RATE_BPS);
    let t0 = Instant::now();
    player.play(&dest, writer.total);
    player.set_volume(0.3);
    let (_, ok) = collect_until(&rx, Duration::from_secs(5), |e| {
        matches!(e, wholeseek_audio::PlayerEvent::Started { .. })
    });
    assert!(ok, "device never started");
    println!(
        "[device] first audio to device {:.1} ms after play(), writer at {} B",
        t0.elapsed().as_secs_f64() * 1000.0,
        writer.written()
    );
    std::thread::sleep(Duration::from_secs(3));
    let snap = player.snapshot();
    println!("[device] ~3 s later: {snap:?}");
    let (events, _) = collect_until(&rx, Duration::from_millis(10), |_| false);
    println!("[device] events meanwhile: {}", trace(&events));
    assert!(
        matches!(snap.state, PlayState::Playing | PlayState::Buffering),
        "{snap:?}"
    );
    assert!(
        snap.position_secs > 1.5 && snap.position_secs < 3.5,
        "{snap:?}"
    );
    player.stop();
    writer.join();
}
