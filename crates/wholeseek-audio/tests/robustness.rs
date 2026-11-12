//! Checkpoint 1: player events, track replacement, source failure modes (deleted, failed,
//! cancelled, stalled, unknown/wrong size) and output failures. Offline; fixtures via ffmpeg.
//!
//! Log lines are prefixed with `[cp1]`.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use wholeseek_audio::{
    AudioSink, CaptureSink, Output, PlayState, Player, PlayerEvent, SeekError, SinkFactory,
};

const RATE_BPS: u64 = 200 * 1024;

fn is_terminal(e: &PlayerEvent) -> bool {
    matches!(e, PlayerEvent::Ended | PlayerEvent::Error(_))
}

/// Plays in (scaled) real time, like a device would: `write` sleeps for the audio duration.
struct RealtimeSink {
    rate: u32,
    ch: usize,
    speed: f64,
    written: Arc<AtomicUsize>,
}

impl AudioSink for RealtimeSink {
    fn start(&mut self, rate: u32, ch: usize) -> Result<(), String> {
        self.rate = rate;
        self.ch = ch;
        Ok(())
    }
    fn write(&mut self, samples: &[f32], abort: &dyn Fn() -> bool) -> usize {
        let per = (self.rate as usize * self.ch) / 100; // 10 ms slices
        let mut done = 0;
        for chunk in samples.chunks(per.max(1)) {
            if abort() {
                break;
            }
            std::thread::sleep(Duration::from_secs_f64(
                chunk.len() as f64 / (self.rate as f64 * self.ch as f64) / self.speed,
            ));
            done += chunk.len();
        }
        self.written.fetch_add(done, Ordering::SeqCst);
        done
    }
}

#[test]
fn events_are_pushed_in_order_with_position_ticks() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture_secs(dir.path(), "flac", 6);
    let dest = dir.path().join("Track.flac");
    let written = Arc::new(AtomicUsize::new(0));
    let w2 = written.clone();
    let factory: SinkFactory = Arc::new(move || {
        Box::new(RealtimeSink {
            rate: 0,
            ch: 0,
            speed: 1.0,
            written: w2.clone(),
        })
    });
    let player = Player::with_sink_factory(factory);
    let rx = player.subscribe();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wk = wakes.clone();
    player.set_waker(move || {
        wk.fetch_add(1, Ordering::SeqCst);
    });
    // Download faster than real time (1 MB/s) so ticks come from playback, not buffering.
    let mut writer = Writer::start(&fixture, &dest, 1024 * 1024);
    let t0 = Instant::now();
    player.play(&dest, writer.total);
    let (events, done) = collect_until(&rx, Duration::from_secs(30), is_terminal);
    let elapsed = t0.elapsed();
    writer.join();
    println!(
        "[cp1] events (6 s FLAC, real-time sink): {}",
        trace(&events)
    );
    assert!(done, "no terminal event");
    assert_eq!(events.last(), Some(&PlayerEvent::Ended));

    let idx = |f: &dyn Fn(&PlayerEvent) -> bool| events.iter().position(f);
    let buffering = idx(&|e| *e == PlayerEvent::Buffering).expect("Buffering");
    let started = idx(&|e| matches!(e, PlayerEvent::Started { .. })).expect("Started");
    let playing = idx(&|e| *e == PlayerEvent::Playing).expect("Playing");
    assert!(buffering < started && started < playing, "order");
    match &events[started] {
        PlayerEvent::Started {
            path,
            duration_secs,
        } => {
            assert_eq!(path, &dest);
            assert!((duration_secs.unwrap() - 6.0).abs() < 0.01);
        }
        _ => unreachable!(),
    }
    let ticks: Vec<f64> = events
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::PositionTick { position_secs, .. } => Some(*position_secs),
            _ => None,
        })
        .collect();
    let rate = ticks.len() as f64 / elapsed.as_secs_f64();
    println!(
        "[cp1] {} PositionTicks in {:.2} s ({rate:.1}/s), waker called {} times",
        ticks.len(),
        elapsed.as_secs_f64(),
        wakes.load(Ordering::SeqCst)
    );
    assert!((3.0..=5.5).contains(&rate), "tick rate {rate}");
    assert!(ticks.windows(2).all(|w| w[1] >= w[0]), "ticks monotonic");
    // The download (1 MB/s) finishes long before playback; the buffered fraction must follow
    // it even though the decoder is held back by the real-time output (regression: it used to
    // freeze at the decoder's read-ahead point).
    let full_at = events.iter().find_map(|e| match e {
        PlayerEvent::PositionTick {
            position_secs,
            buffered_fraction,
        } if *buffered_fraction >= 1.0 => Some(*position_secs),
        _ => None,
    });
    println!("[cp1] buffered_fraction reached 1.0 at position {full_at:?} s");
    assert!(
        full_at.is_some_and(|p| p < 2.0),
        "buffered fraction stale: {full_at:?}"
    );
    assert!(*ticks.last().unwrap() > 5.0);
    assert!(wakes.load(Ordering::SeqCst) >= events.len());
    assert!(elapsed.as_secs_f64() > 5.5, "played in real time");
}

