//! What the context menus, the transfers drawer and settings do: folder deep links, file
//! actions on finished downloads, bulk transfer control, and settings applied as they change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wholeseek_core::{RemoteFile, Settings, TransferId, TransferKind};

use crate::app::{App, TState};
use crate::menu::{Menu, MenuAction, MenuAnchor, MenuItem};
use crate::transfers::{self, TFilter};

/// "Cancel all" asks first above this many.
pub const CONFIRM_CANCEL_OVER: usize = 5;
/// ▶ tries at most this many other sources after the one clicked.
pub const MAX_ALTERNATES: usize = 4;

/// The transfers drawer's own state.
#[derive(Debug, Default)]
pub struct DrawerUi {
    pub filter: TFilter,
    /// Groups (user, folder) folded to their header.
    pub collapsed: HashSet<(String, String)>,
    /// "Cancel N transfers?" is showing, for this many.
    pub confirm_cancel: Option<usize>,
    /// Cleared rows: gone from the list, still known (a finished file stays "downloaded").
    pub hidden: HashSet<TransferId>,
}

impl App {
    // ---- menus ---------------------------------------------------------------------------

    pub fn open_menu(&mut self, anchor: MenuAnchor, items: Vec<MenuItem>) {
        self.menu = (!items.is_empty()).then_some(Menu { anchor, items });
    }

    /// Run a picked item. Returns text for the clipboard (Copy path); the caller copies it.
    pub fn run_menu(&mut self, action: MenuAction) -> Option<String> {
        self.menu = None;
        match action {
            MenuAction::Play(f) => self.play(&f),
            MenuAction::ShowTab(i) => self.activate(i),
            // The session saves on its own: each tick diffs a snapshot of the tabs.
            MenuAction::SetSort(by) => {
                if let Some(st) = self.tab_mut().search_mut() {
                    st.filters.sort = by;
                    self.last_filters.sort = by;
                }
            }
            MenuAction::ToggleFormat(label) => {
                if let Some(st) = self.tab_mut().search_mut() {
                    st.filters.toggle(label);
                    let chosen = st.filters.clone();
                    self.last_filters = chosen;
                }
                self.find_changed();
            }
            MenuAction::Download(f) => self.download(&f),
            MenuAction::Keep(f) => self.keep(&f),
            MenuAction::DownloadFolder { username, folder } => {
                self.download_folder(&username, &folder)
            }
            MenuAction::DownloadTo {
                target,
                dir: Some(dir),
            } => self.download_to(&target, &dir),
            // The view asks for a folder first (it has the dialog); see `choose_download_dir`.
            MenuAction::DownloadTo { dir: None, .. } => {}
            MenuAction::OpenFolder {
                username,
                folder,
                file,
            } => {
                self.open_folder_at(&username, &folder, file.as_deref());
            }
            MenuAction::BrowseUser(u) => self.open_user(&u),
            MenuAction::PlayFolder { username, folder } => self.play_folder(&username, &folder),
            MenuAction::CopyPath(p) => return Some(p),
            MenuAction::PlayLocal(id) => self.play_local(id),
            MenuAction::ShowInFolder(p) => self.desktop.show_item(&p),
            MenuAction::OpenFile(p) => self.desktop.open(&p),
            MenuAction::PlayAlbum { username, folder } => self.play_album(&username, &folder),
            MenuAction::Cancel(id) => self.cancel(id),
            MenuAction::Retry(id) => self.retry(id),
            MenuAction::OpenInNewTab { username, folder } => {
                self.open_in_new_tab(&username, folder.as_deref());
            }
            MenuAction::ExpandAll(d) => {
                if let Some(b) = self.active_browse_mut() {
                    b.expand_all(&d, true);
                }
            }
            MenuAction::CollapseAll(d) => {
                if let Some(b) = self.active_browse_mut() {
                    b.expand_all(&d, false);
                }
            }
            MenuAction::DuplicateTab(i) => self.duplicate_tab(i),
            MenuAction::CloseTab(i) => self.close_tab(i),
            MenuAction::CloseOtherTabs(i) => self.close_other_tabs(i),
            MenuAction::CloseTabsRight(i) => self.close_tabs_right(i),
        }
        None
    }

