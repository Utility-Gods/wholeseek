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
mod demo_big;
mod demo_history;
mod demo_library;
mod desktop;
mod fmt;
mod history;
mod library;
mod media;
mod menu;
mod playback;
mod profile;
mod queue;
mod recent;
mod session;
mod store;
mod tabs;
mod transfers;
mod update;
mod view;

#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod session_tests;
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
    let (mut app, tx) = App::real();
    app.enable_session(session::default_path());
    app.recent = recent::Recent::load(recent::default_path());
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
    // A user tab's folder picked other than in its tree: centre its tree row.
    if let Some((tree, row)) = s.tab_mut().take_tree_reveal() {
        tx.scroll_center_in_instant(tree, row);
    }
    if let Some(w) = wake {
        tx.wake_after(Duration::from_secs_f64(w.max(1.0 / 60.0)));
    }
    changed
}

fn setup(s: &mut App, wake: rune::Waker) {
    s.media = media::Media::start(wake.clone());
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
/// Ctrl+L the search field, Ctrl+F the results filter (the search field before results), Space play/pause, Ctrl+→ / Ctrl+← next / previous
/// track, Escape closes a menu. Only keys the focused control left alone get here.
pub fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.screen != app::Screen::Main {
        return false;
    }
    if s.menu.is_some() && ev.key == Key::Escape {
        s.menu = None;
        return true;
    }
    if ev.key == Key::Escape && matches!(s.panel, Panel::None) && s.tab_mut().close_inspector() {
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
        _ if ev.is_command('f') && s.focus_find() => {
            tx.focus("filter", true);
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

/// A search like a real one: long scene-release folder names, a 1-file folder selected.
fn scene_jungle(s: &mut App) {
    s.demo_login();
    let hit = |u: &str, p: &str, size: u64| wholeseek_core::SearchHit {
        file: wholeseek_core::RemoteFile {
            username: u.into(),
            path: p.into(),
            size,
            attrs: Default::default(),
        },
        free_slot: true,
        speed: 1_200_000,
    };
    let hits = vec![
        hit(
            "K1RZA",
            r"@@xkzqp\Music\Singles\Put It On (Jungle Mix).mp3",
            9_400_000,
        ),
        hit(
            "divided1980",
            r"Music\VA_-_Liondub_and_Deekline_Present_Welcome_To_The_Jungle-(JC175ALBUM)-WEB-FLAC-2023\01-deekline_and_ed_solo-put_it_on_(jungle_mix).flac",
            41_000_000,
        ),
        hit(
            "divided1980",
            r"Music\VA_-_Liondub_and_Deekline_Present_Welcome_To_The_Jungle-(JC175ALBUM)-WEB-FLAC-2023\02-liondub-babylon_burning_(feat_general_levy)_(original_mix)_extended_version_for_djs.flac",
            52_000_000,
        ),
        hit(
            "a_user_with_a_rather_long_name_2024",
            r"D:\Shared\Drum & Bass\Jungle\1994 - 1996\Classics Collection Vol. 3 (Remastered 2019) [FLAC 24-96]\CD2\07 - Put It On (Original 1995 Dubplate Version).flac",
            88_000_000,
        ),
    ];
    let st = s.tab_mut().search_mut().expect("search tab");
    let mut v = crate::tabs::SearchView {
        id: wholeseek_core::SearchId(777),
        query: "put it on jungle".into(),
        hits,
        finished: true,
        keys: Vec::new(),
        matched: Default::default(),
    };
    v.reindex();
    st.search = Some(v);
    st.edit.replace_text("put it on jungle");
    s.tab_mut().group = Some(("K1RZA".into(), r"@@xkzqp\Music\Singles".into()));
}

/// A search that finds nothing ("Wazn't Me").
fn scene_no_results(s: &mut App) {
    s.demo_login();
    s.search_for("Wazn't Me");
}

/// The results narrowed by the filter box ("coast"), Coastline selected for the pane.
fn scene_filtered(s: &mut App) {
    scene_wide(s);
    if let Some(st) = s.tab_mut().search_mut() {
        st.find.replace_text("coast");
    }
}

/// A filter that hides every result.
fn scene_nothing_matches(s: &mut App) {
    scene_results(s);
    if let Some(st) = s.tab_mut().search_mut() {
        st.find.replace_text("xyz");
    }
}

/// The results with alice's Coastline selected: on a wide window the details pane shows it.
fn scene_wide(s: &mut App) {
    scene_results(s);
    let f = demo_file("alice", "02 Salt Roads.flac");
    let tab = s.tab_mut();
    tab.selected = Some(f.path.clone());
    tab.group = Some((f.username.clone(), f.folder().to_string()));
}

/// The album view with the album two users have selected: the inspector lists the other copy.
fn scene_albums(s: &mut App) {
    scene_results(s);
    let f = demo_file("vinyl_rips", "A1 Northern Glass.flac");
    s.tab_mut().group = Some((f.username.clone(), f.folder().to_string()));
}

/// The sort menu open over the album view.
fn scene_sort_menu(s: &mut App) {
    scene_results(s);
    let by = s
        .tab()
        .search()
        .map(|st| st.filters.sort)
        .unwrap_or_default();
    s.open_menu(
        crate::menu::MenuAnchor::Key("sort".into()),
        crate::view::sort_items(by),
    );
}

/// Two folders used with "Download to…" before, as the menus then list them.
fn demo_recent_dirs(s: &mut App) {
    s.commit_settings(|c| {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        c.recent_download_dirs = vec![
            home.join("Music/Vinyl"),
            "/media/usb/Music".into(),
            home.join("Music/Archive/Electronic/Drum and Bass/1995 Dubplates"),
        ];
    });
    s.settings_form.saved_at = None;
}

/// A file's ⋯ menu open in the results.
fn scene_file_menu(s: &mut App) {
    scene_results(s);
    folders_layout(s);
    demo_recent_dirs(s);
    let f = demo_file("dj_ivo", "Hollow Lights - Salt Roads.mp3");
    s.tab_mut().selected = Some(f.path.clone()); // its ⋯ shows, as when clicked
    let items = crate::menu::file_items(&f, false, &s.backend.settings().recent_download_dirs);
    let key = crate::view::files_more_key(s.tab().id, &f.path);
    s.open_menu(crate::menu::MenuAnchor::Key(key), items);
}

/// File and folder ⋯ menus belong to the Folders layout (Albums is the default).
fn folders_layout(s: &mut App) {
    if let Some(st) = s.tab_mut().search_mut() {
        st.filters.layout = crate::tabs::ResultsLayout::Folders;
    }
}

/// An album row's ⋯ menu open in the (default) album view.
fn scene_album_menu(s: &mut App) {
    scene_results(s);
    demo_recent_dirs(s);
    let f = demo_file("alice", "01 Northern Glass.flac");
    let items = s.folder_menu(&f.username, f.folder());
    s.open_menu(crate::menu::MenuAnchor::Key("album-more-0".into()), items);
}

/// A folder line's ⋯ menu open in the results.
fn scene_folder_menu(s: &mut App) {
    scene_results(s);
    folders_layout(s);
    demo_recent_dirs(s);
    let folder = demo_file("alice", "02 Salt Roads.flac")
        .folder()
        .to_string();
    // Results stream in after setup, so say "playable" as the live menu would.
    let recent = s.backend.settings().recent_download_dirs.clone();
    let items = crate::menu::folder_items("alice", &folder, true, &recent);
    let key = crate::view::files_more_key(s.tab().id, &folder);
    s.open_menu(crate::menu::MenuAnchor::Key(key), items);
}

/// The details pane's "Download to…" menu.
fn scene_pane_download_to(s: &mut App) {
    scene_wide(s);
    demo_recent_dirs(s);
    let target = crate::menu::DlTarget::Folder {
        username: "alice".into(),
        folder: demo_file("alice", "02 Salt Roads.flac").folder().into(),
    };
    let items = crate::menu::download_to_items(&target, &s.backend.settings().recent_download_dirs);
    s.open_menu(
        crate::menu::MenuAnchor::Key("details-download-to".into()),
        items,
    );
}

fn scene_browse(s: &mut App) {
    s.demo_search();
    s.play(&demo_file("alice", "02 Salt Roads.flac"));
    let f = demo_file("alice", "03 Low Tide Radio.flac");
    s.download(&f);
    s.open_folder_of(&f);
}

/// A big share opened at a folder far down its tree (as from "Browse user"): the tree
/// scrolls to it.
fn scene_tree_reveal(s: &mut App) {
    s.demo_search();
    s.open_folder_at(demo::DEEP_USER, demo::DEEP_FOLDER, None);
}

/// The long-list share (5,001 folders, 60,000 files) at an album deep in it.
fn scene_big_share(s: &mut App) {
    s.demo_search();
    s.open_folder_at(demo_big::BIG_USER, demo_big::BIG_FOLDER, None);
}

/// Context menus as a right-click opens them (at the pointer), for review.
fn scene_menu_tree(s: &mut App) {
    scene_tree_reveal(s);
    demo_recent_dirs(s);
    let items = crate::menu::tree_items(demo::DEEP_USER, demo::DEEP_FOLDER, true, false, &[]);
    s.open_menu(crate::menu::MenuAnchor::Point(130.0, 420.0), items);
}

/// The tree menu on a folder with subfolders: Expand all / Collapse all.
fn scene_menu_tree_parent(s: &mut App) {
    scene_tree_reveal(s);
    let items = s.tree_menu(demo::DEEP_USER, r"Music\Jungle\Iration Steppas", true);
    s.open_menu(crate::menu::MenuAnchor::Point(130.0, 396.0), items);
}

fn scene_menu_user_file(s: &mut App) {
    s.demo_search();
    s.open_folder_at("dj_ivo", r"mp3\H\Hollow Lights", None);
    s.downloaded = s.backend.downloaded_keys();
    let f = demo_file("dj_ivo", "Hollow Lights - Low Tide Radio.mp3");
    let items = s.file_menu(&f, true);
    s.open_menu(crate::menu::MenuAnchor::Point(420.0, 244.0), items);
}

fn scene_menu_search_album(s: &mut App) {
    scene_results(s);
    demo_recent_dirs(s);
    let f = demo_file("alice", "01 Night Ferry.flac");
    // Built before the demo results arrive: as `album_menu` makes it once they have.
    let recent = s.backend.settings().recent_download_dirs.clone();
    let mut items = crate::menu::folder_items(&f.username, f.folder(), true, &recent);
    items[0].label = "Play album".into();
    s.open_menu(crate::menu::MenuAnchor::Point(300.0, 266.0), items);
}

fn scene_menu_search_file(s: &mut App) {
    scene_results(s);
    folders_layout(s);
    demo_recent_dirs(s);
    let f = demo_file("dj_ivo", "Hollow Lights - Salt Roads.mp3");
    s.tab_mut().selected = Some(f.path.clone());
    let items = s.file_menu(&f, false);
    s.open_menu(crate::menu::MenuAnchor::Point(300.0, 232.0), items);
}

fn scene_menu_tab(s: &mut App) {
    s.demo_search();
    s.open_user("alice");
    s.open_user("vinyl_rips");
    s.activate(1);
    let items = crate::menu::tab_items(1, s.tabs.len());
    s.open_menu(crate::menu::MenuAnchor::Key("tab-more-1".into()), items);
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

/// The transfers drawer's History: the seeded past downloads under the ones running now.
fn scene_history(s: &mut App) {
    scene_transfers(s);
    s.set_drawer_view(crate::history::DrawerView::History);
}

/// The user's report: a search tab and a long-named user tab; the right cluster must show.
/// A newer release is out: the strip's update notice.
fn scene_update_available(s: &mut App) {
    s.demo_search();
    s.demo_update_offer();
}

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
    // Quit (window closed, ⌘Q, logout/shutdown): save the open tabs first.
    rune::App::new(AppSpec::<App> {
        scenes: &[
            ("search-tab", scene_results),
            ("wide", scene_wide),
            ("albums", scene_albums),
            ("sort-menu", scene_sort_menu),
            ("album-menu", scene_album_menu),
            ("jungle", scene_jungle),
            ("file-menu", scene_file_menu),
            ("folder-menu", scene_folder_menu),
            ("pane-download-to", scene_pane_download_to),
            ("no-results", scene_no_results),
            ("filtered", scene_filtered),
            ("nothing-matches", scene_nothing_matches),
            ("user-tab", scene_browse),
            ("tree-reveal", scene_tree_reveal),
            ("big-share", scene_big_share),
            ("menu-tree", scene_menu_tree),
            ("menu-tree-parent", scene_menu_tree_parent),
            ("menu-user-file", scene_menu_user_file),
            ("menu-search-album", scene_menu_search_album),
            ("menu-search-file", scene_menu_search_file),
            ("menu-tab", scene_menu_tab),
            ("three-tabs", scene_three_tabs),
            ("transfers-drawer", scene_transfers),
            ("history", scene_history),
            ("player-buffering", scene_player_buffering),
            ("displaced", scene_displaced),
            ("settings", scene_settings),
            ("strip", scene_strip),
            ("update-available", scene_update_available),
            ("onboarding-error", scene_onboarding_error),
            ("rejected", scene_rejected),
            ("uploads", scene_uploads),
            ("browse-profile", scene_browse_profile),
            ("browse-profile-minimal", scene_browse_profile_minimal),
            ("no-keyring", scene_no_keyring),
            ("library", demo_library::scene_albums),
            ("library-songs", demo_library::scene_songs),
            ("library-search", demo_library::scene_search),
            ("library-starred", demo_library::scene_starred),
            ("library-album", demo_library::scene_album),
            ("library-menu", demo_library::scene_menu),
            ("library-empty", demo_library::scene_empty),
        ],
        min_size: Some((820.0, 520.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("wholeseek", (W, H), init, view::view)
    })
    .on_quit(|s, _| s.flush_session())
    .run();
}
