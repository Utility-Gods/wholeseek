//! Download history: every ↓ that finished, failed or was cancelled.
//!
//! Stored as append-only JSON lines in `<Settings::data_dir>/history.jsonl`
//! (one operation per line: an entry, a removal or a clear). Loading replays
//! the lines, ignores any it cannot parse (a line cut off by a crash), keeps
//! the newest [`HISTORY_CAP`] entries and rewrites the file compactly when it
//! held anything else. ▶ plays are not history unless kept (a keep is a ↓).
//! Nothing secret is stored: user names and paths only.

use crate::model::{RemoteFile, TransferId};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// File name inside the data dir.
pub const HISTORY_FILE: &str = "history.jsonl";
/// Entries kept; older ones are dropped (and compacted away on load).
pub const HISTORY_CAP: usize = 50_000;

/// How a recorded download ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Cancelled,
}

/// One recorded download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Stable id for [`crate::Core::history_remove`].
    pub id: u64,
    /// Unix seconds.
    pub finished_at: u64,
    pub username: String,
    pub remote_path: String,
    /// Final file (done only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_path: Option<PathBuf>,
    pub size: u64,
    /// Lower-case extension (`flac`, `mp3`, …), empty if none.
    #[serde(default)]
    pub format: String,
    /// What the peer advertised (bitrate, sample rate, …), when known.
    #[serde(default)]
    pub bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub sample_rate_hz: Option<u32>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub duration_secs: Option<u32>,
    /// Remote album folder name (last folder of `remote_path`).
    #[serde(default)]
    pub album: String,
    pub outcome: Outcome,
    /// Failure reason (failed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HistoryEntry {
    /// Whether the downloaded file is still where it was saved. Checked on
    /// each call (a `stat`), so call it for the rows on screen only.
    #[must_use]
    pub fn exists_on_disk(&self) -> bool {
        self.outcome == Outcome::Done && self.local_path.as_deref().is_some_and(Path::is_file)
    }

    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.remote_path.to_lowercase().contains(needle)
            || self.username.to_lowercase().contains(needle)
            || self
                .local_path
                .as_deref()
                .is_some_and(|p| p.to_string_lossy().to_lowercase().contains(needle))
    }
}

/// Arguments of [`crate::Core::history`]. `limit == 0` means no limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Case-insensitive substring of remote path, user name or local path;
    /// empty = everything.
    pub text: String,
    pub limit: usize,
    pub offset: usize,
}

/// `(username, remote_path)` of downloads that completed.
pub type DownloadedKeys = Arc<HashSet<(String, String)>>;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Line {
    Add(HistoryEntry),
    Remove(u64),
    Clear(bool),
}

/// The history store. Shared by `Core` (reads, remove, clear) and the worker
/// (appends) behind a mutex.
pub struct History {
    path: Option<PathBuf>,
    file: Option<File>,
    /// Oldest first.
    entries: Vec<HistoryEntry>,
    keys: DownloadedKeys,
    next_id: u64,
    /// Lines appended since load / last compaction.
    appended: usize,
    /// The newest entry recorded for a transfer in this run, so a retried
    /// download replaces its earlier failure instead of adding a second row.
    by_transfer: HashMap<TransferId, u64>,
}

