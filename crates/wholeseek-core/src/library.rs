//! Library: the user's own music files, played where they are.
//!
//! - [`scan`] walks the library folders for audio files ([`crate::rank::AUDIO_EXTENSIONS`],
//!   hidden files and folders skipped, symlinked folders not followed) and reads the tags of
//!   files that are new or whose size or mtime changed. Everything else comes from the
//!   previous index. Deleted files drop out.
//! - Tags come from an injected [`TagReader`] (the app passes `wholeseek_audio::read_tags`,
//!   which uses symphonia), so this crate stays free of decoders. A file without tags falls
//!   back to its folder and file names ([`LibraryIndex::build`]).
//! - [`LibraryIndex`] resolves the entries into tracks and albums (album artist, or artist,
//!   plus album, within one album folder; `CD1`/`Disc 2` sub-folders belong to their parent),
//!   with a lowercase haystack per track so filtering 10k songs is a substring test per row.
//! - [`Library`] runs the scans on a background thread, publishes the index, then stores
//!   folder or embedded covers in [`Covers`] under ([`LOCAL_USER`], album folder).
//! - Persistence is plain files in the data dir: [`INDEX_FILE`] (JSON lines) and
//!   [`STARS_FILE`] ([`Stars`]). Nothing is ever written into a library folder.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::cover::Covers;
use crate::model::{AudioAttrs, RemoteFile};

pub use crate::model::LOCAL_USER;

/// The index, one JSON line per file after a header line, in `Settings::data_dir`.
pub const INDEX_FILE: &str = "library.jsonl";
/// Starred songs and albums, in `Settings::data_dir`.
pub const STARS_FILE: &str = "library-stars.json";
const INDEX_VERSION: u32 = 1;

/// What a tag reader found in one file. Every field is optional; empty strings count as
/// missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: Option<u32>,
    /// Codec name as shown (`FLAC`, `MP3`, `AAC`, `ALAC`, `PCM`, `Vorbis`).
    pub codec: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
}

impl Tags {
    /// Whether any naming tag is present (title, artist or album).
    #[must_use]
    pub fn named(&self) -> bool {
        [&self.title, &self.artist, &self.album]
            .iter()
            .any(|t| clean(t.as_deref()).is_some())
    }
}

/// Reads one file's tags; `None` when the file can't be parsed at all. Blocking I/O: only
/// ever called on the scanner's threads.
pub type TagReader = Arc<dyn Fn(&Path) -> Option<Tags> + Send + Sync>;
/// Reads one file's embedded cover (still encoded); `None` when it has none.
pub type ArtReader = Arc<dyn Fn(&Path) -> Option<Vec<u8>> + Send + Sync>;

/// One file as stored in [`INDEX_FILE`]: where, its size and mtime when it was read, and
/// what the reader found then (`None`: unreadable, names only).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime: u64,
    pub tags: Option<Tags>,
}

/// A song, tags resolved with the name fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    /// `path` as UTF-8 (only UTF-8 paths are indexed): the queue's `RemoteFile::path`.
    pub path_str: String,
    pub size: u64,
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    /// Empty when unknown (shown as "Unknown album").
    pub album: String,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: Option<u32>,
    /// Upper-case extension (`FLAC`).
    pub format: String,
    pub codec: Option<String>,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    /// Average, from size and duration.
    pub bitrate_kbps: Option<u32>,
    /// The folder the album groups in (a disc sub-folder's parent).
    pub album_dir: PathBuf,
    /// Its album in [`LibraryIndex::albums`].
    pub album_index: usize,
    /// Whether the names came from tags (else from the folder and file names).
    pub tagged: bool,
}

impl Track {
    /// File name with extension.
    #[must_use]
    pub fn file_name(&self) -> &str {
        crate::paths::remote_basename(&self.path_str)
    }

    /// The track as a queue item: [`LOCAL_USER`] and its absolute path. The app plays it
    /// from that path; the core refuses to send it anywhere.
    #[must_use]
    pub fn remote_file(&self) -> RemoteFile {
        RemoteFile {
            username: LOCAL_USER.to_string(),
            path: self.path_str.clone(),
            size: self.size,
            attrs: AudioAttrs {
                bitrate_kbps: self.bitrate_kbps,
                duration_secs: self.duration_secs,
                vbr: None,
                sample_rate_hz: self.sample_rate_hz,
                bit_depth: self.bit_depth,
            },
        }
    }

