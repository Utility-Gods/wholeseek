//! The Library tab's state: the user's own music, indexed by `wholeseek_core::library` on its
//! own thread and played in place through the ordinary queue (each track is a `RemoteFile` of
//! [`LOCAL_USER`] at its absolute path, so nothing is fetched, copied or recorded).
//!
//! The rows the view lists come from index vectors ([`Rows`]): filtered and sorted once per
//! change of the index, the filter, the stars or the sort, then drawn by
//! `view::virtual_list` a screenful at a time.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use rune::TextEdit;
use wholeseek_core::RemoteFile;
use wholeseek_core::library::{
    self, Album, Library, LibraryIndex, Query, ScanProgress, ScanReport, Stars, TagReader, Tags,
    Track,
};

pub use wholeseek_core::LOCAL_USER;

use crate::app::App;
use crate::tabs::TabKind;
use crate::view::virtual_list::{ListScroll, Reveal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LibView {
    #[default]
    Albums,
    Songs,
}

/// How the Songs view is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SongSort {
    Title,
    /// Artist, then album, then play order.
    #[default]
    Artist,
    Album,
    Length,
    Format,
}

impl SongSort {
    pub const ALL: [SongSort; 5] = [
        SongSort::Title,
        SongSort::Artist,
        SongSort::Album,
        SongSort::Length,
        SongSort::Format,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SongSort::Title => "Title",
            SongSort::Artist => "Artist",
            SongSort::Album => "Album",
            SongSort::Length => "Length",
            SongSort::Format => "Format",
        }
    }
}

/// The listed rows: indices into the index's tracks and albums, filtered and sorted.
#[derive(Debug, Default)]
pub struct Rows {
    pub songs: Vec<usize>,
    pub albums: Vec<usize>,
    /// Fingerprint of what they were derived from: the lists' `version`.
    pub fp: u64,
}

pub struct LibraryUi {
    /// The scanner (real mode); `None` in the demo and tests, which set [`LibraryUi::index`].
    pub service: Option<Library>,
    seen_generation: u64,
    /// Folders the last scan was asked for.
    scanned: Option<Vec<PathBuf>>,
    pub index: Arc<LibraryIndex>,
    pub progress: ScanProgress,
    pub last: Option<ScanReport>,
    /// The stored index was read: before that the tab says "Loading…", not "empty".
    pub loaded: bool,
    pub stars: Stars,
    /// Bumped on every star change (the rows' fingerprint).
    stars_rev: u64,
    pub view: LibView,
    pub filter: TextEdit,
    pub starred_only: bool,
    pub sort: SongSort,
    pub desc: bool,
    /// The album whose tracks are shown (its [`Album::key`]).
    pub open_album: Option<String>,
    /// The selected song (path) or album (key): its ⋯ stays visible.
    pub selected: Option<String>,
    pub songs_list: ListScroll,
    pub albums_list: ListScroll,
    pub tracks_list: ListScroll,
    /// Settings → Library folders, typed (no folder dialog on this desktop).
    pub folder_input: TextEdit,
    rows: RefCell<Option<Rc<Rows>>>,
    waker: Option<rune::Waker>,
}

impl Default for LibraryUi {
    fn default() -> Self {
        Self {
            service: None,
            seen_generation: u64::MAX,
            scanned: None,
            index: Arc::default(),
            progress: ScanProgress::default(),
            last: None,
            loaded: false,
            stars: Stars::in_memory(),
            stars_rev: 0,
            view: LibView::default(),
            filter: TextEdit::new(""),
            starred_only: false,
            sort: SongSort::default(),
            desc: false,
            open_album: None,
            selected: None,
            songs_list: ListScroll::new("lib-songs"),
            albums_list: ListScroll::new("lib-albums"),
            tracks_list: ListScroll::new("lib-tracks"),
            folder_input: TextEdit::new(""),
            rows: RefCell::default(),
            waker: None,
        }
    }
}

impl LibraryUi {
    pub fn set_waker(&mut self, wake: rune::Waker) {
        if let Some(svc) = &self.service {
            let w = wake.clone();
            svc.set_notify(Arc::new(move || w()));
        }
        self.waker = Some(wake);
    }

