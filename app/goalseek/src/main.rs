//! goalseek — a minimal Soulseek client for music (docs/MVP.md).
//!
//! `cargo run -p goalseek`             the real session (goalseek-core + system audio)
//! `cargo run -p goalseek -- --demo`   fake peers and a simulated player: no network, no audio
//!
//! Headless (Rune tooling, same renderer, virtual clock), e.g.
//! `goalseek --demo --scene results --snap results.png --at 4 --size 1100x700 --scale 1`.
//! Scenes (demo): results, browse, transfers, settings, onboarding-error, rejected.

mod app;
mod backend;
mod demo;
mod fmt;
mod playback;
mod profile;
mod store;
mod view;

#[cfg(test)]
mod tests;

use std::sync::OnceLock;
use std::time::Duration;

use goalseek_core::config;
use rune::{AppSpec, Tx};

use app::{App, Panel};

const W: f32 = 1100.0;
const H: f32 = 700.0;

static DEMO: OnceLock<bool> = OnceLock::new();
/// What `config::load()` found, read in `main` before the window exists (the keyring can take
/// seconds; it never runs on the UI thread).
static LOADED: OnceLock<config::Loaded> = OnceLock::new();

fn init() -> App {
    if *DEMO.get().unwrap_or(&false) {
        return App::demo();
    }
    let loaded = LOADED.get().cloned().unwrap_or_else(|| config::Loaded {
        settings: goalseek_core::Settings::default(),
        credentials: None,
        keyring_available: false,
    });
    let startup = app::Startup {
        credentials: loaded.credentials,
        keyring_available: loaded.keyring_available,
    };
    App::real(loaded.settings, startup)
}

/// Each frame: drain core events and refresh the player snapshot; ask for the next wake while
/// something is moving (playback, demo timers, a notice to expire). Never blocks.
pub fn poll(s: &mut App, t: f64, tx: &mut Tx) -> bool {
    let (changed, wake) = s.tick(t);
    if let Some(w) = wake {
        tx.wake_after(Duration::from_secs_f64(w.max(1.0 / 60.0)));
    }
    changed
}

fn setup(s: &mut App, wake: rune::Waker) {
    s.store.set_waker(wake.clone());
    s.profiles.set_waker(wake.clone());
    s.backend.set_waker(wake);
}

// ---- demo scenes (headless screenshots) ------------------------------------------------------

fn demo_file(user: &str, name: &str) -> goalseek_core::RemoteFile {
    demo::catalog()
        .into_iter()
        .find(|f| f.username == user && f.basename() == name)
        .expect("demo file")
}

impl App {
    /// Sign in as the demo user with one share folder.
    pub fn demo_login(&mut self) {
        self.onboarding.username.replace_text("nadia");
        self.onboarding.password.replace_text("demo-password");
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let music = home
            .map(|h| h.join("Music"))
            .filter(|m| m.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        self.onboarding.shares = vec![music];
        self.connect();
    }

    fn demo_search(&mut self) {
        self.demo_login();
        self.search_edit.replace_text("hollow lights");
        self.submit_search();
    }
}

fn scene_results(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "02 Salt Roads.flac"));
    s.download(&demo_file("alice", "03 Low Tide Radio.flac"));
    s.download(&demo_file("vinyl_rips", "A1 Northern Glass.flac"));
}

fn scene_browse(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "02 Salt Roads.flac"));
    let f = demo_file("alice", "03 Low Tide Radio.flac");
    s.download(&f);
    s.open_folder_of(&f);
}

/// Browse alice: picture, stats, long description (clamped).
fn scene_browse_profile(s: &mut App) {
    s.demo_search();
    s.open_user("alice");
}

/// Browse vinyl_rips: stats only.
fn scene_browse_profile_minimal(s: &mut App) {
    s.demo_search();
    s.open_user("vinyl_rips");
}

fn scene_transfers(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("dj_ivo", "Hollow Lights - Salt Roads.mp3"));
    s.download(&demo_file("alice", "01 Northern Glass.flac"));
    s.download(&demo_file("marten_k", "Hollow Lights - Salt Roads.flac"));
    s.download(&demo_file("nightbus", "hollow_lights_salt_roads_live.mp3"));
    s.download_folder("alice", r"Music\Hollow Lights\Night Ferry EP (2021)");
    s.toggle_panel(Panel::Transfers);
}

fn scene_settings(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "02 Salt Roads.flac"));
    s.toggle_panel(Panel::Settings);
}

/// Transfers panel with only uploads; one of them cancelled.
fn scene_uploads(s: &mut App) {
    s.demo_login();
    s.cancel_upload("tomasz", r"Music\Low Roar\0 (2014)\05 Phantoms.flac");
    s.toggle_panel(Panel::Transfers);
}

/// First launch on a machine without a Secret Service.
fn scene_no_keyring(s: &mut App) {
    s.keyring_available = false;
}

fn scene_onboarding_error(s: &mut App) {
    s.onboarding.username.replace_text("nadia");
    s.connect();
}

fn scene_rejected(s: &mut App) {
    s.onboarding.username.replace_text("nadia");
    s.onboarding.password.replace_text("wrong");
    s.connect();
}

fn main() {
    let demo = std::env::args().any(|a| a == "--demo");
    let _ = DEMO.set(demo);
    if !demo {
        // SAFETY: called first thing in `main`, single-threaded: no thread has been spawned yet
        // (the core, player and window all start after this). It only sets LOG_FILE (G13).
        if let Err(e) = unsafe { config::init_logging(&config::default_log_path()) } {
            eprintln!("goalseek: protocol log stays on stderr ({e})");
        }
        // Before the window: the keyring lookup may block up to its timeout.
        match config::load() {
            Ok(loaded) => {
                let _ = LOADED.set(loaded);
            }
            Err(e) => eprintln!("goalseek: could not read the config, starting fresh ({e})"),
        }
    }
    rune::run(AppSpec::<App> {
        scenes: &[
            ("results", scene_results),
            ("browse", scene_browse),
            ("transfers", scene_transfers),
            ("settings", scene_settings),
            ("onboarding-error", scene_onboarding_error),
            ("rejected", scene_rejected),
            ("uploads", scene_uploads),
            ("browse-profile", scene_browse_profile),
            ("browse-profile-minimal", scene_browse_profile_minimal),
            ("no-keyring", scene_no_keyring),
        ],
        min_size: Some((820.0, 520.0)),
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new("goalseek", (W, H), init, view::view)
    });
}
