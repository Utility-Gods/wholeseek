//! Checkpoint 2: play queue (album playback): auto-advance, prefetch hint, previous/next,
//! next track not ready, manual ▶ elsewhere, end of queue, and the gap between tracks.
//! Offline; fixtures via ffmpeg. Log lines are prefixed with `[cp2]`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use common::*;
use wholeseek_audio::{
    AudioSink, PlayState, Player, PlayerEvent, QueueItem, SinkFactory, UNKNOWN_SIZE,
};

/// Plays in scaled real time (`speed`× faster than a device), like a device would.
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

fn player(speed: f64) -> Player {
    let factory: SinkFactory = Arc::new(move || {
        Box::new(RealtimeSink {
            rate: 0,
            ch: 0,
            speed,
            written: Arc::new(AtomicUsize::new(0)),
        })
    });
    Player::with_sink_factory(factory)
}

/// Complete local copies of fixtures of the given lengths: `track<i>.flac`.
fn album(dir: &Path, lengths: &[u32]) -> Vec<(PathBuf, u64)> {
    lengths
        .iter()
        .enumerate()
        .map(|(i, &secs)| {
            let src = make_fixture_secs(dir, "flac", secs);
            let dest = dir.join(format!("track{i}.flac"));
            std::fs::copy(&src, &dest).unwrap();
            let size = std::fs::metadata(&dest).unwrap().len();
            (dest, size)
        })
        .collect()
}

fn items(tracks: &[(PathBuf, u64)]) -> Vec<QueueItem> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, (p, size))| QueueItem::with_path(format!("id{i}"), p, *size))
        .collect()
}

type Timed = Vec<(Instant, PlayerEvent)>;