    /// Demo and tests: a ready index and in-memory stars, no scanner.
    pub fn set_index(&mut self, index: LibraryIndex) {
        self.index = Arc::new(index);
        self.loaded = true;
        self.rows.replace(None);
    }

    pub fn album_by_key(&self, key: &str) -> Option<(usize, &Album)> {
        self.index
            .albums
            .iter()
            .enumerate()
            .find(|(_, a)| a.key == key)
    }

    pub fn track_by_path(&self, path: &str) -> Option<&Track> {
        self.index
            .track_by_path(path)
            .map(|i| &self.index.tracks[i])
    }

    pub fn scanning(&self) -> bool {
        self.progress.scanning
    }

    /// Whether album `a` shows under "Starred": starred itself, or one of its songs is.
    pub fn album_starred(&self, a: &Album) -> bool {
        self.stars.album(&a.key)
            || a.tracks
                .iter()
                .any(|&i| self.stars.track(&self.index.tracks[i].path_str))
    }

    /// The listed songs and albums, derived again only when an input changed.
    pub fn rows(&self) -> Rc<Rows> {
        let mut h = DefaultHasher::new();
        (Arc::as_ptr(&self.index) as usize).hash(&mut h);
        self.index.tracks.len().hash(&mut h);
        self.filter.text.trim().to_lowercase().hash(&mut h);
        (self.starred_only, self.sort, self.desc, self.stars_rev).hash(&mut h);
        let fp = h.finish();
        if let Some(r) = &*self.rows.borrow()
            && r.fp == fp
        {
            return r.clone();
        }
        let r = Rc::new(self.derive(fp));
        self.rows.replace(Some(r.clone()));
        r
    }

    fn derive(&self, fp: u64) -> Rows {
        let ix = &*self.index;
        let q = Query::parse(&self.filter.text);
        let mut songs: Vec<usize> = (0..ix.tracks.len())
            .filter(|&i| ix.track_matches(i, &q))
            .filter(|&i| !self.starred_only || self.stars.track(&ix.tracks[i].path_str))
            .collect();
        sort_songs(ix, &mut songs, self.sort);
        if self.desc {
            songs.reverse();
        }
        let albums: Vec<usize> = (0..ix.albums.len())
            .filter(|&a| ix.album_matches(a, &q))
            .filter(|&a| !self.starred_only || self.album_starred(&ix.albums[a]))
            .collect();
        Rows { songs, albums, fp }
    }
}

fn sort_songs(ix: &LibraryIndex, v: &mut [usize], by: SongSort) {
    use crate::tabs::natural_cmp;
    let t = &ix.tracks;
    // Index order is album folder, then play order: the tie-break everywhere.
    match by {
        SongSort::Title => v.sort_by(|&a, &b| natural_cmp(&t[a].title, &t[b].title)),
        SongSort::Artist => v.sort_by(|&a, &b| {
            let (x, y) = (&ix.albums[t[a].album_index], &ix.albums[t[b].album_index]);
            natural_cmp(&t[a].artist, &t[b].artist)
                .then_with(|| natural_cmp(&x.title, &y.title))
                .then(t[a].album_index.cmp(&t[b].album_index))
                .then(a.cmp(&b))
        }),
        SongSort::Album => v.sort_by(|&a, &b| {
            let (x, y) = (&ix.albums[t[a].album_index], &ix.albums[t[b].album_index]);
            natural_cmp(&x.title, &y.title)
                .then(t[a].album_index.cmp(&t[b].album_index))
                .then(a.cmp(&b))
        }),
        SongSort::Length => v.sort_by_key(|&i| (t[i].duration_secs.unwrap_or(0), i)),
        SongSort::Format => v.sort_by(|&a, &b| {
            (
                &t[a].format,
                t[a].bit_depth,
                t[a].sample_rate_hz,
                t[a].bitrate_kbps,
            )
                .cmp(&(
                    &t[b].format,
                    t[b].bit_depth,
                    t[b].sample_rate_hz,
                    t[b].bitrate_kbps,
                ))
                .then(a.cmp(&b))
        }),
    }
}

