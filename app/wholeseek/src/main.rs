//! wholeseek — a minimal Soulseek client for music (docs/MVP.md).
//!
//! `cargo run -p wholeseek`             the real session (wholeseek-core + system audio)
//! `cargo run -p wholeseek -- --demo`   fake peers and a simulated player: no network, no audio
//!
//! Headless (Rune tooling, same renderer, virtual clock), e.g.
//! `wholeseek --demo --scene search-tab --snap search-tab.png --at 4 --size 1100x700 --scale 1`.
//! Scenes (demo): search-tab, user-tab, three-tabs, transfers-drawer, player-buffering,
//! settings, uploads, displaced, browse-profile(-minimal), onboarding-error, rejected,
//! no-keyring.

mod actions;
mod app;
mod backend;
mod demo;
mod desktop;
mod fmt;
mod menu;
mod playback;
mod profile;
mod queue;
mod store;
mod tabs;
mod transfers;
mod view;

#[cfg(test)]
mod tests;

use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rune::{AppSpec, Key, KeyEvent, Tx};
use wholeseek_core::config;

use app::{App, Booted, Panel, Startup};

const W: f32 = 1100.0;
const H: f32 = 700.0;

static DEMO: OnceLock<bool> = OnceLock::new();
/// Where the startup loader sends what it read (real mode), taken by `setup`.
static BOOT_TX: Mutex<Option<Sender<Booted>>> = Mutex::new(None);

fn init() -> App {
    if *DEMO.get().unwrap_or(&false) {
        return App::demo();
    }
    let (app, tx) = App::real();
    if let Ok(mut slot) = BOOT_TX.lock() {
        *slot = Some(tx);
    }
    app
}

/// Each frame: drain core events and refresh the player snapshot; ask for the next wake while
/// something is moving (playback, demo timers, a notice to expire). Never blocks.
pub fn poll(s: &mut App, t: f64, tx: &mut Tx) -> bool {
    let (changed, wake) = s.tick(t);
    // A deep link's file, once its folder is listed: scroll it into view.
    if let Some((list, row)) = s.take_reveal() {
        tx.scroll_into_view_in(list, row);
    }
    if let Some(w) = wake {
        tx.wake_after(Duration::from_secs_f64(w.max(1.0 / 60.0)));
    }
    changed
}

fn setup(s: &mut App, wake: rune::Waker) {
    s.set_waker(wake.clone());
    let tx = BOOT_TX.lock().ok().and_then(|mut t| t.take());
    if let Some(tx) = tx {
        spawn_loader(tx, wake);
    }
}

/// Read the config file and the keyring (up to its 5 s timeout) off the UI thread; the
/// window already shows "Signing in…".
fn spawn_loader(tx: Sender<Booted>, wake: rune::Waker) {
    let spawned = std::thread::Builder::new()
        .name("wholeseek-config".into())
        .spawn(move || {
            // First start after the rename: carry the old config and saved password over
            // (idempotent; failures only mean signing in again).
            match config::migrate_legacy() {
                Ok(m) => eprintln!("wholeseek: config migration: {m:?}"),
                Err(e) => eprintln!("wholeseek: config migration failed: {e}"),
            }
            let booted = match config::load() {
                Ok(l) => Booted {
                    settings: l.settings,
                    startup: Startup {
                        credentials: l.credentials,
                        keyring_available: l.keyring_available,
                    },
                },
                Err(e) => {
                    eprintln!("wholeseek: could not read the config, starting fresh ({e})");
                    Booted {
                        settings: wholeseek_core::Settings::default(),
                        startup: Startup {
                            credentials: None,
                            keyring_available: false,
                        },
                    }
                }
            };
            let _ = tx.send(booted);
            wake();
        });
    if let Err(e) = spawned {
        eprintln!("wholeseek: could not start the config loader ({e})");
    }
}

