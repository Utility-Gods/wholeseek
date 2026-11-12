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
    /// A search tab's selected folder group (user, folder): what the details pane shows.
    pub group: Option<(String, String)>,
}

pub enum TabKind {
    Search(SearchTab),
    User(UserTab),
}

pub struct SearchTab {
    pub edit: TextEdit,
    pub search: Option<SearchView>,
    pub filters: Filters,
    /// "Filter results": narrows the received results locally, per tab.
    pub find: Box<TextEdit>,
}

impl SearchTab {
    pub fn new(filters: Filters) -> Self {
        Self {
            edit: TextEdit::new(""),
            search: None,
            filters,
            find: Box::new(TextEdit::new("")),
        }
    }

    /// Whether the filter box holds any words.
    pub fn filtering(&self) -> bool {
        !TextQuery::parse(&self.find.text).is_empty()
    }

    /// Drop the details pane's folder when nothing of it is listed any more.
    pub fn keep_group_visible(&self, group: &mut Option<(String, String)>) {
        let (Some((u, d)), Some(v)) = (group.as_ref(), self.search.as_ref()) else {
            return;
        };
        let f = self.shown_filters();
        let listed = (0..v.hits.len()).any(|i| {
            let h = &v.hits[i].file;
            &h.username == u && h.folder() == d && v.shows(i, &f)
        });
        if !listed {
            *group = None;
        }
    }

    /// The chips plus the filter box: what the results list shows.
    pub fn shown_filters(&self) -> Filters {
        let mut f = self.filters.clone();
        f.text = TextQuery::parse(&self.find.text);
        f
    }
}

pub struct SearchView {
    pub id: SearchId,
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub finished: bool,
    /// Per hit, computed once as results arrive ([`SearchView::reindex`]) so a keystroke in
    /// the filter box only compares strings.
    pub keys: Vec<RowIndex>,
    /// The last filter words and which hits they match: the list, its count and the chips
    /// all ask within one frame, so a keystroke matches each row once.
    pub matched: std::cell::RefCell<Option<(TextQuery, std::rc::Rc<[bool]>)>>,
}

/// What filtering and grouping need of a hit, precomputed.
pub struct RowIndex {
    /// [`search_key`]: the normalized user and path.
    pub key: String,
    pub format: Option<&'static str>,
    /// Position of its file name in natural order among all hits (sorting within folders).
    pub name_order: u32,
}

/// Lowercased, accent-folded words of `s`, punctuation as separators and apostrophes
/// dropped, each preceded by a space: " hollow lights coastline 2019 01 northern glass flac".
/// A word-prefix test is then `key.contains(" " + word)`.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 1);
    let mut gap = true;
    for c in s.chars() {
        if matches!(c, '\'' | '’' | '`') {
            continue;
        }
        for l in c.to_lowercase() {
            let folded = fold(l);
            let word = match folded {
                Some(f) => {
                    if gap {
                        out.push(' ');
                        gap = false;
                    }
                    out.push_str(f);
                    true
                }
                None if l.is_alphanumeric() => {
                    if gap {
                        out.push(' ');
                        gap = false;
                    }
                    out.push(l);
                    true
                }
                None => false,
            };
            if !word {
                gap = true;
            }
        }
    }
    out
}

/// The plain letters of a lowercase accented Latin letter.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ģ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ķ' => "k",
        'ł' | 'ľ' | 'ĺ' | 'ļ' => "l",
        'ñ' | 'ń' | 'ň' | 'ņ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' | 'ŕ' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// What the filter box matches against: the user and the full remote path.
pub fn search_key(h: &SearchHit) -> String {
    normalize(&format!("{} {}", h.file.username, h.file.path))
}