    /// Lossless by format.
    #[must_use]
    pub fn lossless(&self) -> bool {
        is_lossless(&self.format, self.codec.as_deref())
    }
}

fn is_lossless(format: &str, codec: Option<&str>) -> bool {
    matches!(
        format,
        "FLAC" | "WAV" | "AIF" | "AIFF" | "ALAC" | "APE" | "WV"
    ) || matches!(codec, Some("ALAC" | "FLAC" | "PCM"))
}

/// The usual format of an album's tracks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quality {
    pub format: String,
    pub lossless: bool,
    pub bit_depth: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    /// Lossy: the average bitrate over its tracks.
    pub bitrate_kbps: Option<u32>,
    /// Tracks in more than one format.
    pub mixed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    /// Empty when unknown.
    pub title: String,
    /// Album artist, else the tracks' one artist, else "Various artists".
    pub artist: String,
    pub year: Option<u32>,
    pub dir: PathBuf,
    /// `dir` as a string: the cover key's folder (with [`LOCAL_USER`]).
    pub folder: String,
    /// Track indices in play order (disc, track number, file name).
    pub tracks: Vec<usize>,
    pub duration_secs: u64,
    pub quality: Quality,
    /// Stable star key.
    pub key: String,
}

/// Every indexed track and album, ready to list and filter.
#[derive(Debug, Clone, Default)]
pub struct LibraryIndex {
    pub tracks: Vec<Track>,
    /// Sorted by artist, year, title.
    pub albums: Vec<Album>,
    /// Lowercase "title artist album-artist album file-name", per track.
    hay: Vec<String>,
}

/// Words of a filter box: lowercase, every one must occur.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query(Vec<String>);

impl Query {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self(text.split_whitespace().map(str::to_lowercase).collect())
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    #[must_use]
    pub fn matches(&self, hay: &str) -> bool {
        self.0.iter().all(|w| hay.contains(w.as_str()))
    }
}

impl LibraryIndex {
    /// Resolve `entries` (names fall back to folders under `roots`) and group albums.
    #[must_use]
    pub fn build(entries: &[Entry], roots: &[PathBuf]) -> Self {
        let mut tracks: Vec<Track> = entries.iter().filter_map(|e| resolve(e, roots)).collect();
        tracks.sort_by(|a, b| a.album_dir.cmp(&b.album_dir).then_with(|| play_order(a, b)));
        let albums = group_albums(&mut tracks);
        let hay = tracks
            .iter()
            .map(|t| {
                let mut h = String::with_capacity(96);
                for s in [
                    t.title.as_str(),
                    t.artist.as_str(),
                    t.album_artist.as_deref().unwrap_or(""),
                    t.album.as_str(),
                    t.file_name(),
                ] {
                    h.push_str(&s.to_lowercase());
                    h.push('\u{1f}');
                }
                h
            })
            .collect();
        Self {
            tracks,
            albums,
            hay,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Whether track `i` matches `q` (an empty query matches everything).
    #[must_use]
    pub fn track_matches(&self, i: usize, q: &Query) -> bool {
        q.is_empty() || self.hay.get(i).is_some_and(|h| q.matches(h))
    }

    /// Whether any track of album `a` matches `q`.
    #[must_use]
    pub fn album_matches(&self, a: usize, q: &Query) -> bool {
        q.is_empty()
            || self
                .albums
                .get(a)
                .is_some_and(|al| al.tracks.iter().any(|&i| self.track_matches(i, q)))
    }

    /// The track at `path`, if indexed.
    #[must_use]
    pub fn track_by_path(&self, path: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.path_str == path)
    }
}

// ---------------------------------------------------------------------------- name fallback

fn clean(s: Option<&str>) -> Option<String> {
    let t = s?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// `CD1`, `CD 2`, `Disc 1`, `Disk2`, `disc_3` → the disc number.
fn disc_dir(name: &str) -> Option<u32> {
    let l = name.trim().to_lowercase();
    let rest = ["disc", "disk", "cd"]
        .iter()
        .find_map(|p| l.strip_prefix(p))?;
    let digits = rest.trim_start_matches([' ', '_', '-', '.']);
    let n: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if n.is_empty() || n.len() != digits.trim_end().len() {
        return None;
    }
    n.parse().ok()
}

fn year_of(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &s[i..i + 4];
        let before = i == 0 || !b[i - 1].is_ascii_digit();
        let after = i + 4 == b.len() || !b[i + 4].is_ascii_digit();
        (before
            && after
            && w.bytes().all(|c| c.is_ascii_digit())
            && (w.starts_with("19") || w.starts_with("20")))
        .then(|| w.parse().ok())
        .flatten()
    })
}

