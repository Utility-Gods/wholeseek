//! Search quality: relevance scoring, and grouping of search hits into albums and
//! tracks across users.
//!
//! Everything here is pure and format-neutral: relevance never looks at the
//! extension or bitrate, and the format only separates *editions* of the same
//! album or track (a FLAC and an MP3 copy are one album with two editions).
//!
//! Typical use (UI):
//!
//! ```
//! # use wholeseek_core::group::group_hits;
//! # let (query, hits) = ("aphex twin", Vec::new());
//! let grouped = group_hits(query, &hits);
//! for album in &grouped.albums {
//!     // "Selected Ambient Works 85-92 — 12 sources"
//!     let _ = (&album.title, album.source_count());
//!     let best = album.best_source(); // ▶ / download this one
//!     let fallback = album.alternates(); // other users, same edition first
//!     # let _ = (best, fallback);
//! }
//! ```

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;

use crate::model::{RemoteFile, SearchHit};
use crate::paths::{remote_basename, remote_parent};
use crate::query::fold_accents;
use crate::rank::extension;

// ---------------------------------------------------------------------------
// Text normalization
// ---------------------------------------------------------------------------

/// Lower-case, accent-folded words of `s` (split on anything that is not a
/// letter or digit).
#[must_use]
pub fn words(s: &str) -> Vec<String> {
    if s.is_ascii() {
        return s
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
    }
    let folded = fold_accents(s).to_lowercase();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Extension of a basename without allocating (`""` if none).
fn ext_of(base: &str) -> &str {
    base.rfind('.')
        .filter(|&i| i > 0)
        .map_or("", |i| &base[i + 1..])
}

/// [`is_audio`] without allocating.
fn is_audio_fast(path: &str) -> bool {
    let ext = ext_of(remote_basename(path));
    crate::rank::AUDIO_EXTENSIONS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(ext))
}

fn stem(base: &str) -> &str {
    base.rfind('.')
        .filter(|&i| i > 0 && base.len() - i <= 6)
        .map_or(base, |i| &base[..i])
}

/// Words that mark a bracketed or dash-separated segment as a release/encoding
/// note rather than part of the title: `(2011 Remaster)`, `[FLAC 24bit]`.
fn is_note_word(w: &str) -> bool {
    matches!(
        w,
        "remaster"
            | "remastered"
            | "remasterd"
            | "remastering"
            | "deluxe"
            | "explicit"
            | "clean"
            | "bonus"
            | "album"
            | "lp"
            | "single"
            | "version"
            | "edition"
            | "expanded"
            | "anniversary"
            | "digital"
            | "flac"
            | "mp3"
            | "aac"
            | "ogg"
            | "alac"
            | "wav"
            | "kbps"
            | "khz"
            | "bit"
            | "24bit"
            | "16bit"
            | "lossless"
            | "hires"
            | "hi"
            | "res"
            | "web"
            | "cd"
            | "vinyl"
            | "rip"
            | "vbr"
            | "cbr"
            | "v0"
            | "v2"
            | "320"
            | "256"
            | "192"
            | "128"
            | "mix"
            | "original"
    ) || w.chars().all(|c| c.is_ascii_digit())
}

/// Whether a bracketed/dash segment is only a release note (and has a word
/// that is clearly one, so `(1999)` alone also counts but `(Live)` does not).
fn is_note(segment: &str) -> bool {
    let ws = words(segment);
    !ws.is_empty() && ws.iter().all(|w| is_note_word(w))
}

/// Words that, as a last " - " segment, describe the take rather than being the
/// title: "Song - Live" keeps both parts.
fn is_take_word(w: &str) -> bool {
    matches!(
        w,
        "live"
            | "remix"
            | "edit"
            | "mix"
            | "acoustic"
            | "demo"
            | "instrumental"
            | "version"
            | "radio"
            | "extended"
            | "original"
            | "single"
            | "mono"
            | "stereo"
            | "reprise"
            | "unplugged"
    ) || w.chars().all(|c| c.is_ascii_digit())
}

/// Strip a leading track number: `01 `, `01. `, `1-02 `, `101 - `, `A1 `, `CD1-03 `.
fn strip_track_number(s: &str) -> &str {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    // "cd1-" / "d1." prefixes
    for p in ["cd", "d"] {
        if b.len() > p.len() + 1
            && b[..p.len()].eq_ignore_ascii_case(p.as_bytes())
            && b[p.len()].is_ascii_digit()
        {
            i = p.len();
            break;
        }
    }
    // Vinyl side "A1 "
    if i == 0
        && b.len() > 2
        && matches!(b[0], b'a'..=b'd' | b'A'..=b'D')
        && b[1].is_ascii_digit()
        && b[2..]
            .iter()
            .find(|c| !c.is_ascii_digit())
            .is_some_and(|c| matches!(c, b' ' | b'.' | b'-' | b'_'))
    {
        i = 1;
    }
    let start = i;
    while i < b.len() && (b[i].is_ascii_digit() || matches!(b[i], b' ' | b'.' | b'-' | b'_')) {
        i += 1;
    }
    let digits = b[start..i].iter().filter(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return t;
    }
    let rest = &t[i..];
    // "1999.flac" or "99 Luftballons" with nothing left: keep the whole name.
    if rest.chars().any(char::is_alphanumeric) {
        // Don't eat a leading number that is glued to the title ("2Pac", "50 Cent"
        // after a separator is still stripped; consistent across copies).
        if b[i - 1].is_ascii_digit() && i < b.len() && b[i].is_ascii_alphanumeric() {
            return t;
        }
        rest
    } else {
        // Only numbers ("08 - 1999"): drop just the first one if more follow.
        let mut j = start;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let k = j + b[j..]
            .iter()
            .take_while(|c| matches!(c, b' ' | b'.' | b'-' | b'_'))
            .count();
        if k > j && k < b.len() { &t[k..] } else { t }
    }
}

/// Remove bracketed segments that are release notes: `(Remastered 2011)`,
/// `[FLAC]`, `{24bit}`; keeps `(Live)`, `(feat. X)`, `(Remix)`.
fn strip_notes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find(['(', '[', '{']) {
        let close_ch = match rest.as_bytes()[open] {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        };
        let Some(len) = rest[open + 1..].find(close_ch) else {
            break;
        };
        let inner = &rest[open + 1..open + 1 + len];
        out.push_str(&rest[..open]);
        if !is_note(inner) {
            out.push(' ');
            out.push_str(inner);
            out.push(' ');
        }
        rest = &rest[open + 1 + len + 1..];
    }
    out.push_str(rest);
    out
}