    // ---- tabs ----------------------------------------------------------------------------

    /// "Open in new tab" / middle-click: a user tab beside the shown one, at `folder`, left
    /// in the background. It reuses the user's listing if a tab already has it, else browses
    /// when first shown.
    pub fn open_in_new_tab(&mut self, username: &str, folder: Option<&str>) {
        let mut browse = crate::tabs::BrowseView::new(username);
        let listed = self
            .tabs
            .iter()
            .filter_map(|t| t.browse())
            .find(|b| b.username == username && b.dirs.is_some())
            .and_then(|b| b.dirs.clone());
        let fetch = listed.is_none();
        browse.dirs = listed;
        if let Some(f) = folder {
            browse.select(f);
        }
        let at = self.active + 1;
        let kind = crate::tabs::TabKind::User(crate::tabs::UserTab { browse });
        self.insert_tab(at, kind).restore = fetch;
    }

    /// The strip's "Duplicate tab": a copy right after tab `i`, shown. A search runs again
    /// (its query, chips, filter and selection kept); a user tab keeps its listing and folder.
    pub fn duplicate_tab(&mut self, i: usize) {
        use crate::tabs::{BrowseView, SearchTab, TabKind, UserTab};
        let Some(src) = self.tabs.get(i) else {
            return;
        };
        let group = src.group.clone();
        let (kind, fetch) = match &src.kind {
            TabKind::Search(st) => {
                let mut copy = SearchTab::new(st.filters.clone());
                let query = st.search.as_ref().map_or(&st.edit.text, |v| &v.query);
                copy.edit.replace_text(query.clone());
                copy.find.replace_text(st.find.text.clone());
                let fetch = st.search.is_some();
                (TabKind::Search(copy), fetch)
            }
            TabKind::User(u) => {
                let b = &u.browse;
                let mut copy = BrowseView::new(&b.username);
                copy.dirs = b.dirs.clone();
                copy.expanded = b.expanded.clone();
                if let Some(sel) = &b.selected {
                    copy.select(sel);
                }
                let fetch = copy.dirs.is_none();
                (TabKind::User(UserTab { browse: copy }), fetch)
            }
        };
        let tab = self.insert_tab(i + 1, kind);
        tab.group = group;
        tab.restore = fetch;
        self.active = i + 1;
    }

    /// The strip's "Close other tabs": only tab `i` stays, shown.
    pub fn close_other_tabs(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.close_tabs_right(i);
        for j in (0..i).rev() {
            self.close_tab(j);
        }
        self.active = 0;
    }

    /// The strip's "Close tabs to the right" of tab `i`; a closed tab that was shown leaves
    /// tab `i` shown.
    pub fn close_tabs_right(&mut self, i: usize) {
        while self.tabs.len() > i + 1 {
            self.close_tab(self.tabs.len() - 1);
        }
        if self.active > i {
            self.active = i;
        }
    }

    // ---- folders -------------------------------------------------------------------------

    /// The user's tab (opened or focused) at `folder`, its ancestors expanded and `file`
    /// highlighted. Before the share list arrives the tab says "Opening folder…"; the
    /// selection is applied when it lands.
    pub fn open_folder_at(&mut self, username: &str, folder: &str, file: Option<&str>) {
        self.open_user(username);
        let tab = &mut self.tabs[self.active];
        tab.scroll.set(0.0);
        if let Some(b) = tab.browse_mut() {
            b.select(folder);
            b.highlight = file.map(str::to_string);
        }
        self.reveal = true;
    }

