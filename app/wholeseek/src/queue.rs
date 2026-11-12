//! Playing in order. Every ▶ plays a queue: the clicked track's folder (user tab) or result
//! folder (search), audio only, starting at it; a lone file is a queue of one. The player
//! (`wholeseek_audio`'s queue) advances by itself; the app fetches each item when it becomes
//! current or the player asks to prefetch it, and hands over the path once bytes arrive.
//! An item whose fetch fails before any sound quietly tries other sources
//! (`rank::alternates`), then is skipped.

use std::path::PathBuf;

use wholeseek_audio::{PlayerEvent, QueueItem};
use wholeseek_core::{PlayTicket, RemoteFile, TransferId, TransferKind};

use crate::app::{App, NowPlaying, TState};

#[derive(Debug, Clone)]
pub struct QItem {
    pub file: RemoteFile,
    /// The ▶ fetch, once started (a finished download has none).
    pub ticket: Option<PlayTicket>,
    /// The file the player reads, once known: a `.part` with bytes in it, or a download.
    pub path: Option<PathBuf>,
    /// Sources still to try if this one fails before any sound.
    pub sources: Vec<RemoteFile>,
    /// Which source this is, from 1.
    pub attempt: usize,
    /// Gave up on it (no source answered).
    pub failed: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PlayQueue {
    pub items: Vec<QItem>,
    pub index: usize,
    /// Which queue this is: player item ids are "<generation>:<index>", so an event the
    /// player sent for an earlier queue (a late `PrefetchNext` after ▶ on another album) is
    /// told apart and dropped.
    pub generation: u64,
}

/// The player id of queue item `index` in queue `generation`.
pub fn item_id(generation: u64, index: usize) -> String {
    format!("{generation}:{index}")
}

impl PlayQueue {
    pub fn current(&self) -> Option<&QItem> {
        self.items.get(self.index)
    }
    pub fn next_title(&self) -> Option<String> {
        self.items
            .get(self.index + 1)
            .map(|i| crate::fmt::title(&i.file))
    }
}

impl App {
    /// ▶ on `file` with its folder as the queue (`list`, in order; non-audio dropped). ▶ on the
    /// current track toggles pause instead.
    pub fn play_in(&mut self, file: &RemoteFile, list: &[RemoteFile]) {
        if !crate::tabs::is_playable(file) {
            return;
        }
        if self
            .playing
            .as_ref()
            .is_some_and(|p| &p.file == file && p.failed.is_none())
        {
            self.toggle_pause();
            return;
        }
        let mut files: Vec<RemoteFile> = list
            .iter()
            .filter(|f| crate::tabs::is_playable(f))
            .cloned()
            .collect();
        let start = match files.iter().position(|f| f == file) {
            Some(i) => i,
            None => {
                files = vec![file.clone()];
                0
            }
        };
        self.start_queue(files, start);
    }

    /// ▶ a lone file.
    pub fn play(&mut self, file: &RemoteFile) {
        self.play_in(file, std::slice::from_ref(file));
    }

    /// ▶ a finished download from disk: no network.
    pub fn play_local(&mut self, id: TransferId) {
        if let Some(f) = self
            .transfers
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.file.clone())
            .filter(crate::tabs::is_playable)
        {
            self.start_queue(vec![f], 0);
        }
    }

    /// "Play album": a drawer group's finished tracks in order, from disk.
    pub fn play_album(&mut self, username: &str, folder: &str) {
        let files = self.group_done_files(username, folder);
        if !files.is_empty() {
            self.start_queue(files, 0);
        }
    }

    /// "Play folder": a remote folder's audio, from the user's listing if open, else the
    /// search hits in it.
    pub fn play_folder(&mut self, username: &str, folder: &str) {
        let files = self.folder_files(username, folder);
        if let Some(first) = files.iter().find(|f| crate::tabs::is_playable(f)).cloned() {
            self.play_in(&first, &files);
        }
    }

    /// A folder line's menu, with "Play folder" only if something there can play.
    pub fn folder_menu(&self, username: &str, folder: &str) -> Vec<crate::menu::MenuItem> {
        let playable = self
            .folder_files(username, folder)
            .iter()
            .any(crate::tabs::is_playable);
        crate::menu::folder_items(username, folder, playable)
    }