/// Normalized track title of a remote path, for matching the same track across
/// users and formats: no extension, track number, release notes
/// (`(2011 Remaster)`, `[FLAC]`), leading `Artist - ` part, case, accents or
/// punctuation. Words are joined by single spaces.
///
/// `"01 - Aphex Twin - Xtal (Remastered).flac"` → `"xtal"`.
#[must_use]
pub fn track_title(path: &str) -> String {
    let base = remote_basename(path);
    let s = strip_track_number(stem(base));
    let s = strip_notes(s);
    // " - " segments: drop trailing release notes, then keep the title part.
    let spaced = s.replace('_', " ");
    let mut segs = dash_segments(&spaced);
    while segs.len() > 1 && segs.last().is_some_and(|l| is_note(l)) {
        segs.pop();
    }
    let title = match segs.len() {
        0 => String::new(),
        1 => segs[0].to_owned(),
        n => {
            let last = segs[n - 1];
            let lw = words(last);
            if !lw.is_empty() && lw.iter().all(|w| is_take_word(w)) {
                format!("{} {last}", strip_track_number(segs[n - 2]))
            } else {
                last.to_owned()
            }
        }
    };
    let ws = words(&title);
    if ws.is_empty() {
        // Nothing but notes/numbers: fall back to all words of the stem.
        return words(stem(base)).join(" ");
    }
    ws.join(" ")
}

/// Split on a dash between spaces (" - ", " – ", " — "), trimming parts and
/// dropping empty ones.
fn dash_segments(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let dash_len = if b[i] == b'-' {
            1
        } else if b[i..].starts_with("\u{2013}".as_bytes())
            || b[i..].starts_with("\u{2014}".as_bytes())
        {
            3
        } else {
            0
        };
        if dash_len > 0 && i > 0 && b[i - 1] == b' ' && b.get(i + dash_len) == Some(&b' ') {
            out.push(s[start..i].trim());
            start = i + dash_len;
            i += dash_len;
            continue;
        }
        i += 1;
    }
    out.push(s[start..].trim());
    out.retain(|p| p.chars().any(char::is_alphanumeric));
    out
}

/// `CD1`, `Disc 2`, `disk 02 - Bonus`, `CD 1`: a disc sub-folder of an album.
#[must_use]
pub fn is_disc_folder(name: &str) -> bool {
    is_disc_words(&words(name))
}

fn is_disc_words(ws: &[String]) -> bool {
    let Some(first) = ws.first() else {
        return false;
    };
    for p in ["cd", "disc", "disk", "disque", "disco", "side"] {
        if first == p {
            return ws.get(1).is_some_and(|w| {
                w.chars().all(|c| c.is_ascii_digit()) || matches!(w.as_str(), "a" | "b" | "c" | "d")
            });
        }
        if let Some(n) = first.strip_prefix(p)
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// The album folder of a file: its parent, or the grandparent when the parent
/// is a disc folder (`Album\CD2\01.flac` → `Album`).
#[must_use]
pub fn album_folder(path: &str) -> &str {
    let parent = remote_parent(path);
    if is_disc_folder(remote_basename(parent)) {
        let gp = remote_parent(parent);
        if !gp.is_empty() {
            return gp;
        }
    }
    parent
}

// ---------------------------------------------------------------------------
// Relevance
// ---------------------------------------------------------------------------

fn is_noise_word(w: &str) -> bool {
    matches!(
        w,
        "incomplete"
            | "incompleted"
            | "unfinished"
            | "temp"
            | "tmp"
            | "partial"
            | "sample"
            | "samples"
            | "preview"
            | "previews"
            | "snippet"
            | "snippets"
            | "trash"
            | "recycle"
            | "deleted"
    )
}

/// Relevance of `path` to a typed `query`; higher is better, 0 = excluded or no
/// query word found.
///
/// Per query word, the best place it occurs: file name (whole word 100, word
/// prefix 60) > album folder (50 / 30) > deeper parents (20 / 12). Bonuses:
/// every word found (+60), words found both in the file name and in the
/// folders, i.e. artist + title (+40), file-name words in query order (+20).
/// Paths with noise folders or names (`incomplete`, `temp`, `sample`,
/// `preview`, …) get a quarter of the score. `-word` excludes (score 0) and
/// `*part` matches inside words, as Soulseek does. Never looks at the format.
#[must_use]
pub fn relevance(query: &str, path: &str) -> u32 {
    Ctx::default().relevance(&Query::parse(query), path)
}

/// A typed query split into wanted words (with the partial flag of `*part`)
/// and excluded words (`-word`).
#[derive(Debug, Default)]
struct Query {
    want: Vec<(String, bool)>,
    exclude: Vec<String>,
}

impl Query {
    fn parse(query: &str) -> Self {
        let mut q = Query::default();
        for raw in query.split_whitespace() {
            if let Some(x) = raw.strip_prefix('-') {
                q.exclude.extend(words(x));
            } else if let Some(x) = raw.strip_prefix('*') {
                q.want.extend(words(x).into_iter().map(|w| (w, true)));
            } else {
                q.want.extend(words(raw).into_iter().map(|w| (w, false)));
            }
        }
        q
    }
}

type Words = Rc<[String]>;

/// Small, fast non-cryptographic hasher (FxHash) for the per-call maps: keys are
/// short path strings and the maps live for one grouping call only.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl Fx {
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut v = [0u8; 8];
            v[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(v));
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

struct FileInfo {
    words: Words,
    title: String,
}

/// Per-call memo: search results repeat the same folder names, file names and
/// titles across thousands of hits (every user's copy of an album), so each
/// distinct string is tokenized once.
#[derive(Default)]
struct Ctx {
    /// Path component (or file stem) → its words.
    words: FxMap<String, Words>,
    /// Basename → its stem's words and track title.
    files: FxMap<String, Rc<FileInfo>>,
    /// Last `album_folder` lookup (hits arrive folder by folder).
    last_dir: Option<(String, usize)>,
    /// Path → album folder length (prefix of the path).
    album_dirs: FxMap<String, usize>,
}

impl Ctx {
    fn words(&mut self, s: &str) -> Words {
        if let Some(w) = self.words.get(s) {
            return w.clone();
        }
        let w: Words = words(s).into();
        self.words.insert(s.to_owned(), w.clone());
        w
    }

    fn file(&mut self, path: &str) -> Rc<FileInfo> {
        let base = remote_basename(path);
        if let Some(f) = self.files.get(base) {
            return f.clone();
        }
        let f = Rc::new(FileInfo {
            words: words(stem(base)).into(),
            title: track_title(base),
        });
        self.files.insert(base.to_owned(), f.clone());
        f
    }

    fn is_disc_folder(&mut self, name: &str) -> bool {
        is_disc_words(&self.words(name))
    }

    /// [`album_folder`] with memoized disc checks.
    fn album_folder<'p>(&mut self, path: &'p str) -> &'p str {
        let parent = remote_parent(path);
        if let Some((p, n)) = &self.last_dir
            && p == parent
        {
            return &parent[..*n];
        }
        if let Some(&n) = self.album_dirs.get(parent) {
            self.last_dir = Some((parent.to_owned(), n));
            return &parent[..n];
        }
        let mut dir = parent;
        if self.is_disc_folder(remote_basename(parent)) {
            let gp = remote_parent(parent);
            if !gp.is_empty() {
                dir = gp;
            }
        }
        self.album_dirs.insert(parent.to_owned(), dir.len());
        self.last_dir = Some((parent.to_owned(), dir.len()));
        dir
    }

    /// Words of an album folder and of its ancestors, for scoring all its files.
    fn folder_words(&mut self, album_dir: &str) -> (Words, Vec<Words>) {
        let album = self.words(remote_basename(album_dir));
        let deeper = remote_parent(album_dir)
            .split(['\\', '/'])
            .filter(|c| !c.is_empty())
            .map(|c| self.words(c))
            .collect();
        (album, deeper)
    }

    /// Relevance of a file whose album folder words are already known.
    fn relevance_in(
        &mut self,
        q: &Query,
        path: &str,
        file: &FileInfo,
        album_dir: &str,
        folder: &(Words, Vec<Words>),
    ) -> u32 {
        if q.want.is_empty() {
            return 0;
        }
        let parent = remote_parent(path);
        let tilde = path.contains('~');
        if parent != album_dir && parent.len() > album_dir.len() {
            let both = [folder.0.clone(), self.words(remote_basename(parent))];
            score_words(q, &file.words, &both, &folder.1, tilde)
        } else {
            score_words(
                q,
                &file.words,
                std::slice::from_ref(&folder.0),
                &folder.1,
                tilde,
            )
        }
    }

    fn relevance(&mut self, q: &Query, path: &str) -> u32 {
        if q.want.is_empty() {
            return 0;
        }
        let album_dir = self.album_folder(path);
        let folder = self.folder_words(album_dir);
        let file = self.file(path);
        self.relevance_in(q, path, &file, album_dir, &folder)
    }
}

