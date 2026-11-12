//! Album covers in the UI: the [`cover`] square (player bar, album headers, folder rows) and
//! [`Art`], which decides which covers to ask for and holds their decoded images.
//!
//! What may fetch: only the folder the user selected (search inspector, user tab) and the
//! playing track's folder, from [`App::sync_covers`] once per frame — never list rendering.
//! Rows call [`row_thumb`], which reads the cache in memory and draws nothing otherwise.
//! For the playing track the art embedded in the file is tried first (off the UI thread);
//! the folder's image is fetched only when the file has none.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use rune::tok::Theme;
use rune::{El, Image, ImageCache, SurfaceFit, Waker, el};
use wholeseek_audio::{ArtLookup, embedded_art};
use wholeseek_core::{Covers, RemoteFile, TransferId};

use super::icons;
use crate::app::{App, TState};
use crate::tabs::TabKind;

/// Player bar cover, points.
pub const PLAYER: f32 = 44.0;
/// Search inspector / user folder header cover.
pub const HEADER: f32 = 104.0;
/// Folder row thumbnail (only when cached). Rows are not wired yet (v0.1.2 ships without).
#[allow(dead_code)]
pub const ROW: f32 = 28.0;
/// Height of a folder row that shows a thumbnail.
#[allow(dead_code)]
pub const ROW_H: f32 = 32.0;
/// Decoded handles kept at most (folder rows of a long list).
const MAX_HANDLES: usize = 256;

/// (ticket, art found) from an embedded-art attempt.
type ArtResult = (TransferId, bool);
/// A folder to ask for: (user, folder, its listing if known).
type Want = (String, String, Option<Vec<(String, u64)>>);

/// Which embedded-art attempts were made for the playing track.
#[derive(Default)]
struct Embedded {
    ticket: Option<TransferId>,
    tried_open: bool,
    tried_done: bool,
    /// The folder's image was asked for (file has no art, or not readable yet).
    fell_back: bool,
}

/// Cover state the app keeps (decode cache, requests made).
pub struct Art {
    images: ImageCache,
    wake: Arc<Mutex<Option<Waker>>>,
    handles: RefCell<HashMap<PathBuf, Image>>,
    /// Folders asked for last frame; a fetch is asked once per change of selection.
    wanted: Vec<(String, String)>,
    /// Cover cache the notify was installed on (the backend changes after boot).
    hooked: Option<PathBuf>,
    seen_generation: u64,
    ready_handles: usize,
    embedded: Embedded,
    results: (Sender<ArtResult>, Receiver<ArtResult>),
}

impl Default for Art {
    fn default() -> Self {
        let wake: Arc<Mutex<Option<Waker>>> = Arc::default();
        let forward = wake.clone();
        let images = ImageCache::new(Arc::new(move || {
            if let Some(w) = forward.lock().ok().and_then(|w| w.clone()) {
                w();
            }
        }));
        Self {
            images,
            wake,
            handles: RefCell::default(),
            wanted: Vec::new(),
            hooked: None,
            seen_generation: 0,
            ready_handles: 0,
            embedded: Embedded::default(),
            results: mpsc::channel(),
        }
    }
}

impl Art {
    pub fn set_waker(&self, wake: Waker) {
        if let Ok(mut w) = self.wake.lock() {
            *w = Some(wake);
        }
    }

    fn waker(&self) -> Option<Waker> {
        self.wake.lock().ok().and_then(|w| w.clone())
    }

    /// The decoded image of a cached cover file, once it is ready (nothing while decoding).
    pub fn image(&self, path: &PathBuf) -> Option<Image> {
        let mut handles = self.handles.borrow_mut();
        if !handles.contains_key(path) {
            if handles.len() >= MAX_HANDLES {
                handles.retain(|_, i| !i.is_ready());
            }
            handles.insert(path.clone(), self.images.file(path, HEADER, 2.0));
        }
        handles
            .get(path)
            .filter(|i| i.is_ready() && i.error().is_none())
            .cloned()
    }

    /// Folders currently wanted (tests).
    #[cfg(test)]
    pub fn wanted(&self) -> &[(String, String)] {
        &self.wanted
    }
}

/// The cover of (user, folder) if it is cached and decoded. Memory only.
pub fn cached_image(s: &App, username: &str, folder: &str) -> Option<Image> {
    let path = s.backend.covers()?.cached(username, folder)?;
    s.art.image(&path)
}

/// A square cover `size` points wide, corners 4; a disc glyph on the surface colour when
/// there is no image (yet). No transitions: the image just appears.
pub fn cover(s: &App, key: &str, username: &str, folder: &str, size: f32, t: &Theme) -> El {
    let base = el()
        .key(key.to_string())
        .size(size, size)
        .radius(4.0)
        .fill(t.hover)
        .label("Album cover");
    match cached_image(s, username, folder) {
        Some(img) => base.pixels_fit(img.pixels(), SurfaceFit::Fill),
        None => base.center().child(icons::draw(
            &icons::DISC_3,
            (size * 0.42).round(),
            t.text_secondary.alpha(0.55),
        )),
    }
}

/// A folder row's thumbnail, only when the cover is already cached and decoded; `None` keeps
/// the row as it was. Never asks for a fetch.
#[allow(dead_code)]
pub fn row_thumb(s: &App, key: &str, username: &str, folder: &str) -> Option<El> {
    let img = cached_image(s, username, folder)?;
    Some(
        el().key(key.to_string())
            .size(ROW, ROW)
            .radius(4.0)
            .pixels_fit(img.pixels(), SurfaceFit::Fill),
    )
}

fn is_image(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".jpg") || p.ends_with(".jpeg") || p.ends_with(".png")
}