/// A parsed "Filter results" text: every word must start a word of the row's key; a
/// `-word` must not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextQuery {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl TextQuery {
    pub fn parse(text: &str) -> Self {
        let mut q = Self::default();
        for token in text.split_whitespace() {
            let (neg, body) = match token.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, token),
            };
            for w in normalize(body).split_whitespace() {
                let w = format!(" {w}");
                if neg {
                    q.exclude.push(w)
                } else {
                    q.include.push(w)
                }
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Byte ranges of `text` that the query's words match (word prefixes), to highlight.
    pub fn spans(&self, text: &str) -> Vec<(usize, usize)> {
        if self.include.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            let in_word = |c: char| c.is_alphanumeric() || matches!(c, '\'' | '’' | '`');
            if !chars[k].1.is_alphanumeric() {
                k += 1;
                continue;
            }
            // One word: its folded form, and where each original char ends in it.
            let start = chars[k].0;
            let mut folded = String::new();
            let mut ends: Vec<(usize, usize)> = Vec::new(); // (folded len, byte end)
            while k < chars.len() && in_word(chars[k].1) {
                let (b, c) = chars[k];
                folded.push_str(&normalize(&c.to_string()).replace(' ', ""));
                ends.push((folded.len(), b + c.len_utf8()));
                k += 1;
            }
            let best = self
                .include
                .iter()
                .map(|w| &w[1..])
                .filter(|w| folded.starts_with(*w))
                .map(str::len)
                .max();
            if let Some(n) = best
                && let Some(&(_, end)) = ends.iter().find(|(l, _)| *l >= n)
            {
                out.push((start, end));
            }
        }
        out
    }

    pub fn matches(&self, key: &str) -> bool {
        self.include.iter().all(|w| key.contains(w.as_str()))
            && !self.exclude.iter().any(|w| key.contains(w.as_str()))
    }
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
    /// The filter box (per tab: never carried into a new tab).
    pub text: TextQuery,
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
    /// Rebuild [`SearchView::keys`] after hits arrive or are re-ranked.
    pub fn reindex(&mut self) {
        let mut by_name: Vec<usize> = (0..self.hits.len()).collect();
        by_name.sort_by(|&a, &b| {
            natural_cmp(self.hits[a].file.basename(), self.hits[b].file.basename())
        });
        let mut order = vec![0u32; self.hits.len()];
        for (rank, i) in by_name.into_iter().enumerate() {
            order[i] = rank as u32;
        }
        self.keys = self
            .hits
            .iter()
            .zip(order)
            .map(|(h, name_order)| RowIndex {
                key: search_key(h),
                format: format_of(&h.file.path),
                name_order,
            })
            .collect();
        self.matched.take();
    }

    /// Which hits the filter words match (cached per query until the hits change).
    fn text_mask(&self, q: &TextQuery) -> Option<std::rc::Rc<[bool]>> {
        if q.is_empty() {
            return None;
        }
        if let Some((cq, m)) = &*self.matched.borrow()
            && cq == q
            && m.len() == self.hits.len()
        {
            return Some(m.clone());
        }
        let m: std::rc::Rc<[bool]> = (0..self.hits.len()).map(|i| self.text_ok(i, q)).collect();
        *self.matched.borrow_mut() = Some((q.clone(), m.clone()));
        Some(m)
    }

    /// [`Self::shows`] with the text match looked up in `mask`.
    fn shows_in(&self, i: usize, filters: &Filters, mask: Option<&[bool]>) -> bool {
        filters.accepts_except_format(&self.hits[i])
            && (filters.formats.is_empty()
                || self.format(i).is_some_and(|f| filters.formats.contains(f)))
            && mask.is_none_or(|m| m[i])
    }

    /// The index of hit `i`, when it is current (a test may build a view without one).
    fn indexed(&self, i: usize) -> Option<&RowIndex> {
        (self.keys.len() == self.hits.len()).then(|| &self.keys[i])
    }

    fn format(&self, i: usize) -> Option<&'static str> {
        match self.indexed(i) {
            Some(r) => r.format,
            None => format_of(&self.hits[i].file.path),
        }
    }

    fn text_ok(&self, i: usize, q: &TextQuery) -> bool {
        if q.is_empty() {
            return true;
        }
        match self.indexed(i) {
            Some(r) => q.matches(&r.key),
            None => q.matches(&search_key(&self.hits[i])),
        }
    }

    /// Whether hit `i` is listed under `filters` (chips, Free slot and the filter box).
    pub fn shows(&self, i: usize, filters: &Filters) -> bool {
        let mask = self.text_mask(&filters.text);
        self.shows_in(i, filters, mask.as_deref())
    }

    /// Hits that pass `filters`, grouped by user then folder, in rank order: a group sits
    /// where its best file ranks, and its folders and files keep their order.
    pub fn groups(&self, filters: &Filters) -> Vec<Group> {
        use std::collections::HashMap;
        let mut groups: Vec<Group> = Vec::new();
        let mut by_user: HashMap<&str, usize> = HashMap::new();
        let mut by_folder: HashMap<(usize, &str), usize> = HashMap::new();
        let mask = self.text_mask(&filters.text);
        for (i, h) in self.hits.iter().enumerate() {
            if !self.shows_in(i, filters, mask.as_deref()) {
                continue;
            }
            let gi = *by_user.entry(h.file.username.as_str()).or_insert_with(|| {
                groups.push(Group {
                    username: h.file.username.clone(),
                    free_slot: h.free_slot,
                    speed: h.speed,
                    folders: Vec::new(),
                    files: 0,
                });
                groups.len() - 1
            });
            let g = &mut groups[gi];
            g.files += 1;
            let folder = h.file.folder();
            let fi = *by_folder.entry((gi, folder)).or_insert_with(|| {
                g.folders.push((folder.to_string(), Vec::new()));
                g.folders.len() - 1
            });
            g.folders[fi].1.push(i);
        }
        // Ranking orders users and their folders; inside a folder, files go by name.
        let indexed = self.keys.len() == self.hits.len();
        for g in &mut groups {
            for (_, v) in &mut g.folders {
                if indexed {
                    v.sort_unstable_by_key(|&i| self.keys[i].name_order);
                } else {
                    v.sort_by(|&a, &b| {
                        natural_cmp(self.hits[a].file.basename(), self.hits[b].file.basename())
                    });
                }
            }
        }
        groups
    }

    pub fn visible(&self, filters: &Filters) -> usize {
        let mask = self.text_mask(&filters.text);
        (0..self.hits.len())
            .filter(|&i| self.shows_in(i, filters, mask.as_deref()))
            .count()
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
        let mask = self.text_mask(&filters.text);
        for (i, h) in self.hits.iter().enumerate() {
            if let Some(f) = self.format(i) {
                let ok = filters.accepts_except_format(h) && mask.as_ref().is_none_or(|m| m[i]);
                add(f, usize::from(ok));
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
            group: None,
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
