//! CP0 protocol smoke: log in, run one search, print the first 10 audio
//! results (FLAC first), exit.
//!
//! ```sh
//! GOALSEEK_USER=… GOALSEEK_PASS=… cargo run -p goalseek-core --example smoke -- "<query>"
//! ```
//!
//! Optional: `GOALSEEK_SEARCH_SECS` (default 12), `GOALSEEK_LISTEN_PORT`
//! (default 2234). Credentials come only from the environment and are never
//! printed. Talks to the live Soulseek network — run only with permission.

use goalseek_core::{CLIENT_VERSION, rank};
use soulseek_rs::{Client, ClientSettings};
use std::process::ExitCode;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let query = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if query.trim().is_empty() {
        eprintln!("usage: smoke <query>   (needs GOALSEEK_USER / GOALSEEK_PASS)");
        return ExitCode::from(2);
    }
    let (Ok(user), Ok(pass)) = (
        std::env::var("GOALSEEK_USER"),
        std::env::var("GOALSEEK_PASS"),
    ) else {
        eprintln!("GOALSEEK_USER and GOALSEEK_PASS must be set");
        return ExitCode::from(2);
    };
    let secs = env_num("GOALSEEK_SEARCH_SECS", 12);
    let port = u16::try_from(env_num("GOALSEEK_LISTEN_PORT", 2234)).unwrap_or(2234);

    let settings = ClientSettings {
        listen_port: port,
        version: CLIENT_VERSION,
        ..ClientSettings::new(user.clone(), pass)
    };
    let mut client = Client::with_settings(settings);

    let t0 = Instant::now();
    if let Err(e) = client.connect() {
        eprintln!("connect failed: {e}");
        return ExitCode::FAILURE;
    }
    match client.login() {
        Ok(true) => println!(
            "logged in as {user} in {:.1}s (listen port {:?})",
            t0.elapsed().as_secs_f32(),
            client.listen_port()
        ),
        Ok(false) => {
            eprintln!("login refused");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("login failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    println!("searching {query:?} for {secs}s …");
    let results = match client.search(&query, Duration::from_secs(secs)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("search failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(loss) = client.session_loss() {
        eprintln!("session lost during search: {loss}");
    }

    let mut hits = rank::hits_from_results(&results);
    let total = hits.len();
    rank::rank(&mut hits, true);
    println!(
        "{} responses, {total} files, {} audio; top 10:",
        results.len(),
        hits.len()
    );
    for (i, h) in hits.iter().take(10).enumerate() {
        let a = &h.file.attrs;
        let quality = match (a.bit_depth, a.sample_rate_hz, a.bitrate_kbps) {
            (Some(b), Some(s), _) => format!("{b}/{:.1}", f64::from(s) / 1000.0),
            (_, _, Some(k)) => format!("{k}kbps"),
            _ => "-".into(),
        };
        println!(
            "{:>2}. [{:<4}] {:<9} {:>7.1} MB  slot:{} {:>6} KB/s  {}  {}",
            i + 1,
            h.file.extension(),
            quality,
            h.file.size as f64 / 1_048_576.0,
            if h.free_slot { "free" } else { "busy" },
            h.speed / 1024,
            h.file.username,
            h.file.path,
        );
    }

    let _ = client.forget_search(&query);
    client.disconnect();
    ExitCode::SUCCESS
}

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
