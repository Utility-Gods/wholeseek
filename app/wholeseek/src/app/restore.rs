//! Session restore wired into the app (the file format is [`crate::session`]).
//!
//! - Save: each tick the open tabs are compared with the last snapshot; a change is saved at
//!   once, then at most once a second ([`SaveDebouncer`]), and on quit ([`App::flush_session`]).
//! - Restore: once per run, when signing in starts (saved account) or succeeds (onboarding),
//!   the saved tabs come back as shells — typed query, filter box, chips, selected group;
//!   user and folder — each marked [`Tab::restore`]. A marked tab fetches when it is shown
//!   while connected: the active one right after sign-in (the only request at startup), the
//!   others when first switched to. Offline, the shells wait for the connection.
//! - Closed tabs are gone; nothing runs before a path is set ([`App::enable_session`]), so the
//!   demo and tests never touch the user's file.

use std::path::PathBuf;
use std::time::Instant;

use super::{App, Conn};
use crate::session::{self, SaveDebouncer, Session, SessionTab};
use crate::tabs::{BrowseView, Filters, ResultsLayout, SearchTab, SortBy, Tab, TabKind, UserTab};

#[derive(Default)]
pub struct SessionState {
    path: Option<PathBuf>,
    restored: bool,
    last: Option<Session>,
    debouncer: SaveDebouncer,
}

/// Format chip labels ([`crate::tabs::format_of`]) a saved chip may name.
const FORMATS: [&str; 10] = [
    "FLAC", "MP3", "WAV", "AAC/M4A", "AIFF", "OGG", "Opus", "ALAC", "APE", "WavPack",
];

impl App {
    /// Remember the open tabs at `path` (real mode: [`session::default_path`]).
    pub fn enable_session(&mut self, path: Option<PathBuf>) {
        self.session.path = path;
    }

    /// The open tabs as stored: the submitted query (else the typed text), filter box, chips,
    /// selected group; user and open folder.
    pub fn session_snapshot(&self) -> Session {
        let tabs = self.tabs.iter().map(|t| match &t.kind {
            TabKind::Search(st) => SessionTab::Search {
                typed: st
                    .search
                    .as_ref()
                    .map_or(&st.edit.text, |v| &v.query)
                    .clone(),
                filter: st.find.text.clone(),
                formats: st.filters.formats.iter().map(|f| f.to_string()).collect(),
                free_slot_only: st.filters.free_slot,
                sort: Some(st.filters.sort.name().to_string()),
                layout: Some(layout_name(st.filters.layout).to_string()),
                selected_group: t.group.as_ref().map(|(u, d)| format!("{u}\n{d}")),
            },
            TabKind::User(u) => SessionTab::User {
                username: u.browse.username.clone(),
                folder: u.browse.selected.clone(),
            },
        });
        Session::new(tabs.collect(), self.active)
    }

    /// Bring back the saved tabs as shells (once per run; a corrupt or missing file keeps
    /// the fresh empty search tab).
    pub(super) fn restore_session(&mut self) {
        if std::mem::replace(&mut self.session.restored, true) {
            return;
        }
        let Some(path) = self.session.path.clone() else {
            return;
        };
        let saved = session::load(&path);
        if !saved.tabs.is_empty() {
            self.tabs.clear();
            for t in &saved.tabs {
                let id = self.next_tab;
                self.next_tab += 1;
                let mut tab = Tab::new(id, shell(t));
                if let SessionTab::Search { selected_group, .. } = t {
                    tab.group = selected_group
                        .as_deref()
                        .and_then(|g| g.split_once('\n'))
                        .map(|(u, d)| (u.to_string(), d.to_string()));
                }
                self.tabs.push(tab);
            }
            // Every tab fetches when shown; the active one is shown now, so it goes first.
            for a in session::restore_plan(&saved) {
                self.tabs[a.tab()].restore = true;
            }
            self.active = saved.active;
        }
        self.session.last = Some(self.session_snapshot());
    }

    /// Each tick: fetch a restored tab once it is shown and connected, and save changes.
    /// Returns whether a fetch started and how long until a held-back save is due.
    pub(super) fn session_tick(&mut self) -> (bool, Option<f64>) {
        if !self.session.restored {
            return (false, None);
        }
        let fetched = self.fetch_shown_tab();
        let Some(path) = self.session.path.clone() else {
            return (fetched, None);
        };
        let snap = self.session_snapshot();
        if self.session.last.as_ref() != Some(&snap) {
            self.session.last = Some(snap);
            self.session.debouncer.changed();
        }
        let now = Instant::now();
        if self.session.debouncer.due(now) {
            self.save_session(&path, now);
        }
        let wait = self.session.debouncer.next_due();
        (
            fetched,
            wait.map(|t| t.saturating_duration_since(now).as_secs_f64()),
        )
    }

    /// Quitting: save what the debouncer still holds back.
    pub fn flush_session(&mut self) {
        let Some(path) = self.session.path.clone().filter(|_| self.session.restored) else {
            return;
        };
        let snap = self.session_snapshot();
        if self.session.debouncer.is_dirty() || self.session.last.as_ref() != Some(&snap) {
            self.session.last = Some(snap);
            self.save_session(&path, Instant::now());
        }
    }

    fn save_session(&mut self, path: &std::path::Path, now: Instant) {
        if let Some(s) = &self.session.last
            && let Err(e) = session::save(path, s)
        {
            eprintln!("wholeseek: could not save the open tabs ({e})");
        }
        self.session.debouncer.saved(now);
    }

    fn fetch_shown_tab(&mut self) -> bool {
        if !matches!(self.conn, Conn::Connected { .. }) {
            return false;
        }
        let Some(tab) = self.tabs.get_mut(self.active).filter(|t| t.restore) else {
            return false;
        };
        tab.restore = false;
        match &tab.kind {
            TabKind::Search(st) => {
                if st.search.is_some() {
                    return false;
                }
                self.submit_search();
            }
            TabKind::User(u) => {
                let name = u.browse.username.clone();
                self.backend.browse(&name);
                self.backend.user_profile(&name);
                self.profiles.requested(&name);
            }
        }
        true
    }
}

fn layout_name(l: ResultsLayout) -> &'static str {
    match l {
        ResultsLayout::Albums => "albums",
        ResultsLayout::Folders => "folders",
    }
}

/// A restored tab before anything is fetched.
fn shell(t: &SessionTab) -> TabKind {
    match t {
        SessionTab::Search {
            typed,
            filter,
            formats,
            free_slot_only,
            sort,
            layout,
            ..
        } => {
            // Unknown or missing names (older or newer files) keep the defaults.
            let mut filters = Filters {
                free_slot: *free_slot_only,
                sort: sort
                    .as_deref()
                    .and_then(SortBy::from_name)
                    .unwrap_or_default(),
                layout: match layout.as_deref() {
                    Some("folders") => ResultsLayout::Folders,
                    _ => ResultsLayout::Albums,
                },
                ..Filters::default()
            };
            filters.formats = FORMATS
                .into_iter()
                .filter(|l| formats.iter().any(|f| f == l))
                .collect();
            let mut st = SearchTab::new(filters);
            st.edit.replace_text(typed);
            st.find.replace_text(filter);
            TabKind::Search(st)
        }
        SessionTab::User { username, folder } => {
            let mut browse = BrowseView::new(username);
            if let Some(f) = folder {
                browse.select(f);
            }
            TabKind::User(UserTab { browse })
        }
    }
}