impl History {
    /// In memory only (nothing persisted).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            file: None,
            entries: Vec::new(),
            keys: DownloadedKeys::default(),
            next_id: 1,
            appended: 0,
            by_transfer: HashMap::new(),
        }
    }

    /// Load `dir/history.jsonl` (missing = empty) and open it for appending.
    ///
    /// # Errors
    /// I/O errors creating `dir`, reading, compacting or opening the file.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let path = dir.join(HISTORY_FILE);
        let mut h = Self::in_memory();
        let mut lines = 0usize;
        let mut dirty = false;
        match File::open(&path) {
            Ok(f) => {
                for line in BufReader::new(f).split(b'\n') {
                    let line = line?;
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    lines += 1;
                    match serde_json::from_slice::<Line>(&line) {
                        Ok(Line::Add(e)) => {
                            h.next_id = h.next_id.max(e.id + 1);
                            h.entries.push(e);
                        }
                        Ok(Line::Remove(id)) => h.entries.retain(|e| e.id != id),
                        Ok(Line::Clear(_)) => h.entries.clear(),
                        Err(_) => dirty = true, // truncated/corrupt line: skip
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if h.entries.len() > HISTORY_CAP {
            let extra = h.entries.len() - HISTORY_CAP;
            h.entries.drain(..extra);
        }
        h.rebuild_keys();
        fs::create_dir_all(dir)?;
        h.path = Some(path);
        if dirty || lines != h.entries.len() {
            h.compact()?;
        } else {
            h.reopen()?;
        }
        Ok(h)
    }

    fn reopen(&mut self) -> io::Result<()> {
        if let Some(p) = &self.path {
            self.file = Some(OpenOptions::new().create(true).append(true).open(p)?);
        }
        Ok(())
    }

    /// Rewrite the file with exactly the current entries (temp + rename).
    fn compact(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        self.file = None;
        let tmp = path.with_extension("jsonl.tmp");
        {
            let mut w = io::BufWriter::new(File::create(&tmp)?);
            for e in &self.entries {
                serde_json::to_writer(&mut w, &Line::Add(e.clone()))?;
                w.write_all(b"\n")?;
            }
            w.into_inner()
                .map_err(io::IntoInnerError::into_error)?
                .sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        self.appended = 0;
        self.reopen()
    }

    fn append(&mut self, line: &Line) -> io::Result<()> {
        let Some(f) = self.file.as_mut() else {
            return Ok(());
        };
        let mut buf = serde_json::to_vec(line)?;
        buf.push(b'\n');
        f.write_all(&buf)?; // one write per line: a crash leaves at most one partial line
        f.flush()?;
        self.appended += 1;
        if self.appended > HISTORY_CAP {
            self.compact()?;
        }
        Ok(())
    }

    fn rebuild_keys(&mut self) {
        self.keys = Arc::new(
            self.entries
                .iter()
                .filter(|e| e.outcome == Outcome::Done)
                .map(|e| (e.username.clone(), e.remote_path.clone()))
                .collect(),
        );
    }

    /// Record how download `transfer` ended. A later record for the same
    /// transfer (a retry) replaces the earlier one.
    ///
    /// # Errors
    /// I/O errors appending to the file (the entry is kept in memory).
    pub fn record(
        &mut self,
        transfer: TransferId,
        file: &RemoteFile,
        outcome: Outcome,
        local_path: Option<PathBuf>,
        reason: Option<String>,
    ) -> io::Result<()> {
        let (username, remote_path, attrs) = (&file.username, &file.path, file.attrs);
        let mut res = Ok(());
        if let Some(old) = self.by_transfer.remove(&transfer) {
            res = self.remove(old).map(|_| ());
        }
        let id = self.next_id;
        self.next_id += 1;
        let entry = HistoryEntry {
            id,
            finished_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            username: username.clone(),
            remote_path: remote_path.clone(),
            local_path,
            size: file.size,
            format: crate::rank::extension(remote_path),
            bitrate_kbps: attrs.bitrate_kbps,
            sample_rate_hz: attrs.sample_rate_hz,
            bit_depth: attrs.bit_depth,
            duration_secs: attrs.duration_secs,
            album: paths::remote_basename(paths::remote_parent(remote_path)).to_string(),
            outcome,
            reason,
        };
        self.by_transfer.insert(transfer, id);
        if outcome == Outcome::Done {
            let mut keys = (*self.keys).clone();
            keys.insert((entry.username.clone(), entry.remote_path.clone()));
            self.keys = Arc::new(keys);
        }
        let line = Line::Add(entry.clone());
        self.entries.push(entry);
        if self.entries.len() > HISTORY_CAP {
            self.entries.remove(0);
            self.rebuild_keys();
        }
        res.and(self.append(&line))
    }

    /// Newest first, filtered and paged.
    #[must_use]
    pub fn query(&self, q: &HistoryQuery) -> Vec<HistoryEntry> {
        let needle = q.text.trim().to_lowercase();
        let it = self
            .entries
            .iter()
            .rev()
            .filter(|e| e.matches(&needle))
            .skip(q.offset);
        if q.limit == 0 {
            it.cloned().collect()
        } else {
            it.take(q.limit).cloned().collect()
        }
    }

    /// Remove one entry. Returns whether it existed.
    ///
    /// # Errors
    /// I/O errors appending the removal.
    pub fn remove(&mut self, id: u64) -> io::Result<bool> {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() == before {
            return Ok(false);
        }
        self.rebuild_keys();
        self.append(&Line::Remove(id))?;
        Ok(true)
    }

    /// Forget everything (and shrink the file).
    ///
    /// # Errors
    /// I/O errors rewriting the file.
    pub fn clear(&mut self) -> io::Result<()> {
        self.entries.clear();
        self.by_transfer.clear();
        self.rebuild_keys();
        self.compact()
    }

    #[must_use]
    pub fn was_downloaded(&self, username: &str, remote_path: &str) -> bool {
        self.keys
            .contains(&(username.to_string(), remote_path.to_string()))
    }

    #[must_use]
    pub fn downloaded_keys(&self) -> DownloadedKeys {
        self.keys.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(h: &mut History, t: u64, user: &str, path: &str, outcome: Outcome) {
        let local = (outcome == Outcome::Done).then(|| PathBuf::from(format!("/dl/{t}.flac")));
        let file = RemoteFile {
            username: user.into(),
            path: path.into(),
            size: 10,
            attrs: crate::model::AudioAttrs {
                bitrate_kbps: Some(320),
                ..Default::default()
            },
        };
        h.record(
            TransferId(t),
            &file,
            outcome,
            local,
            (outcome == Outcome::Failed).then(|| "peer offline".into()),
        )
        .unwrap();
    }

    fn lines(dir: &Path) -> usize {
        fs::read_to_string(dir.join(HISTORY_FILE))
            .unwrap()
            .lines()
            .count()
    }

    #[test]
    fn persists_and_reloads_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = History::open(tmp.path()).unwrap();
        rec(&mut h, 1, "alice", r"Music\Album\01.flac", Outcome::Done);
        rec(&mut h, 2, "bob", r"Music\B\x.mp3", Outcome::Failed);
        rec(&mut h, 3, "carol", r"y.ogg", Outcome::Cancelled);
        drop(h);
        let h = History::open(tmp.path()).unwrap();
        let all = h.query(&HistoryQuery::default());
        let users: Vec<&str> = all.iter().map(|e| e.username.as_str()).collect();
        assert_eq!(users, ["carol", "bob", "alice"]);
        let a = &all[2];
        assert_eq!(
            (
                a.format.as_str(),
                a.album.as_str(),
                a.bitrate_kbps,
                a.outcome
            ),
            ("flac", "Album", Some(320), Outcome::Done)
        );
        assert_eq!(all[1].reason.as_deref(), Some("peer offline"));
        assert!(all[1].local_path.is_none());
        assert!(h.was_downloaded("alice", r"Music\Album\01.flac"));
        assert!(
            !h.was_downloaded("bob", r"Music\B\x.mp3"),
            "failed is not downloaded"
        );
        assert_eq!(h.downloaded_keys().len(), 1);
        let text = fs::read_to_string(tmp.path().join(HISTORY_FILE)).unwrap();
        assert!(!text.to_lowercase().contains("password"));
    }

    #[test]
    fn truncated_last_line_is_ignored_and_repaired() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = History::open(tmp.path()).unwrap();
        rec(&mut h, 1, "alice", "a.flac", Outcome::Done);
        rec(&mut h, 2, "alice", "b.flac", Outcome::Done);
        drop(h);
        let p = tmp.path().join(HISTORY_FILE);
        let mut text = fs::read_to_string(&p).unwrap();
        text.truncate(text.len() - 20); // crash mid-write of the 2nd line
        fs::write(&p, &text).unwrap();
        let mut h = History::open(tmp.path()).unwrap();
        assert_eq!(h.len(), 1);
        assert!(h.was_downloaded("alice", "a.flac"));
        // The partial line is gone, so the next append starts on a fresh line.
        rec(&mut h, 3, "alice", "c.flac", Outcome::Done);
        drop(h);
        let h = History::open(tmp.path()).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(lines(tmp.path()), 2);
    }

    #[test]
    fn remove_and_clear_persist_and_compact() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = History::open(tmp.path()).unwrap();
        rec(&mut h, 1, "alice", "a.flac", Outcome::Done);
        rec(&mut h, 2, "alice", "b.flac", Outcome::Done);
        let id = h.query(&HistoryQuery::default())[1].id;
        assert!(h.remove(id).unwrap());
        assert!(!h.remove(id).unwrap());
        assert!(!h.was_downloaded("alice", "a.flac"));
        assert_eq!(lines(tmp.path()), 3, "append-only: add, add, remove");
        drop(h);
        let mut h = History::open(tmp.path()).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(lines(tmp.path()), 1, "compacted on load");
        h.clear().unwrap();
        assert!(h.is_empty() && h.downloaded_keys().is_empty());
        drop(h);
        assert!(History::open(tmp.path()).unwrap().is_empty());
        assert_eq!(lines(tmp.path()), 0);
    }

    #[test]
    fn retry_replaces_the_earlier_record_of_the_same_transfer() {
        let tmp = tempfile::tempdir().unwrap();
        let mut h = History::open(tmp.path()).unwrap();
        rec(&mut h, 7, "alice", "a.flac", Outcome::Failed);
        rec(&mut h, 7, "alice", "a.flac", Outcome::Done);
        let all = h.query(&HistoryQuery::default());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].outcome, Outcome::Done);
        drop(h);
        assert_eq!(History::open(tmp.path()).unwrap().len(), 1);
    }

    #[test]
    fn cap_keeps_the_newest_and_compacts_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(HISTORY_FILE);
        let mut text = String::new();
        for i in 1..=(HISTORY_CAP as u64 + 5) {
            let e = HistoryEntry {
                id: i,
                finished_at: i,
                username: "u".into(),
                remote_path: format!("{i}.flac"),
                local_path: None,
                size: 1,
                format: "flac".into(),
                bitrate_kbps: None,
                sample_rate_hz: None,
                bit_depth: None,
                duration_secs: None,
                album: String::new(),
                outcome: Outcome::Done,
                reason: None,
            };
            text.push_str(&serde_json::to_string(&Line::Add(e)).unwrap());
            text.push('\n');
        }
        fs::write(&p, text).unwrap();
        let h = History::open(tmp.path()).unwrap();
        assert_eq!(h.len(), HISTORY_CAP);
        assert!(!h.was_downloaded("u", "5.flac"), "oldest dropped");
        assert!(h.was_downloaded("u", "6.flac"));
        assert_eq!(lines(tmp.path()), HISTORY_CAP);
        let newest = &h.query(&HistoryQuery {
            limit: 1,
            ..HistoryQuery::default()
        })[0];
        assert_eq!(newest.id, HISTORY_CAP as u64 + 5);
    }

    #[test]
    fn query_filters_and_pages() {
        let mut h = History::in_memory();
        for (t, p) in [
            (1, r"Queen\Jazz\Don't Stop.flac"),
            (2, r"Shaggy\Wasn't Me.mp3"),
            (3, r"Queen\II\x.flac"),
        ] {
            rec(&mut h, t, "alice", p, Outcome::Done);
        }
        let q = |text: &str, limit, offset| {
            h.query(&HistoryQuery {
                text: text.into(),
                limit,
                offset,
            })
            .into_iter()
            .map(|e| e.remote_path)
            .collect::<Vec<_>>()
        };
        assert_eq!(
            q("QUEEN", 0, 0),
            [r"Queen\II\x.flac", r"Queen\Jazz\Don't Stop.flac"]
        );
        assert_eq!(q("queen", 1, 1), [r"Queen\Jazz\Don't Stop.flac"]);
        assert_eq!(q("", 2, 0).len(), 2);
        assert_eq!(q("alice", 0, 0).len(), 3, "user name matches");
    }

    #[test]
    fn exists_on_disk_is_checked_lazily() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("01.flac");
        fs::write(&f, b"x").unwrap();
        let mut h = History::in_memory();
        let file = RemoteFile {
            username: "a".into(),
            path: "01.flac".into(),
            size: 1,
            attrs: crate::model::AudioAttrs::default(),
        };
        h.record(TransferId(1), &file, Outcome::Done, Some(f.clone()), None)
            .unwrap();
        let e = h.query(&HistoryQuery::default()).remove(0);
        assert!(e.exists_on_disk());
        fs::remove_file(&f).unwrap();
        assert!(
            !e.exists_on_disk(),
            "moved/deleted is noticed without reloading"
        );
    }
}
