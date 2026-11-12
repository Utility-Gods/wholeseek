//! Tabs: each search and each browsed user is a tab that owns its own state, so switching
//! tabs keeps the query, results, filters, selected folder and scroll position.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use rune::TextEdit;
use wholeseek_core::{BrowseDir, RemoteFile, SearchHit, SearchId, rank};

pub type TabId = u64;

pub struct Tab {
    pub id: TabId,
    pub kind: TabKind,
    /// Main list scroll offset (results or files), saved as it scrolls and put back when the
    /// tab is shown again.
    pub scroll: Rc<Cell<f32>>,
    /// Folder tree scroll offset (user tabs).
    pub tree_scroll: Rc<Cell<f32>>,
    /// The selected file row (remote path): Enter plays it, its ⋯ stays visible.
    pub selected: Option<String>,
}

pub enum TabKind {
    Search(SearchTab),
    User(UserTab),
}

pub struct SearchTab {
    pub edit: TextEdit,
    pub search: Option<SearchView>,
    pub filters: Filters,
}

pub struct SearchView {
    pub id: SearchId,
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub finished: bool,
}

/// Natural filename order, the one order for files inside a folder (search groups, user
/// tabs, album queues): case-insensitive, digit runs compared as numbers, so "2" < "10" and
/// disc prefixes "1-02" < "1-10" < "2-01". Ties fall back to the raw strings.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
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

/// Sorts a folder's files by [`natural_cmp`] on their names.
pub fn sort_files(files: &mut [RemoteFile]) {
    files.sort_by(|a, b| natural_cmp(a.basename(), b.basename()));
}

/// The format chip a file counts under, by extension, with aliases grouped (m4a/aac →
/// "AAC/M4A", aif/aiff → "AIFF"). `None` for non-audio. Every audio extension the core
/// knows (`rank::AUDIO_EXTENSIONS`) has a group.
pub fn format_of(path: &str) -> Option<&'static str> {
    Some(match rank::extension(path).as_str() {
        "flac" => "FLAC",
        "mp3" => "MP3",
        "wav" => "WAV",
        "m4a" | "aac" => "AAC/M4A",
        "aif" | "aiff" => "AIFF",
        "ogg" => "OGG",
        "opus" => "Opus",
        "alac" => "ALAC",
        "ape" => "APE",
        "wv" => "WavPack",
        _ => return None,
    })
}

/// Element key (and menu id) of a format chip.
pub fn format_key(label: &str) -> &'static str {
    match label {
        "FLAC" => "fmt-flac",
        "MP3" => "fmt-mp3",
        "WAV" => "fmt-wav",
        "AAC/M4A" => "fmt-aac",
        "AIFF" => "fmt-aiff",
        "OGG" => "fmt-ogg",
        "Opus" => "fmt-opus",
        "ALAC" => "fmt-alac",
        "APE" => "fmt-ape",
        "WavPack" => "fmt-wv",
        _ => "fmt-other",
    }
}

/// Client-side result filters (chips). Changing one never re-runs the search.
/// No format selected (the default) means every format; several mean any of them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// Selected format chips (labels from [`format_of`]).
    pub formats: BTreeSet<&'static str>,
    pub free_slot: bool,
}

impl Filters {
    pub fn has(&self, format: &str) -> bool {
        self.formats.contains(format)
    }

    pub fn toggle(&mut self, format: &'static str) {
        if !self.formats.remove(format) {
            self.formats.insert(format);
        }
    }

    pub fn accepts(&self, h: &SearchHit) -> bool {
        self.accepts_except_format(h)
            && (self.formats.is_empty()
                || format_of(&h.file.path).is_some_and(|f| self.formats.contains(f)))
    }

    fn accepts_except_format(&self, h: &SearchHit) -> bool {
        !self.free_slot || h.free_slot
    }
}

/// One user's results: header, then folders, then files. Indices point into `hits`.
pub struct Group {
    pub username: String,
    pub free_slot: bool,
    pub speed: u32,
    pub folders: Vec<(String, Vec<usize>)>,
    pub files: usize,
}

