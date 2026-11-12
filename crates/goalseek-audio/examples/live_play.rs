//! End-to-end ▶: log in, search, pick the top FLAC hit, fetch it into the cache and play it
//! through the real output device while it downloads.
//!
//! ```sh
//! GOALSEEK_USER=… GOALSEEK_PASS=… cargo run -p goalseek-audio --example live_play -- "<query>"
//! ```
//!
//! Optional env: `GOALSEEK_PLAY_SECS` (how long to listen, default 30), `GOALSEEK_SEARCH_SECS`
//! (max wait for results, default 15), `GOALSEEK_LISTEN_PORT` (default 2234),
//! `GOALSEEK_WORK_DIR` (where the cache and the (watched, must-stay-empty) downloads dir live;
//! default a fresh dir under the system temp dir). Credentials come only from the environment
//! and are never printed. Talks to the live Soulseek network — run only with permission.
//!
//! Prints time-to-first-audio (from ▶ and from the first downloaded byte) and the cache path,
//! and asserts nothing was written to the downloads dir.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use goalseek_audio::{Output, PlayState, Player, PlayerEvent};
use goalseek_core::{Core, CoreEvent, Credentials, PlayTicket, SearchHit, Settings};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("live_play: {e}");
            ExitCode::FAILURE
        }
    }
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// All file paths below `dir` (relative), for the "nothing in downloads" check.
fn list_files(dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(dir).unwrap_or(&p).to_path_buf());
            }
        }
    }
    out
}

/// Wait for a core event matching `f`, failing on session errors.
fn wait_core<T>(
    rx: &Receiver<CoreEvent>,
    timeout: Duration,
    what: &str,
    mut f: impl FnMut(CoreEvent) -> Option<T>,
) -> Result<T, String> {
    let end = Instant::now() + timeout;
    loop {
        let left = end
            .checked_duration_since(Instant::now())
            .ok_or_else(|| format!("timed out waiting for {what}"))?;
        match rx.recv_timeout(left) {
            Ok(CoreEvent::Disconnected { reason, .. }) => {
                return Err(format!("disconnected while waiting for {what}: {reason:?}"));
            }
            Ok(CoreEvent::Error { context, message }) => {
                return Err(format!(
                    "core error ({context}) while waiting for {what}: {message}"
                ));
            }
            Ok(ev) => {
                if let Some(v) = f(ev) {
                    return Ok(v);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!("timed out waiting for {what}"));
            }
            Err(RecvTimeoutError::Disconnected) => return Err("core stopped".into()),
        }
    }
}

fn pick_flac(hits: &[SearchHit]) -> Option<SearchHit> {
    hits.iter()
        .filter(|h| h.file.extension() == "flac" && h.file.size > 0)
        .max_by_key(|h| (h.free_slot, h.speed))
        .cloned()
}

