//! Recent searches: what was searched, newest first, for ↓ in the query box. Kept in
//! `<state dir>/wholeseek/searches.json` (a JSON list of strings); without a path (demo,
//! tests) only in memory. A missing or broken file is an empty list.

use std::path::{Path, PathBuf};

use wholeseek_core::config::APP_NAME;

/// How many searches are kept.
pub const MAX: usize = 20;

#[derive(Debug, Default)]
pub struct Recent {
    path: Option<PathBuf>,
    /// Newest first; no two equal ignoring case.
    pub items: Vec<String>,
}

/// `<state dir>/wholeseek/searches.json`, beside the session.
pub fn default_path() -> Option<PathBuf> {
    let base = dirs::state_dir().or_else(dirs::data_local_dir)?;
    Some(base.join(APP_NAME).join("searches.json"))
}

impl Recent {
    /// The list saved at `path` (none: in memory only).
    pub fn load(path: Option<PathBuf>) -> Self {
        let items = path.as_deref().map(read).unwrap_or_default();
        Self { path, items }
    }

    /// `query` searched: to the front (an earlier equal one goes), at most [`MAX`].
    pub fn add(&mut self, query: &str) {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if q.is_empty() {
            return;
        }
        self.items.retain(|i| !i.eq_ignore_ascii_case(&q));
        self.items.insert(0, q);
        self.items.truncate(MAX);
        self.save();
    }

    /// Forget one search.
    pub fn remove(&mut self, query: &str) {
        let before = self.items.len();
        self.items.retain(|i| i != query);
        if self.items.len() != before {
            self.save();
        }
    }

    /// Those that contain every word of `typed` (all of them for nothing typed), newest first.
    pub fn matching(&self, typed: &str) -> Vec<&str> {
        let words: Vec<String> = typed.split_whitespace().map(str::to_lowercase).collect();
        self.items
            .iter()
            .filter(|i| {
                let l = i.to_lowercase();
                words.iter().all(|w| l.contains(w.as_str()))
            })
            .map(String::as_str)
            .collect()
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&self.items)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            eprintln!("wholeseek: could not save recent searches: {e}");
        }
    }
}

fn read(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut items: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    items.retain(|i| !i.trim().is_empty());
    items.truncate(MAX);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let mut r = Recent::default();
        for i in 0..25 {
            r.add(&format!("q{i}"));
        }
        assert_eq!(r.items.len(), MAX);
        assert_eq!(r.items[0], "q24");
        r.add("Q20");
        assert_eq!(r.items[0], "Q20");
        assert_eq!(
            r.items
                .iter()
                .filter(|i| i.eq_ignore_ascii_case("q20"))
                .count(),
            1
        );
        r.add("  spaced   out  ");
        assert_eq!(r.items[0], "spaced out");
        r.add("   ");
        assert_eq!(r.items[0], "spaced out");
    }

    #[test]
    fn saves_and_loads_and_survives_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("wholeseek-recent-{}", std::process::id()));
        let path = dir.join("searches.json");
        let mut r = Recent::load(Some(path.clone()));
        assert!(r.items.is_empty());
        r.add("hollow lights");
        r.add("night ferry");
        r.remove("hollow lights");
        assert_eq!(Recent::load(Some(path.clone())).items, ["night ferry"]);
        std::fs::write(&path, b"{not json").unwrap();
        assert!(Recent::load(Some(path)).items.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn matching_needs_every_typed_word() {
        let mut r = Recent::default();
        r.add("hollow lights coastline");
        r.add("night ferry");
        r.add("Hollow Lights");
        assert_eq!(
            r.matching(""),
            ["Hollow Lights", "night ferry", "hollow lights coastline"]
        );
        assert_eq!(
            r.matching("hol"),
            ["Hollow Lights", "hollow lights coastline"]
        );
        assert_eq!(r.matching("lights coast"), ["hollow lights coastline"]);
    }
}