/// An album folder name without its trailing `(2019)` / `[FLAC]` groups or a leading
/// `2019 - `, and the year found in them.
fn strip_folder_extras(name: &str) -> (String, Option<u32>) {
    let mut s = name.trim().to_string();
    let mut year = None;
    loop {
        let t = s.trim_end();
        let Some(close) = t.chars().last().filter(|c| matches!(c, ')' | ']')) else {
            break;
        };
        let open = if close == ')' { '(' } else { '[' };
        let Some(i) = t.rfind(open) else { break };
        let inner = &t[i + 1..t.len() - 1];
        year = year.or_else(|| year_of(inner));
        let head = t[..i].trim_end().to_string();
        if head.is_empty() {
            break;
        }
        s = head;
    }
    // "2019 - Album" / "(2019) Album" already handled above only when trailing.
    if let Some(y) = year_of(&s[..s.len().min(4)])
        && let Some(rest) = s.get(4..)
    {
        let rest = rest.trim_start_matches([' ', '-', '.', '_']);
        if !rest.is_empty() && s.len() >= 4 {
            year = year.or(Some(y));
            s = rest.to_string();
        }
    }
    (s.trim().to_string(), year)
}

/// `01 Title`, `01. Title`, `01 - Title`, `1-02 Title` → (disc, track, rest).
fn split_number(stem: &str) -> (Option<u32>, Option<u32>, &str) {
    let digits = |s: &str| s.chars().take_while(char::is_ascii_digit).count();
    let n = digits(stem);
    if n == 0 || n > 3 {
        return (None, None, stem);
    }
    let first: u32 = stem[..n].parse().unwrap_or(0);
    let rest = &stem[n..];
    // Disc-track: "1-02 Title" / "1.02 Title".
    if let Some(r) = rest.strip_prefix(['-', '.']) {
        let m = digits(r);
        if (1..=3).contains(&m) && r[m..].starts_with([' ', '.', '-', '_']) {
            let track: u32 = r[..m].parse().unwrap_or(0);
            let tail = r[m..].trim_start_matches([' ', '.', '-', '_']);
            if !tail.is_empty() {
                return (Some(first), Some(track), tail);
            }
        }
    }
    if !rest.starts_with([' ', '.', '-', '_', ')']) {
        return (None, None, stem);
    }
    let tail = rest.trim_start_matches([' ', '.', '-', '_', ')']);
    if tail.is_empty() {
        return (None, None, stem);
    }
    (None, Some(first), tail)
}