    /// "Show in list" (the player's title): the playing file's row, selected and scrolled
    /// into view, in the tab it was started from; else any open tab listing it; else its
    /// user's tab at its folder.
    pub fn reveal_playing(&mut self) {
        let Some(file) = self.playing.as_ref().map(|p| p.file.clone()) else {
            return;
        };
        let lists = |t: &crate::tabs::Tab| match &t.kind {
            crate::tabs::TabKind::Search(st) => st.search.as_ref().is_some_and(|v| {
                v.hits
                    .iter()
                    .any(|h| crate::tabs::same_file(&h.file, &file))
            }),
            crate::tabs::TabKind::User(u) => {
                u.browse.username == file.username
                    && u.browse.dirs.as_ref().is_some_and(|d| {
                        d.iter()
                            .any(|d| d.files.iter().any(|f| f.path == file.path))
                    })
            }
        };
        let from = self
            .play_from
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .filter(|&i| lists(&self.tabs[i]));
        let Some(i) = from.or_else(|| self.tabs.iter().position(lists)) else {
            self.open_folder_at(&file.username, file.folder(), Some(&file.path));
            self.tab_mut().selected = Some(file.path.clone());
            return;
        };
        self.active = i;
        let tab = self.tab_mut();
        tab.selected = Some(file.path.clone());
        if let Some(b) = tab.browse_mut() {
            b.select(file.folder());
            b.highlight = Some(file.path.clone());
        } else {
            tab.group = Some((file.username.clone(), file.folder().to_string()));
        }
        self.reveal = true;
    }

