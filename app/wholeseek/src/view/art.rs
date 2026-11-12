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
use wholeseek_core::{CoverState, Covers, RemoteFile, TransferId};

use super::icons;
use crate::app::{App, TState};
use crate::tabs::TabKind;

/// Player bar cover, points.
pub const PLAYER: f32 = 44.0;
/// Search inspector / user folder header cover.
pub const HEADER: f32 = 104.0;
/// Album row thumbnail (fixed slot; rows are 48 pt). Only when cached.
pub const ROW: f32 = 32.0;
/// Thumbnail in dense 22–24 pt rows (search folder lines, user folder tree).
pub const SMALL: f32 = 16.0;
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
    /// Tests: a cover cache for a backend that has none (the demo).
    #[cfg(test)]
    pub test_covers: Option<Covers>,
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
            #[cfg(test)]
            test_covers: None,
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

/// The backend's cover cache (or a test's).
fn covers_of(s: &App) -> Option<&Covers> {
    #[cfg(test)]
    if let Some(c) = &s.art.test_covers {
        return Some(c);
    }
    s.backend.covers()
}

/// The cover of (user, folder) if it is cached and decoded. Memory only.
pub fn cached_image(s: &App, username: &str, folder: &str) -> Option<Image> {
    let path = covers_of(s)?.cached(username, folder)?;
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

/// A row's thumbnail of `size`, only when the cover is already cached and decoded; `None`
/// draws nothing. Never asks for a fetch.
pub fn row_thumb(s: &App, key: &str, username: &str, folder: &str, size: f32) -> Option<El> {
    let img = cached_image(s, username, folder)?;
    Some(
        el().key(key.to_string())
            .size(size, size)
            .radius(if size < 20.0 { 2.0 } else { 4.0 })
            .pixels_fit(img.pixels(), SurfaceFit::Fill),
    )
}

/// A fixed `size` slot (album rows): the cached thumbnail, or empty — the row never moves.
pub fn row_slot(s: &App, key: &str, username: &str, folder: &str, size: f32) -> El {
    row_thumb(s, key, username, folder, size).unwrap_or_else(|| el().size(size, size))
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
        let Some(covers) = covers_of(self).cloned() else {
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
/// cover up with the header's text. Only while a cover exists or is being fetched (a plain
/// square then); no cover known → just the header, no space spent.
pub fn above(s: &App, username: &str, folder: &str, inset: f32, header: El, t: &Theme) -> El {
    let square = match covers_of(s).map(|c| c.state(username, folder)) {
        Some(CoverState::Ready(_)) => match cached_image(s, username, folder) {
            Some(_) => cover(s, "header-cover", username, folder, HEADER, t),
            // Decoding: a plain square for a moment.
            None => loading(HEADER, t),
        },
        Some(CoverState::Pending) => loading(HEADER, t),
        _ => return header,
    };
    rune::col()
        .w_full()
        .child(rune::row().pl(inset).pt(14.0).child(square))
        .child(header)
}

fn loading(size: f32, t: &Theme) -> El {
    el().key("header-cover")
        .size(size, size)
        .radius(4.0)
        .fill(t.hover)
        .label("Album cover")
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

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};
    use wholeseek_core::{CoverState, Covers};

    use crate::app::App;
    use crate::{poll, view};

    fn png() -> Vec<u8> {
        wholeseek_core::cover::test_png(2, 2)
    }

    fn run(rt: &mut Runtime<App>, secs: f64) {
        for _ in 0..(secs * 60.0).round() as usize {
            rt.frame(1.0 / 60.0);
        }
    }

    fn title_y(rt: &Runtime<App>) -> f32 {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("details-title"))
            .expect("inspector title")
            .rect[1]
    }

    fn has_square(rt: &Runtime<App>) -> bool {
        rt.elements()
            .into_iter()
            .any(|e| e.key.as_deref() == Some("header-cover"))
    }

    /// No cover known → no space; fetching → a square; cached → the image.
    #[test]
    fn inspector_cover_takes_space_only_when_there_is_one() {
        let dir = std::env::temp_dir().join(format!("wholeseek-art-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let covers = Covers::open(&dir);
        let mut app = App::demo();
        app.art.test_covers = Some(covers.clone());
        let mut rt = Runtime::new(
            app,
            view::view,
            (1920.0, 1080.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.state.demo_login();
        run(&mut rt, 1.0);
        rt.update(|s, _| s.search_for("hollow lights"));
        run(&mut rt, 1.6);
        let f = crate::demo_file("alice", "02 Salt Roads.flac");
        let (u, d) = (f.username.clone(), f.folder().to_string());
        let pick = (u.clone(), d.clone());
        rt.update(move |s, _| s.tab_mut().group = Some(pick.clone()));
        run(&mut rt, 0.1);

        assert_eq!(covers.state(&u, &d), CoverState::Unknown);
        assert!(!has_square(&rt), "no cover known: nothing drawn");
        let bare = title_y(&rt);

        assert!(covers.mark_pending(&u, &d));
        rt.frame(1.0 / 60.0);
        assert!(has_square(&rt), "fetching: a plain square");
        assert!(
            title_y(&rt) >= bare + super::HEADER,
            "the square sits above the title"
        );
        assert!(super::cached_image(&rt.state, &u, &d).is_none());

        let path = covers.store_embedded(&u, &d, &png()).unwrap();
        let img = rt.state.art.image(&path);
        assert!(img.is_none(), "first ask starts the decode");
        let handle = rt.state.art.handles.borrow().get(&path).cloned().unwrap();
        handle.wait().expect("decodes");
        rt.frame(1.0 / 60.0);
        assert!(
            super::cached_image(&rt.state, &u, &d).is_some(),
            "cached: the image"
        );
        assert!(has_square(&rt));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