#[test]
fn play_replaces_previous_track_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let a_fix = make_fixture(dir.path(), "flac");
    let b_fix = make_fixture_secs(dir.path(), "mp3", 4);
    let (a_ref, _, _) = reference_decode(&a_fix);
    let (b_ref, _, _) = reference_decode(&b_fix);
    let a_dest = dir.path().join("A.flac");

    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let rx = player.subscribe();
    let mut writer = Writer::start(&a_fix, &a_dest, RATE_BPS);
    player.play(&a_dest, writer.total);
    let (ev_a, ok) = collect_until(&rx, Duration::from_secs(10), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    assert!(ok, "A never started: {}", trace(&ev_a));
    std::thread::sleep(Duration::from_millis(300)); // A is now parked at the download edge
    // Drain A's events so far; everything after play(B) must start with A's `Stopped`.
    let (ev_a_more, _) = collect_until(&rx, Duration::from_millis(1), |_| false);
    println!(
        "[cp1] replace: A events before play(B): {} → {}",
        trace(&ev_a),
        trace(&ev_a_more)
    );

    // B: an already complete file.
    let t = Instant::now();
    player.play(&b_fix, std::fs::metadata(&b_fix).unwrap().len());
    let call = t.elapsed();
    let (ev_b, ok) = collect_until(&rx, Duration::from_secs(20), is_terminal);
    println!(
        "[cp1] replace: play(B) returned in {:.3} ms; events after: {}",
        call.as_secs_f64() * 1000.0,
        trace(&ev_b)
    );
    assert!(ok);
    assert!(call < Duration::from_millis(20), "play must not block");
    // A may still emit state flips (it is parked at the download edge) right up to play(B);
    // the guarantee is that nothing of A follows its `Stopped`, and B starts after it.
    let stopped = ev_b
        .iter()
        .position(|e| *e == PlayerEvent::Stopped)
        .expect("A's Stopped");
    assert!(
        ev_b[..stopped]
            .iter()
            .all(|e| matches!(e, PlayerEvent::Playing | PlayerEvent::Buffering)),
        "only A state flips may precede Stopped"
    );
    assert_eq!(ev_b[stopped + 1], PlayerEvent::Buffering, "B's first event");
    assert_eq!(ev_b.last(), Some(&PlayerEvent::Ended));
    let b_started: Vec<_> = ev_b[stopped..]
        .iter()
        .filter_map(|e| match e {
            PlayerEvent::Started { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(b_started, vec![b_fix.clone()], "exactly one Started, for B");

    // The capture holds a clean prefix of A, then exactly all of B; nothing interleaved.
    let d = capture.data();
    assert_eq!(d.start_marks.len(), 2);
    let (s_a, s_b) = (d.start_marks[0], d.start_marks[1]);
    let a_part = &d.samples[s_a..s_b];
    let b_part = &d.samples[s_b..];
    assert!(!a_part.is_empty());
    assert_eq!(max_abs_diff(a_part, &a_ref[..a_part.len()]), 0.0);
    assert_eq!(b_part.len(), b_ref.len());
    assert_eq!(max_abs_diff(b_part, &b_ref), 0.0);
    println!(
        "[cp1] replace: A contributed {} samples (clean prefix), B {} samples == full decode",
        a_part.len(),
        b_part.len()
    );
    writer.join();
}

/// Write 30% then misbehave; returns (terminal event, events, captured samples, reference
/// samples, time from `after_stop` to the terminal event).
fn partial_run(
    plan: Plan,
    after_stop: impl FnOnce(&Player),
) -> (PlayerEvent, Vec<PlayerEvent>, usize, usize, Duration) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), "flac");
    let (reference, _, _) = reference_decode(&fixture);
    let dest = dir.path().join("Track.flac");
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let rx = player.subscribe();
    let mut writer = Writer::start_with(&fixture, &dest, 1024 * 1024, plan);
    player.play(&dest, writer.total);
    writer.join();
    let t = Instant::now();
    after_stop(&player);
    let (events, ok) = collect_until(&rx, Duration::from_secs(10), is_terminal);
    let took = t.elapsed();
    assert!(ok, "hung: {}", trace(&events));
    // Never panics, never hangs: the snapshot still works and stop is immediate.
    let _ = player.snapshot();
    let s = Instant::now();
    player.stop();
    assert!(s.elapsed() < Duration::from_millis(20));
    (
        events.last().unwrap().clone(),
        events.clone(),
        capture.sample_count(),
        reference.len(),
        took,
    )
}

