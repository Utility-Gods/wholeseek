//! Session restore: the open tabs, so reopening the app brings them back. Only
//! what the user set up is stored (query, filters, sort, selection, browsed
//! user/folder) — never results, file lists or credentials.
#![allow(dead_code)] // Parts of the API (debounce internals, tab helpers) only tests use.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use wholeseek_core::config::APP_NAME;

/// File format version; files with a higher one (a newer wholeseek) are ignored.
pub const SESSION_VERSION: u32 = 1;
/// At most this many tabs are stored and restored.
pub const MAX_TABS: usize = 50;
/// Saves happen at most this often.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// Files larger than this are not read.
const MAX_BYTES: u64 = 1 << 20;

/// One restorable tab, stored as `{"kind": "search" | "user", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionTab {
    Search {
        typed: String,
        #[serde(default)]
        filter: String,
        /// Enabled format filters (e.g. `["flac"]`); empty = all.
        #[serde(default)]
        formats: Vec<String>,
        #[serde(default)]
        free_slot_only: bool,
        /// Sort key name, `None` = default order.
        #[serde(default)]
        sort: Option<String>,
        #[serde(default)]
        selected_group: Option<String>,
    },
    User {
        username: String,
        /// Folder being viewed, `None` = the share root.
        #[serde(default)]
        folder: Option<String>,
    },
}

impl SessionTab {
    pub fn search(typed: impl Into<String>) -> Self {
        SessionTab::Search {
            typed: typed.into(),
            filter: String::new(),
            formats: Vec::new(),
            free_slot_only: false,
            sort: None,
            selected_group: None,
        }
    }

    pub fn user(username: impl Into<String>) -> Self {
        SessionTab::User {
            username: username.into(),
            folder: None,
        }
    }

    fn is_valid(&self) -> bool {
        match self {
            SessionTab::Search { typed: s, .. } | SessionTab::User { username: s, .. } => {
                !s.trim().is_empty()
            }
        }
    }
}

/// The open tabs and which one is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub version: u32,
    pub tabs: Vec<SessionTab>,
    /// Index into `tabs` (0 when empty).
    pub active: usize,
}

impl Default for Session {
    fn default() -> Self {
        Session::new(Vec::new(), 0)
    }
}

impl Session {
    /// Drops empty tabs, keeps at most [`MAX_TABS`] (always including the
    /// active one) and clamps `active`.
    pub fn new(tabs: Vec<SessionTab>, active: usize) -> Self {
        let mut kept = Vec::new();
        let mut act = 0;
        for (i, t) in tabs.into_iter().enumerate() {
            if t.is_valid() {
                if i == active {
                    act = kept.len();
                }
                kept.push(t);
            }
        }
        // The first window of MAX_TABS tabs that contains the active one.
        let start = act.saturating_sub(MAX_TABS - 1);
        kept.truncate(start + MAX_TABS);
        kept.drain(..start);
        Session {
            version: SESSION_VERSION,
            active: (act - start).min(kept.len().saturating_sub(1)),
            tabs: kept,
        }
    }
}

/// `<state dir>/wholeseek/session.json`: `$XDG_STATE_HOME` (else
/// `~/.local/state`) on Linux, the local data dir on macOS/Windows.
pub fn default_path() -> Option<PathBuf> {
    let base = dirs::state_dir().or_else(dirs::data_local_dir)?;
    Some(base.join(APP_NAME).join("session.json"))
}

/// As read back: tabs stay raw so an unknown or malformed tab is skipped
/// rather than failing the whole file.
#[derive(Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    active: usize,
    tabs: Vec<serde_json::Value>,
}

/// Read the session at `path`. Missing, unreadable, corrupt, over 1 MiB or
/// written by a newer version → an empty [`Session`]. Never panics.
pub fn load(path: &Path) -> Session {
    let small = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES);
    let stored = small
        .then(|| std::fs::read(path).ok())
        .flatten()
        .and_then(|b| serde_json::from_slice::<Stored>(&b).ok())
        .filter(|s| (1..=SESSION_VERSION).contains(&s.version));
    // An unreadable tab becomes an empty one, which `Session::new` drops while
    // keeping `active` on the same tab.
    let tab = |v| serde_json::from_value(v).unwrap_or_else(|_| SessionTab::search(""));
    stored
        .map(|s| Session::new(s.tabs.into_iter().map(tab).collect(), s.active))
        .unwrap_or_default()
}

/// Write `session` to `path` atomically (temp file in the same directory,
/// synced, renamed over `path`): a crash leaves the old or the new file, never
/// a partial one. Creates the directory if needed.
pub fn save(path: &Path, session: &Session) -> std::io::Result<()> {
    use std::io::Write;
    let s = Session::new(session.tabs.clone(), session.active);
    let json = serde_json::to_vec_pretty(&s)?;
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    let tmp = tmp_path(path);
    let result = std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(&json).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp{}", std::process::id()));
    path.with_file_name(name)
}