fn listing(files: &[RemoteFile]) -> Vec<(String, u64)> {
    files.iter().map(|f| (f.path.clone(), f.size)).collect()
}

impl App {
    /// Once per frame: ask for the covers of the selected and the playing folder (each once
    /// per change), try the playing file's embedded art, and notice covers that landed or
    /// decoded. Returns true if something visible changed.
    pub fn sync_covers(&mut self) -> bool {
        let Some(covers) = self.backend.covers().cloned() else {
            return false;
        };
        let mut changed = false;
        if self.art.hooked.as_deref() != Some(covers.dir()) {
            self.art.hooked = Some(covers.dir().to_path_buf());
            if let Some(w) = self.art.waker() {
                covers.set_notify(Arc::new(move || w()));
            }
        }
        // Embedded-art attempts that finished.
        while let Ok((ticket, found)) = self.art.results.1.try_recv() {
            if !found
                && self.art.embedded.ticket == Some(ticket)
                && !self.art.embedded.fell_back
                && let Some(p) = &self.playing
            {
                self.art.embedded.fell_back = true;
                let (u, d) = (p.file.username.clone(), p.file.folder().to_string());
                let files = self.folder_files(&u, &d);
                let known = files.iter().any(|f| is_image(&f.path));
                self.backend
                    .fetch_cover(&u, &d, known.then(|| listing(&files)));
            }
        }
        self.try_embedded(&covers);

        // The selected folder: search inspector or user tab.
        let mut wanted: Vec<Want> = Vec::new();
        let tab = self.tab();
        match &tab.kind {
            TabKind::Search(_) => {
                if let Some((u, d)) = &tab.group {
                    let files = self.folder_files(u, d);
                    let known = files.iter().any(|f| is_image(&f.path));
                    wanted.push((u.clone(), d.clone(), known.then(|| listing(&files))));
                }
            }
            TabKind::User(ut) => {
                if let Some(dir) = ut.browse.selected_dir() {
                    wanted.push((
                        ut.browse.username.clone(),
                        dir.path.clone(),
                        Some(listing(&dir.files)),
                    ));
                }
            }
        }
        let keys: Vec<(String, String)> = wanted
            .iter()
            .map(|(u, d, _)| (u.clone(), d.clone()))
            .collect();
        if keys != self.art.wanted {
            for (u, d, l) in wanted {
                if !self.art.wanted.contains(&(u.clone(), d.clone())) {
                    self.backend.fetch_cover(&u, &d, l);
                }
            }
            self.art.wanted = keys;
        }

        let generation = covers.generation();
        if generation != self.art.seen_generation {
            self.art.seen_generation = generation;
            changed = true;
        }
        let ready = self
            .art
            .handles
            .borrow()
            .values()
            .filter(|i| i.is_ready())
            .count();
        if ready != self.art.ready_handles {
            self.art.ready_handles = ready;
            changed = true;
        }
        changed
    }

    /// The playing file's own art: once its first bytes are open (FLAC/MP3 tags come first),
    /// again when it is complete. Read on a helper thread.
    fn try_embedded(&mut self, covers: &Covers) {
        let Some(p) = &self.playing else {
            self.art.embedded = Embedded::default();
            return;
        };
        if self.art.embedded.ticket != Some(p.ticket.id) {
            self.art.embedded = Embedded {
                ticket: Some(p.ticket.id),
                ..Embedded::default()
            };
        }
        let done = self.transfers.iter().find_map(|t| match &t.state {
            TState::Done(path) if t.id == p.ticket.id => Some(path.clone()),
            _ => None,
        });
        let path = match (&done, p.opened) {
            (Some(path), _) if !self.art.embedded.tried_done => {
                self.art.embedded.tried_done = true;
                self.art.embedded.tried_open = true;
                path.clone()
            }
            (None, true) if !self.art.embedded.tried_open => {
                self.art.embedded.tried_open = true;
                p.ticket.part_path.clone()
            }
            _ => return,
        };
        let (store, tx, ticket) = (covers.clone(), self.art.results.0.clone(), p.ticket.id);
        let (u, d) = (p.file.username.clone(), p.file.folder().to_string());
        let wake = self.art.waker();
        let _ = std::thread::Builder::new()
            .name("wholeseek-art".into())
            .spawn(move || {
                let found = match embedded_art(&path) {
                    ArtLookup::Found(a) => store.store_embedded(&u, &d, &a.data).is_ok(),
                    ArtLookup::None | ArtLookup::Incomplete => false,
                };
                let _ = tx.send((ticket, found));
                if let Some(w) = wake {
                    w();
                }
            });
    }
}

/// Space between a header cover and the header's text.
pub const GAP: f32 = 14.0;

/// The folder's cover above an album header (the narrow search inspector); `inset` lines the
/// cover up with the header's text.
pub fn above(s: &App, username: &str, folder: &str, inset: f32, header: El, t: &Theme) -> El {
    rune::col()
        .w_full()
        .child(rune::row().pl(inset).pt(14.0).child(cover(
            s,
            "header-cover",
            username,
            folder,
            HEADER,
            t,
        )))
        .child(header)
}

/// The folder's cover left of an album header (a user tab's folder view). Build the header
/// with `inset: GAP`; `inset` is where the cover starts.
pub fn beside(s: &App, username: &str, folder: &str, inset: f32, header: El, t: &Theme) -> El {
    rune::row()
        .w_full()
        .pl(inset)
        .child(rune::col().pt(14.0).pb(12.0).child(cover(
            s,
            "header-cover",
            username,
            folder,
            HEADER,
            t,
        )))
        .child(rune::col().grow(1.0).shrink().min_w(0.0).child(header))
}
