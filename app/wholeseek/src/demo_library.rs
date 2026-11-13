//! The Library's demo: ~10k made-up songs in 840 albums, the same every run, and the scenes
//! that screenshot it (`--scene library`, `library-songs`, …). Nothing is read from disk.

use std::path::PathBuf;

use wholeseek_core::library::{Entry, LibraryIndex, Tags};

use crate::app::App;
use crate::library::LibView;

/// Where the demo's music "is" (never touched).
pub const ROOT: &str = "/music";
const ALBUMS: usize = 840;

const ADJ: [&str; 24] = [
    "Hollow", "Northern", "Quiet", "Velvet", "Paper", "Silver", "Low", "Amber", "Glass", "Distant",
    "Electric", "Static", "Golden", "Pale", "Winter", "Neon", "Slow", "Copper", "Lunar", "Wild",
    "Cold", "Bright", "Salt", "Hidden",
];
const NOUN: [&str; 24] = [
    "Lights",
    "Glass",
    "Harbour",
    "Signal",
    "Rooms",
    "Rivers",
    "Tide",
    "Engines",
    "Parade",
    "Orchard",
    "Machines",
    "Lanterns",
    "Coast",
    "Satellites",
    "Gardens",
    "Ghosts",
    "Fields",
    "Wires",
    "Birds",
    "Hours",
    "Mirrors",
    "Choir",
    "Forest",
    "Season",
];
const WORDS: [&str; 40] = [
    "Salt", "Roads", "Low", "Tide", "Radio", "Harbour", "Night", "Drive", "Paper", "Moon", "Last",
    "Train", "Golden", "Hour", "Open", "Water", "Slow", "Burn", "Northern", "Line", "Echo", "Park",
    "Glass", "House", "Blue", "Hours", "Static", "Bloom", "Long", "Way", "Home", "Signal", "Fire",
    "Escape", "Winter", "Coast", "Small", "Wonders", "Weather", "Song",
];

/// A tiny deterministic generator (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() % v.len() as u64) as usize]
    }
}

fn artist(i: usize) -> String {
    // 210 artists, ~4 albums each.
    let a = i % 210;
    format!(
        "{} {}",
        ADJ[a % ADJ.len()],
        NOUN[(a * 7 + a / ADJ.len()) % NOUN.len()]
    )
}

/// The demo's albums: (artist, album, year, folder).
fn albums() -> Vec<(String, String, u32, String)> {
    let mut r = Rng(0x5eed_1234_abcd_ef01);
    (0..ALBUMS)
        .map(|i| {
            let artist = artist(i);
            let title = match r.next() % 3 {
                0 => format!("{} {}", r.pick(&WORDS), r.pick(&WORDS)),
                1 => r.pick(&WORDS).to_string(),
                _ => format!("The {} {}", r.pick(&WORDS), r.pick(&WORDS)),
            };
            let year = 1968 + (r.next() % 57) as u32;
            let folder = format!("{ROOT}/{artist}/{title} ({year})");
            (artist, title, year, folder)
        })
        .collect()
}

/// Album folders, for the demo's generated covers.
pub fn album_folders() -> Vec<String> {
    albums().into_iter().map(|a| a.3).collect()
}