    /// The files of a remote folder as far as the app knows them, in path order.
    pub fn folder_files(&self, username: &str, folder: &str) -> Vec<RemoteFile> {
        let listed = self
            .tabs
            .iter()
            .filter_map(|t| t.browse())
            .find(|b| b.username == username)
            .and_then(|b| b.dirs.as_ref())
            .and_then(|d| d.iter().find(|d| d.path == folder))
            .map(|d| d.files.clone());
        if let Some(files) = listed {
            return files;
        }
        let mut seen = std::collections::HashSet::new();
        let mut files: Vec<RemoteFile> = self
            .all_hits()
            .into_iter()
            .map(|h| h.file)
            .filter(|f| f.username == username && f.folder() == folder)
            .filter(|f| seen.insert(f.path.clone()))
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    fn start_queue(&mut self, files: Vec<RemoteFile>, start: usize) {
        self.stop_current();
        let items: Vec<QItem> = files
            .into_iter()
            .map(|f| QItem {
                path: self.local_path_of(&f),
                sources: Vec::new(),
                ticket: None,
                attempt: 1,
                failed: None,
                file: f,
            })
            .collect();
        self.queue_generation += 1;
        let generation = self.queue_generation;
        let for_player: Vec<QueueItem> = items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let id = item_id(generation, i);
                match &it.path {
                    Some(p) => QueueItem::with_path(id, p.clone(), it.file.size),
                    None => QueueItem::new(id, it.file.size),
                }
            })
            .collect();
        self.queue = Some(PlayQueue {
            items,
            index: start,
            generation,
        });
        self.notice = None;
        self.ensure_fetch(start);
        self.set_now_playing(start);
        self.player.set_queue(for_player, start);
        self.player.set_volume(self.volume);
        self.refresh_snapshot_now();
    }

    /// Start item `i`'s ▶ fetch if it has neither a file nor a fetch yet.
    fn ensure_fetch(&mut self, i: usize) {
        let Some(item) = self.queue.as_ref().and_then(|q| q.items.get(i)) else {
            return;
        };
        if item.path.is_some() || item.ticket.is_some() || item.failed.is_some() {
            return;
        }
        let file = item.file.clone();
        let sources = self.alternates_for(&file);
        let ticket = self.backend.play_fetch(&file);
        self.upsert_play(ticket.id, &file);
        if let Some(item) = self.queue.as_mut().and_then(|q| q.items.get_mut(i)) {
            item.ticket = Some(ticket);
            item.sources = sources;
        }
    }

    /// The player strip follows queue item `i`.
    fn set_now_playing(&mut self, i: usize) {
        let Some(item) = self.queue.as_ref().and_then(|q| q.items.get(i)).cloned() else {
            return;
        };
        let ticket = item.ticket.clone().unwrap_or_else(|| {
            // A finished download: the transfer it came from, at its path.
            let path = item.path.clone().unwrap_or_default();
            let id = self.download_of(&item.file).map_or(TransferId(0), |d| d.id);
            PlayTicket {
                id,
                part_path: path.clone(),
                final_path: path,
            }
        });
        self.playing = Some(NowPlaying {
            ticket,
            file: item.file.clone(),
            opened: item.path.is_some(),
            keep: item.ticket.is_none(),
            snapshot: None,
            failed: item.failed.clone(),
            attempt: item.attempt,
        });
    }

    pub(crate) fn refresh_snapshot_now(&mut self) {
        if let Some(p) = &mut self.playing
            && p.opened
        {
            p.snapshot = Some(self.player.snapshot());
        }
    }

    /// Stop the current queue; its unfinished ▶ fetches are cancelled unless kept (↓ Keep).
    pub(crate) fn stop_current(&mut self) {
        self.player.stop();
        let kept = self
            .playing
            .as_ref()
            .filter(|p| p.keep)
            .map(|p| p.ticket.id);
        if let Some(q) = self.queue.take() {
            for id in q
                .items
                .iter()
                .filter_map(|i| i.ticket.as_ref().map(|t| t.id))
            {
                if Some(id) != kept && self.play_fetch_live(id) {
                    self.backend.cancel(id);
                }
            }
        }
        self.playing = None;
    }

    fn play_fetch_live(&self, id: TransferId) -> bool {
        self.transfers
            .iter()
            .any(|t| t.id == id && t.kind == TransferKind::Play && t.is_live())
    }

    pub fn next_track(&mut self) {
        self.player.next();
    }

    pub fn previous_track(&mut self) {
        self.player.previous();
    }

    /// (position from 1, length) for "3 / 12", when more than one track is queued.
    pub fn queue_position(&self) -> Option<(usize, usize)> {
        let q = self.queue.as_ref()?;
        (q.items.len() > 1).then_some((q.index + 1, q.items.len()))
    }

    // ---- player and transfer events ------------------------------------------------------

    pub(crate) fn apply_player(&mut self, e: PlayerEvent) {
        // A queue event for a queue that's gone (stopped or replaced) changes nothing.
        if let PlayerEvent::TrackChanged { index, id } | PlayerEvent::PrefetchNext { index, id } =
            &e
        {
            let current = self.queue.as_ref().map(|q| item_id(q.generation, *index));
            if current.as_deref() != Some(id.as_str()) {
                return;
            }
        }
        match e {
            PlayerEvent::TrackChanged { index, .. } => {
                let Some(q) = self.queue.as_mut() else { return };
                if index >= q.items.len() {
                    return;
                }
                let prev = std::mem::replace(&mut q.index, index);
                if q.items[index].failed.is_some() {
                    // No source for it: on to the next one.
                    self.player.next();
                    return;
                }
                self.ensure_fetch(index);
                // Moving on: a fetch neither current nor next is no longer needed.
                if prev != index {
                    self.cancel_stale_fetches();
                }
                let keep = self.playing.as_ref().is_some_and(|p| p.keep);
                let was = self.playing.as_ref().map(|p| p.file.clone());
                self.set_now_playing(index);
                if let Some(p) = self.playing.as_mut()
                    && was.as_ref() == Some(&p.file)
                {
                    p.keep |= keep;
                }
                self.refresh_snapshot_now();
            }
            PlayerEvent::PrefetchNext { index, .. } => self.ensure_fetch(index),
            _ => {}
        }
    }

    fn cancel_stale_fetches(&mut self) {
        let Some(q) = &self.queue else { return };
        let keep = [q.index, q.index + 1];
        let stale: Vec<(usize, TransferId)> = q
            .items
            .iter()
            .enumerate()
            .filter(|(i, it)| !keep.contains(i) && it.path.is_none())
            .filter_map(|(i, it)| it.ticket.as_ref().map(|t| (i, t.id)))
            .collect();
        for (i, id) in stale {
            if self.play_fetch_live(id) {
                self.backend.cancel(id);
                if let Some(it) = self.queue.as_mut().and_then(|q| q.items.get_mut(i)) {
                    it.ticket = None;
                }
            }
        }
    }

    /// First bytes of a queued fetch: give the player its file.
    pub(crate) fn queue_progress(
        &mut self,
        id: TransferId,
        part_path: &std::path::Path,
        total: u64,
    ) {
        let Some(q) = self.queue.as_mut() else { return };
        let Some(i) = q
            .items
            .iter()
            .position(|it| it.path.is_none() && it.ticket.as_ref().is_some_and(|t| t.id == id))
        else {
            return;
        };
        q.items[i].path = Some(part_path.to_path_buf());
        let current = q.index == i;
        self.player.set_queue_path(i, part_path, total);
        if current && let Some(p) = self.playing.as_mut() {
            p.opened = true;
            self.player.set_volume(self.volume);
        }
        self.refresh_snapshot_now();
    }

    /// A queued fetch failed. Before any sound: next source, quietly; none left: skip it
    /// (or, as the last track, say so). True if the failure was handled here.
    pub(crate) fn queue_failed(&mut self, id: TransferId, reason: &str) -> bool {
        let Some(q) = self.queue.as_mut() else {
            return false;
        };
        let Some(i) = q
            .items
            .iter()
            .position(|it| it.ticket.as_ref().is_some_and(|t| t.id == id))
        else {
            return false;
        };
        if q.items[i].path.is_some() {
            return false; // it was playing: the player reports the cut
        }
        let current = q.index == i;
        let last = i + 1 >= q.items.len();
        let item = &mut q.items[i];
        if !item.sources.is_empty() {
            let alt = item.sources.remove(0);
            item.attempt += 1;
            item.file = alt.clone();
            let ticket = self.backend.play_fetch(&alt);
            self.upsert_play(ticket.id, &alt);
            if let Some(item) = self.queue.as_mut().and_then(|q| q.items.get_mut(i)) {
                item.ticket = Some(ticket);
            }
            if current {
                self.set_now_playing(i);
            }
            return true;
        }
        let gave_up = if item.attempt > 1 {
            "No source responded".to_string()
        } else {
            reason.to_string()
        };
        item.failed = Some(gave_up.clone());
        item.ticket = None;
        if current {
            if last {
                if let Some(p) = self.playing.as_mut() {
                    p.failed = Some(gave_up);
                }
            } else {
                self.player.next();
            }
        }
        true
    }

    /// A drawer group's finished audio files, in request order.
    pub(crate) fn group_done_files(&self, username: &str, folder: &str) -> Vec<RemoteFile> {
        crate::transfers::listed(&self.transfers, &self.drawer.hidden)
            .filter(|t| t.file.username == username && t.file.folder() == folder)
            .filter(|t| matches!(t.state, TState::Done(_)) && crate::tabs::is_playable(&t.file))
            .map(|t| t.file.clone())
            .collect()
    }
}