    /// The scroll target for a highlighted file, once its folder is listed: (list key, row key).
    /// In a search tab, its selected row.
    pub fn take_reveal(&mut self) -> Option<(String, String)> {
        if !self.reveal {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        if let Some(st) = tab.search() {
            self.reveal = false;
            let (sel, (user, _)) = (tab.selected.as_deref()?, tab.group.as_ref()?);
            let i = st
                .search
                .as_ref()?
                .hits
                .iter()
                .position(|h| &h.file.username == user && h.file.path == sel)?;
            return Some((tab.list_key(), format!("row-{i}")));
        }
        let b = tab.browse()?;
        if b.error.is_some() {
            self.reveal = false;
            return None;
        }
        let dir = b.selected_dir()?;
        self.reveal = false;
        let hl = b.highlight.as_deref()?;
        let i = dir.files.iter().position(|f| f.path == hl)?;
        Some((tab.list_key(), format!("file-{i}")))
    }

    /// ↓ a folder: the core expands it and skips system files (`wholeseek_core::DOWNLOAD_SKIP`).
    pub fn download_folder(&mut self, username: &str, folder: &str) {
        self.backend.download_folder(username, folder);
        let root = self.backend.settings().download_dir.clone();
        self.remember_folder_dest(username, folder, &root);
    }

    // ---- finished downloads --------------------------------------------------------------

    /// Where a finished download of `file` is on disk.
    pub fn local_path_of(&self, file: &RemoteFile) -> Option<PathBuf> {
        match &self.download_of(file)?.state {
            TState::Done(p) => Some(p.clone()),
            _ => None,
        }
    }

    pub(crate) fn group_done_path(&self, username: &str, folder: &str) -> Option<PathBuf> {
        self.group_done_files(username, folder)
            .first()
            .and_then(|f| self.local_path_of(f))
    }

    pub fn open_downloads(&mut self) {
        let dir = self.backend.settings().download_dir.clone();
        self.desktop.open(&dir);
    }

    // ---- ▶ fallback ----------------------------------------------------------------------

    /// Every search hit in open tabs (for alternates).
    pub(crate) fn all_hits(&self) -> Vec<wholeseek_core::SearchHit> {
        self.tabs
            .iter()
            .filter_map(|t| t.search()?.search.as_ref())
            .flat_map(|s| s.hits.iter().cloned())
            .collect()
    }

    /// Other sources for `file`, best first, at most [`MAX_ALTERNATES`].
    pub(crate) fn alternates_for(&self, file: &RemoteFile) -> Vec<RemoteFile> {
        let mut v = wholeseek_core::rank::alternates(file, &self.all_hits());
        v.retain(crate::tabs::is_playable);
        v.truncate(MAX_ALTERNATES);
        v
    }

    // ---- bulk transfers ------------------------------------------------------------------

    fn listed_ids(&self, pick: impl Fn(&TState) -> bool) -> Vec<TransferId> {
        transfers::listed(&self.transfers, &self.drawer.hidden)
            .filter(|t| pick(&t.state))
            .map(|t| t.id)
            .collect()
    }

    fn group_ids(
        &self,
        username: &str,
        folder: &str,
        pick: impl Fn(&TState) -> bool,
    ) -> Vec<TransferId> {
        transfers::listed(&self.transfers, &self.drawer.hidden)
            .filter(|t| t.file.username == username && t.file.folder() == folder && pick(&t.state))
            .map(|t| t.id)
            .collect()
    }

    pub fn live_count(&self) -> usize {
        self.listed_ids(|s| !transfers::is_finished(s)).len()
    }

    /// "Cancel all": active and queued downloads; asks first above [`CONFIRM_CANCEL_OVER`].
    pub fn cancel_all(&mut self) {
        let n = self.live_count();
        if n > CONFIRM_CANCEL_OVER && self.drawer.confirm_cancel.is_none() {
            self.drawer.confirm_cancel = Some(n);
            return;
        }
        self.drawer.confirm_cancel = None;
        for id in self.listed_ids(|s| !transfers::is_finished(s)) {
            self.cancel(id);
        }
    }

    /// "Keep" on the confirm line.
    pub fn keep_transfers(&mut self) {
        self.drawer.confirm_cancel = None;
    }

    /// "Retry failed": every failed or cancelled download (a loop of `Core::retry`).
    pub fn retry_all(&mut self) {
        for id in self.listed_ids(transfers::is_retryable) {
            self.retry(id);
        }
    }

    /// "Clear finished": done, failed and cancelled rows leave the list. Files stay on disk.
    pub fn clear_finished(&mut self) {
        for id in self.listed_ids(transfers::is_finished) {
            self.drawer.hidden.insert(id);
        }
    }

    pub fn cancel_group(&mut self, username: &str, folder: &str) {
        for id in self.group_ids(username, folder, |s| !transfers::is_finished(s)) {
            self.cancel(id);
        }
    }

    pub fn retry_group(&mut self, username: &str, folder: &str) {
        for id in self.group_ids(username, folder, transfers::is_retryable) {
            self.retry(id);
        }
    }

    pub fn clear_group(&mut self, username: &str, folder: &str) {
        for id in self.group_ids(username, folder, transfers::is_finished) {
            self.drawer.hidden.insert(id);
        }
    }

    pub fn toggle_group(&mut self, username: &str, folder: &str) {
        let k = (username.to_string(), folder.to_string());
        if !self.drawer.collapsed.remove(&k) {
            self.drawer.collapsed.insert(k);
        }
    }

    // ---- settings, applied as they change -------------------------------------------------

    /// Change the settings, save them and hand them to the core; flashes "Saved".
    pub fn commit_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.backend.settings().clone();
        change(&mut settings);
        if &settings == self.backend.settings() {
            return;
        }
        let shares_changed = settings.share_dirs != self.backend.settings().share_dirs;
        self.store.save_settings(&settings);
        self.backend.apply_settings(settings.clone());
        if shares_changed {
            self.backend.set_shares(settings.share_dirs.clone());
            self.onboarding.shares.clone_from(&settings.share_dirs);
        }
        self.settings_form.sync(&settings);
        self.settings_form.saved_at = Some(self.now);
    }