/// `wholeseek_audio::read_tags` as the scanner's reader.
pub fn tag_reader() -> TagReader {
    Arc::new(|p: &Path| {
        wholeseek_audio::read_tags(p).map(|t| Tags {
            title: t.title,
            artist: t.artist,
            album_artist: t.album_artist,
            album: t.album,
            track: t.track,
            disc: t.disc,
            year: t.year,
            duration_secs: t.duration_secs,
            codec: t.codec,
            bit_depth: t.bit_depth,
            sample_rate_hz: t.sample_rate_hz,
        })
    })
}

fn art_reader() -> library::ArtReader {
    Arc::new(|p: &Path| match wholeseek_audio::embedded_art(p) {
        wholeseek_audio::ArtLookup::Found(a) => Some(a.data),
        _ => None,
    })
}

/// "FLAC 24/96", "MP3 320", "FLAC · mixed".
pub fn quality_label(q: &library::Quality) -> String {
    let mut s = q.format.clone();
    if q.lossless {
        if let (Some(b), Some(r)) = (q.bit_depth, q.sample_rate_hz) {
            s.push_str(&format!(" {b}/{}", khz(r)));
        }
    } else if let Some(k) = q.bitrate_kbps {
        s.push_str(&format!(" {k}"));
    }
    if q.mixed {
        s.push_str(" · mixed");
    }
    s
}

/// A track's format: "FLAC 16/44.1", "MP3 320".
pub fn track_quality(t: &Track) -> String {
    match (t.lossless(), t.bit_depth, t.sample_rate_hz, t.bitrate_kbps) {
        (true, Some(b), Some(r), _) => format!("{} {b}/{}", t.format, khz(r)),
        (false, _, _, Some(k)) => format!("{} {k}", t.format),
        _ => t.format.clone(),
    }
}

fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    if (k - k.round()).abs() < 0.05 {
        format!("{}", k.round() as u32)
    } else {
        format!("{k:.1}")
    }
}

impl App {
    /// The Library button: show the Library tab, opening it if there is none.
    pub fn open_library(&mut self) {
        match self
            .tabs
            .iter()
            .position(|t| matches!(t.kind, TabKind::Library))
        {
            Some(i) => self.active = i,
            None => self.push_tab(TabKind::Library),
        }
    }

    pub fn library_tab_open(&self) -> bool {
        self.tabs.iter().any(|t| matches!(t.kind, TabKind::Library))
    }

    /// Once per frame: start the scanner (real mode, after boot), rescan when the folders
    /// changed, take what it published. True if something visible changed.
    pub fn library_tick(&mut self, wake: &mut Option<f64>) -> bool {
        let _ = wake; // the scanner wakes the window itself
        if self.library.service.is_none() && !self.demo && self.boot.is_none() {
            let settings = self.backend.settings().clone();
            let svc = Library::start(library::Config {
                data_dir: settings.data_dir.clone(),
                tags: tag_reader(),
                art: Some(art_reader()),
                covers: self.backend.covers().cloned(),
            });
            self.library.stars = Stars::load(&settings.data_dir);
            self.library.service = Some(svc);
            if let Some(w) = self.library.waker.clone() {
                self.library.set_waker(w);
            }
        }
        let Some(svc) = &self.library.service else {
            return false;
        };
        let dirs = &self.backend.settings().library_dirs;
        if self.library.scanned.as_ref() != Some(dirs) {
            self.library.scanned = Some(dirs.clone());
            svc.rescan(dirs.clone());
        }
        let g = svc.generation();
        if g == self.library.seen_generation {
            return false;
        }
        self.library.seen_generation = g;
        let snap = svc.snapshot();
        let ui = &mut self.library;
        ui.index = snap.index;
        ui.progress = snap.progress;
        ui.last = snap.last;
        ui.loaded = snap.loaded;
        if let Some(e) = snap.error {
            self.error = Some(e);
        }
        true
    }

    /// Rescan: the library folders again (only changed files are read).
    pub fn rescan_library(&mut self) {
        if let Some(svc) = &self.library.service {
            let dirs = self.backend.settings().library_dirs.clone();
            self.library.scanned = Some(dirs.clone());
            svc.rescan(dirs);
        }
    }

    pub fn add_library_dir(&mut self, dir: PathBuf) {
        if self.backend.settings().library_dirs.contains(&dir) {
            return;
        }
        self.commit_settings(|s| s.library_dirs.push(dir));
    }

    pub fn remove_library_dir(&mut self, dir: &Path) {
        self.commit_settings(|s| s.library_dirs.retain(|d| d != dir));
    }

