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
            MenuAction::ToggleFormat(label) => {
                if let Some(st) = self.tab_mut().search_mut() {
                    st.filters.toggle(label);
                    let chosen = st.filters.clone();
                    self.last_filters = chosen;
                }
                self.find_changed();
            }
            MenuAction::Download(f) => self.download(&f),
            MenuAction::DownloadFolder { username, folder } => {
                self.download_folder(&username, &folder)
            }
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
        }
        None
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

    /// The scroll target for a highlighted file, once its folder is listed: (list key, row key).
    pub fn take_reveal(&mut self) -> Option<(String, String)> {
        if !self.reveal {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
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