/// Window shortcuts (Ctrl off macOS, ⌘ on it — Rune's `mods.cmd` / `is_command`; Ctrl also
/// works for Tab and arrows there): Ctrl+T new search tab, Ctrl+W close tab, Ctrl+(Shift+)Tab cycle tabs,
/// Ctrl+L / Ctrl+F the search field, Space play/pause, Ctrl+→ / Ctrl+← next / previous
/// track, Escape closes a menu. Only keys the focused control left alone get here.
pub fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.screen != app::Screen::Main {
        return false;
    }
    if s.menu.is_some() && ev.key == Key::Escape {
        s.menu = None;
        return true;
    }
    let plain = !(ev.mods.ctrl || ev.mods.cmd || ev.mods.alt);
    match &ev.key {
        Key::Text(t) if t == " " && plain => s.toggle_pause(),
        Key::Right if ev.mods.cmd || ev.mods.ctrl => s.next_track(),
        Key::Left if ev.mods.cmd || ev.mods.ctrl => s.previous_track(),
        Key::Tab if ev.mods.cmd || ev.mods.ctrl => {
            s.cycle_tabs(if ev.mods.shift { -1 } else { 1 });
            s.restore_scroll(tx);
        }
        _ if ev.is_command('t') => {
            s.new_search_tab();
            tx.focus("query", false);
        }
        _ if ev.is_command('w') => {
            s.close_active();
            s.restore_scroll(tx);
        }
        _ if ev.is_command('l') || ev.is_command('f') => {
            s.show_search_tab();
            s.restore_scroll(tx);
            tx.focus("query", true);
        }
        _ => return false,
    }
    true
}

// ---- demo scenes (headless screenshots) ------------------------------------------------------

fn demo_file(user: &str, name: &str) -> wholeseek_core::RemoteFile {
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
        self.search_for("hollow lights");
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

/// Results tab, then alice's and vinyl_rips' tabs; vinyl_rips is shown.
fn scene_three_tabs(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "02 Salt Roads.flac"));
    s.open_user("alice");
    s.activate(0);
    s.open_user("vinyl_rips");
}

/// A big track a few seconds in: the downloaded region runs ahead of the play position.
fn scene_player_buffering(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "04 Morning Crossing.flac"));
}

/// Another client took the account (demo user "twice", 1.5 s in): the banner, no
/// reconnect loop.
fn scene_displaced(s: &mut App) {
    s.onboarding.username.replace_text("twice");
    s.onboarding.password.replace_text("demo-password");
    s.onboarding.shares = vec![std::env::temp_dir()];
    s.connect();
    s.search_for("hollow lights");
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

/// The user's report: a search tab and a long-named user tab; the right cluster must show.
fn scene_strip(s: &mut App) {
    s.demo_login();
    s.search_for("roadblock iration");
    s.open_user("infectiphibian");
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
            eprintln!("wholeseek: protocol log stays on stderr ({e})");
        }
    }
    // X11 WM_CLASS / Wayland app_id: the desktop matches the window to wholeseek.desktop
    // (Icon=wholeseek, StartupWMClass=wholeseek) and shows its icon. Rune's `set_app_icon` sets the
    // window/taskbar icon on macOS and Windows only; on Linux the .desktop entry provides it.
    rune::set_app_id("wholeseek");
    if let Err(e) =
        rune::set_app_icon(include_bytes!("../../../assets/wholeseek-256.png").as_slice())
    {
        eprintln!("wholeseek: app icon not set ({e})");
    }
    rune::run(AppSpec::<App> {
        scenes: &[
            ("search-tab", scene_results),
            ("user-tab", scene_browse),
            ("three-tabs", scene_three_tabs),
            ("transfers-drawer", scene_transfers),
            ("player-buffering", scene_player_buffering),
            ("displaced", scene_displaced),
            ("settings", scene_settings),
            ("strip", scene_strip),
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
        on_key: Some(on_key),
        ..AppSpec::new("wholeseek", (W, H), init, view::view)
    });
}
