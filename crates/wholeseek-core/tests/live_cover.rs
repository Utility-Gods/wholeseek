//! Live: fetch one real album folder's cover from a peer. `#[ignore]`d; needs a throwaway
//! account in WHOLESEEK_USER / WHOLESEEK_PASS (never your own) and `--nocapture`.
//! Optional WHOLESEEK_QUERY (default "aphex twin"), WHOLESEEK_LISTEN_PORT (default 2245).
//! Asks at most 6 folders, one at a time (one peer each); nothing is written outside a temp dir.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use wholeseek_core::cover::{CoverState, Covers};
use wholeseek_core::{Core, CoreEvent, Credentials, Settings};

#[test]
#[ignore = "live network; throwaway account only"]
fn live_fetch_one_folder_cover() {
    let creds = Credentials {
        username: std::env::var("WHOLESEEK_USER").expect("WHOLESEEK_USER not set"),
        password: std::env::var("WHOLESEEK_PASS").expect("WHOLESEEK_PASS not set"),
    };
    let query = std::env::var("WHOLESEEK_QUERY").unwrap_or_else(|_| "aphex twin".into());
    let port = std::env::var("WHOLESEEK_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2245);
    let tmp = tempfile::tempdir().unwrap();
    let (core, ev) = Core::start(Settings {
        download_dir: tmp.path().join("downloads"),
        cache_dir: tmp.path().join("cache"),
        data_dir: tmp.path().join("data"),
        listen_port: port,
        port_mapping: false,
        ..Settings::default()
    })
    .unwrap();
    core.login(creds);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match ev.recv_timeout(Duration::from_secs(1)) {
            Ok(CoreEvent::Connected { .. }) => break,
            Ok(CoreEvent::Disconnected { reason, .. }) => panic!("login: {reason:?}"),
            _ => assert!(Instant::now() < deadline, "no login"),
        }
    }
    println!("[cover] logged in; searching {query:?}");
    let id = core.search(&query);
    // Folders whose search hits include a cover-ish image, free slot first.
    let mut folders: Vec<(bool, u32, String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if let Ok(CoreEvent::SearchResults { search, hits }) =
            ev.recv_timeout(Duration::from_millis(500))
            && search == id
        {
            for h in hits {
                let key = (h.file.username.clone(), h.file.folder().to_string());
                if h.file.extension() == "flac" && seen.insert(key.clone()) {
                    folders.push((!h.free_slot, u32::MAX - h.speed, key.0, key.1));
                }
            }
        }
    }
    core.cancel_search(id);
    folders.sort();
    println!("[cover] {} candidate folders", folders.len());
    let covers: Covers = core.covers().clone();
    let mut users = BTreeSet::new();
    for (_, _, user, folder) in folders
        .into_iter()
        .filter(|f| users.insert(f.2.clone()))
        .take(6)
    {
        let t0 = Instant::now();
        println!(
            "[cover] asking a peer for folder …\\{}",
            folder.rsplit('\\').next().unwrap_or("")
        );
        core.fetch_cover(&user, &folder, None);
        let state = loop {
            let _ = ev.recv_timeout(Duration::from_millis(200));
            let s = covers.state(&user, &folder);
            if !matches!(s, CoverState::Pending | CoverState::Unknown)
                || t0.elapsed() > Duration::from_secs(150)
            {
                break s;
            }
        };
        println!(
            "[cover]   → {state:?} after {:.1} s",
            t0.elapsed().as_secs_f64()
        );
        if let CoverState::Ready(path) = state {
            let bytes = std::fs::read(&path).unwrap();
            let img = image_size(&bytes);
            println!(
                "[cover] OK: {} bytes, {img:?} px, cached in {}",
                bytes.len(),
                path.display()
            );
            assert!(path.starts_with(tmp.path()), "cache stays in the cache dir");
            assert!(
                !tmp.path().join("downloads").exists()
                    || std::fs::read_dir(tmp.path().join("downloads"))
                        .unwrap()
                        .next()
                        .is_none(),
                "downloads folder untouched"
            );
            let out = std::env::var_os("WHOLESEEK_COVER_OUT");
            if let Some(out) = out {
                std::fs::copy(&path, out).unwrap();
            }
            return;
        }
    }
    panic!("no folder cover fetched from any of the candidates");
}

/// (width, height) of a baseline JPEG, from its SOF0/SOF2 marker.
fn image_size(b: &[u8]) -> Option<(u16, u16)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(marker, 0xC0 | 0xC2) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}