fn level(ws: &[String], w: &str, partial: bool) -> u8 {
    let mut best = 0;
    for x in ws {
        if x == w {
            return 2;
        }
        if (partial && x.contains(w)) || (!partial && x.starts_with(w)) {
            best = 1;
        }
    }
    best
}

fn level_any(groups: &[Words], w: &str, partial: bool) -> u8 {
    groups
        .iter()
        .map(|g| level(g, w, partial))
        .max()
        .unwrap_or(0)
}

fn score_words(q: &Query, file: &[String], album: &[Words], deeper: &[Words], tilde: bool) -> u32 {
    let has = |groups: &[Words], x: &String| groups.iter().any(|g| g.contains(x));
    if q.exclude
        .iter()
        .any(|x| file.contains(x) || has(album, x) || has(deeper, x))
    {
        return 0;
    }
    let mut score = 0u32;
    let mut found = 0usize;
    let (mut in_file, mut in_dirs) = (false, false);
    let mut file_pos: Vec<usize> = Vec::new();
    for (w, partial) in &q.want {
        let s = match level(file, w, *partial) {
            2 => 100,
            1 => 60,
            _ => 0,
        };
        let s = if s > 0 {
            in_file = true;
            if let Some(p) = file.iter().position(|x| x == w) {
                file_pos.push(p);
            }
            s
        } else {
            let a = match level_any(album, w, *partial) {
                2 => 50,
                1 => 30,
                _ => 0,
            };
            let d = match level_any(deeper, w, *partial) {
                2 => 20,
                1 => 12,
                _ => 0,
            };
            if a.max(d) > 0 {
                in_dirs = true;
            }
            a.max(d)
        };
        if s > 0 {
            found += 1;
        }
        score += s;
    }
    if found == 0 {
        return 0;
    }
    if found == q.want.len() {
        score += 60;
    }
    if in_file && in_dirs {
        score += 40;
    }
    if file_pos.len() >= 2 && file_pos.windows(2).all(|p| p[0] < p[1]) {
        score += 20;
    }
    let noisy = tilde
        || file.iter().any(|w| is_noise_word(w))
        || album
            .iter()
            .chain(deeper)
            .any(|g| g.iter().any(|w| is_noise_word(w)));
    if noisy {
        score /= 4;
    }
    score.max(1)
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// Durations within this many seconds count as the same recording (encoders
/// differ by padding; peers round differently).
pub const DURATION_TOLERANCE_SECS: u32 = 2;

fn durations_match(a: Option<u32>, b: Option<u32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.abs_diff(b) <= DURATION_TOLERANCE_SECS,
        _ => true,
    }
}

/// Identity of a track across users and formats: [`track_title`] plus the
/// advertised duration. Two keys are the same track when [`TrackKey::matches`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub title: String,
    pub duration_secs: Option<u32>,
}

impl TrackKey {
    /// Same title, and durations within [`DURATION_TOLERANCE_SECS`] when both
    /// are known.
    #[must_use]
    pub fn matches(&self, other: &TrackKey) -> bool {
        !self.title.is_empty()
            && self.title == other.title
            && durations_match(self.duration_secs, other.duration_secs)
    }
}

/// [`TrackKey`] of one file.
#[must_use]
pub fn track_key(file: &RemoteFile) -> TrackKey {
    TrackKey {
        title: track_title(&file.path),
        duration_secs: file.attrs.duration_secs,
    }
}

/// Identity of an album across users and formats: its track titles (sorted, so
/// disc layout and numbering don't matter) and their durations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlbumKey {
    /// Normalized titles, sorted.
    pub titles: Vec<String>,
    /// Duration of each title (same order), if advertised.
    pub durations: Vec<Option<u32>>,
}

impl AlbumKey {
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.titles.len()
    }

    /// Sum of the known durations.
    #[must_use]
    pub fn total_secs(&self) -> u32 {
        self.durations.iter().flatten().sum()
    }

    /// Same titles and, where both copies advertise them, track durations within
    /// [`DURATION_TOLERANCE_SECS`].
    #[must_use]
    pub fn matches(&self, other: &AlbumKey) -> bool {
        self.titles == other.titles
            && self
                .durations
                .iter()
                .zip(&other.durations)
                .all(|(a, b)| durations_match(*a, *b))
    }
}

/// [`AlbumKey`] of the audio files of one folder (non-audio files are ignored;
/// the format is not part of the key).
#[must_use]
pub fn album_key<'a>(files: impl IntoIterator<Item = &'a RemoteFile>) -> AlbumKey {
    let files: Vec<&RemoteFile> = files.into_iter().collect();
    album_key_ctx(&mut Ctx::default(), files)
}

fn album_key_ctx<'a>(ctx: &mut Ctx, files: impl IntoIterator<Item = &'a RemoteFile>) -> AlbumKey {
    album_key_from(
        files
            .into_iter()
            .filter(|f| is_audio_fast(&f.path))
            .map(|f| (ctx.file(&f.path).title.clone(), f.attrs.duration_secs))
            .collect(),
    )
}

fn album_key_from(mut tracks: Vec<(String, Option<u32>)>) -> AlbumKey {
    tracks.sort();
    AlbumKey {
        titles: tracks.iter().map(|t| t.0.clone()).collect(),
        durations: tracks.iter().map(|t| t.1).collect(),
    }
}

// ---------------------------------------------------------------------------
// Editions and grouping
// ---------------------------------------------------------------------------