/// Pieces of a `A - B - C` name, trimmed, empty ones dropped.
fn dash_parts(s: &str) -> Vec<&str> {
    s.split(" - ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn file_mtime(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
}

/// The track an entry is, its names from tags, else from its file and folder names.
fn resolve(e: &Entry, roots: &[PathBuf]) -> Option<Track> {
    let path_str = e.path.to_str()?.to_string();
    let root = roots
        .iter()
        .filter(|r| e.path.starts_with(r))
        .max_by_key(|r| r.as_os_str().len());
    let inside = |p: &Path| root.is_none_or(|r| p.starts_with(r) && p != r.as_path());
    let parent = e.path.parent().unwrap_or(Path::new(""));
    let pname = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let (album_dir, dir_disc) = match (disc_dir(pname), parent.parent()) {
        (Some(d), Some(gp)) if inside(parent) && inside(gp) => (gp.to_path_buf(), Some(d)),
        _ => (parent.to_path_buf(), None),
    };
    // Folder guesses: only folders inside the library root say anything.
    let (mut f_artist, mut f_album, mut f_year) = (None, None, None);
    if inside(&album_dir) {
        let name = album_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let (base, year) = strip_folder_extras(name);
        f_year = year;
        let parts = dash_parts(&base);
        if parts.len() >= 2 {
            f_artist = Some(parts[0].to_string());
            f_album = Some(parts[1..].join(" - "));
        } else if !base.is_empty() {
            f_album = Some(base);
        }
        if f_artist.is_none()
            && let Some(gp) = album_dir.parent()
            && inside(gp)
        {
            f_artist = gp.file_name().and_then(|n| n.to_str()).map(str::to_string);
        }
    }
    let name = e.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, x)) if !s.is_empty() => (s, x),
        _ => (name, ""),
    };
    let (n_disc, n_track, rest) = split_number(stem);
    let parts = dash_parts(rest);
    let (n_artist, mut n_title) = if parts.len() >= 2 {
        (
            Some(parts[0].to_string()),
            parts.last().map(|s| (*s).to_string()),
        )
    } else {
        (None, Some(rest.trim().to_string()))
    };
    // "Artist - 01 - Title": the middle number is the track.
    let mut n_track = n_track;
    if parts.len() >= 3
        && n_track.is_none()
        && let Ok(t) = parts[parts.len() - 2].parse::<u32>()
    {
        n_track = Some(t);
    }
    if n_title.as_deref().is_none_or(str::is_empty) {
        n_title = Some(stem.to_string());
    }
    let tags = e.tags.clone().unwrap_or_default();
    let tagged = tags.named();
    let artist = clean(tags.artist.as_deref())
        .or_else(|| clean(tags.album_artist.as_deref()))
        .or(n_artist)
        .or(f_artist)
        .unwrap_or_else(|| "Unknown artist".to_string());
    let duration_secs = tags.duration_secs;
    let bitrate_kbps = duration_secs
        .filter(|&d| d > 0)
        .map(|d| (e.size.saturating_mul(8) / u64::from(d) / 1000) as u32);
    Some(Track {
        title: clean(tags.title.as_deref()).or(n_title).unwrap_or_default(),
        artist,
        album_artist: clean(tags.album_artist.as_deref()),
        album: clean(tags.album.as_deref()).or(f_album).unwrap_or_default(),
        track: tags.track.filter(|&t| t > 0).or(n_track),
        disc: tags.disc.filter(|&d| d > 0).or(dir_disc).or(n_disc),
        year: tags.year.or(f_year),
        duration_secs,
        format: ext.to_ascii_uppercase(),
        codec: tags.codec.clone(),
        bit_depth: tags.bit_depth,
        sample_rate_hz: tags.sample_rate_hz,
        bitrate_kbps,
        album_dir,
        album_index: 0,
        tagged,
        size: e.size,
        path: e.path.clone(),
        path_str,
    })
}