    /// A typed folder: added if it exists.
    pub fn add_typed_library_dir(&mut self) {
        let text = self.library.folder_input.text.trim().to_string();
        let dir = match text.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(rest),
            None => PathBuf::from(&text),
        };
        if text.is_empty() || !dir.is_dir() {
            self.settings_form.path_error = Some("No such folder".into());
            return;
        }
        self.settings_form.path_error = None;
        self.library.folder_input.replace_text("");
        self.add_library_dir(dir);
    }

    // ---- playing ---------------------------------------------------------------------------

    fn library_files(&self, tracks: impl IntoIterator<Item = usize>) -> Vec<RemoteFile> {
        let ix = &self.library.index;
        tracks
            .into_iter()
            .filter_map(|i| ix.tracks.get(i))
            .map(Track::remote_file)
            .collect()
    }

    /// ▶ a song, the list it's shown in as the queue: the open album's tracks, else the
    /// Songs view's rows, else its album.
    pub fn play_library_song(&mut self, path: &str) {
        let Some(i) = self.library.index.track_by_path(path) else {
            return;
        };
        let ui = &self.library;
        let open = ui
            .open_album
            .as_deref()
            .and_then(|k| ui.album_by_key(k))
            .filter(|(_, a)| a.tracks.contains(&i));
        let list: Vec<usize> = match open {
            Some((_, a)) => a.tracks.clone(),
            None if ui.view == LibView::Songs => ui.rows().songs.clone(),
            None => ui.index.albums[ui.index.tracks[i].album_index]
                .tracks
                .clone(),
        };
        let file = ui.index.tracks[i].remote_file();
        let files = self.library_files(list);
        self.play_in(&file, &files);
    }

    /// ▶ an album from its first track.
    pub fn play_library_album(&mut self, key: &str) {
        let Some(tracks) = self
            .library
            .album_by_key(key)
            .map(|(_, a)| a.tracks.clone())
        else {
            return;
        };
        let files = self.library_files(tracks);
        if let Some(first) = files.first().cloned() {
            self.play_in(&first, &files);
        }
    }

    /// "Play all": the listed songs (Songs) or the listed albums' tracks (Albums), in order.
    pub fn play_library_list(&mut self) {
        let rows = self.library.rows();
        let ix = self.library.index.clone();
        let list: Vec<usize> = match self.library.view {
            LibView::Songs => rows.songs.clone(),
            LibView::Albums => rows
                .albums
                .iter()
                .flat_map(|&a| ix.albums[a].tracks.iter().copied())
                .collect(),
        };
        let files = self.library_files(list);
        if let Some(first) = files.first().cloned() {
            self.play_in(&first, &files);
        }
    }

    // ---- stars -----------------------------------------------------------------------------

    pub fn toggle_star_song(&mut self, path: &str) {
        if let Err(e) = self.library.stars.toggle_track(path) {
            self.error = Some(format!("Stars not saved: {e}"));
        }
        self.library.stars_rev += 1;
    }

    pub fn toggle_star_album(&mut self, key: &str) {
        if let Err(e) = self.library.stars.toggle_album(key) {
            self.error = Some(format!("Stars not saved: {e}"));
        }
        self.library.stars_rev += 1;
    }

    pub fn library_stars_rev(&self) -> u64 {
        self.library.stars_rev
    }

    // ---- navigation ------------------------------------------------------------------------

    pub fn set_library_view(&mut self, v: LibView) {
        self.library.view = v;
        self.library.open_album = None;
        self.library.selected = None;
    }

    pub fn open_library_album(&mut self, key: &str) {
        self.library.open_album = Some(key.to_string());
        self.library.selected = None;
    }

    pub fn close_library_album(&mut self) {
        if let Some(k) = self.library.open_album.take() {
            self.library.selected = Some(k);
        }
    }

    /// A Songs column header: sort by it, again to reverse.
    pub fn sort_library(&mut self, by: SongSort) {
        if self.library.sort == by {
            self.library.desc = !self.library.desc;
        } else {
            self.library.sort = by;
            self.library.desc = false;
        }
    }

    pub fn toggle_library_starred(&mut self) {
        self.library.starred_only = !self.library.starred_only;
    }

    /// A Library file's (title, artist, album) for the player and media keys; the file name
    /// when it's no longer indexed.
    pub fn library_names(&self, f: &RemoteFile) -> (String, String, String) {
        match self.library.track_by_path(&f.path) {
            Some(t) => (t.title.clone(), t.artist.clone(), t.album.clone()),
            None => (crate::fmt::title(f), "Library".into(), String::new()),
        }
    }

    /// "Show in list" for a Library file: the Library tab, its row selected and scrolled to
    /// (in the open album, else the Songs list, else its album opened).
    pub fn reveal_library_playing(&mut self, tx: &mut rune::Tx) {
        let Some(path) = self
            .playing
            .as_ref()
            .filter(|p| p.file.is_local())
            .map(|p| p.file.path.clone())
        else {
            return;
        };
        self.open_library();
        let Some(i) = self.library.index.track_by_path(&path) else {
            return;
        };
        self.library.selected = Some(path);
        let ui = &mut self.library;
        let in_album = |ui: &LibraryUi| {
            let k = ui.open_album.as_deref()?;
            ui.album_by_key(k)?.1.tracks.iter().position(|&t| t == i)
        };
        if let Some(row) = in_album(ui) {
            ui.tracks_list.scroll_to_index(tx, row, Reveal::Center);
            return;
        }
        if ui.view == LibView::Songs
            && let Some(row) = ui.rows().songs.iter().position(|&t| t == i)
        {
            ui.songs_list.scroll_to_index(tx, row, Reveal::Center);
            return;
        }
        let key = ui.index.albums[ui.index.tracks[i].album_index].key.clone();
        ui.open_album = Some(key);
        if let Some(row) = in_album(ui) {
            ui.tracks_list.scroll_to_index(tx, row, Reveal::Center);
        }
    }

    // ---- menus -----------------------------------------------------------------------------

    pub fn library_song_menu(&self, path: &str) -> Vec<crate::menu::MenuItem> {
        let starred = self.library.stars.track(path);
        crate::menu::library_song_items(path, starred)
    }

    pub fn library_album_menu(&self, key: &str) -> Vec<crate::menu::MenuItem> {
        let Some((_, a)) = self.library.album_by_key(key) else {
            return Vec::new();
        };
        crate::menu::library_album_items(key, &a.dir, self.library.stars.album(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wholeseek_core::library::Entry;

    fn ix() -> LibraryIndex {
        let mk = |p: &str, title: &str, artist: &str, album: &str, n: u32, secs: u32| Entry {
            path: PathBuf::from(p),
            size: 1_000_000,
            mtime: 1,
            tags: Some(Tags {
                title: Some(title.into()),
                artist: Some(artist.into()),
                album: Some(album.into()),
                track: Some(n),
                duration_secs: Some(secs),
                ..Tags::default()
            }),
        };
        LibraryIndex::build(
            &[
                mk("/m/B/Two/01.flac", "Zed", "Bee", "Two", 1, 300),
                mk("/m/B/Two/02.flac", "Alpha", "Bee", "Two", 2, 100),
                mk("/m/A/One/01.mp3", "Mid", "Ant", "One", 1, 200),
            ],
            &[PathBuf::from("/m")],
        )
    }

    fn titles(ui: &LibraryUi) -> Vec<String> {
        ui.rows()
            .songs
            .iter()
            .map(|&i| ui.index.tracks[i].title.clone())
            .collect()
    }

    #[test]
    fn rows_sort_filter_and_star() {
        let mut ui = LibraryUi::default();
        ui.set_index(ix());
        assert_eq!(
            titles(&ui),
            ["Mid", "Zed", "Alpha"],
            "artist, album, play order"
        );
        ui.sort = SongSort::Title;
        assert_eq!(titles(&ui), ["Alpha", "Mid", "Zed"]);
        ui.desc = true;
        assert_eq!(titles(&ui), ["Zed", "Mid", "Alpha"]);
        ui.sort = SongSort::Length;
        ui.desc = false;
        assert_eq!(titles(&ui), ["Alpha", "Mid", "Zed"]);
        ui.filter.replace_text("bee");
        assert_eq!(titles(&ui).len(), 2);
        assert_eq!(ui.rows().albums.len(), 1);
        ui.filter.replace_text("");
        let fp = ui.rows().fp;
        ui.stars.toggle_track("/m/A/One/01.mp3").unwrap();
        ui.stars_rev += 1;
        ui.starred_only = true;
        assert_ne!(ui.rows().fp, fp);
        assert_eq!(titles(&ui), ["Mid"]);
        let albums = ui.rows().albums.clone();
        assert_eq!(albums.len(), 1);
        assert_eq!(ui.index.albums[albums[0]].title, "One");
    }

    /// Library songs play from their own path through the queue: no `play_fetch`, browse,
    /// download or transfer row; Next goes to the next song of the list it was played from.
    #[test]
    fn library_plays_in_place_without_the_network() {
        use crate::demo::Call;
        let mut app = App::demo();
        app.demo_library();
        assert!(matches!(app.tab().kind, TabKind::Library));
        let before = app.backend.calls().len();
        let ix = app.library.index.clone();
        let album = &ix.albums[7];
        let second = ix.tracks[album.tracks[1]].path_str.clone();
        app.play_library_song(&second);
        let p = app.playing.as_ref().expect("playing");
        assert!(p.file.is_local() && p.opened);
        assert_eq!(p.file.path, second);
        let q = app.queue.as_ref().unwrap();
        assert_eq!((q.index, q.items.len()), (1, album.tracks.len()));
        assert!(
            q.items
                .iter()
                .all(|i| i.path.as_deref() == Some(Path::new(&i.file.path)))
        );
        app.next_track();
        app.tick(1.0);
        assert_eq!(app.queue.as_ref().unwrap().index, 2);
        app.play_library_album(&ix.albums[3].key);
        app.set_library_view(LibView::Songs);
        app.library.filter.replace_text("hollow");
        app.play_library_list();
        assert_eq!(
            app.queue.as_ref().unwrap().items.len(),
            app.library.rows().songs.len()
        );
        let net = app.backend.calls()[before..]
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Call::PlayFetch(_)
                        | Call::Browse(_)
                        | Call::Download(_)
                        | Call::DownloadTo(..)
                        | Call::DownloadFolder(..)
                        | Call::UserProfile(_)
                )
            })
            .count();
        assert_eq!(net, 0, "no network request for a Library file");
        assert!(
            app.transfers.iter().all(|t| !t.file.is_local()),
            "no transfer rows"
        );
    }

    #[test]
    fn stars_menu_and_session() {
        let mut app = App::demo();
        app.demo_library();
        let ix = app.library.index.clone();
        let path = ix.tracks[5].path_str.clone();
        let was = app.library.stars.track(&path);
        let items = app.library_song_menu(&path);
        let ids: Vec<&str> = items.iter().map(|i| i.id).collect();
        assert_eq!(ids, ["play", "star", "show-in-folder", "copy-path"]);
        app.run_menu(items[1].action.clone());
        assert_ne!(app.library.stars.track(&path), was);
        assert_eq!(
            app.run_menu(crate::menu::MenuAction::CopyPath(path.clone())),
            Some(path.clone())
        );
        let a = ix.albums[0].key.clone();
        let ids: Vec<&str> = app.library_album_menu(&a).iter().map(|i| i.id).collect();
        assert_eq!(ids, ["play-album", "star", "show-in-folder", "copy-path"]);
        // The tab comes back after a restart, on its view.
        app.set_library_view(LibView::Songs);
        let snap = app.session_snapshot();
        assert!(snap.tabs.iter().any(|t| matches!(
            t,
            crate::session::SessionTab::Library { view: Some(v) } if v == "songs"
        )));
        assert!(
            crate::session::restore_plan(&snap).len() < snap.tabs.len(),
            "the Library fetches nothing on restore"
        );
        // One Library tab, however often the button is pressed.
        app.open_library();
        app.open_library();
        let n = app
            .tabs
            .iter()
            .filter(|t| matches!(t.kind, TabKind::Library))
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn labels() {
        assert_eq!(khz(44_100), "44.1");
        assert_eq!(khz(96_000), "96");
        let q = library::Quality {
            format: "FLAC".into(),
            lossless: true,
            bit_depth: Some(24),
            sample_rate_hz: Some(96_000),
            ..Default::default()
        };
        assert_eq!(quality_label(&q), "FLAC 24/96");
    }
}