/// Receive events until `stop` matches one (inclusive) or `limit` passes.
fn until(
    rx: &Receiver<PlayerEvent>,
    limit: Duration,
    stop: impl Fn(&PlayerEvent) -> bool,
) -> Timed {
    let end = Instant::now() + limit;
    let mut out = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) => {
                let hit = stop(&ev);
                out.push((Instant::now(), ev));
                if hit {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

/// Compact trace without position ticks.
fn brief(events: &Timed) -> String {
    let t0 = events.first().map(|e| e.0);
    events
        .iter()
        .filter(|(_, e)| !matches!(e, PlayerEvent::PositionTick { .. }))
        .map(|(t, e)| {
            let ms = t0.map_or(0, |t0| t.duration_since(t0).as_millis());
            let name = match e {
                PlayerEvent::Started { path, .. } => format!(
                    "Started({})",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                PlayerEvent::TrackChanged { index, .. } => format!("TrackChanged({index})"),
                PlayerEvent::PrefetchNext { index, .. } => format!("PrefetchNext({index})"),
                PlayerEvent::TrackEnded { index } => format!("TrackEnded({index})"),
                PlayerEvent::Seeked { position_secs } => format!("Seeked({position_secs:.2})"),
                other => format!("{other:?}"),
            };
            format!("{ms}ms:{name}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Events until the queue has ended and the last track was stopped.
fn until_queue_end(rx: &Receiver<PlayerEvent>, limit: Duration) -> Timed {
    let mut ev = until(rx, limit, |e| *e == PlayerEvent::QueueEnded);
    ev.extend(until(rx, Duration::from_secs(2), |e| {
        *e == PlayerEvent::Stopped
    }));
    ev
}

fn indices(events: &Timed, f: impl Fn(&PlayerEvent) -> Option<usize>) -> Vec<usize> {
    events.iter().filter_map(|(_, e)| f(e)).collect()
}

fn changed(e: &PlayerEvent) -> Option<usize> {
    match e {
        PlayerEvent::TrackChanged { index, .. } => Some(*index),
        _ => None,
    }
}

fn prefetch(e: &PlayerEvent) -> Option<usize> {
    match e {
        PlayerEvent::PrefetchNext { index, .. } => Some(*index),
        _ => None,
    }
}

fn track_ended(e: &PlayerEvent) -> Option<usize> {
    match e {
        PlayerEvent::TrackEnded { index } => Some(*index),
        _ => None,
    }
}

/// Wall-clock gaps from `TrackEnded(i)` to the next `Started`.
fn gaps(events: &Timed) -> Vec<Duration> {
    let mut out = Vec::new();
    let mut ended_at = None;
    for (t, e) in events {
        match e {
            PlayerEvent::TrackEnded { .. } => ended_at = Some(*t),
            PlayerEvent::Started { .. } => {
                if let Some(a) = ended_at.take() {
                    out.push(t.duration_since(a));
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn queue_advances_in_order_prefetches_once_and_stops_at_end() {
    let dir = tempfile::tempdir().unwrap();
    let tracks = album(dir.path(), &[6, 7, 8]);
    let player = player(4.0);
    let rx = player.subscribe();
    player.set_queue(items(&tracks), 0);
    let events = until_queue_end(&rx, Duration::from_secs(30));
    println!("[cp2] 3-track queue (4× real time): {}", brief(&events));

    assert_eq!(indices(&events, changed), vec![0, 1, 2]);
    assert_eq!(indices(&events, track_ended), vec![0, 1, 2]);
    // Exactly once per track that has a successor (short tracks: < 20 s remain at once).
    assert_eq!(indices(&events, prefetch), vec![1, 2]);
    let started: Vec<PathBuf> = events
        .iter()
        .filter_map(|(_, e)| match e {
            PlayerEvent::Started { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let want: Vec<PathBuf> = tracks.iter().map(|t| t.0.clone()).collect();
    assert_eq!(started, want, "played in queue order");
    let tail: Vec<&PlayerEvent> = events.iter().rev().take(4).rev().map(|e| &e.1).collect();
    assert_eq!(
        tail,
        vec![
            &PlayerEvent::Ended,
            &PlayerEvent::TrackEnded { index: 2 },
            &PlayerEvent::QueueEnded,
            &PlayerEvent::Stopped
        ]
    );
    // Order within one advance: TrackEnded, Stopped (old track), TrackChanged, …, Started.
    let pos = |want: &PlayerEvent| events.iter().position(|(_, e)| e == want).unwrap();
    let e0 = pos(&PlayerEvent::TrackEnded { index: 0 });
    let c1 = events
        .iter()
        .position(|(_, e)| changed(e) == Some(1))
        .unwrap();
    assert!(e0 < c1);
    assert_eq!(events[e0 + 1].1, PlayerEvent::Stopped);

    let g = gaps(&events);
    println!(
        "[cp2] inter-track gap TrackEnded→Started (cached files, test sink): {:?}",
        g.iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect::<Vec<_>>()
    );
    assert_eq!(g.len(), 2);
    assert!(
        g.iter().all(|d| *d < Duration::from_millis(300)),
        "gap {g:?}"
    );
    assert_eq!(player.snapshot().state, PlayState::Idle);
    assert_eq!(player.queue_index(), None);
}

/// Position at which `PrefetchNext` arrives for a 50 s track (first of two).
fn prefetch_position(download_complete: bool) -> (f64, usize) {
    let dir = tempfile::tempdir().unwrap();
    let src = make_fixture_secs(dir.path(), "flac", 50);
    let next = album(dir.path(), &[6]);
    let dest = dir.path().join("long.flac");
    let mut writer = if download_complete {
        Writer::start(&src, &dest, 50 * 1024 * 1024)
    } else {
        // Fast, but the last 5% only arrive after a 6 s pause.
        let total = std::fs::metadata(&src).unwrap().len();
        let plan = Plan {
            stall: Some((total * 95 / 100, Duration::from_secs(6))),
            ..Plan::default()
        };
        Writer::start_with(&src, &dest, 20 * 1024 * 1024, plan)
    };
    let player = player(10.0);
    let rx = player.subscribe();
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut list = vec![QueueItem::with_path("long", &part, writer.total)];
    list.extend(items(&next));
    player.set_queue(list, 0);
    let mut at = None;
    let mut count = 0;
    let events = until(&rx, Duration::from_secs(20), |e| {
        matches!(e, PlayerEvent::TrackEnded { .. })
    });
    for (_, e) in &events {
        if prefetch(e).is_some() {
            count += 1;
        }
    }
    // Position when the hint was sent: last tick before it (ticks are 250 ms = 2.5 s audio
    // apart at 10×), so use the tick right after it as the upper bound instead.
    let idx = events.iter().position(|(_, e)| prefetch(e).is_some());
    if let Some(i) = idx {
        let before = events[..i].iter().rev().find_map(|(_, e)| match e {
            PlayerEvent::PositionTick { position_secs, .. } => Some(*position_secs),
            _ => None,
        });
        let after = events[i..].iter().find_map(|(_, e)| match e {
            PlayerEvent::PositionTick { position_secs, .. } => Some(*position_secs),
            _ => None,
        });
        println!(
            "[cp2] 50 s track, download {}: PrefetchNext(1) between ticks {before:?} and \
             {after:?} s",
            if download_complete {
                "complete"
            } else {
                "incomplete"
            }
        );
        at = after;
        assert!(before.unwrap_or(0.0) <= after.unwrap_or(f64::MAX));
    }
    // Remaining events of the track: still no second hint.
    let rest = until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    count += rest.iter().filter(|(_, e)| prefetch(e).is_some()).count();
    player.stop();
    writer.join();
    (at.expect("PrefetchNext sent"), count)
}

#[test]
fn prefetch_hint_at_half_when_downloaded_else_20s_before_end() {
    // Downloaded: more than half played (25 s) comes before "20 s left" (30 s).
    let (pos, n) = prefetch_position(true);
    assert_eq!(n, 1, "exactly once");
    assert!(
        (25.0..=28.0).contains(&pos),
        "complete: hint near 25 s, got {pos}"
    );
    // Not downloaded yet: only "20 s left" applies.
    let (pos, n) = prefetch_position(false);
    assert_eq!(n, 1, "exactly once");
    assert!(
        (30.0..=33.0).contains(&pos),
        "incomplete: hint near 30 s, got {pos}"
    );
}

fn wait_tick(rx: &Receiver<PlayerEvent>, f: impl Fn(f64) -> bool) -> Timed {
    until(
        rx,
        Duration::from_secs(10),
        |e| matches!(e, PlayerEvent::PositionTick { position_secs, .. } if f(*position_secs)),
    )
}

#[test]
fn previous_restarts_after_3s_else_goes_back() {
    let dir = tempfile::tempdir().unwrap();
    let tracks = album(dir.path(), &[10, 11, 12]);
    let player = player(4.0);
    let rx = player.subscribe();
    player.set_queue(items(&tracks), 1);
    wait_tick(&rx, |p| p > 3.5);
    player.previous();
    let ev = until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Seeked { .. })
    });
    println!("[cp2] previous at >3 s: {}", brief(&ev));
    assert!(
        ev.iter().any(
            |(_, e)| matches!(e, PlayerEvent::Seeked { position_secs } if *position_secs < 0.1)
        ),
        "restarted"
    );
    assert!(
        ev.iter().all(|(_, e)| changed(e).is_none()),
        "no track change"
    );
    assert_eq!(player.queue_index(), Some(1));

    // Right after the restart (< 3 s): back to the previous item.
    assert!(player.snapshot().position_secs < 3.0);
    player.previous();
    let ev = until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    println!("[cp2] previous at <3 s: {}", brief(&ev));
    assert_eq!(indices(&ev, changed), vec![0]);
    assert_eq!(player.queue_index(), Some(0));
    assert!(
        ev.iter()
            .any(|(_, e)| matches!(e, PlayerEvent::Started { path, .. } if *path == tracks[0].0))
    );

    // On the first item: restart it.
    player.previous();
    let ev = until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    assert_eq!(indices(&ev, changed), vec![0]);
    assert_eq!(player.queue_index(), Some(0));

    // next() twice: 1, 2; next() on the last item ends the queue.
    player.next();
    player.next();
    assert_eq!(player.queue_index(), Some(2));
    player.next();
    let ev = until_queue_end(&rx, Duration::from_secs(5));
    println!("[cp2] next ×3 from item 0: {}", brief(&ev));
    assert_eq!(indices(&ev, changed), vec![1, 2]);
    assert!(ev.iter().any(|(_, e)| *e == PlayerEvent::QueueEnded));
    assert_eq!(player.queue_index(), None);
    assert_eq!(player.snapshot().state, PlayState::Idle);
}

#[test]
fn next_not_ready_buffers_then_plays() {
    let dir = tempfile::tempdir().unwrap();
    let first = album(dir.path(), &[6]);
    let src_b = make_fixture_secs(dir.path(), "flac", 7);
    let src_c = make_fixture_secs(dir.path(), "flac", 8);
    let dest_b = dir.path().join("b.flac");
    let dest_c = dir.path().join("c.flac");
    let part_b = PathBuf::from(format!("{}.part", dest_b.display()));
    let size_b = std::fs::metadata(&src_b).unwrap().len();
    let size_c = std::fs::metadata(&src_c).unwrap().len();

    let player = player(4.0);
    let rx = player.subscribe();
    let list = vec![
        QueueItem::with_path("a", &first[0].0, first[0].1),
        // B: fetch "started" (path known) but no bytes on disk until later.
        QueueItem::with_path("b", &part_b, size_b),
        // C: not fetched at all; the app only learns the path when asked.
        QueueItem::new("c", UNKNOWN_SIZE),
    ];
    player.set_queue(list, 0);

    // B: the file appears 1 s after B became current.
    let ev = until(&rx, Duration::from_secs(10), |e| changed(e) == Some(1));
    let t_b = Instant::now();
    let mut writer_b = None;
    let ev2 = until(&rx, Duration::from_secs(10), |e| {
        if *e == PlayerEvent::Buffering {
            return true;
        }
        matches!(e, PlayerEvent::Started { .. })
    });
    assert!(
        ev2.iter().any(|(_, e)| *e == PlayerEvent::Buffering),
        "B not ready → Buffering: {}",
        brief(&ev2)
    );
    assert_eq!(player.snapshot().state, PlayState::Buffering);
    std::thread::sleep(Duration::from_secs(1).saturating_sub(t_b.elapsed()));
    writer_b.get_or_insert(Writer::start(&src_b, &dest_b, 2 * 1024 * 1024));
    let ev3 = until(&rx, Duration::from_secs(10), |e| changed(e) == Some(2));
    let started_b = ev3.iter().find_map(|(t, e)| match e {
        PlayerEvent::Started { path, .. } if *path == part_b => Some(*t),
        _ => None,
    });
    println!(
        "[cp2] B (file appears after 1 s): {} | {}",
        brief(&ev2),
        brief(&ev3)
    );
    let started_b = started_b.expect("B started once its file appeared");
    assert!(started_b.duration_since(t_b) >= Duration::from_millis(900));
    assert!(ev3.iter().any(|(_, e)| *e == PlayerEvent::Playing));
    assert!(
        ev3.iter()
            .any(|(_, e)| *e == PlayerEvent::TrackEnded { index: 1 })
    );

    // C: no path → Buffering, snapshot Buffering, waits; then the app provides it.
    let ev4 = until(&rx, Duration::from_millis(1500), |_| false);
    println!("[cp2] C (no path) for 1.5 s: {}", brief(&ev4));
    assert!(
        ev3.iter()
            .chain(ev4.iter())
            .any(|(_, e)| *e == PlayerEvent::Buffering),
        "C waits in Buffering"
    );
    assert!(
        !ev4.iter()
            .any(|(_, e)| matches!(e, PlayerEvent::Started { .. }))
    );
    assert_eq!(player.snapshot().state, PlayState::Buffering);
    assert_eq!(player.queue_index(), Some(2));
    let mut writer_c = Writer::start(&src_c, &dest_c, 4 * 1024 * 1024);
    let t_c = Instant::now();
    assert!(player.set_queue_path(2, &dest_c, size_c));
    assert!(!player.set_queue_path(9, &dest_c, size_c));
    let ev5 = until_queue_end(&rx, Duration::from_secs(10));
    println!("[cp2] C after set_queue_path: {}", brief(&ev5));
    let started_c = ev5.iter().find_map(|(t, e)| match e {
        PlayerEvent::Started { path, .. } if *path == dest_c => Some(*t),
        _ => None,
    });
    println!(
        "[cp2] C first audio {:.0} ms after set_queue_path",
        started_c
            .expect("C started")
            .duration_since(t_c)
            .as_secs_f64()
            * 1000.0
    );
    assert!(ev5.iter().any(|(_, e)| *e == PlayerEvent::QueueEnded));
    let _ = ev;
    writer_b.as_mut().unwrap().join();
    writer_c.join();
}

#[test]
fn manual_play_elsewhere_clears_queue() {
    let dir = tempfile::tempdir().unwrap();
    let tracks = album(dir.path(), &[6, 7, 8]);
    let player = player(4.0);
    let rx = player.subscribe();
    player.set_queue(items(&tracks), 0);
    until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    assert_eq!(player.queue_index(), Some(0));
    // ▶ on some other track (e.g. a single search result): the album queue is gone.
    player.play(&tracks[2].0, tracks[2].1);
    assert_eq!(player.queue_index(), None);
    assert!(player.queue_items().is_empty());
    let ev = until(&rx, Duration::from_secs(10), |e| *e == PlayerEvent::Ended);
    let after = until(&rx, Duration::from_millis(800), |_| false);
    println!(
        "[cp2] manual play during queue: {} | then {}",
        brief(&ev),
        brief(&after)
    );
    let all: Vec<_> = ev.iter().chain(after.iter()).collect();
    assert!(all.iter().all(|(_, e)| changed(e).is_none()
        && track_ended(e).is_none()
        && prefetch(e).is_none()
        && *e != PlayerEvent::QueueEnded));
    assert_eq!(player.snapshot().state, PlayState::Ended);

    // stop() also clears; clear_queue() keeps the track playing but stops advancing.
    player.set_queue(items(&tracks), 0);
    player.stop();
    assert_eq!(player.queue_index(), None);
    player.set_queue(items(&tracks), 0);
    until(&rx, Duration::from_secs(5), |e| {
        matches!(e, PlayerEvent::Started { .. })
    });
    player.clear_queue();
    let ev = until(&rx, Duration::from_secs(10), |e| *e == PlayerEvent::Ended);
    let after = until(&rx, Duration::from_millis(800), |_| false);
    assert!(ev.iter().any(|(_, e)| *e == PlayerEvent::Ended));
    assert!(
        ev.iter()
            .chain(after.iter())
            .all(|(_, e)| changed(e).is_none() && track_ended(e).is_none())
    );
}

/// Two local FLACs back-to-back on the real device; reports the inter-track gap.
/// `cargo test -p wholeseek-audio --test queue -- --ignored device_queue --nocapture`
#[cfg(feature = "output")]
#[test]
#[ignore = "needs an audio device"]
fn device_queue_smoke() {
    use wholeseek_audio::Output;
    let dir = tempfile::tempdir().unwrap();
    let tracks = album(dir.path(), &[4, 5]);
    let player = Player::new(Output::Device);
    let rx = player.subscribe();
    let t0 = Instant::now();
    player.set_queue(items(&tracks), 0);
    let events = until_queue_end(&rx, Duration::from_secs(20));
    println!("[cp2] device: {}", brief(&events));
    let first = events
        .iter()
        .find(|(_, e)| matches!(e, PlayerEvent::Started { .. }))
        .map(|(t, _)| t.duration_since(t0));
    println!("[cp2] device: first audio {first:?} after set_queue");
    let g = gaps(&events);
    println!(
        "[cp2] device: inter-track gap TrackEnded(0)→Started(1) = {:?} ms",
        g.iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect::<Vec<_>>()
    );
    assert!(
        events
            .iter()
            .all(|(_, e)| !matches!(e, PlayerEvent::Error(_))),
        "device error"
    );
    assert_eq!(indices(&events, track_ended), vec![0, 1]);
    assert_eq!(g.len(), 1);
    assert!(g[0] < Duration::from_millis(300), "gap {:?}", g[0]);
}
