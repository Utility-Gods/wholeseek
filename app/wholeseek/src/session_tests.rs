//! Session restore end to end: the demo app quits with tabs open, a second run restores them.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rune::{Env, Runtime};

use crate::app::{App, Boot, Booted, Conn, Screen, Startup};
use crate::demo::{Call, DemoBackend, FakePlayer, World};
use crate::store::MemoryStore;
use crate::tabs::{ResultsLayout, SortBy, TabKind};
use crate::{H, W, poll, view};
use wholeseek_core::Credentials;

const COAST: &str = r"Music\Hollow Lights\Coastline (2019)";

struct TempDir(PathBuf); // removed on drop
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> TempDir {
    let pid = std::process::id();
    let p = std::env::temp_dir().join(format!("wholeseek-restore-{pid}-{name}"));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    TempDir(p)
}

/// A demo run remembering its tabs at `path`, started like the real app: the window first,
/// then the startup load lands with (`saved_account`) or without a remembered account.
fn launch(path: &Path, saved_account: bool) -> Runtime<App> {
    let world = || Rc::new(RefCell::new(World::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let boot = Boot {
        rx,
        make: |_| Box::new(DemoBackend::new(Rc::new(RefCell::new(World::default())))),
    };
    let player = Box::new(FakePlayer::new(world()));
    let mut app = App::booting(player, Box::new(MemoryStore::new(true)), boot, true);
    app.enable_session(Some(path.to_path_buf()));
    let credentials = saved_account.then(|| Credentials {
        username: "nadia".into(),
        password: "demo-password".into(),
    });
    let startup = Startup {
        credentials,
        keyring_available: true,
    };
    let settings = wholeseek_core::Settings::default();
    tx.send(Booted { settings, startup }).unwrap();
    let env = Env {
        system_dark: true,
        scale: 1.0,
    };
    let mut rt = Runtime::new(app, view::view, (W, H), env);
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt
}

fn run(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs * 60.0).round() as usize {
        rt.frame(1.0 / 60.0);
    }
}

/// Search, browse and profile requests (the network work a tab starts).
fn fetches(rt: &Runtime<App>) -> Vec<Call> {
    let calls = rt.state.backend.calls();
    let fetch = |c: &&Call| matches!(c, Call::Search(_) | Call::Browse(_) | Call::UserProfile(_));
    calls.iter().filter(fetch).cloned().collect()
}

/// First run: a filtered search with chips, alice's tab in a folder, another search; the
/// user tab is shown when the app quits.
fn first_run(path: &Path) {
    let mut rt = launch(path, false);
    rt.state.demo_login();
    run(&mut rt, 1.0);
    rt.state.search_for("hollow lights");
    run(&mut rt, 1.6);
    let st = rt.state.tab_mut().search_mut().unwrap();
    st.find.replace_text("coastline");
    st.filters.toggle("FLAC");
    st.filters.toggle("MP3");
    st.filters.free_slot = true;
    st.filters.sort = SortBy::Size;
    st.filters.layout = ResultsLayout::Folders;
    rt.state.tab_mut().group = Some(("alice".into(), COAST.into()));
    rt.state.open_user("alice");
    rt.state.active_browse_mut().unwrap().select(COAST);
    rt.state.new_search_tab();
    rt.state.search_for("pale harbour");
    rt.state.activate(1);
    run(&mut rt, 0.5);
    rt.state.flush_session(); // the quit hook
}

#[test]
fn quit_and_relaunch_restores_tabs_and_fetches_only_the_shown_one() {
    let d = dir("relaunch");
    let path = d.0.join("session.json");
    first_run(&path);
    assert!(path.is_file(), "saved on quit");

    let mut rt = launch(&path, true);
    assert_eq!(rt.state.screen, Screen::Main);
    let titles: Vec<_> = rt.state.tabs.iter().map(|t| t.title()).collect();
    assert_eq!(
        titles,
        ["hollow lights", "alice", "pale harbour"],
        "order, before connecting"
    );
    assert_eq!(rt.state.active, 1, "alice's tab is shown");
    assert!(
        fetches(&rt).is_empty(),
        "nothing fetched before the connection"
    );
    run(&mut rt, 1.0);
    assert!(matches!(rt.state.conn, Conn::Connected { .. }));
    let alice = vec![
        Call::Browse("alice".into()),
        Call::UserProfile("alice".into()),
    ];
    assert_eq!(
        fetches(&rt),
        alice,
        "one request at startup: the shown tab's browse"
    );
    let b = rt.state.active_browse().unwrap();
    assert_eq!(b.selected.as_deref(), Some(COAST), "the open folder");
    assert!(b.dirs.is_some(), "listed");

    // The first search tab: filter box, chips, free slot and group as left; fetched only
    // when it is shown.
    let TabKind::Search(st) = &rt.state.tabs[0].kind else {
        panic!("search tab")
    };
    assert!(st.search.is_none(), "not searched until shown");
    assert_eq!(st.find.text, "coastline");
    assert!(st.filters.has("FLAC") && st.filters.has("MP3") && st.filters.free_slot);
    assert_eq!(
        (st.filters.sort, st.filters.layout),
        (SortBy::Size, ResultsLayout::Folders)
    );
    let third = &rt.state.tabs[2].search().unwrap().filters;
    assert_eq!(
        (third.sort, third.layout),
        (SortBy::default(), ResultsLayout::default())
    );
    assert_eq!(rt.state.tabs[0].group, Some(("alice".into(), COAST.into())));
    rt.state.activate(0);
    run(&mut rt, 1.6);
    assert_eq!(fetches(&rt)[2..], [Call::Search("hollow lights".into())]);
    let v = rt.state.active_search().unwrap();
    assert!(!v.hits.is_empty(), "results came in");
    // The third tab is still a shell; switching back and forth never re-fetches.
    rt.state.activate(1);
    rt.state.activate(0);
    run(&mut rt, 0.2);
    assert_eq!(fetches(&rt).len(), 3);
    assert!(rt.state.tabs[2].search().unwrap().search.is_none());
}

#[test]
fn signing_in_from_onboarding_restores_once_connected() {
    let d = dir("offline");
    let path = d.0.join("session.json");
    first_run(&path);
    // No saved account: onboarding, then signing in brings the tabs back.
    let mut rt = launch(&path, false);
    assert_eq!(rt.state.tabs.len(), 1, "nothing restored before signing in");
    rt.state.demo_login();
    assert_eq!(rt.state.tabs.len(), 1, "signing in…");
    run(&mut rt, 1.0);
    assert_eq!(rt.state.tabs.len(), 3);
    assert_eq!(fetches(&rt).len(), 2, "alice's browse + profile only");
}

#[test]
fn corrupt_or_missing_session_starts_empty_and_is_replaced() {
    let d = dir("corrupt");
    let path = d.0.join("session.json");
    std::fs::write(&path, "{\"version\": 1, \"tabs\": [").unwrap();
    let mut rt = launch(&path, true);
    run(&mut rt, 1.0);
    assert_eq!(rt.state.tabs.len(), 1);
    assert_eq!(rt.state.tabs[0].title(), "New search");
    assert!(fetches(&rt).is_empty());
    // Using the app writes a good file over the corrupt one (debounced, first change at once).
    rt.state.search_for("pale harbour");
    run(&mut rt, 0.1);
    assert_eq!(crate::session::load(&path).tabs.len(), 1);
    // A missing file is the same fresh start.
    let mut rt = launch(&d.0.join("none.json"), true);
    run(&mut rt, 1.0);
    assert_eq!(rt.state.tabs.len(), 1);
    assert!(fetches(&rt).is_empty());
}

#[test]
fn unknown_or_missing_sort_and_layout_are_the_defaults() {
    let d = dir("sortlayout");
    let path = d.0.join("session.json");
    let text = r#"{"version": 1, "active": 0, "tabs": [
        {"kind": "search", "typed": "a", "sort": "loudness", "layout": "grid"},
        {"kind": "search", "typed": "b"},
        {"kind": "search", "typed": "c", "sort": "length", "layout": "folders"}]}"#;
    std::fs::write(&path, text).unwrap();
    let rt = launch(&path, true);
    let got: Vec<_> = (0..3)
        .map(|i| {
            let f = &rt.state.tabs[i].search().unwrap().filters;
            (f.sort, f.layout)
        })
        .collect();
    let default = (SortBy::default(), ResultsLayout::default());
    let c = (SortBy::Length, ResultsLayout::Folders);
    assert_eq!(got, [default, default, c]);
    // Saved back by name.
    let saved = rt.state.session_snapshot();
    let crate::session::SessionTab::Search { sort, layout, .. } = &saved.tabs[2] else {
        panic!("search tab")
    };
    assert_eq!(
        (sort.as_deref(), layout.as_deref()),
        (Some("length"), Some("folders"))
    );
}