impl SearchView {
    /// Hits that pass `filters`, grouped by user then folder, in rank order: a group sits
    /// where its best file ranks, and its folders and files keep their order.
    pub fn groups(&self, filters: &Filters) -> Vec<Group> {
        let mut groups: Vec<Group> = Vec::new();
        for (i, h) in self.hits.iter().enumerate() {
            if !filters.accepts(h) {
                continue;
            }
            let g = match groups.iter().position(|g| g.username == h.file.username) {
                Some(g) => &mut groups[g],
                None => {
                    groups.push(Group {
                        username: h.file.username.clone(),
                        free_slot: h.free_slot,
                        speed: h.speed,
                        folders: Vec::new(),
                        files: 0,
                    });
                    groups.last_mut().expect("just pushed")
                }
            };
            g.files += 1;
            let folder = h.file.folder();
            match g.folders.iter_mut().find(|(f, _)| f == folder) {
                Some((_, v)) => v.push(i),
                None => g.folders.push((folder.to_string(), vec![i])),
            }
        }
        // Ranking orders users and their folders; inside a folder, files go by name.
        for g in &mut groups {
            for (_, v) in &mut g.folders {
                v.sort_by(|&a, &b| {
                    natural_cmp(self.hits[a].file.basename(), self.hits[b].file.basename())
                });
            }
        }
        groups
    }

    pub fn visible(&self, filters: &Filters) -> usize {
        self.hits.iter().filter(|h| filters.accepts(h)).count()
    }

    /// One chip per format present in the results, with its count under the other filters
    /// (what selecting it would show), most common first (ties by name). A selected format
    /// stays listed, at 0, even when no result has it any more.
    pub fn format_counts(&self, filters: &Filters) -> Vec<(&'static str, usize)> {
        let mut n: Vec<(&'static str, usize)> = Vec::new();
        let mut add = |f: &'static str, k: usize| match n.iter_mut().find(|(g, _)| *g == f) {
            Some((_, c)) => *c += k,
            None => n.push((f, k)),
        };
        for h in &self.hits {
            if let Some(f) = format_of(&h.file.path) {
                add(f, usize::from(filters.accepts_except_format(h)));
            }
        }
        for f in &filters.formats {
            add(f, 0);
        }
        n.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        n
    }
}

pub struct UserTab {
    pub browse: BrowseView,
}

pub struct BrowseView {
    pub username: String,
    pub dirs: Option<Vec<BrowseDir>>,
    pub error: Option<String>,
    pub selected: Option<String>,
    pub expanded: BTreeSet<String>,
    /// Folder tree filter box.
    pub filter: TextEdit,
    /// A file (remote path) to mark in the selected folder (deep links from search).
    pub highlight: Option<String>,
}

impl BrowseView {
    pub fn new(username: &str) -> Self {
        Self {
            username: username.to_string(),
            dirs: None,
            error: None,
            selected: None,
            expanded: BTreeSet::new(),
            filter: TextEdit::new(""),
            highlight: None,
        }
    }

    /// Select a folder and expand its ancestors so it is visible in the tree.
    pub fn select(&mut self, path: &str) {
        let mut prefix = String::new();
        for part in path.split('\\') {
            if !prefix.is_empty() {
                self.expanded.insert(prefix.clone());
                prefix.push('\\');
            }
            prefix.push_str(part);
        }
        self.selected = Some(path.to_string());
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
    }

    /// A click on a tree row: select it; a second click (or a click on a closed folder)
    /// opens or closes it.
    pub fn press_dir(&mut self, path: &str) {
        if self.selected.as_deref() == Some(path) || !self.expanded.contains(path) {
            self.toggle(path);
        }
        self.selected = Some(path.to_string());
    }

    pub fn selected_dir(&self) -> Option<&BrowseDir> {
        let sel = self.selected.as_deref()?;
        self.dirs.as_ref()?.iter().find(|d| d.path == sel)
    }

