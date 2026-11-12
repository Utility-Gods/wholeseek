//! Live-network check of the whole `Core`. Ignored by default; needs
//! permission and WHOLESEEK_USER / WHOLESEEK_PASS in the environment:
//!
//! ```sh
//! cargo test -p wholeseek-core --test live -- --ignored --nocapture
//! ```
//!
//! Optional: WHOLESEEK_QUERY (default "aphex twin"), WHOLESEEK_LISTEN_PORT
//! (default 2234). Uses temp download/cache dirs; credentials are never printed.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use wholeseek_core::{Core, CoreEvent, Credentials, SearchHit, Settings};

fn creds() -> Credentials {
    Credentials {
        username: std::env::var("WHOLESEEK_USER").expect("WHOLESEEK_USER not set"),
        password: std::env::var("WHOLESEEK_PASS").expect("WHOLESEEK_PASS not set"),
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
#[ignore = "live Soulseek network; needs WHOLESEEK_USER/WHOLESEEK_PASS and permission"]
fn live_core_end_to_end() {
    let creds = creds();
    let query = std::env::var("WHOLESEEK_QUERY").unwrap_or_else(|_| "aphex twin".into());
    let port = std::env::var("WHOLESEEK_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2234);
    let tmp = tempfile::tempdir().unwrap();
    let downloads = tmp.path().join("downloads");
    let settings = Settings {
        download_dir: downloads.clone(),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
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
    wholeseek_core::rank::rank(&mut hits, true);
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

    // 3b. Profile of the browsed user (server presence/stats + peer UserInfo).
    let t_p = Instant::now();
    core.user_profile(&top.file.username);
    let mut last = None;
    let deadline = Instant::now() + Duration::from_secs(12);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match ev.recv_timeout(left) {
            Ok(CoreEvent::UserProfile { username, profile }) if username == top.file.username => {
                println!(
                    "3b. profile of {username} after {:.1}s: online={:?} avg_speed={:?} B/s files={:?} dirs={:?} \
                     slots={:?} free_slot={:?} queue={:?} picture={:?} description={:?}",
                    t_p.elapsed().as_secs_f32(),
                    profile.online,
                    profile.avg_speed,
                    profile.shared_files,
                    profile.shared_dirs,
                    profile.upload_slots,
                    profile.free_slot,
                    profile.queue_size,
                    profile.picture.as_ref().map(Vec::len),
                    profile
                        .description
                        .as_deref()
                        .map(|d| d.chars().take(120).collect::<String>()),
                );
                last = Some(profile);
            }
            Ok(CoreEvent::UserProfileFailed { username, reason })
                if username == top.file.username =>
            {
                println!("3b. profile of {username} failed: {reason}");
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let profile = last.expect("no UserProfile event for the browsed user");
    assert!(
        profile.online.is_some() || profile.shared_files.is_some(),
        "server part missing"
    );

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

fn env2(user: &str, pass: &str) -> Credentials {
    Credentials {
        username: std::env::var(user).unwrap_or_else(|_| panic!("{user} not set")),
        password: std::env::var(pass).unwrap_or_else(|_| panic!("{pass} not set")),
    }
}

fn port(var: &str, default: u16) -> u16 {
    std::env::var(var)
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(default)
}

/// Network citizenship: account A shares a real FLAC, account B browses A,
/// downloads it, and the bytes must match; A must report the upload.
///
/// Needs WHOLESEEK_USER/WHOLESEEK_PASS (A) and WHOLESEEK_USER2/WHOLESEEK_PASS2
/// (B); ports WHOLESEEK_PORT_A (2241) / WHOLESEEK_PORT_B (2242); ffmpeg on PATH.
#[test]
#[ignore = "live Soulseek network; needs two accounts and permission"]
fn live_share_upload_between_two_accounts() {
    let a_creds = env2("WHOLESEEK_USER", "WHOLESEEK_PASS");
    let b_creds = env2("WHOLESEEK_USER2", "WHOLESEEK_PASS2");
    assert_ne!(a_creds.username, b_creds.username);
    let a_user = a_creds.username.clone();
    let b_user = b_creds.username.clone();
    let tmp = tempfile::tempdir().unwrap();

    // A small real FLAC (3 s, 440 Hz) in a shared folder.
    let share = tmp.path().join("share");
    let album = share.join("wholeseek share test");
    std::fs::create_dir_all(&album).unwrap();
    let flac = album.join("01 Tone.flac");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:a",
            "flac",
        ])
        .arg(&flac)
        .status()
        .expect("ffmpeg");
    assert!(ok.success());
    let original = std::fs::read(&flac).unwrap();
    println!(
        "0. generated {} ({} B)",
        flac.file_name().unwrap().to_string_lossy(),
        original.len()
    );

    let mk = |name: &str, port: u16, shares: Vec<std::path::PathBuf>| Settings {
        download_dir: tmp.path().join(name).join("downloads"),
        cache_dir: tmp.path().join(name).join("cache"),
        data_dir: tmp.path().join(name).join("data"),
        share_dirs: shares,
        listen_port: port,
        ..Settings::default()
    };
    let (a, a_ev) =
        Core::start(mk("a", port("WHOLESEEK_PORT_A", 2241), vec![share.clone()])).unwrap();
    let (b, b_ev) = Core::start(mk("b", port("WHOLESEEK_PORT_B", 2242), Vec::new())).unwrap();

    // 1. Both log in. (Throwaway usernames may appear in failure messages.)
    for (label, core, ev, creds) in [("A", &a, &a_ev, a_creds), ("B", &b, &b_ev, b_creds)] {
        let t = Instant::now();
        core.login(creds);
        let e = want(ev, Duration::from_secs(60), "login", |e| {
            matches!(
                e,
                CoreEvent::Connected { .. } | CoreEvent::Disconnected { .. }
            )
        })
        .expect("login answer");
        let CoreEvent::Connected { listen_port, .. } = e else {
            panic!("{label} login failed: {e:?}")
        };
        println!(
            "1. {label} logged in in {:.1}s, listen port {listen_port:?}",
            t.elapsed().as_secs_f32()
        );
    }
    let mut a_seen: Vec<CoreEvent> = Vec::new();
    if let Some(CoreEvent::ShareCounts { folders, files }) = want(
        &a_ev,
        Duration::from_secs(10),
        "A share counts",
        |e| matches!(e, CoreEvent::ShareCounts { files, .. } if *files > 0),
    ) {
        println!("1. A shares {folders} folder(s), {files} file(s)");
    }
    // Port mapping outcomes tell whether peers can reach A/B at all.
    for (label, ev) in [("A", &a_ev), ("B", &b_ev)] {
        if let Some(CoreEvent::PortMapping { port, outcome }) =
            want(ev, Duration::from_secs(8), "port mapping", |e| {
                matches!(e, CoreEvent::PortMapping { .. })
            })
        {
            println!("1. {label} port mapping for {port}: {outcome:?}");
        }
    }

    // 2. B browses A and finds the file.
    let t = Instant::now();
    b.browse(&a_user);
    let e = want(&b_ev, Duration::from_secs(70), "browse", |e| {
        matches!(
            e,
            CoreEvent::BrowseResult { .. } | CoreEvent::BrowseFailed { .. }
        )
    })
    .expect("browse answer");
    let CoreEvent::BrowseResult { dirs, .. } = e else {
        panic!(
            "browse failed: {e:?}\n   A and B run behind the same NAT: B can only reach A if A's \
             listen port is reachable at A's public address (router port mapping or a manual \
             forward, plus NAT hairpinning). See the port mapping lines above."
        )
    };
    let file = dirs
        .iter()
        .flat_map(|d| d.files.iter())
        .find(|f| f.path.ends_with("01 Tone.flac"))
        .cloned()
        .expect("shared FLAC not in A's listing");
    println!(
        "2. B browsed A in {:.1}s: {} dirs; found {} ({} B, duration {:?}s)",
        t.elapsed().as_secs_f32(),
        dirs.len(),
        file.path,
        file.size,
        file.attrs.duration_secs
    );
    assert_eq!(file.size, original.len() as u64);

    // 3. B downloads it.
    let t = Instant::now();
    let id = b.download(&file);
    let e = want(
        &b_ev,
        Duration::from_secs(90),
        "download end",
        |e| match e {
            CoreEvent::TransferDone { id: i, .. } | CoreEvent::TransferFailed { id: i, .. } => {
                *i == id
            }
            _ => false,
        },
    )
    .expect("download answer");
    let CoreEvent::TransferDone { path, .. } = e else {
        panic!("download failed: {e:?}")
    };
    let got = std::fs::read(&path).unwrap();
    println!(
        "3. B downloaded to {} in {:.1}s; {} B; identical={}",
        path.strip_prefix(tmp.path()).unwrap_or(&path).display(),
        t.elapsed().as_secs_f32(),
        got.len(),
        got == original
    );
    assert_eq!(got, original, "downloaded bytes differ");

    // 4. A reported the upload to B.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut upload = None;
    while upload.is_none() && Instant::now() < deadline {
        if let Ok(e) = a_ev.recv_timeout(Duration::from_millis(200)) {
            if let CoreEvent::Uploads(rows) = &e {
                upload = rows
                    .iter()
                    .find(|r| r.username == b_user && r.path.ends_with("01 Tone.flac"))
                    .cloned();
            }
            a_seen.push(e);
        }
    }
    let uploads_events = a_seen
        .iter()
        .filter(|e| matches!(e, CoreEvent::Uploads(_)))
        .count();
    let counts = a_seen.iter().find_map(|e| match e {
        CoreEvent::ShareCounts { folders, files } => Some((*folders, *files)),
        _ => None,
    });
    println!("4. A: share counts {counts:?}, {uploads_events} Uploads snapshot(s)");
    let row = upload.expect("A emitted no Uploads row for B's download");
    println!(
        "   A upload row: to=B path={} size={} sent={} state={:?}",
        row.path, row.size, row.bytes_sent, row.state
    );
    assert_eq!(row.size, original.len() as u64);

    a.logout();
    b.logout();
}

/// Result counts for query variants (punctuation / accent handling), all
/// variants of one query searched at the same time, two rounds. Prints a table;
/// no user names. Port WHOLESEEK_LISTEN_PORT (default 2243).
#[test]
#[ignore = "live Soulseek network; needs WHOLESEEK_USER/WHOLESEEK_PASS and permission"]
fn live_query_variants() {
    use std::collections::{HashMap, HashSet};
    use wholeseek_core::query::{fold_accents, normalize_query};

    let queries = [
        "wasn't me shaggy",
        "don't stop me now",
        "guns n' roses",
        "ac/dc back in black",
        "p!nk",
        "beyoncé halo",
        "sigur rós",
    ];
    type Variant = (&'static str, fn(&str) -> String);
    let variants: [Variant; 5] = [
        ("typed", |q| q.trim().to_string()),
        ("no_apostrophe", |q| q.replace(['\'', '’'], "")),
        ("punct_space", normalize_query),
        ("punct_space_drop1", |q| {
            normalize_query(q)
                .split(' ')
                .filter(|w| w.chars().count() > 1)
                .collect::<Vec<_>>()
                .join(" ")
        }),
        ("fold_accents", |q| fold_accents(&normalize_query(q))),
    ];
    let port = std::env::var("WHOLESEEK_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2243);
    let tmp = tempfile::tempdir().unwrap();
    let settings = Settings {
        download_dir: tmp.path().join("downloads"),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
        listen_port: port,
        ..Settings::default()
    };
    let (core, ev) = Core::start(settings).expect("start");
    core.login(creds());
    let e = want(&ev, Duration::from_secs(60), "Connected", |e| {
        matches!(
            e,
            CoreEvent::Connected { .. } | CoreEvent::Disconnected { .. }
        )
    })
    .expect("login answer");
    assert!(
        matches!(e, CoreEvent::Connected { .. }),
        "login failed: {e:?}"
    );

    // (query, variant) -> per round (users, files)
    let mut table: HashMap<(usize, usize), Vec<(usize, usize)>> = HashMap::new();
    for round in 0..2 {
        for (qi, q) in queries.iter().enumerate() {
            // Distinct strings only; rotate start order per round.
            let mut sent: Vec<(String, Vec<usize>)> = Vec::new();
            for k in 0..variants.len() {
                let vi = (k + round) % variants.len();
                let s = (variants[vi].1)(q);
                match sent.iter_mut().find(|(t, _)| *t == s) {
                    Some((_, vis)) => vis.push(vi),
                    None => sent.push((s, vec![vi])),
                }
            }
            let mut ids = HashMap::new();
            for (s, vis) in &sent {
                ids.insert(core.search(s), vis.clone());
            }
            let mut users: HashMap<_, HashSet<String>> = HashMap::new();
            let mut files: HashMap<_, usize> = HashMap::new();
            let mut open = ids.len();
            let deadline = Instant::now() + Duration::from_secs(40);
            while open > 0 {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                match ev.recv_timeout(left) {
                    Ok(CoreEvent::SearchResults { search, hits }) if ids.contains_key(&search) => {
                        *files.entry(search).or_default() += hits.len();
                        let u = users.entry(search).or_default();
                        u.extend(hits.into_iter().map(|h| h.file.username));
                    }
                    Ok(CoreEvent::SearchFinished { search, .. }) if ids.contains_key(&search) => {
                        open -= 1;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            for (id, vis) in &ids {
                let row = (
                    users.get(id).map_or(0, HashSet::len),
                    files.get(id).copied().unwrap_or(0),
                );
                for vi in vis {
                    table.entry((qi, *vi)).or_default().push(row);
                }
            }
            println!(
                "round {} query {qi} done ({} distinct variants)",
                round + 1,
                sent.len()
            );
        }
    }
    println!("\nusers/files per round (r1 | r2); identical strings share one search");
    println!(
        "{:<22} {}",
        "query",
        variants.map(|v| format!("{:<26}", v.0)).join("")
    );
    for (qi, q) in queries.iter().enumerate() {
        let mut line = format!("{q:<22} ");
        for vi in 0..variants.len() {
            let r = &table[&(qi, vi)];
            let s = (variants[vi].1)(q);
            let cell = r
                .iter()
                .map(|(u, f)| format!("{u}/{f}"))
                .collect::<Vec<_>>()
                .join(" | ");
            line.push_str(&format!("{:<26}", format!("{cell} «{s}»")));
        }
        println!("{line}");
    }
}

/// When do search responses arrive? Five queries at once over a 60 s window,
/// straight on the library (no Core window), polled every 250 ms. Prints the
/// share of responses / files that arrived after 5, 10, 15, 20, 30 s.
#[test]
#[ignore = "live Soulseek network; needs WHOLESEEK_USER/WHOLESEEK_PASS and permission"]
fn live_search_arrival_times() {
    use soulseek_rs::{Client, ClientSettings};
    let queries = [
        "aphex twin",
        "daft punk discovery",
        "radiohead ok computer",
        "miles davis kind of blue",
        "shaggy wasn t me",
    ];
    let port = std::env::var("WHOLESEEK_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2243);
    let c = creds();
    let mut client = Client::with_settings(ClientSettings {
        listen_port: port,
        version: wholeseek_core::CLIENT_VERSION,
        ..ClientSettings::new(c.username, c.password)
    });
    client.connect().expect("connect");
    assert!(client.login().expect("login"), "login rejected");
    let client = std::sync::Arc::new(client);
    let window = Duration::from_secs(60);
    let t0 = Instant::now();
    let handles: Vec<_> = queries
        .iter()
        .map(|q| {
            let (c, q) = (client.clone(), q.to_string());
            std::thread::spawn(move || {
                let _ = c.search(&q, window);
            })
        })
        .collect();
    // per query: (arrival secs, files) per response, in arrival order
    let mut arrivals: Vec<Vec<(f32, usize)>> = vec![Vec::new(); queries.len()];
    while t0.elapsed() < window {
        std::thread::sleep(Duration::from_millis(250));
        let at = t0.elapsed().as_secs_f32();
        for (i, q) in queries.iter().enumerate() {
            let r = client.get_search_results(q);
            for res in r.iter().skip(arrivals[i].len()) {
                arrivals[i].push((at, res.files.len()));
            }
        }
    }
    for h in handles {
        let _ = h.join();
    }
    let marks = [5.0f32, 10.0, 15.0, 20.0, 30.0];
    println!(
        "\nquery                      responses  files   % resp after {marks:?}   % files after"
    );
    let mut all: Vec<(f32, usize)> = Vec::new();
    for (i, q) in queries.iter().enumerate() {
        let a = &arrivals[i];
        all.extend(a.iter().copied());
        println!("{q:<26} {}", line(a, &marks));
    }
    println!("{:<26} {}", "ALL", line(&all, &marks));

    fn line(a: &[(f32, usize)], marks: &[f32]) -> String {
        let n = a.len().max(1) as f32;
        let files: usize = a.iter().map(|x| x.1).sum();
        let fl = files.max(1) as f32;
        let pr: Vec<String> = marks
            .iter()
            .map(|m| {
                format!(
                    "{:.0}",
                    100.0 * a.iter().filter(|x| x.0 > *m).count() as f32 / n
                )
            })
            .collect();
        let pf: Vec<String> = marks
            .iter()
            .map(|m| {
                let late: usize = a.iter().filter(|x| x.0 > *m).map(|x| x.1).sum();
                format!("{:.0}", 100.0 * late as f32 / fl)
            })
            .collect();
        let last = a.iter().map(|x| x.0).fold(0.0, f32::max);
        format!(
            "{:>9} {files:>6}   {:<24} {:<20} last at {last:.1}s",
            a.len(),
            pr.join("/"),
            pf.join("/")
        )
    }
}