/// Display label of a file's format and quality, e.g. `("FLAC", "24-bit 96 kHz")`,
/// `("MP3", "320 kbps")`, `("MP3", "VBR")`. Only used to split editions and for
/// display; never for ordering.
#[must_use]
pub fn format_quality(file: &RemoteFile) -> (String, String) {
    let ext = extension(&file.path);
    let a = &file.attrs;
    let lossless = matches!(
        ext.as_str(),
        "flac" | "wav" | "aif" | "aiff" | "alac" | "ape" | "wv"
    );
    let quality = if lossless {
        match (a.bit_depth, a.sample_rate_hz) {
            (Some(b), Some(r)) => format!("{b}-bit {}", khz(r)),
            (Some(b), None) => format!("{b}-bit"),
            (None, Some(r)) => khz(r),
            (None, None) => String::new(),
        }
    } else if a.vbr == Some(true) {
        "VBR".to_owned()
    } else {
        a.bitrate_kbps
            .map(|k| format!("{k} kbps"))
            .unwrap_or_default()
    };
    (ext.to_uppercase(), quality)
}

fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", f64::from(hz) / 1000.0)
    }
}

/// Folder quality: the most common per-file label (ties: the first seen).
fn folder_format_quality(files: &[RemoteFile]) -> (String, String) {
    // Count by raw (extension, attrs) and format the winner once.
    let mut counts: Vec<(&RemoteFile, usize)> = Vec::new();
    for f in files {
        let ext = ext_of(remote_basename(&f.path));
        let same = |g: &RemoteFile| {
            g.attrs == f.attrs && ext_of(remote_basename(&g.path)).eq_ignore_ascii_case(ext)
        };
        match counts.iter_mut().find(|c| same(c.0)) {
            Some(c) => c.1 += 1,
            None => counts.push((f, 1)),
        }
    }
    let mut best: Option<(&RemoteFile, usize)> = None;
    for c in counts {
        if best.is_none_or(|b| c.1 > b.1) {
            best = Some(c);
        }
    }
    best.map(|b| format_quality(b.0)).unwrap_or_default()
}

/// One user's copy: an album folder (with its matching audio files) or a single
/// file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub username: String,
    /// Album folder (disc sub-folders folded in) or the file's folder.
    pub folder: String,
    pub free_slot: bool,
    pub speed: u32,
    /// Audio files of this copy that were in the results, in path order.
    pub files: Vec<RemoteFile>,
}

impl Source {
    /// Availability order used everywhere (same as [`crate::rank::rank`]): free
    /// slot first, then faster; then by user name for a stable order.
    fn availability_cmp(&self, other: &Source) -> std::cmp::Ordering {
        other
            .free_slot
            .cmp(&self.free_slot)
            .then(other.speed.cmp(&self.speed))
            .then_with(|| self.username.cmp(&other.username))
            .then_with(|| self.folder.cmp(&other.folder))
    }
}

/// One format/quality of an album or track, with every user who has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edition {
    /// Upper-case extension, e.g. `FLAC`.
    pub format: String,
    /// e.g. `16-bit 44.1 kHz`, `320 kbps`, `VBR`; empty if not advertised.
    pub quality: String,
    /// Best available first (free slot, then speed).
    pub sources: Vec<Source>,
}

/// One album (or one track, for single files) found at one or more users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    pub key: AlbumKey,
    /// Display name: the album folder name of the best source.
    pub title: String,
    /// Best [`relevance`] among its files.
    pub relevance: u32,
    /// Most sources first (format-neutral), then best availability.
    pub editions: Vec<Edition>,
}

impl Album {
    /// Number of user copies across all editions ("12 sources").
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.editions.iter().map(|e| e.sources.len()).sum()
    }

    /// The copy to use: best availability across all editions.
    #[must_use]
    pub fn best_source(&self) -> Option<&Source> {
        self.editions
            .iter()
            .flat_map(|e| &e.sources)
            .min_by(|a, b| a.availability_cmp(b))
    }

    /// Edition of [`best_source`](Self::best_source).
    #[must_use]
    pub fn best_edition(&self) -> Option<&Edition> {
        let best = self.best_source()?;
        self.editions
            .iter()
            .find(|e| e.sources.iter().any(|s| std::ptr::eq(s, best)))
    }

    /// Fallback copies for ▶ when the best one queues or fails: the rest of the
    /// best source's edition first (same files, same format), then the other
    /// editions; each by availability. Excludes the best source's user.
    #[must_use]
    pub fn alternates(&self) -> Vec<&Source> {
        let Some(best) = self.best_source() else {
            return Vec::new();
        };
        let mut same: Vec<&Source> = Vec::new();
        let mut other: Vec<&Source> = Vec::new();
        for e in &self.editions {
            let is_best = e.sources.iter().any(|s| std::ptr::eq(s, best));
            for s in &e.sources {
                if s.username == best.username {
                    continue;
                }
                if is_best { &mut same } else { &mut other }.push(s);
            }
        }
        same.sort_by(|a, b| a.availability_cmp(b));
        other.sort_by(|a, b| a.availability_cmp(b));
        same.extend(other);
        same
    }
}

/// Group hits into one [`Source`] per (user, album folder); disc sub-folders
/// are folded into their album. Only audio files are kept. Order: first
/// arrival.
#[must_use]
pub fn folder_groups(hits: &[SearchHit]) -> Vec<Source> {
    folder_groups_ctx(&mut Ctx::default(), hits)
}

fn folder_groups_ctx(ctx: &mut Ctx, hits: &[SearchHit]) -> Vec<Source> {
    let mut index: FxMap<(&str, &str), usize> = FxMap::default();
    index.reserve(hits.len() / 4);
    let mut out: Vec<Source> = Vec::new();
    for h in hits {
        if !is_audio_fast(&h.file.path) {
            continue;
        }
        let folder = ctx.album_folder(&h.file.path);
        let i = *index
            .entry((h.file.username.as_str(), folder))
            .or_insert_with(|| {
                out.push(Source {
                    username: h.file.username.clone(),
                    folder: folder.to_owned(),
                    free_slot: h.free_slot,
                    speed: h.speed,
                    files: Vec::new(),
                });
                out.len() - 1
            });
        let s = &mut out[i];
        s.free_slot |= h.free_slot;
        s.speed = s.speed.max(h.speed);
        s.files.push(h.file.clone());
    }
    for s in &mut out {
        s.files.sort_by(|a, b| a.path.cmp(&b.path));
        s.files.dedup_by(|a, b| a.path == b.path);
    }
    out
}

/// Merge copies of the same album at different users: sources with matching
/// [`AlbumKey`]s become one [`Album`]; within it, sources split into editions by
/// format and quality. `query` only sets [`Album::relevance`] (empty: 0).
/// Result order: relevance, then source count, then title.
#[must_use]
pub fn merge_sources(query: &str, groups: Vec<Source>) -> Vec<Album> {
    merge_sources_ctx(&mut Ctx::default(), &Query::parse(query), groups)
}