#[test]
fn deleted_download_ends_in_error_after_buffered_audio() {
    let total = std::fs::metadata(make_fixture(tempfile::tempdir().unwrap().path(), "flac"))
        .unwrap()
        .len();
    let (last, events, got, full, took) = partial_run(
        Plan {
            stop_at: Some(total * 3 / 10),
            delete_after_stop: true,
            ..Plan::default()
        },
        |_| {},
    );
    println!(
        "[cp1] deleted .part at 30%: {} ; played {got}/{full} samples; terminal after {:.0} ms",
        trace(&events),
        took.as_secs_f64() * 1000.0
    );
    match last {
        PlayerEvent::Error(m) => assert!(m.contains("missing"), "{m}"),
        other => panic!("expected Error, got {other:?}"),
    }
    assert!(got > full / 5, "buffered audio should still play ({got})");
}

#[test]
fn failed_download_plays_buffered_audio_then_errors() {
    let total = std::fs::metadata(make_fixture(tempfile::tempdir().unwrap().path(), "flac"))
        .unwrap()
        .len();
    let (last, events, got, full, took) = partial_run(
        Plan {
            stop_at: Some(total * 3 / 10),
            ..Plan::default()
        },
        |p| p.fail_source("peer went away"),
    );
    println!(
        "[cp1] fail_source at 30%: {} ; played {got}/{full}; terminal after {:.0} ms",
        trace(&events),
        took.as_secs_f64() * 1000.0
    );
    assert_eq!(
        last,
        PlayerEvent::Error("download failed: peer went away".into())
    );
    assert!(got > full / 5);
}

#[test]
fn externally_cancelled_source_is_an_error_not_a_hang() {
    let total = std::fs::metadata(make_fixture(tempfile::tempdir().unwrap().path(), "flac"))
        .unwrap()
        .len();
    let (last, events, _, _, took) = partial_run(
        Plan {
            stop_at: Some(total * 3 / 10),
            ..Plan::default()
        },
        |p| p.source_handle().unwrap().cancel(),
    );
    println!(
        "[cp1] source cancelled: {} ; terminal after {:.0} ms",
        trace(&events),
        took.as_secs_f64() * 1000.0
    );
    assert_eq!(last, PlayerEvent::Error("playback source cancelled".into()));
    assert!(took < Duration::from_secs(1));
}

#[test]
fn stalled_download_reports_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), "flac");
    let (reference, _, _) = reference_decode(&fixture);
    let dest = dir.path().join("Track.flac");
    let total = std::fs::metadata(&fixture).unwrap().len();
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    player.set_stall_threshold(Duration::from_secs(1));
    let rx = player.subscribe();
    let mut writer = Writer::start_with(
        &fixture,
        &dest,
        1024 * 1024,
        Plan {
            stall: Some((total * 3 / 10, Duration::from_secs(3))),
            ..Plan::default()
        },
    );
    player.play(&dest, total);

    let (ev1, ok) = collect_until(&rx, Duration::from_secs(10), |e| {
        matches!(e, PlayerEvent::Notice(Some(_)))
    });
    assert!(ok, "no stall notice: {}", trace(&ev1));
    let snap = player.snapshot();
    println!(
        "[cp1] stall: snapshot during stall: state={:?} notice={:?} buffered={:.1}%",
        snap.state,
        snap.notice,
        snap.buffered_fraction * 100.0
    );
    assert_eq!(snap.state, PlayState::Buffering);
    assert!(snap.notice.unwrap().contains("stalled"));

    let (ev2, ok) = collect_until(&rx, Duration::from_secs(20), is_terminal);
    assert!(ok);
    writer.join();
    println!("[cp1] stall events: {} → {}", trace(&ev1), trace(&ev2));
    assert!(ev2.contains(&PlayerEvent::Notice(None)), "notice cleared");
    assert_eq!(ev2.last(), Some(&PlayerEvent::Ended));
    assert_eq!(capture.sample_count(), reference.len());
    assert_eq!(player.snapshot().notice, None);
}