/// Natural order: digit runs compare as numbers, case-insensitive.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        s.push(c);
                    }
                    s
                };
                let (m, n) = (run(&mut x), run(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn play_order(a: &Track, b: &Track) -> std::cmp::Ordering {
    (a.disc.unwrap_or(1), a.track.unwrap_or(u32::MAX))
        .cmp(&(b.disc.unwrap_or(1), b.track.unwrap_or(u32::MAX)))
        .then_with(|| natural_cmp(a.file_name(), b.file_name()))
}

/// Albums of `tracks` (which must be sorted by album folder, then play order): within one
/// album folder, by album name, then by album artist (or artist). A folder whose tracks share
/// an album name but have different artists and no album artist is one "Various artists"
/// album. Sets each track's `album_index`.
fn group_albums(tracks: &mut [Track]) -> Vec<Album> {
    // (album dir, album lowercase) → its tracks, in first-seen (play) order.
    let mut groups: Vec<((PathBuf, String), Vec<usize>)> = Vec::new();
    let mut at: HashMap<(PathBuf, String), usize> = HashMap::new();
    for (i, t) in tracks.iter().enumerate() {
        let k = (t.album_dir.clone(), t.album.to_lowercase());
        let g = *at.entry(k.clone()).or_insert_with(|| {
            groups.push((k, Vec::new()));
            groups.len() - 1
        });
        groups[g].1.push(i);
    }
    let mut albums = Vec::new();
    for ((dir, album_key), members) in groups {
        let who = |i: usize| {
            let t = &tracks[i];
            t.album_artist.clone().unwrap_or_else(|| t.artist.clone())
        };
        let distinct: BTreeSet<String> = members.iter().map(|&i| who(i).to_lowercase()).collect();
        let any_album_artist = members.iter().any(|&i| tracks[i].album_artist.is_some());
        let split: Vec<(String, Vec<usize>)> = if distinct.len() <= 1 {
            vec![(who(members[0]), members)]
        } else if !any_album_artist {
            vec![("Various artists".to_string(), members)]
        } else {
            let mut by: Vec<(String, Vec<usize>)> = Vec::new();
            for i in members {
                let w = who(i);
                match by.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(&w)) {
                    Some((_, v)) => v.push(i),
                    None => by.push((w, vec![i])),
                }
            }
            by
        };
        for (artist, idx) in split {
            albums.push(make_album(tracks, &dir, &album_key, artist, idx));
        }
    }
    albums.sort_by(|a, b| {
        natural_cmp(&a.artist, &b.artist)
            .then(a.year.cmp(&b.year))
            .then_with(|| natural_cmp(&a.title, &b.title))
            .then_with(|| a.dir.cmp(&b.dir))
    });
    for (ai, a) in albums.iter().enumerate() {
        for &i in &a.tracks {
            tracks[i].album_index = ai;
        }
    }
    albums
}

fn make_album(
    tracks: &[Track],
    dir: &Path,
    album_key: &str,
    artist: String,
    idx: Vec<usize>,
) -> Album {
    let first = &tracks[idx[0]];
    let mut years: HashMap<u32, usize> = HashMap::new();
    for &i in &idx {
        if let Some(y) = tracks[i].year {
            *years.entry(y).or_default() += 1;
        }
    }
    let year = years
        .into_iter()
        .max_by_key(|(y, n)| (*n, std::cmp::Reverse(*y)))
        .map(|(y, _)| y);
    // The most common (format, depth, rate); lossy gets the average bitrate.
    let mut kinds: HashMap<(String, Option<u32>, Option<u32>), usize> = HashMap::new();
    for &i in &idx {
        let t = &tracks[i];
        let k = if t.lossless() {
            (t.format.clone(), t.bit_depth, t.sample_rate_hz)
        } else {
            (t.format.clone(), None, None)
        };
        *kinds.entry(k).or_default() += 1;
    }
    let formats: HashSet<&str> = idx.iter().map(|&i| tracks[i].format.as_str()).collect();
    let ((format, bit_depth, sample_rate_hz), _) = kinds
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .unwrap_or_default();
    let lossless = idx
        .iter()
        .map(|&i| &tracks[i])
        .find(|t| t.format == format)
        .is_some_and(Track::lossless);
    let rates: Vec<u32> = idx
        .iter()
        .filter(|&&i| tracks[i].format == format)
        .filter_map(|&i| tracks[i].bitrate_kbps)
        .collect();
    let bitrate_kbps = (!lossless && !rates.is_empty())
        .then(|| (rates.iter().map(|&r| u64::from(r)).sum::<u64>() / rates.len() as u64) as u32);
    let folder = dir.to_string_lossy().into_owned();
    Album {
        title: first.album.clone(),
        key: format!("{folder}\u{1f}{album_key}\u{1f}{}", artist.to_lowercase()),
        artist,
        year,
        folder,
        dir: dir.to_path_buf(),
        duration_secs: idx
            .iter()
            .filter_map(|&i| tracks[i].duration_secs)
            .map(u64::from)
            .sum(),
        quality: Quality {
            format,
            lossless,
            bit_depth,
            sample_rate_hz,
            bitrate_kbps,
            mixed: formats.len() > 1,
        },
        tracks: idx,
    }
}

// ---------------------------------------------------------------------------- scanning

/// How a scan is going.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub scanning: bool,
    /// Audio files found so far.
    pub found: usize,
    /// Files whose tags must be read (new or changed), and how many are done.
    pub to_read: usize,
    pub read: usize,
}