#[test]
fn demo_without_a_path_never_writes() {
    let mut rt = Runtime::new(
        App::demo(),
        view::view,
        (W, H),
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.poll = Some(poll);
    rt.state.demo_login();
    run(&mut rt, 1.0);
    rt.state.search_for("hollow lights");
    run(&mut rt, 1.6);
    rt.state.flush_session();
    assert_eq!(rt.state.tabs.len(), 1);
}

#[test]
fn a_restored_user_tab_scrolls_its_tree_to_the_open_folder() {
    use crate::demo::{DEEP_FOLDER, DEEP_USER};
    let d = dir("tree-reveal");
    let path = d.0.join("session.json");
    {
        let mut rt = launch(&path, false);
        rt.state.demo_login();
        run(&mut rt, 1.0);
        rt.state.open_folder_at(DEEP_USER, DEEP_FOLDER, None);
        run(&mut rt, 1.0);
        rt.state.flush_session();
    }
    let mut rt = launch(&path, true);
    run(&mut rt, 1.5);
    let tab = rt.state.tab();
    let b = tab.browse().expect("the user tab is shown");
    assert_eq!(b.selected.as_deref(), Some(DEEP_FOLDER));
    let i = b.tree_row_of_selected().unwrap();
    let rect = |k: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(k))
            .unwrap_or_else(|| panic!("no {k}"))
            .rect
    };
    let (r, v) = (rect(&format!("dir-{i}")), rect(&tab.tree_key()));
    assert!(
        r[1] >= v[1] && r[1] + r[3] <= v[1] + v[3],
        "row {r:?} inside the tree {v:?}"
    );
}

#[test]
fn tabs_opened_in_new_tabs_or_duplicated_are_restored() {
    let d = dir("new-tabs");
    let path = d.0.join("session.json");
    let pale = r"Music\Hollow Lights\Pale Harbour";
    {
        let mut rt = launch(&path, false);
        rt.state.demo_login();
        run(&mut rt, 1.0);
        rt.state.open_folder_at("alice", COAST, None);
        run(&mut rt, 1.0);
        // Beside it, in the background: never shown before the quit.
        rt.state.open_in_new_tab("alice", Some(pale));
        rt.state.duplicate_tab(1);
        run(&mut rt, 0.5);
        rt.state.flush_session();
    }
    let mut rt = launch(&path, true);
    run(&mut rt, 1.5);
    let tabs: Vec<_> = rt
        .state
        .tabs
        .iter()
        .filter_map(|t| t.browse())
        .map(|b| (b.username.clone(), b.selected.clone()))
        .collect();
    let at = |f: &str| ("alice".to_string(), Some(f.to_string()));
    assert_eq!(tabs, [at(COAST), at(COAST), at(pale)]);
}