fn run() -> Result<(), String> {
    let query = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if query.trim().is_empty() {
        return Err("usage: live_play <query>   (needs GOALSEEK_USER / GOALSEEK_PASS)".into());
    }
    let (Ok(username), Ok(password)) = (
        std::env::var("GOALSEEK_USER"),
        std::env::var("GOALSEEK_PASS"),
    ) else {
        return Err("GOALSEEK_USER and GOALSEEK_PASS must be set".into());
    };
    let play_secs = env_num("GOALSEEK_PLAY_SECS", 30);
    let search_secs = env_num("GOALSEEK_SEARCH_SECS", 15);

    let work = std::env::var_os("GOALSEEK_WORK_DIR").map_or_else(
        || std::env::temp_dir().join(format!("goalseek-live-{}", std::process::id())),
        PathBuf::from,
    );
    let settings = Settings {
        download_dir: work.join("downloads"),
        cache_dir: work.join("cache"),
        listen_port: u16::try_from(env_num("GOALSEEK_LISTEN_PORT", 2234)).unwrap_or(2234),
        ..Settings::default()
    };
    std::fs::create_dir_all(&settings.download_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&settings.cache_dir).map_err(|e| e.to_string())?;
    let downloads_before = list_files(&settings.download_dir);
    println!("work dir: {}", work.display());

    let (core, events) = Core::start(settings.clone()).map_err(|e| e.to_string())?;
    let t_login = Instant::now();
    core.login(Credentials { username, password });
    let port = wait_core(&events, Duration::from_secs(60), "login", |ev| match ev {
        CoreEvent::Connected { listen_port, .. } => Some(listen_port),
        _ => None,
    })?;
    println!(
        "logged in in {:.1} s (listen port {port:?})",
        t_login.elapsed().as_secs_f64()
    );

    // Search: take batches until the window closes or a FLAC with a free slot shows up and the
    // search has run a few seconds.
    let t_search = Instant::now();
    let search = core.search(&query);
    let mut hits: Vec<SearchHit> = Vec::new();
    wait_core(
        &events,
        Duration::from_secs(search_secs),
        "search results",
        |ev| match ev {
            CoreEvent::SearchResults { search: s, hits: h } if s == search => {
                hits.extend(h);
                let good = hits
                    .iter()
                    .any(|h| h.free_slot && h.file.extension() == "flac");
                (good && t_search.elapsed() > Duration::from_secs(4)).then_some(())
            }
            CoreEvent::SearchFinished { search: s, .. } if s == search => Some(()),
            _ => None,
        },
    )
    .or_else(|e| if hits.is_empty() { Err(e) } else { Ok(()) })?;
    core.cancel_search(search);
    let hit = pick_flac(&hits).ok_or_else(|| {
        format!(
            "no FLAC among {} hits for {query:?} after {:.1} s",
            hits.len(),
            t_search.elapsed().as_secs_f64()
        )
    })?;
    println!(
        "picked: {} — {} ({} bytes, free slot {}, speed {} B/s) from {} hits",
        hit.file.username,
        hit.file.path,
        hit.file.size,
        hit.free_slot,
        hit.speed,
        hits.len()
    );

    // ▶
    let player = Player::new(Output::Device);
    let player_events = player.subscribe();
    let t_play = Instant::now();
    let ticket: PlayTicket = core.play_fetch(&hit.file);
    println!("cache path: {}", ticket.part_path.display());
    if ticket.part_path.starts_with(&settings.download_dir)
        || ticket.final_path.starts_with(&settings.download_dir)
        || !ticket.part_path.starts_with(&settings.cache_dir)
    {
        return Err("play_fetch must target the cache dir, not downloads".into());
    }

    let mut t_first_byte: Option<Instant> = None;
    let mut t_first_audio: Option<Instant> = None;
    let mut done_path: Option<PathBuf> = None;
    let mut last_print = Instant::now();
    let deadline_no_audio = Duration::from_secs(120);
    let mut outcome: Result<(), String> = Ok(());

    loop {
        // Core events (transfer progress) …
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(CoreEvent::TransferQueued { id, .. }) if id == ticket.id => {
                println!("[{:>6.2}s] queued", t_play.elapsed().as_secs_f64());
            }
            Ok(CoreEvent::TransferRemoteQueued { id, place }) if id == ticket.id => {
                println!(
                    "[{:>6.2}s] remote queue place {place:?}",
                    t_play.elapsed().as_secs_f64()
                );
            }
            Ok(CoreEvent::TransferProgress {
                id, bytes, total, ..
            }) if id == ticket.id => {
                if t_first_byte.is_none() {
                    t_first_byte = Some(Instant::now());
                    println!(
                        "[{:>6.2}s] transfer started ({bytes}/{total} B); starting player",
                        t_play.elapsed().as_secs_f64()
                    );
                    player.play(&ticket.final_path, total.max(hit.file.size));
                }
                if let Some(h) = player.source_handle() {
                    if total > 0 && total != h.total_size() {
                        h.set_total_size(total);
                    }
                    h.notify();
                }
            }
            Ok(CoreEvent::TransferDone { id, path }) if id == ticket.id => {
                println!(
                    "[{:>6.2}s] transfer complete: {}",
                    t_play.elapsed().as_secs_f64(),
                    path.display()
                );
                if t_first_byte.is_none() {
                    t_first_byte = Some(Instant::now());
                    player.play(&path, std::fs::metadata(&path).map_or(0, |m| m.len()));
                }
                done_path = Some(path);
            }
            Ok(CoreEvent::TransferFailed { id, reason }) if id == ticket.id => {
                println!(
                    "[{:>6.2}s] transfer failed: {reason}",
                    t_play.elapsed().as_secs_f64()
                );
                if t_first_byte.is_none() {
                    outcome = Err(format!("transfer failed before any data: {reason}"));
                    break;
                }
                player.fail_source(reason);
            }
            Ok(CoreEvent::TransferCancelled { id }) if id == ticket.id => {
                player.fail_source("transfer cancelled");
            }
            Ok(CoreEvent::Disconnected { reason, .. }) => {
                player.fail_source(format!("disconnected: {reason:?}"));
            }
            Ok(CoreEvent::Error { context, message }) => {
                eprintln!("core error ({context}): {message}");
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                outcome = Err("core stopped".into());
                break;
            }
        }

        // … and player events.
        let mut finished = false;
        while let Ok(ev) = player_events.try_recv() {
            match ev {
                PlayerEvent::Started { duration_secs, .. } => {
                    let now = Instant::now();
                    t_first_audio = Some(now);
                    println!(
                        "[{:>6.2}s] FIRST AUDIO: {:.2} s after ▶, {:.2} s after first byte \
                         (duration {:?})",
                        t_play.elapsed().as_secs_f64(),
                        (now - t_play).as_secs_f64(),
                        t_first_byte.map_or(f64::NAN, |t| (now - t).as_secs_f64()),
                        duration_secs
                    );
                }
                PlayerEvent::PositionTick { .. } => {}
                PlayerEvent::Ended => {
                    println!("[{:>6.2}s] ended", t_play.elapsed().as_secs_f64());
                    finished = true;
                }
                PlayerEvent::Error(e) => {
                    outcome = Err(format!("player error: {e}"));
                    finished = true;
                }
                other => println!(
                    "[{:>6.2}s] player: {other:?}",
                    t_play.elapsed().as_secs_f64()
                ),
            }
        }
        if finished {
            break;
        }
        if last_print.elapsed() >= Duration::from_secs(2) {
            last_print = Instant::now();
            let s = player.snapshot();
            if s.state != PlayState::Idle {
                println!(
                    "[{:>6.2}s] {:?} {:.1}/{} s, downloaded {:.1}%{}",
                    t_play.elapsed().as_secs_f64(),
                    s.state,
                    s.position_secs,
                    s.duration_secs.map_or("?".into(), |d| format!("{d:.1}")),
                    s.buffered_fraction * 100.0,
                    s.notice.map(|n| format!(" — {n}")).unwrap_or_default()
                );
            }
        }
        match t_first_audio {
            Some(t) if t.elapsed() >= Duration::from_secs(play_secs) => break,
            None if t_play.elapsed() >= deadline_no_audio => {
                outcome = Err(format!(
                    "no audio within {} s of ▶",
                    deadline_no_audio.as_secs()
                ));
                break;
            }
            _ => {}
        }
    }
    player.stop();
    if done_path.is_none() {
        core.cancel(ticket.id);
    }
    core.logout();

    let downloads_after = list_files(&settings.download_dir);
    let new: Vec<_> = downloads_after.difference(&downloads_before).collect();
    if !new.is_empty() {
        return Err(format!("playing wrote to the downloads dir: {new:?}"));
    }
    println!(
        "downloads dir unchanged ({} files) ✓; cache: {}",
        downloads_after.len(),
        done_path.as_ref().unwrap_or(&ticket.part_path).display()
    );
    if let (Some(a), Some(b)) = (t_first_audio, t_first_byte) {
        println!(
            "time-to-first-audio: {:.2} s from ▶ ({:.2} s from first byte)",
            (a - t_play).as_secs_f64(),
            (a - b).as_secs_f64()
        );
    }
    outcome
}