/// What the last scan did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanReport {
    pub tracks: usize,
    /// Tags read (new or changed files).
    pub reread: usize,
    /// Files gone since the previous index.
    pub removed: usize,
    pub elapsed: Duration,
}

/// Audio files under `roots`: (path, size, mtime), each once. Hidden names are skipped and
/// symlinked folders not followed (no loops); symlinked files are included.
#[must_use]
pub fn walk(roots: &[PathBuf], cancel: &AtomicBool) -> Vec<(PathBuf, u64, u64)> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !crate::rank::AUDIO_EXTENSIONS.contains(&crate::rank::extension(name).as_str()) {
                continue;
            }
            // `metadata` follows a symlinked file; a dangling link is skipped.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_file() && seen.insert(path.clone()) {
                out.push((path, meta.len(), file_mtime(&meta)));
            }
        }
    }
    out
}

/// Walk `roots` and build the new entries: unchanged files (same size and mtime) keep their
/// previous entry; new and changed ones are read with `reader` (on up to 4 threads); missing
/// ones drop out. `progress` is called from the calling thread now and then.
pub fn scan(
    roots: &[PathBuf],
    prev: &[Entry],
    reader: &TagReader,
    progress: &mut dyn FnMut(ScanProgress),
    cancel: &AtomicBool,
) -> (Vec<Entry>, ScanReport) {
    let started = Instant::now();
    progress(ScanProgress {
        scanning: true,
        ..ScanProgress::default()
    });
    let found = walk(roots, cancel);
    let before: HashMap<&Path, &Entry> = prev.iter().map(|e| (e.path.as_path(), e)).collect();
    let mut keep = Vec::with_capacity(found.len());
    let mut todo = Vec::new();
    for (path, size, mtime) in found {
        match before.get(path.as_path()) {
            Some(e) if e.size == size && e.mtime == mtime => keep.push((*e).clone()),
            _ => todo.push((path, size, mtime)),
        }
    }
    let present: HashSet<&Path> = keep
        .iter()
        .map(|e| e.path.as_path())
        .chain(todo.iter().map(|t| t.0.as_path()))
        .collect();
    let removed = prev
        .iter()
        .filter(|e| !present.contains(e.path.as_path()))
        .count();
    let total = keep.len() + todo.len();
    let to_read = todo.len();
    let done = AtomicUsize::new(0);
    let next = AtomicUsize::new(0);
    let read: Mutex<Vec<Entry>> = Mutex::new(Vec::with_capacity(to_read));
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4);
    std::thread::scope(|sc| {
        for _ in 0..threads.min(to_read.max(1)) {
            sc.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, size, mtime)) = todo.get(i) else {
                        break;
                    };
                    // A malformed file may trip a demuxer bug: that is "no tags", not a crash.
                    let tags =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader(path)))
                            .ok()
                            .flatten();
                    mine.push(Entry {
                        path: path.clone(),
                        size: *size,
                        mtime: *mtime,
                        tags,
                    });
                    done.fetch_add(1, Ordering::Relaxed);
                }
                if let Ok(mut r) = read.lock() {
                    r.extend(mine);
                }
            });
        }
        // Report while the readers run.
        loop {
            let d = done.load(Ordering::Relaxed);
            progress(ScanProgress {
                scanning: true,
                found: total,
                to_read,
                read: d,
            });
            if d >= to_read || cancel.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let reread = done.load(Ordering::Relaxed);
    let mut entries = keep;
    entries.extend(read.into_inner().unwrap_or_default());
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let report = ScanReport {
        tracks: entries.len(),
        reread,
        removed,
        elapsed: started.elapsed(),
    };
    (entries, report)
}

// ---------------------------------------------------------------------------- persistence

#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    version: u32,
    roots: Vec<PathBuf>,
}

