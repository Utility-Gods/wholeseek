//! Live-network check of the whole `Core`. Ignored by default; needs
//! permission and GOALSEEK_USER / GOALSEEK_PASS in the environment:
//!
//! ```sh
//! cargo test -p goalseek-core --test live -- --ignored --nocapture
//! ```
//!
//! Optional: GOALSEEK_QUERY (default "aphex twin"), GOALSEEK_LISTEN_PORT
//! (default 2234). Uses temp download/cache dirs; credentials are never printed.

use goalseek_core::{Core, CoreEvent, Credentials, SearchHit, Settings};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

fn creds() -> Credentials {
    Credentials {
        username: std::env::var("GOALSEEK_USER").expect("GOALSEEK_USER not set"),
        password: std::env::var("GOALSEEK_PASS").expect("GOALSEEK_PASS not set"),
    }
}

/// Next event matching `pred` within `timeout`, logging the rest briefly.
fn want(
    ev: &Receiver<CoreEvent>,
    timeout: Duration,
    what: &str,
    pred: impl Fn(&CoreEvent) -> bool,
) -> Option<CoreEvent> {
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match ev.recv_timeout(left) {
            Ok(e) if pred(&e) => return Some(e),
            Ok(CoreEvent::Error { context, message }) => println!("  [error {context}] {message}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    println!("  (no {what} within {}s)", timeout.as_secs());
    None
}

fn files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
#[ignore = "live Soulseek network; needs GOALSEEK_USER/GOALSEEK_PASS and permission"]
fn live_core_end_to_end() {
    let creds = creds();
    let query = std::env::var("GOALSEEK_QUERY").unwrap_or_else(|_| "aphex twin".into());
    let port = std::env::var("GOALSEEK_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2234);
    let tmp = tempfile::tempdir().unwrap();
    let downloads = tmp.path().join("downloads");
    let settings = Settings {
        download_dir: downloads.clone(),
        cache_dir: tmp.path().join("cache"),
        listen_port: port,
        ..Settings::default()
    };
    let (core, ev) = Core::start(settings).expect("start");

    // 1. Login.
    let t0 = Instant::now();
    core.login(creds);
    let e = want(&ev, Duration::from_secs(60), "Connected", |e| {
        matches!(
            e,
            CoreEvent::Connected { .. } | CoreEvent::Disconnected { .. }
        )
    })
    .expect("login answer");
    let CoreEvent::Connected { listen_port, .. } = e else {
        panic!("login failed: {e:?}")
    };
    println!(
        "1. logged in in {:.1}s, listen port {listen_port:?}",
        t0.elapsed().as_secs_f32()
    );

    // 2. Search; collect until finished.
    let t1 = Instant::now();
    let id = core.search(&query);
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut first_batch = None;
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        match ev.recv_timeout(Duration::from_millis(500)) {
            Ok(CoreEvent::SearchResults { search, hits: h }) if search == id => {
                first_batch.get_or_insert(t1.elapsed());
                hits.extend(h);
            }
            Ok(CoreEvent::SearchFinished {
                search,
                total_files,
            }) if search == id => {
                println!(
                    "2. search {query:?}: {total_files} audio files in {:.1}s (first batch after {:.1}s)",
                    t1.elapsed().as_secs_f32(),
                    first_batch.unwrap_or_default().as_secs_f32()
                );
                break;
            }
            _ => {}
        }
    }
    goalseek_core::rank::rank(&mut hits, true);
    let flac: Vec<&SearchHit> = hits
        .iter()
        .filter(|h| h.file.path.to_lowercase().ends_with(".flac"))
        .collect();
    println!("   top FLAC hits:");
    for h in flac.iter().take(10) {
        println!(
            "   - {} | {} | {} B | slots={} speed={}",
            h.file.username, h.file.path, h.file.size, h.free_slot, h.speed
        );
    }
    assert!(!hits.is_empty(), "expected some audio results");

    // 3. Browse the owner of the top hit.
    let top = flac.first().copied().unwrap_or(&hits[0]).clone();
    let t2 = Instant::now();
    core.browse(&top.file.username);
    match want(&ev, Duration::from_secs(70), "browse answer", |e| {
        matches!(
            e,
            CoreEvent::BrowseResult { .. } | CoreEvent::BrowseFailed { .. }
        )
    }) {
        Some(CoreEvent::BrowseResult { username, dirs }) => println!(
            "3. browsed {username}: {} dirs, {} files in {:.1}s",
            dirs.len(),
            dirs.iter().map(|d| d.files.len()).sum::<usize>(),
            t2.elapsed().as_secs_f32()
        ),
        other => println!("3. browse: {other:?}"),
    }

    // 4. Play-fetch candidates (top FLAC first) until one sends bytes.
    let mut started = false;
    for cand in flac.iter().copied().chain(hits.iter()).take(5) {
        let t3 = Instant::now();
        let ticket = core.play_fetch(&cand.file);
        println!(
            "4. play_fetch {} from {} → {}",
            cand.file.path,
            cand.file.username,
            ticket.part_path.display()
        );
        let e = want(
            &ev,
            Duration::from_secs(45),
            "first progress",
            |e| match e {
                CoreEvent::TransferProgress { id, bytes, .. } => *id == ticket.id && *bytes > 0,
                CoreEvent::TransferFailed { id, .. } | CoreEvent::TransferDone { id, .. } => {
                    *id == ticket.id
                }
                _ => false,
            },
        );
        match e {
            Some(CoreEvent::TransferProgress { bytes, total, .. }) => {
                println!(
                    "   first TransferProgress after {:.2}s ({bytes}/{total} B)",
                    t3.elapsed().as_secs_f32()
                );
                assert!(ticket.part_path.exists() || ticket.final_path.exists());
                core.cancel(ticket.id);
                want(&ev, Duration::from_secs(5), "TransferCancelled", |e| {
                    *e == CoreEvent::TransferCancelled { id: ticket.id }
                });
                println!("   cancelled");
                started = true;
                break;
            }
            other => {
                println!("   did not start: {other:?}");
                core.cancel(ticket.id);
            }
        }
    }

    // 5. Nothing may land in the downloads dir.
    let leaked = files_under(&downloads);
    println!("5. files under downloads dir: {}", leaked.len());
    assert!(leaked.is_empty(), "play wrote to downloads: {leaked:?}");
    core.logout();
    assert!(started, "no candidate started sending within the timeout");
}