/// Save on change, at most once per [`SAVE_INTERVAL`]: `changed()` on every tab
/// change; each tick, if `due(now)`, save then `saved(now)`. On exit, save if
/// `is_dirty()`.
#[derive(Debug, Clone, Default)]
pub struct SaveDebouncer {
    dirty: bool,
    last_save: Option<Instant>,
}

impl SaveDebouncer {
    pub fn changed(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn due(&self, now: Instant) -> bool {
        self.dirty
            && self
                .last_save
                .is_none_or(|t| now.saturating_duration_since(t) >= SAVE_INTERVAL)
    }

    /// When the next save is due, to schedule a wake-up.
    pub fn next_due(&self) -> Option<Instant> {
        self.last_save
            .filter(|_| self.dirty)
            .map(|t| t + SAVE_INTERVAL)
    }

    /// Call after saving (a failed save is retried after the next change).
    pub fn saved(&mut self, now: Instant) {
        self.dirty = false;
        self.last_save = Some(now);
    }
}

/// What to do for a restored tab: `*Now` at startup (active tab only),
/// `*WhenShown` when the tab is first shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreAction {
    SearchNow { tab: usize },
    BrowseNow { tab: usize },
    SearchWhenShown { tab: usize },
    BrowseWhenShown { tab: usize },
}

impl RestoreAction {
    pub fn tab(self) -> usize {
        match self {
            Self::SearchNow { tab }
            | Self::BrowseNow { tab }
            | Self::SearchWhenShown { tab }
            | Self::BrowseWhenShown { tab } => tab,
        }
    }

    pub fn is_immediate(self) -> bool {
        matches!(self, Self::SearchNow { .. } | Self::BrowseNow { .. })
    }
}