#[test]
fn unknown_total_size_plays_fully() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), "flac");
    let (reference, _, _) = reference_decode(&fixture);
    let dest = dir.path().join("Track.flac");
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let rx = player.subscribe();
    let mut writer = Writer::start(&fixture, &dest, RATE_BPS * 4);
    player.play_unknown_size(&dest);
    let (_, ok) = collect_until(&rx, Duration::from_secs(10), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    assert!(ok);
    std::thread::sleep(Duration::from_millis(500));
    let snap = player.snapshot();
    let fwd = player.seek(snap.position_secs + 3.0);
    println!(
        "[cp1] unknown size mid-download: fraction={} buffered_secs={:?} forward seek -> {fwd:?}",
        snap.buffered_fraction, snap.buffered_secs
    );
    assert_eq!(snap.buffered_fraction, 0.0);
    assert!(matches!(fwd, Err(SeekError::NotBuffered { .. })));
    assert!(player.seek(0.5).is_ok(), "backward seek always allowed");
    let (ev, ok) = collect_until(&rx, Duration::from_secs(20), is_terminal);
    assert!(ok);
    writer.join();
    assert_eq!(ev.last(), Some(&PlayerEvent::Ended));
    let d = capture.data();
    let mark = *d.flush_marks.last().unwrap();
    let start = (0.5 * RATE as f64) as usize * 2;
    assert_eq!(d.samples.len() - mark, reference.len() - start);
    assert_eq!(max_abs_diff(&d.samples[mark..], &reference[start..]), 0.0);
    let snap = player.snapshot();
    assert_eq!(snap.buffered_fraction, 1.0);
    println!(
        "[cp1] unknown size: ended, post-seek audio == reference[0.5 s..], fraction={}",
        snap.buffered_fraction
    );
}

fn mismatch_run(delta: i64) -> (Option<String>, usize, usize) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture(dir.path(), "flac");
    let (reference, _, _) = reference_decode(&fixture);
    let dest = dir.path().join("Track.flac");
    let capture = CaptureSink::new();
    let player = Player::new(Output::Capture(capture.clone()));
    let rx = player.subscribe();
    let mut writer = Writer::start(&fixture, &dest, RATE_BPS * 4);
    let announced = (writer.total as i64 + delta) as u64;
    player.play(&dest, announced);
    let (ev, ok) = collect_until(&rx, Duration::from_secs(20), is_terminal);
    writer.join();
    assert!(ok, "{}", trace(&ev));
    println!(
        "[cp1] size mismatch (announced {announced}, actual {}): {}",
        writer.total,
        trace(&ev)
    );
    assert_eq!(ev.last(), Some(&PlayerEvent::Ended));
    (
        player.snapshot().notice,
        capture.sample_count(),
        reference.len(),
    )
}

#[test]
fn announced_size_too_large_ends_on_rename() {
    let (notice, got, full) = mismatch_run(50_000);
    assert_eq!(got, full);
    assert!(notice.unwrap().contains("mismatch"));
}

#[test]
fn announced_size_too_small_keeps_reading_until_rename() {
    let (notice, got, full) = mismatch_run(-50_000);
    assert_eq!(got, full);
    assert!(notice.unwrap().contains("larger than announced"));
}

/// A sink that dies after a few writes (like an unplugged device).
struct DyingSink {
    writes: usize,
    dead: bool,
}