    /// A folder picked (or typed, without a dialog) for downloads.
    pub fn set_download_dir(&mut self, dir: PathBuf) {
        self.commit_settings(|s| s.download_dir = dir);
    }

    pub fn add_share(&mut self, dir: PathBuf) {
        if self.backend.settings().share_dirs.contains(&dir) {
            return;
        }
        self.commit_settings(|s| s.share_dirs.push(dir));
    }

    pub fn remove_share(&mut self, dir: &Path) {
        self.commit_settings(|s| s.share_dirs.retain(|d| d != dir));
    }

    pub fn step_upload_slots(&mut self, step: i64) {
        let n = (self.backend.settings().upload_slots as i64 + step).clamp(1, 100) as usize;
        self.commit_settings(|s| s.upload_slots = n);
    }

    pub fn step_play_timeout(&mut self, step: i64) {
        let n = (self.backend.settings().play_start_timeout_secs as i64 + step).clamp(2, 60) as u64;
        self.commit_settings(|s| s.play_start_timeout_secs = n);
    }

    /// Port typed + Enter: ask before reconnecting on it.
    pub fn propose_port(&mut self) {
        let text = self.settings_form.listen_port.text.trim().to_string();
        match text.parse::<u16>() {
            Ok(p) if p >= 1024 && p != self.backend.settings().listen_port => {
                self.settings_form.port_confirm = Some(p);
            }
            Ok(p) if p == self.backend.settings().listen_port => {
                self.settings_form.port_confirm = None;
                self.settings_form.port_editing = false;
            }
            _ => {
                self.settings_form.port_confirm = None;
                self.settings_form.port_invalid = true;
            }
        }
    }

    /// "Reconnect" on the port confirm: the core reconnects on the new port.
    pub fn confirm_port(&mut self) {
        if let Some(p) = self.settings_form.port_confirm.take() {
            self.settings_form.port_editing = false;
            self.commit_settings(|s| s.listen_port = p);
        }
    }

    pub fn undo_port(&mut self) {
        self.settings_form.port_confirm = None;
        self.settings_form.port_invalid = false;
        self.settings_form.port_editing = false;
        let p = self.backend.settings().listen_port;
        self.settings_form.listen_port.replace_text(p.to_string());
    }

    /// "Clear cache" can't run while the playing track streams from the cache.
    pub fn cache_in_use(&self) -> bool {
        self.playing.as_ref().is_some_and(|p| {
            self.transfers
                .iter()
                .any(|t| t.id == p.ticket.id && t.kind == TransferKind::Play)
        })
    }

    pub fn clear_cache(&mut self) {
        if !self.cache_in_use() {
            self.backend.clear_cache();
        }
    }
}

/// The folder-picker answer: a folder, a cancel, or no dialog on this desktop (fall back to
/// typing the path).
pub fn picked(s: &mut App, result: rune::FileDialogResult, apply: fn(&mut App, PathBuf)) {
    match result {
        Ok(Some(p)) => apply(s, p),
        Ok(None) => {}
        Err(e) => {
            s.settings_form.manual = Some(e.to_string());
        }
    }
}

/// "Download to…": the folder picker, starting at the most recent such folder (else the
/// downloads folder); the pick saves `target` there. With no dialog on this desktop, a note
/// points at Settings.
pub fn choose_download_dir(s: &App, tx: &mut rune::Tx, target: crate::menu::DlTarget) {
    let settings = s.backend.settings();
    let start = settings
        .recent_download_dirs
        .first()
        .unwrap_or(&settings.download_dir)
        .clone();
    let options = rune::FileDialogOptions {
        title: "Download to".into(),
        directory: Some(start),
        ..Default::default()
    };
    tx.open_folder(options, move |s: &mut App, result, _| match result {
        Ok(Some(dir)) => s.download_to(&target, &dir),
        Ok(None) => {}
        Err(_) => s.show_notice("No folder dialog on this desktop".into()),
    });
}