    /// Folder tree rows to draw: (path, depth, has children, expanded), in order. With a
    /// filter, only folders whose name matches (and their ancestors) show, all open.
    pub fn visible_tree(&self) -> Vec<(String, usize, bool, bool)> {
        let mut all = BTreeSet::new();
        for d in self.dirs.iter().flatten() {
            let mut prefix = String::new();
            for part in d.path.split('\\') {
                if !prefix.is_empty() {
                    prefix.push('\\');
                }
                prefix.push_str(part);
                all.insert(prefix.clone());
            }
        }
        let needle = self.filter.text.trim().to_lowercase();
        let keep: Option<BTreeSet<String>> = (!needle.is_empty()).then(|| {
            let mut keep = BTreeSet::new();
            for p in &all {
                let name = p.rsplit('\\').next().unwrap_or(p);
                if name.to_lowercase().contains(&needle) {
                    let mut anc = p.as_str();
                    keep.insert(p.clone());
                    while let Some(i) = anc.rfind('\\') {
                        anc = &anc[..i];
                        keep.insert(anc.to_string());
                    }
                }
            }
            keep
        });
        let mut out = Vec::new();
        for p in &all {
            if keep.as_ref().is_some_and(|k| !k.contains(p)) {
                continue;
            }
            let open = |a: &str| keep.is_some() || self.expanded.contains(a);
            let mut visible = true;
            let mut anc = p.as_str();
            while let Some(i) = anc.rfind('\\') {
                anc = &anc[..i];
                if !open(anc) {
                    visible = false;
                    break;
                }
            }
            if !visible {
                continue;
            }
            let child_prefix = format!("{p}\\");
            let has_children = all
                .range(child_prefix.clone()..)
                .next()
                .is_some_and(|n| n.starts_with(&child_prefix));
            out.push((p.clone(), p.matches('\\').count(), has_children, open(p)));
        }
        out
    }
}

impl Tab {
    pub fn new(id: TabId, kind: TabKind) -> Self {
        Self {
            id,
            kind,
            scroll: Rc::default(),
            tree_scroll: Rc::default(),
            selected: None,
        }
    }

    pub fn search(&self) -> Option<&SearchTab> {
        match &self.kind {
            TabKind::Search(s) => Some(s),
            TabKind::User(_) => None,
        }
    }

    pub fn search_mut(&mut self) -> Option<&mut SearchTab> {
        match &mut self.kind {
            TabKind::Search(s) => Some(s),
            TabKind::User(_) => None,
        }
    }

    pub fn browse(&self) -> Option<&BrowseView> {
        match &self.kind {
            TabKind::User(u) => Some(&u.browse),
            TabKind::Search(_) => None,
        }
    }

    pub fn browse_mut(&mut self) -> Option<&mut BrowseView> {
        match &mut self.kind {
            TabKind::User(u) => Some(&mut u.browse),
            TabKind::Search(_) => None,
        }
    }

    pub fn username(&self) -> Option<&str> {
        self.browse().map(|b| b.username.as_str())
    }

    /// The tab's label in the strip.
    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::Search(s) => s
                .search
                .as_ref()
                .map_or_else(|| "New search".into(), |v| v.query.clone()),
            TabKind::User(u) => u.browse.username.clone(),
        }
    }

    /// Scroll viewport keys, unique per tab so a switch never reuses another tab's offset.
    pub fn list_key(&self) -> String {
        format!("list-{}", self.id)
    }

    pub fn tree_key(&self) -> String {
        format!("tree-{}", self.id)
    }
}

/// Audio by extension (rows of anything else are dimmed).
pub fn is_audio(f: &RemoteFile) -> bool {
    rank::is_audio(&f.path)
}

/// Whether the player can decode `f`: the one test for ▶, "Play" menu items, queues and
/// ▶ fallback sources (`wholeseek_audio::PLAYABLE_EXTENSIONS`; Opus, APE, WavPack, … are
/// audio but not playable).
pub fn is_playable(f: &RemoteFile) -> bool {
    wholeseek_audio::is_playable(&f.path)
}