fn merge_sources_ctx(ctx: &mut Ctx, q: &Query, groups: Vec<Source>) -> Vec<Album> {
    struct Pending {
        key: AlbumKey,
        relevance: u32,
        sources: Vec<(Source, (String, String))>,
    }
    // Bucket by titles (hashable), then split each bucket by duration agreement.
    let mut buckets: FxMap<Vec<String>, Vec<Pending>> = FxMap::default();
    for src in groups {
        if src.files.is_empty() {
            continue;
        }
        let infos: Vec<Rc<FileInfo>> = src.files.iter().map(|f| ctx.file(&f.path)).collect();
        let key = album_key_from(
            src.files
                .iter()
                .zip(&infos)
                .filter(|(f, _)| is_audio_fast(&f.path))
                .map(|(f, i)| (i.title.clone(), f.attrs.duration_secs))
                .collect(),
        );
        let mut rel = 0;
        if !q.want.is_empty() {
            let folder = ctx.folder_words(&src.folder);
            for (f, info) in src.files.iter().zip(&infos) {
                rel = rel.max(ctx.relevance_in(q, &f.path, info, &src.folder, &folder));
            }
        }
        let fq = folder_format_quality(&src.files);
        let bucket = buckets.entry(key.titles.clone()).or_default();
        match bucket.iter_mut().find(|p| p.key.matches(&key)) {
            Some(p) => {
                p.relevance = p.relevance.max(rel);
                // Fill in durations the first copy did not advertise.
                for (d, n) in p.key.durations.iter_mut().zip(&key.durations) {
                    if d.is_none() {
                        *d = *n;
                    }
                }
                p.sources.push((src, fq));
            }
            None => bucket.push(Pending {
                key,
                relevance: rel,
                sources: vec![(src, fq)],
            }),
        }
    }
    let mut albums: Vec<Album> = buckets
        .into_values()
        .flatten()
        .map(|p| {
            let mut editions: Vec<Edition> = Vec::new();
            for (src, (format, quality)) in p.sources {
                match editions
                    .iter_mut()
                    .find(|e| e.format == format && e.quality == quality)
                {
                    Some(e) => e.sources.push(src),
                    None => editions.push(Edition {
                        format,
                        quality,
                        sources: vec![src],
                    }),
                }
            }
            for e in &mut editions {
                e.sources.sort_by(|a, b| a.availability_cmp(b));
            }
            editions.sort_by(|a, b| {
                b.sources
                    .len()
                    .cmp(&a.sources.len())
                    .then_with(|| a.sources[0].availability_cmp(&b.sources[0]))
                    .then_with(|| a.format.cmp(&b.format))
                    .then_with(|| a.quality.cmp(&b.quality))
            });
            let mut album = Album {
                key: p.key,
                title: String::new(),
                relevance: p.relevance,
                editions,
            };
            album.title = album
                .best_source()
                .map(|s| remote_basename(&s.folder).to_owned())
                .unwrap_or_default();
            album
        })
        .collect();
    albums.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then(b.source_count().cmp(&a.source_count()))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.key.titles.cmp(&b.key.titles))
    });
    albums
}

/// One track found at one or more users (from single-file results).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub key: TrackKey,
    /// Display name: the best source's file name.
    pub title: String,
    pub relevance: u32,
    /// Each source has exactly one file.
    pub editions: Vec<Edition>,
}

impl Track {
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.editions.iter().map(|e| e.sources.len()).sum()
    }

    /// The copy to play: best availability across editions.
    #[must_use]
    pub fn best_source(&self) -> Option<&Source> {
        self.editions
            .iter()
            .flat_map(|e| &e.sources)
            .min_by(|a, b| a.availability_cmp(b))
    }

    /// ▶ fallbacks, same edition first (see [`Album::alternates`]).
    #[must_use]
    pub fn alternates(&self) -> Vec<&Source> {
        // Same rules as albums; reuse via a borrowed view.
        let view = AlbumView(&self.editions);
        view.alternates()
    }
}

struct AlbumView<'a>(&'a [Edition]);

impl<'a> AlbumView<'a> {
    fn alternates(&self) -> Vec<&'a Source> {
        let Some(best) = self
            .0
            .iter()
            .flat_map(|e| &e.sources)
            .min_by(|a, b| a.availability_cmp(b))
        else {
            return Vec::new();
        };
        let (mut same, mut other) = (Vec::new(), Vec::new());
        for e in self.0 {
            let is_best = e.sources.iter().any(|s| std::ptr::eq(s, best));
            for s in &e.sources {
                if s.username != best.username {
                    if is_best { &mut same } else { &mut other }.push(s);
                }
            }
        }
        same.sort_by(|a: &&Source, b: &&Source| a.availability_cmp(b));
        other.sort_by(|a: &&Source, b: &&Source| a.availability_cmp(b));
        same.extend(other);
        same
    }
}

/// Merge single-file sources of the same track ([`TrackKey::matches`]) into
/// [`Track`]s with editions. Order: relevance, source count, title.
#[must_use]
pub fn merge_tracks(query: &str, singles: Vec<Source>) -> Vec<Track> {
    merge_tracks_ctx(&mut Ctx::default(), &Query::parse(query), singles)
}

fn merge_tracks_ctx(ctx: &mut Ctx, q: &Query, singles: Vec<Source>) -> Vec<Track> {
    struct Pending {
        key: TrackKey,
        relevance: u32,
        sources: Vec<(Source, (String, String))>,
    }
    let mut buckets: FxMap<String, Vec<Pending>> = FxMap::default();
    let mut untitled: Vec<Pending> = Vec::new();
    for src in singles {
        let Some(file) = src.files.first() else {
            continue;
        };
        let key = TrackKey {
            title: ctx.file(&file.path).title.clone(),
            duration_secs: file.attrs.duration_secs,
        };
        let rel = ctx.relevance(q, &file.path);
        let fq = format_quality(file);
        let bucket = if key.title.is_empty() {
            &mut untitled
        } else {
            buckets.entry(key.title.clone()).or_default()
        };
        match bucket.iter_mut().find(|p| p.key.matches(&key)) {
            Some(p) => {
                p.relevance = p.relevance.max(rel);
                p.key.duration_secs = p.key.duration_secs.or(key.duration_secs);
                p.sources.push((src, fq));
            }
            None => bucket.push(Pending {
                key,
                relevance: rel,
                sources: vec![(src, fq)],
            }),
        }
    }
    let mut tracks: Vec<Track> = buckets
        .into_values()
        .flatten()
        .chain(untitled)
        .map(|p| {
            let mut editions: Vec<Edition> = Vec::new();
            for (src, (format, quality)) in p.sources {
                match editions
                    .iter_mut()
                    .find(|e| e.format == format && e.quality == quality)
                {
                    Some(e) => e.sources.push(src),
                    None => editions.push(Edition {
                        format,
                        quality,
                        sources: vec![src],
                    }),
                }
            }
            for e in &mut editions {
                e.sources.sort_by(|a, b| a.availability_cmp(b));
            }
            editions.sort_by(|a, b| {
                b.sources
                    .len()
                    .cmp(&a.sources.len())
                    .then_with(|| a.sources[0].availability_cmp(&b.sources[0]))
                    .then_with(|| a.format.cmp(&b.format))
                    .then_with(|| a.quality.cmp(&b.quality))
            });
            let title = editions
                .iter()
                .flat_map(|e| &e.sources)
                .min_by(|a, b| a.availability_cmp(b))
                .and_then(|s| s.files.first())
                .map(|f| remote_basename(&f.path).to_owned())
                .unwrap_or_default();
            Track {
                key: p.key,
                title,
                relevance: p.relevance,
                editions,
            }
        })
        .collect();
    tracks.sort_by(|a, b| {
        b.relevance
            .cmp(&a.relevance)
            .then(b.source_count().cmp(&a.source_count()))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.key.title.cmp(&b.key.title))
    });
    tracks
}