/// Write the index atomically (temp file + rename) into `dir`.
///
/// # Errors
/// I/O errors creating or writing the file.
pub fn save_index(dir: &Path, roots: &[PathBuf], entries: &[Entry]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(INDEX_FILE);
    let tmp = dir.join(format!("{INDEX_FILE}.tmp"));
    {
        let mut w = io::BufWriter::new(std::fs::File::create(&tmp)?);
        let head = Header {
            version: INDEX_VERSION,
            roots: roots.to_vec(),
        };
        serde_json::to_writer(&mut w, &head)?;
        w.write_all(b"\n")?;
        for e in entries {
            serde_json::to_writer(&mut w, e)?;
            w.write_all(b"\n")?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, &path)
}

/// The stored index: (roots, entries). Missing file, other version or unreadable header:
/// empty. Lines that don't parse are skipped.
#[must_use]
pub fn load_index(dir: &Path) -> (Vec<PathBuf>, Vec<Entry>) {
    let Ok(f) = std::fs::File::open(dir.join(INDEX_FILE)) else {
        return (Vec::new(), Vec::new());
    };
    let mut lines = io::BufReader::new(f).lines();
    let head: Option<Header> = lines
        .next()
        .and_then(Result::ok)
        .and_then(|l| serde_json::from_str(&l).ok());
    let Some(head) = head.filter(|h| h.version == INDEX_VERSION) else {
        return (Vec::new(), Vec::new());
    };
    let entries = lines
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    (head.roots, entries)
}

// ---------------------------------------------------------------------------- stars

/// Starred songs (by path) and albums (by [`Album::key`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Stars {
    pub tracks: BTreeSet<String>,
    pub albums: BTreeSet<String>,
    /// Where they are saved; `None` keeps them in memory (demo, tests).
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Stars {
    /// The stars saved in `dir` ([`STARS_FILE`]); none when missing or unreadable.
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(STARS_FILE);
        let mut s: Self = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.path = Some(path);
        s
    }

    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn track(&self, path: &str) -> bool {
        self.tracks.contains(path)
    }

    #[must_use]
    pub fn album(&self, key: &str) -> bool {
        self.albums.contains(key)
    }

    /// Star or unstar a song, and save.
    ///
    /// # Errors
    /// The file could not be written (the change is kept in memory).
    pub fn toggle_track(&mut self, path: &str) -> io::Result<bool> {
        let on = !self.tracks.remove(path);
        if on {
            self.tracks.insert(path.to_string());
        }
        self.save().map(|()| on)
    }

    /// Star or unstar an album, and save.
    ///
    /// # Errors
    /// The file could not be written (the change is kept in memory).
    pub fn toggle_album(&mut self, key: &str) -> io::Result<bool> {
        let on = !self.albums.remove(key);
        if on {
            self.albums.insert(key.to_string());
        }
        self.save().map(|()| on)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

// ---------------------------------------------------------------------------- service

/// Called from the scanner thread when the index or progress changes.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// What the scanner has published.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub index: Arc<LibraryIndex>,
    pub progress: ScanProgress,
    pub last: Option<ScanReport>,
    /// The stored index was read (the first scan may still run).
    pub loaded: bool,
    pub error: Option<String>,
}

struct Shared {
    snap: Mutex<Snapshot>,
    generation: AtomicU64,
    notify: Mutex<Option<Notify>>,
    /// A newer scan is waiting: the running one stops.
    cancel: AtomicBool,
}

impl Shared {
    fn publish(&self, f: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut s) = self.snap.lock() {
            f(&mut s);
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
        let n = self.notify.lock().ok().and_then(|n| n.clone());
        if let Some(n) = n {
            n();
        }
    }
}

/// Where the scanner keeps its files and what it reads with.
pub struct Config {
    pub data_dir: PathBuf,
    pub tags: TagReader,
    /// Embedded covers, tried when an album folder has no image.
    pub art: Option<ArtReader>,
    /// Album covers go here, keyed ([`LOCAL_USER`], album folder).
    pub covers: Option<Covers>,
}

/// The background scanner. Cheap to poll from the UI: [`Library::snapshot`] clones an `Arc`.
pub struct Library {
    shared: Arc<Shared>,
    tx: Sender<Vec<PathBuf>>,
}

impl Library {
    /// Start the thread: it reads the stored index, publishes it, then waits for
    /// [`Library::rescan`].
    #[must_use]
    pub fn start(cfg: Config) -> Self {
        let shared = Arc::new(Shared {
            snap: Mutex::new(Snapshot::default()),
            generation: AtomicU64::new(0),
            notify: Mutex::new(None),
            cancel: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel();
        let sh = shared.clone();
        let _ = std::thread::Builder::new()
            .name("wholeseek-library".into())
            .spawn(move || run(&sh, &cfg, &rx));
        Self { shared, tx }
    }

    /// Scan `roots` (replacing the previous folders). A scan in progress stops first.
    pub fn rescan(&self, roots: Vec<PathBuf>) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(roots);
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .snap
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Bumped on every publish; compare to notice changes.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    pub fn set_notify(&self, n: Notify) {
        if let Ok(mut s) = self.shared.notify.lock() {
            *s = Some(n);
        }
    }
}

fn run(sh: &Shared, cfg: &Config, rx: &Receiver<Vec<PathBuf>>) {
    let (mut roots, mut entries) = load_index(&cfg.data_dir);
    let index = Arc::new(LibraryIndex::build(&entries, &roots));
    sh.publish(|s| {
        s.index = index;
        s.loaded = true;
    });
    let mut covered: HashSet<String> = HashSet::new();
    while let Ok(mut want) = rx.recv() {
        // Only the latest request matters.
        while let Ok(w) = rx.try_recv() {
            want = w;
        }
        sh.cancel.store(false, Ordering::Relaxed);
        let mut last = Instant::now() - Duration::from_secs(1);
        let (new, report) = scan(
            &want,
            &entries,
            &cfg.tags,
            &mut |p| {
                if last.elapsed() >= Duration::from_millis(200) || p.read == p.to_read {
                    last = Instant::now();
                    sh.publish(|s| s.progress = p);
                }
            },
            &sh.cancel,
        );
        if sh.cancel.load(Ordering::Relaxed) {
            continue; // a newer request is queued
        }
        let changed = new != entries || want != roots;
        entries = new;
        roots = want;
        let error = if changed {
            save_index(&cfg.data_dir, &roots, &entries)
                .err()
                .map(|e| format!("library index not saved: {e}"))
        } else {
            None
        };
        let index = Arc::new(LibraryIndex::build(&entries, &roots));
        sh.publish(|s| {
            s.index = index.clone();
            s.progress = ScanProgress::default();
            s.last = Some(report);
            s.error = error;
        });
        if let Some(covers) = &cfg.covers {
            store_covers(&index, covers, cfg.art.as_ref(), &mut covered, &sh.cancel);
        }
    }
}

/// Covers for albums that have none cached yet: the folder's image ([`crate::cover::pick_cover`]),
/// else art embedded in its first track. Each album folder is tried once per run.
fn store_covers(
    index: &LibraryIndex,
    covers: &Covers,
    art: Option<&ArtReader>,
    tried: &mut HashSet<String>,
    cancel: &AtomicBool,
) {
    for a in &index.albums {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if covers.cached(LOCAL_USER, &a.folder).is_some() || !tried.insert(a.folder.clone()) {
            continue;
        }
        let first = a.tracks.first().map(|&i| &index.tracks[i]);
        let mut dirs = vec![a.dir.clone()];
        if let Some(d) = first.and_then(|t| t.path.parent())
            && d != a.dir
        {
            dirs.push(d.to_path_buf());
        }
        let image = dirs.iter().find_map(|d| {
            let files: Vec<(String, u64)> = std::fs::read_dir(d)
                .ok()?
                .flatten()
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    m.is_file()
                        .then(|| (e.path().to_string_lossy().into_owned(), m.len()))
                })
                .collect();
            let i = crate::cover::pick_cover(&files)?;
            std::fs::read(&files[i].0).ok()
        });
        let key = crate::cover::cover_key(LOCAL_USER, &a.folder);
        if let Some(bytes) = image {
            let _ = covers.store_folder(&key, &bytes);
        } else if let (Some(read), Some(t)) = (art, first)
            && let Some(bytes) = read(&t.path)
        {
            let _ = covers.store_embedded(LOCAL_USER, &a.folder, &bytes);
        }
    }
}

#[cfg(test)]
mod tests;