/// One action per tab: the active tab fetches now, every other tab when first
/// shown — so at most one request (search or browse) at startup.
pub fn restore_plan(session: &Session) -> Vec<RestoreAction> {
    let plan = |(tab, t): (usize, &SessionTab)| match (t, tab == session.active) {
        (SessionTab::Search { .. }, true) => RestoreAction::SearchNow { tab },
        (SessionTab::User { .. }, true) => RestoreAction::BrowseNow { tab },
        (SessionTab::Search { .. }, false) => RestoreAction::SearchWhenShown { tab },
        (SessionTab::User { .. }, false) => RestoreAction::BrowseWhenShown { tab },
    };
    session.tabs.iter().enumerate().map(plan).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        let search = SessionTab::Search {
            typed: "aphex twin \"saw\" \\ ambient".into(),
            filter: "85-92".into(),
            formats: vec!["flac".into(), "mp3".into()],
            free_slot_only: true,
            sort: Some("relevance".into()),
            selected_group: Some("xtal|tha".into()),
        };
        let browse = SessionTab::User {
            username: "björk fan ☃".into(),
            folder: Some("@@abc\\Björk\\Homogenic\ttab\nline".into()),
        };
        let (dash, boc) = (SessionTab::user("DashNode"), SessionTab::search("boc"));
        Session::new(vec![search, dash, browse, boc], 2)
    }

    struct TempDir(PathBuf); // removed on drop
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> TempDir {
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("wholeseek-session-{pid}-{name}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn json(s: &Session) -> String {
        serde_json::to_string_pretty(s).unwrap()
    }

    #[test]
    fn roundtrip() {
        let d = dir("roundtrip");
        let p = d.0.join("nested").join("session.json");
        save(&p, &sample()).unwrap();
        assert_eq!(load(&p), sample());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"kind\": \"user\""));
        save(&p, &Session::default()).unwrap();
        assert_eq!(load(&p), Session::default());
    }

    #[test]
    fn missing_corrupt_or_future_file_is_an_empty_session() {
        let d = dir("corrupt");
        let p = d.0.join("session.json");
        assert_eq!(load(&p), Session::default(), "missing");
        assert_eq!(load(&d.0), Session::default(), "a directory");
        let deep = "[".repeat(10_000);
        // One corrupt document per line (plus the empty file and deep nesting).
        let bad = r#"{
null
[]
{"version": 1}
{"version": 1, "tabs": 5}
{"version": "1", "tabs": []}
{"version": 1, "tabs": [] } trailing
{"version": 1, "tabs": [{"kind": "search", "typed": "x\uZZZZ"}]}
\u{feff}garbage"#;
        for bad in bad.lines().chain(["", deep.as_str()]) {
            std::fs::write(&p, bad).unwrap();
            assert_eq!(load(&p), Session::default(), "{bad:.40}");
        }
        std::fs::write(&p, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        assert_eq!(load(&p), Session::default(), "not UTF-8");
        let big = json(&sample()).replace("\"85-92\"", &format!("\"{}\"", "x".repeat(2 << 20)));
        std::fs::write(&p, big).unwrap();
        assert_eq!(load(&p), Session::default(), "over 1 MiB");
        for v in ["2", "0"] {
            let text = json(&sample()).replace("\"version\": 1", &format!("\"version\": {v}"));
            std::fs::write(&p, text).unwrap();
            assert_eq!(load(&p), Session::default(), "version {v}");
        }
    }

    #[test]
    fn malformed_tabs_are_skipped_and_active_follows() {
        let d = dir("malformed");
        let p = d.0.join("session.json");
        let text = r#"{"version": 1, "active": 3, "tabs": [
            {"kind": "search", "typed": "a"}, {"kind": "video", "url": "x"}, {"kind": "user"},
            {"kind": "user", "username": "bob", "folder": null, "extra": [1, {"x": true}]},
            {"kind": "search", "typed": "   "}]}"#;
        std::fs::write(&p, text).unwrap();
        let s = load(&p);
        let want = [SessionTab::search("a"), SessionTab::user("bob")];
        assert_eq!(s.tabs, want);
        assert_eq!(s.active, 1, "active tab (bob) survives the skipped ones");
    }

    #[test]
    fn atomic_write_never_leaves_a_partial_file() {
        let d = dir("atomic");
        let p = d.0.join("session.json");
        save(&p, &sample()).unwrap();
        // Crash simulation: a writer died mid temp file (no rename happened).
        let new = Session::new(vec![SessionTab::search("new")], 0);
        std::fs::write(tmp_path(&p), &json(&new)[..20]).unwrap();
        assert_eq!(load(&p), sample(), "old file untouched");
        save(&p, &new).unwrap();
        assert_eq!(load(&p), new);
        assert!(!tmp_path(&p).exists(), "temp renamed away");
        // A failing save (target is a non-empty directory) cleans up its temp.
        let blocked = d.0.join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("x"), "x").unwrap();
        assert!(save(&blocked, &new).is_err());
        assert!(!tmp_path(&blocked).exists());
        assert_eq!(load(&p), new);
        let left = std::fs::read_dir(&d.0).unwrap().count();
        assert_eq!(left, 2, "session + blocked only");
    }

    #[test]
    fn at_most_50_tabs_keeping_the_active_one() {
        let q = |i| SessionTab::search(format!("q{i}"));
        let tabs: Vec<_> = (0..80).map(q).collect();
        for (active, first) in [(3, 0), (75, 26), (79, 30)] {
            let s = Session::new(tabs.clone(), active);
            assert_eq!(s.tabs.len(), MAX_TABS);
            assert_eq!((&s.tabs[s.active], &s.tabs[0]), (&q(active), &q(first)));
        }
        // An oversized file written by hand is capped on load, and on save.
        let d = dir("cap");
        let p = d.0.join("session.json");
        let raw = Session {
            version: 1,
            tabs,
            active: 79,
        };
        std::fs::write(&p, json(&raw)).unwrap();
        let loaded = load(&p);
        assert_eq!(loaded.tabs.len(), MAX_TABS);
        assert_eq!(loaded.tabs[loaded.active], q(79));
        save(&p, &raw).unwrap();
        assert_eq!(load(&p), loaded);
        assert_eq!(Session::new(vec![q(0)], 9).active, 0, "clamped");
    }

    #[test]
    fn restore_plan_fetches_only_the_active_tab() {
        let mut s = sample(); // active = 2, a user tab
        use RestoreAction::*;
        let want = [
            SearchWhenShown { tab: 0 },
            BrowseWhenShown { tab: 1 },
            BrowseNow { tab: 2 },
            SearchWhenShown { tab: 3 },
        ];
        assert_eq!(restore_plan(&s), want);
        s.active = 3;
        assert_eq!(restore_plan(&s)[3], SearchNow { tab: 3 });
        for active in 0..s.tabs.len() {
            s.active = active;
            let now: Vec<_> = restore_plan(&s)
                .into_iter()
                .filter(|a| a.is_immediate())
                .collect();
            assert_eq!(now.len(), 1, "one fetch at startup");
            assert_eq!(now[0].tab(), active);
        }
        assert!(restore_plan(&Session::default()).is_empty());
    }

    #[test]
    fn debouncer_saves_on_change_at_most_once_per_second() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut d = SaveDebouncer::default();
        assert!(!d.due(t0), "nothing changed");
        d.changed();
        assert!(d.due(t0), "first change saves at once");
        d.saved(t0);
        d.changed();
        assert!(!d.due(ms(999)) && d.due(ms(1000)) && d.next_due() == Some(ms(1000)));
        d.saved(ms(1000));
        assert!(!d.is_dirty() && d.next_due().is_none());
        // A burst of 100 changes over 3 s → about 3 saves.
        let mut d = SaveDebouncer::default();
        let mut saves = 0;
        for n in (0..3000).step_by(30) {
            d.changed();
            if d.due(ms(n)) {
                d.saved(ms(n));
                saves += 1;
            }
        }
        assert!((3..=4).contains(&saves), "{saves}");
    }

    #[test]
    fn default_path_is_under_the_app_state_dir() {
        let p = default_path().expect("home dir");
        assert!(p.is_absolute() && p.ends_with("wholeseek/session.json"));
    }
}