/// Search results grouped for display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grouped {
    /// Folders with two or more matching audio files, merged across users.
    pub albums: Vec<Album>,
    /// Folders with a single matching audio file, merged across users.
    pub tracks: Vec<Track>,
}

/// Group search hits: [`folder_groups`], then folders with ≥ 2 matching audio
/// files → [`merge_sources`], single files → [`merge_tracks`]. Both lists are
/// sorted by relevance to `query`.
#[must_use]
pub fn group_hits(query: &str, hits: &[SearchHit]) -> Grouped {
    let mut ctx = Ctx::default();
    let q = Query::parse(query);
    let (albums, singles): (Vec<Source>, Vec<Source>) = folder_groups_ctx(&mut ctx, hits)
        .into_iter()
        .partition(|s| s.files.len() >= 2);
    Grouped {
        albums: merge_sources_ctx(&mut ctx, &q, albums),
        tracks: merge_tracks_ctx(&mut ctx, &q, singles),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioAttrs;

    fn file(user: &str, path: &str, dur: Option<u32>) -> RemoteFile {
        RemoteFile {
            username: user.into(),
            path: path.into(),
            size: 1000,
            attrs: AudioAttrs {
                duration_secs: dur,
                ..AudioAttrs::default()
            },
        }
    }

    fn hit(user: &str, path: &str, dur: Option<u32>, free: bool, speed: u32) -> SearchHit {
        SearchHit {
            file: file(user, path, dur),
            free_slot: free,
            speed,
        }
    }

    // --- track_title: messy real-world names -------------------------------------

    #[test]
    fn track_titles_normalize_messy_names() {
        let cases: &[(&str, &str)] = &[
            ("@@abc\\Music\\Aphex Twin\\SAW\\01 Xtal.flac", "xtal"),
            ("x\\01. Xtal.mp3", "xtal"),
            ("x\\01 - Xtal.flac", "xtal"),
            ("x\\1-01 Xtal.flac", "xtal"),
            ("x\\101 - Xtal.flac", "xtal"),
            ("x\\01_xtal.mp3", "xtal"),
            ("x\\A1 - Xtal.flac", "xtal"),
            ("x\\CD1-03 Xtal.flac", "xtal"),
            ("x\\01 - Aphex Twin - Xtal.flac", "xtal"),
            (
                "x\\Aphex Twin - Selected Ambient Works - 01 - Xtal.mp3",
                "xtal",
            ),
            ("x\\01 Xtal (Remastered).flac", "xtal"),
            ("x\\01 Xtal (2008 Remaster).flac", "xtal"),
            ("x\\01 Xtal - Remastered 2011.flac", "xtal"),
            ("x\\01 Xtal [FLAC 24bit].flac", "xtal"),
            ("x\\01 XTAL.FLAC", "xtal"),
            ("x\\03 Björk - Jóga.flac", "joga"),
            ("x\\03 Bjork - Joga.mp3", "joga"),
            ("x\\05 Don't Stop Me Now.mp3", "don t stop me now"),
            ("x\\05 Dont Stop Me Now.flac", "dont stop me now"),
            ("x\\02 Song (Live).flac", "song live"),
            ("x\\02 Song - Live.flac", "song live"),
            ("x\\02 Song (feat. Someone).flac", "song feat someone"),
            ("x\\1999.flac", "1999"),
            ("x\\08 - 1999.flac", "1999"),
            ("x\\99 Luftballons.mp3", "luftballons"),
            ("x\\07 2Pac - Changes.mp3", "changes"),
            ("x\\Track 05.flac", "track 05"),
            ("x\\01 Xtal [Explicit].m4a", "xtal"),
        ];
        let mut bad = Vec::new();
        for (path, want) in cases {
            let got = track_title(path);
            if got != *want {
                bad.push(format!("{path:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn disc_folders_fold_into_album() {
        for d in [
            "CD1",
            "cd 2",
            "Disc 1",
            "disc02",
            "Disk 3 - Bonus",
            "CD1 (Remastered)",
        ] {
            assert!(is_disc_folder(d), "{d}");
        }
        for d in [
            "CDs",
            "Discography",
            "Disco Inferno",
            "Selected Ambient Works",
            "cd",
        ] {
            assert!(!is_disc_folder(d), "{d}");
        }
        assert_eq!(album_folder("u\\Album\\CD2\\01 a.flac"), "u\\Album");
        assert_eq!(album_folder("u\\Album\\01 a.flac"), "u\\Album");
        assert_eq!(album_folder("CD1\\01 a.flac"), "CD1");
    }

    // --- relevance -----------------------------------------------------------------

    #[test]
    fn filename_beats_album_folder_beats_parents() {
        let q = "xtal";
        let in_file = relevance(q, "m\\Aphex Twin\\SAW 85-92\\01 Xtal.flac");
        let in_album = relevance(q, "m\\Aphex Twin\\Xtal EP\\01 Something.flac");
        let deeper = relevance(q, "m\\Xtal Collection\\Album\\01 Something.flac");
        let none = relevance(q, "m\\Other\\Album\\01 Something.flac");
        assert!(
            in_file > in_album && in_album > deeper && deeper > none,
            "{in_file} {in_album} {deeper} {none}"
        );
        assert_eq!(none, 0);
    }

    #[test]
    fn whole_word_beats_prefix() {
        let whole = relevance("love", "m\\a\\01 Love.mp3");
        let prefix = relevance("love", "m\\a\\01 Lovely.mp3");
        assert!(whole > prefix && prefix > 0);
    }

    #[test]
    fn artist_and_title_both_present_wins() {
        let q = "aphex twin xtal";
        let both = relevance(q, "m\\Aphex Twin\\SAW 85-92\\01 Xtal.flac");
        let title_only = relevance(q, "m\\Various\\Chill\\05 Xtal.flac");
        let artist_only = relevance(q, "m\\Aphex Twin\\SAW 85-92\\02 Tha.flac");
        assert!(
            both > title_only && both > artist_only,
            "{both} {title_only} {artist_only}"
        );
        // Artist in the file name too is at least as good.
        let in_name = relevance(q, "m\\misc\\Aphex Twin - Xtal.mp3");
        assert!(in_name >= title_only);
    }

    #[test]
    fn noise_paths_are_penalised() {
        let q = "xtal";
        let good = relevance(q, "m\\Aphex Twin\\SAW\\01 Xtal.flac");
        for noisy in [
            "m\\incomplete\\SAW\\01 Xtal.flac",
            "m\\Temp\\01 Xtal.flac",
            "m\\Aphex Twin\\SAW\\Samples\\01 Xtal.flac",
            "m\\previews\\Xtal.mp3",
            "m\\Aphex Twin\\SAW\\01 Xtal (sample).mp3",
        ] {
            let s = relevance(q, noisy);
            assert!(s > 0 && s * 2 < good, "{noisy}: {s} vs {good}");
        }
    }

    #[test]
    fn relevance_is_format_neutral() {
        let q = "aphex twin xtal";
        let a = relevance(q, "m\\Aphex Twin\\SAW\\01 Xtal.flac");
        let b = relevance(q, "m\\Aphex Twin\\SAW\\01 Xtal.mp3");
        let c = relevance(q, "m\\Aphex Twin\\SAW\\01 Xtal.ogg");
        assert!(a == b && b == c);
    }

    #[test]
    fn relevance_operators_case_and_accents() {
        assert_eq!(relevance("xtal -live", "m\\a\\Xtal (Live).flac"), 0);
        assert!(relevance("xtal -live", "m\\a\\Xtal.flac") > 0);
        assert!(relevance("*phex", "m\\Aphex Twin\\a\\01 x.flac") > 0);
        assert_eq!(
            relevance("bjork joga", "m\\Björk\\Homogenic\\03 Jóga.flac"),
            relevance("BJÖRK JÓGA", "m\\Björk\\Homogenic\\03 Jóga.flac")
        );
        assert!(relevance("bjork joga", "m\\Björk\\Homogenic\\03 Jóga.flac") > 200);
        assert_eq!(relevance("", "m\\a.flac"), 0);
        // Query order in the file name earns a little.
        assert!(
            relevance("stop me now", "m\\a\\Don't Stop Me Now.mp3")
                > relevance("stop me now", "m\\a\\Now Me Stop.mp3")
        );
    }

    // --- albums --------------------------------------------------------------------

    fn album(user: &str, root: &str, ext: &str, names: &[(&str, u32)]) -> Vec<SearchHit> {
        names
            .iter()
            .map(|(n, d)| hit(user, &format!("{root}\\{n}.{ext}"), Some(*d), false, 100))
            .collect()
    }

    const SAW: &[(&str, u32)] = &[("01 Xtal", 291), ("02 Tha", 545), ("03 Pulsewidth", 227)];

    #[test]
    fn same_album_across_users_formats_and_naming() {
        let mut hits = Vec::new();
        hits.extend(album(
            "alice",
            "@@a\\Music\\Aphex Twin\\1992 - Selected Ambient Works 85-92",
            "flac",
            SAW,
        ));
        hits.extend(album(
            "bob",
            "@@b\\mp3\\Aphex Twin - SAW 85-92 [320]",
            "mp3",
            &[
                ("Aphex Twin - 01 - Xtal", 292),
                ("Aphex Twin - 02 - Tha", 544),
                ("Aphex Twin - 03 - Pulsewidth", 227),
            ],
        ));
        hits.extend(album(
            "carol",
            "@@c\\Selected Ambient Works (Remastered)\\CD1",
            "flac",
            &[
                ("1-01 Xtal (Remastered)", 290),
                ("1-02 Tha (Remastered)", 546),
                ("1-03 PULSEWIDTH", 228),
            ],
        ));
        // Different album that shares a title: not merged.
        hits.extend(album(
            "dave",
            "@@d\\Other\\Live",
            "flac",
            &[("01 Xtal", 400), ("02 Tha", 600), ("03 Pulsewidth", 200)],
        ));
        let g = group_hits("aphex twin", &hits);
        assert!(g.tracks.is_empty());
        assert_eq!(
            g.albums.len(),
            2,
            "{:#?}",
            g.albums.iter().map(|a| &a.title).collect::<Vec<_>>()
        );
        let saw = g
            .albums
            .iter()
            .find(|a| a.source_count() == 3)
            .expect("merged album");
        assert_eq!(saw.key.track_count(), 3);
        let formats: Vec<_> = saw.editions.iter().map(|e| e.format.as_str()).collect();
        assert_eq!(formats.len(), 2);
        assert!(formats.contains(&"FLAC") && formats.contains(&"MP3"));
        // FLAC edition has 2 sources (alice + carol, disc folder folded).
        let flac = saw.editions.iter().find(|e| e.format == "FLAC").unwrap();
        assert_eq!(flac.sources.len(), 2);
        assert!(
            flac.sources
                .iter()
                .any(|s| s.folder == "@@c\\Selected Ambient Works (Remastered)")
        );
    }

    #[test]
    fn sources_ordered_by_availability_best_and_alternates() {
        let mut hits = Vec::new();
        for (user, free, speed) in [
            ("slow", true, 10),
            ("busy", false, 9999),
            ("fast", true, 500),
            ("mid", true, 100),
        ] {
            for (n, d) in SAW {
                hits.push(hit(
                    user,
                    &format!("@@{user}\\SAW\\{n}.flac"),
                    Some(*d),
                    free,
                    speed,
                ));
            }
        }
        for (n, d) in SAW {
            hits.push(hit(
                "mp3er",
                &format!("@@m\\SAW\\{n}.mp3"),
                Some(*d),
                true,
                100_000,
            ));
        }
        let g = group_hits("xtal", &hits);
        let a = &g.albums[0];
        assert_eq!(a.source_count(), 5);
        // Editions: most sources first (format-neutral), not "FLAC first".
        assert_eq!(a.editions[0].format, "FLAC");
        let users: Vec<_> = a.editions[0]
            .sources
            .iter()
            .map(|s| s.username.as_str())
            .collect();
        assert_eq!(users, ["fast", "mid", "slow", "busy"]);
        // Best overall: the fastest free source, whatever its format.
        assert_eq!(a.best_source().unwrap().username, "mp3er");
        assert_eq!(a.best_edition().unwrap().format, "MP3");
        let alt: Vec<_> = a.alternates().iter().map(|s| s.username.as_str()).collect();
        assert_eq!(alt, ["fast", "mid", "slow", "busy"]);
    }

    #[test]
    fn album_key_ignores_order_extension_and_non_audio() {
        let a = [
            file("u", "x\\02 Tha.flac", Some(545)),
            file("u", "x\\01 Xtal.flac", Some(291)),
            file("u", "x\\cover.jpg", None),
            file("u", "x\\album.cue", None),
        ];
        let b = [
            file("v", "y\\Xtal.mp3", Some(292)),
            file("v", "y\\Tha.mp3", None),
        ];
        assert!(album_key(&a).matches(&album_key(&b)));
        assert_eq!(album_key(&a).track_count(), 2);
        let c = [
            file("v", "y\\Xtal.mp3", Some(320)),
            file("v", "y\\Tha.mp3", Some(545)),
        ];
        assert!(
            !album_key(&a).matches(&album_key(&c)),
            "durations differ by 29 s"
        );
        let d = [file("v", "y\\Xtal.mp3", Some(291))];
        assert!(
            !album_key(&a).matches(&album_key(&d)),
            "track count differs"
        );
    }

    #[test]
    fn edition_quality_labels_split_editions() {
        let mut hits = Vec::new();
        for (user, br, vbr) in [("a", 320, false), ("b", 320, false), ("c", 245, true)] {
            for (n, d) in SAW {
                let mut h = hit(user, &format!("@@{user}\\SAW\\{n}.mp3"), Some(*d), true, 1);
                h.file.attrs.bitrate_kbps = Some(br);
                h.file.attrs.vbr = Some(vbr);
                hits.push(h);
            }
        }
        for (n, d) in SAW {
            let mut h = hit("hi", &format!("@@h\\SAW\\{n}.flac"), Some(*d), true, 1);
            h.file.attrs.bit_depth = Some(24);
            h.file.attrs.sample_rate_hz = Some(96_000);
            hits.push(h);
        }
        let g = group_hits("", &hits);
        assert_eq!(g.albums.len(), 1);
        let labels: Vec<_> = g.albums[0]
            .editions
            .iter()
            .map(|e| format!("{} {}", e.format, e.quality))
            .collect();
        // Most sources first; single-source editions by availability (equal here,
        // then by user name: "c" < "hi").
        assert_eq!(labels, ["MP3 320 kbps", "MP3 VBR", "FLAC 24-bit 96 kHz"]);
    }

    #[test]
    fn albums_sorted_by_relevance_then_sources() {
        let mut hits = Vec::new();
        hits.extend(album("a", "@@a\\Aphex Twin\\SAW", "flac", SAW));
        hits.extend(album(
            "b",
            "@@b\\Compilations\\Chill Out",
            "flac",
            &[("05 Xtal", 291), ("06 Other", 100)],
        ));
        hits.extend(album(
            "c",
            "@@c\\Compilations\\Chill Out",
            "mp3",
            &[("05 Xtal", 291), ("06 Other", 100)],
        ));
        let g = group_hits("aphex twin xtal", &hits);
        assert_eq!(g.albums.len(), 2);
        assert_eq!(g.albums[0].title, "SAW");
        assert_eq!(g.albums[1].source_count(), 2);
    }

    // --- tracks --------------------------------------------------------------------

    #[test]
    fn same_track_across_users() {
        let hits = vec![
            hit(
                "a",
                "@@a\\singles\\Aphex Twin - Xtal.flac",
                Some(291),
                true,
                50,
            ),
            hit("b", "@@b\\dl\\01 Xtal.mp3", Some(292), true, 500),
            hit("c", "@@c\\random\\xtal (remastered).mp3", None, false, 900),
            hit("d", "@@d\\x\\Xtal.mp3", Some(400), true, 10), // different recording
            hit("e", "@@e\\x\\Xtal (Live).mp3", Some(291), true, 10), // different take
        ];
        let g = group_hits("xtal", &hits);
        assert!(g.albums.is_empty());
        assert_eq!(
            g.tracks.len(),
            3,
            "{:#?}",
            g.tracks
                .iter()
                .map(|t| (&t.key, t.source_count()))
                .collect::<Vec<_>>()
        );
        let t = g.tracks.iter().find(|t| t.source_count() == 3).unwrap();
        assert_eq!(t.key.title, "xtal");
        assert_eq!(t.best_source().unwrap().username, "b");
        let alt: Vec<_> = t.alternates().iter().map(|s| s.username.as_str()).collect();
        assert_eq!(alt, ["c", "a"], "same edition (MP3) first, then FLAC");
        assert!(track_key(&hits[0].file).matches(&track_key(&hits[1].file)));
        assert!(!track_key(&hits[0].file).matches(&track_key(&hits[3].file)));
    }

    #[test]
    fn non_audio_hits_are_ignored_by_grouping() {
        let hits = vec![
            hit("a", "@@a\\Album\\cover.jpg", None, true, 1),
            hit("a", "@@a\\Album\\01 Xtal.flac", Some(291), true, 1),
            hit("a", "@@a\\Album\\info.nfo", None, true, 1),
        ];
        let g = group_hits("xtal", &hits);
        assert!(g.albums.is_empty());
        assert_eq!(g.tracks.len(), 1);
    }

    // --- performance ---------------------------------------------------------------

    #[test]
    fn groups_20k_hits_fast() {
        let mut hits = Vec::with_capacity(20_000);
        let titles = [
            "Intro",
            "Xtal",
            "Tha",
            "Pulsewidth",
            "Ageispolis",
            "I",
            "Green Calx",
            "Heliosphan",
            "We Are The Music Makers",
            "Schottkey 7th Path",
        ];
        let exts = ["flac", "mp3", "ogg", "m4a"];
        let mut n = 0;
        'outer: for user in 0..400 {
            for album in 0..5 {
                for (t, title) in titles.iter().enumerate() {
                    let ext = exts[(user + album) % exts.len()];
                    hits.push(hit(
                        &format!("user{user}"),
                        &format!("@@u{user}\\Music\\Artist {}\\{} - Album {album} (Remastered)\\CD1\\{:02} - Artist - {title}.{ext}", album % 7, 1990 + album, t + 1),
                        Some(100 + (t as u32) * 30 + (album as u32) * 10),
                        user % 3 == 0,
                        (user * 37 % 1000) as u32,
                    ));
                    n += 1;
                    if n == 20_000 {
                        break 'outer;
                    }
                }
            }
        }
        assert_eq!(hits.len(), 20_000);
        // A regression guard, not a benchmark: warm up, take the median of 5 runs
        // (robust to a busy machine), and keep the budgets generous. Release is what
        // users run; unoptimized builds (~15× slower) only guard against quadratic work.
        let _ = group_hits("artist album", &hits[..1000]);
        let mut times = Vec::new();
        let mut g = None;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            g = Some(group_hits("artist album xtal", &hits));
            times.push(t.elapsed());
        }
        let g = g.unwrap();
        times.sort();
        let el = times[times.len() / 2];
        println!(
            "[group] 20000 hits → {} albums ({} sources), {} tracks, median {:.1} ms of {times:?}",
            g.albums.len(),
            g.albums.iter().map(Album::source_count).sum::<usize>(),
            g.tracks.len(),
            el.as_secs_f64() * 1000.0
        );
        assert_eq!(g.albums.len(), 5);
        assert_eq!(
            g.albums.iter().map(Album::source_count).sum::<usize>(),
            2000
        );
        let limit = if cfg!(debug_assertions) { 5000 } else { 200 };
        assert!(el.as_millis() < limit, "median {el:?} (limit {limit} ms)");
    }
}