impl AudioSink for DyingSink {
    fn start(&mut self, _: u32, _: usize) -> Result<(), String> {
        Ok(())
    }
    fn write(&mut self, s: &[f32], _: &dyn Fn() -> bool) -> usize {
        self.writes += 1;
        if self.writes > 5 {
            self.dead = true;
            return 0;
        }
        s.len()
    }
    fn error(&self) -> Option<String> {
        self.dead
            .then(|| "audio output lost: device unplugged".to_string())
    }
}

struct NoDeviceSink;

impl AudioSink for NoDeviceSink {
    fn start(&mut self, _: u32, _: usize) -> Result<(), String> {
        Err("no audio output device found".into())
    }
    fn write(&mut self, s: &[f32], _: &dyn Fn() -> bool) -> usize {
        s.len()
    }
}

fn sink_failure(factory: SinkFactory) -> Vec<PlayerEvent> {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture_secs(dir.path(), "flac", 4);
    let player = Player::with_sink_factory(factory);
    let rx = player.subscribe();
    player.play(&fixture, std::fs::metadata(&fixture).unwrap().len());
    let (ev, ok) = collect_until(&rx, Duration::from_secs(10), is_terminal);
    assert!(ok, "{}", trace(&ev));
    // The player stays usable after an output failure.
    assert!(matches!(player.snapshot().state, PlayState::Error(_)));
    player.play(&fixture, std::fs::metadata(&fixture).unwrap().len());
    player.stop();
    ev
}

#[test]
fn output_device_lost_or_absent_is_an_error_state() {
    let lost = sink_failure(Arc::new(|| {
        Box::new(DyingSink {
            writes: 0,
            dead: false,
        })
    }));
    println!("[cp1] device lost mid-play: {}", trace(&lost));
    assert_eq!(
        lost.last(),
        Some(&PlayerEvent::Error(
            "audio output lost: device unplugged".into()
        ))
    );
    let absent = sink_failure(Arc::new(|| Box::new(NoDeviceSink)));
    println!("[cp1] no device: {}", trace(&absent));
    assert_eq!(
        absent.last(),
        Some(&PlayerEvent::Error("no audio output device found".into()))
    );
    assert!(
        !absent
            .iter()
            .any(|e| matches!(e, PlayerEvent::Started { .. }))
    );
}

/// The real cpal sink with ALSA configured to have no usable default device: must end in an
/// Error state with a clear message, not a panic. Runs this test binary again as a child with
/// `ALSA_CONFIG_PATH` pointing at a config that defines no devices.
#[cfg(feature = "output")]
#[test]
fn real_output_without_device_is_an_error_state() {
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("empty-asound.conf");
    std::fs::write(&conf, "# no pcm devices\n").unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "no_device_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("ALSA_CONFIG_PATH", &conf)
        .env("WHOLESEEK_NO_DEVICE_CHILD", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines().filter(|l| l.contains("[cp1]")) {
        println!("{line}");
    }
    assert!(
        out.status.success(),
        "child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(feature = "output")]
#[test]
#[ignore = "child of real_output_without_device_is_an_error_state"]
fn no_device_child() {
    if std::env::var_os("WHOLESEEK_NO_DEVICE_CHILD").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture_secs(dir.path(), "flac", 4);
    let player = Player::new(Output::Device);
    let rx = player.subscribe();
    player.play(&fixture, std::fs::metadata(&fixture).unwrap().len());
    let (ev, ok) = collect_until(&rx, Duration::from_secs(10), is_terminal);
    println!("[cp1] real cpal sink, ALSA without devices: {}", trace(&ev));
    assert!(ok, "hung");
    assert!(
        matches!(ev.last(), Some(PlayerEvent::Error(_))),
        "{}",
        trace(&ev)
    );
}

/// Waker + subscriber plumbing survives dropped receivers.
#[test]
fn dropped_subscribers_are_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = make_fixture_secs(dir.path(), "flac", 4);
    let player = Player::new(Output::Null);
    drop(player.subscribe());
    let rx = player.subscribe();
    let seen = Arc::new(Mutex::new(0usize));
    player.play(&fixture, std::fs::metadata(&fixture).unwrap().len());
    let (ev, ok) = collect_until(&rx, Duration::from_secs(10), is_terminal);
    *seen.lock().unwrap() = ev.len();
    assert!(ok);
    assert_eq!(ev.last(), Some(&PlayerEvent::Ended));
}