/// The index entries: ~10k tagged songs (a few left untagged to show the name fallback).
pub fn entries() -> Vec<Entry> {
    let mut r = Rng(0x0dd_ba11_cafe_f00d);
    let mut out = Vec::with_capacity(ALBUMS * 12);
    for (i, (artist, album, year, folder)) in albums().into_iter().enumerate() {
        let n = 8 + (r.next() % 9) as u32;
        let (ext, codec, depth, rate, kbps) = match i % 10 {
            0..=4 => ("flac", "FLAC", Some(16), 44_100, 900),
            5 | 6 => ("flac", "FLAC", Some(24), 96_000, 2_600),
            7 => ("mp3", "MP3", None, 44_100, 320),
            8 => ("m4a", "AAC", None, 44_100, 256),
            _ => ("ogg", "Vorbis", None, 44_100, 192),
        };
        for t in 1..=n {
            let title = format!("{} {}", r.pick(&WORDS), r.pick(&WORDS));
            let secs = 140 + (r.next() % 300) as u32;
            let path = PathBuf::from(format!("{folder}/{t:02} {title}.{ext}"));
            let tagged = i % 97 != 5;
            out.push(Entry {
                size: u64::from(secs) * kbps * 125,
                mtime: 1,
                tags: Some(Tags {
                    title: tagged.then(|| title.clone()),
                    artist: tagged.then(|| artist.clone()),
                    album: tagged.then(|| album.clone()),
                    track: tagged.then_some(t),
                    year: tagged.then_some(year),
                    duration_secs: Some(secs),
                    codec: Some(codec.into()),
                    bit_depth: depth,
                    sample_rate_hz: Some(rate),
                    ..Tags::default()
                }),
                path,
            });
        }
    }
    out
}

pub fn index() -> LibraryIndex {
    LibraryIndex::build(&entries(), &[PathBuf::from(ROOT)])
}

impl App {
    /// The demo signed in, the Library tab open on the demo index, a few stars, a song
    /// playing from album 2.
    pub fn demo_library(&mut self) {
        self.demo_login();
        self.commit_settings(|s| s.library_dirs = vec![PathBuf::from(ROOT)]);
        self.settings_form.saved_at = None;
        self.library.set_index(index());
        self.open_library();
        let ix = self.library.index.clone();
        for a in [1, 4, 9] {
            let key = ix.albums[a].key.clone();
            self.toggle_star_album(&key);
        }
        for t in [0, 3, 14, 15, 40, 41, 77, 120, 300, 640] {
            let p = ix.tracks[ix.albums[t % ix.albums.len()].tracks[0]]
                .path_str
                .clone();
            self.toggle_star_song(&p);
        }
        let playing = ix.albums[2].tracks[1];
        let p = ix.tracks[playing].path_str.clone();
        self.play_library_song(&p);
        self.library.selected = None;
    }
}

pub fn scene_albums(s: &mut App) {
    s.demo_library();
}

pub fn scene_songs(s: &mut App) {
    s.demo_library();
    s.set_library_view(LibView::Songs);
}

pub fn scene_search(s: &mut App) {
    s.demo_library();
    s.set_library_view(LibView::Songs);
    s.library.filter.replace_text("hollow tide");
}

pub fn scene_starred(s: &mut App) {
    s.demo_library();
    s.set_library_view(LibView::Songs);
    s.library.starred_only = true;
}

pub fn scene_album(s: &mut App) {
    s.demo_library();
    let key = s.library.index.albums[2].key.clone();
    s.open_library_album(&key);
}

/// A song's ⋯ menu, as clicked.
pub fn scene_menu(s: &mut App) {
    s.demo_library();
    s.set_library_view(LibView::Songs);
    let rows = s.library.rows();
    let i = rows.songs[3];
    let path = s.library.index.tracks[i].path_str.clone();
    s.library.selected = Some(path.clone());
    let items = s.library_song_menu(&path);
    s.open_menu(crate::menu::MenuAnchor::Key(format!("lib-more-{i}")), items);
}

/// No folders: the empty state pointing at Settings.
pub fn scene_empty(s: &mut App) {
    s.demo_login();
    s.commit_settings(|c| c.library_dirs.clear());
    s.settings_form.saved_at = None;
    s.open_library();
}

#[cfg(test)]
mod tests {
    #[test]
    fn about_ten_thousand_songs_same_every_time() {
        let a = super::index();
        let b = super::index();
        assert!(
            (9_000..=11_000).contains(&a.tracks.len()),
            "{}",
            a.tracks.len()
        );
        assert_eq!(a.albums.len(), b.albums.len());
        assert_eq!(a.tracks[123].path, b.tracks[123].path);
        assert!(a.tracks.iter().all(|t| t.path.starts_with(super::ROOT)));
    }
}
