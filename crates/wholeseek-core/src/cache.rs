//! Play cache bookkeeping (size cap, eviction, reuse index) and the file
//! moves used when a fetched file is handed to the downloads folder.

use crate::paths;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Name of the small metadata file written into each `cache/t<id>/` so a
/// completed play can be reused (▶ again, or ↓ without refetching) after a
/// restart. Two lines: username, remote path.
pub const META_FILE: &str = ".wholeseek-meta";

/// One `t<id>` directory in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub id: u64,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: SystemTime,
}

/// List `t<id>` directories under `cache_dir` with their total size. Anything
/// else in the cache dir (e.g. `slsk.log`) is ignored and never evicted.
#[must_use]
pub fn scan(cache_dir: &Path) -> Vec<CacheEntry> {
    let Ok(rd) = fs::read_dir(cache_dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let id = paths::parse_transfer_dir_name(e.file_name().to_str()?)?;
            let md = e.metadata().ok()?;
            if !md.is_dir() {
                return None;
            }
            let path = e.path();
            let (bytes, newest) = dir_size_and_mtime(&path);
            Some(CacheEntry {
                id,
                bytes,
                modified: newest.max(md.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                path,
            })
        })
        .collect()
}

fn dir_size_and_mtime(dir: &Path) -> (u64, SystemTime) {
    let mut bytes = 0;
    let mut newest = SystemTime::UNIX_EPOCH;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            if let Ok(md) = e.metadata() {
                if md.is_file() {
                    bytes += md.len();
                }
                if let Ok(m) = md.modified() {
                    newest = newest.max(m);
                }
            }
        }
    }
    (bytes, newest)
}

/// Which entries to delete so the total fits in `cap`: oldest first, never a
/// protected id (playing, or a transfer still writing into it). May leave the
/// cache over the cap when everything left is protected.
#[must_use]
pub fn plan_eviction(entries: &[CacheEntry], cap: u64, protected: &HashSet<u64>) -> Vec<u64> {
    let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
    let mut candidates: Vec<&CacheEntry> = entries
        .iter()
        .filter(|e| !protected.contains(&e.id))
        .collect();
    candidates.sort_by_key(|e| (e.modified, e.id));
    let mut evict = Vec::new();
    for e in candidates {
        if total <= cap {
            break;
        }
        total = total.saturating_sub(e.bytes);
        evict.push(e.id);
    }
    evict
}

/// Highest `t<id>` found in any of `dirs`, so ids of a new run never reuse a
/// directory (and a stale `.part` in it) from a previous run.
#[must_use]
pub fn max_transfer_id(dirs: &[&Path]) -> u64 {
    dirs.iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|rd| rd.filter_map(Result::ok))
        .filter_map(|e| paths::parse_transfer_dir_name(e.file_name().to_str()?))
        .max()
        .unwrap_or(0)
}

/// Record which remote file a cache dir holds.
///
/// # Errors
/// I/O errors creating the directory or writing the file.
pub fn write_meta(dir: &Path, username: &str, remote_path: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(META_FILE), format!("{username}\n{remote_path}\n"))
}

/// Completed cached files by `(username, remote path)`, from the meta files
/// of every `t<id>` dir whose final (non-`.part`) file exists.
#[must_use]
pub fn index(cache_dir: &Path) -> HashMap<(String, String), PathBuf> {
    let mut out = HashMap::new();
    let mut entries = scan(cache_dir);
    entries.sort_by_key(|e| e.id);
    for e in entries {
        let Ok(meta) = fs::read_to_string(e.path.join(META_FILE)) else {
            continue;
        };
        let mut lines = meta.lines();
        let (Some(user), Some(remote)) = (lines.next(), lines.next()) else {
            continue;
        };
        let final_path = paths::final_path(&e.path, remote);
        if final_path.is_file() {
            out.insert((user.to_string(), remote.to_string()), final_path);
        }
    }
    out
}

/// `dir/name`, or `dir/stem (2).ext`, `(3)`… if taken — downloads never
/// overwrite an existing file.
#[must_use]
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Make `dst` the same file as `src`: a hard link when possible (no extra
/// space, and an open reader of either name keeps working), else a copy.
///
/// # Errors
/// I/O errors from creating the parent directory or copying.
pub fn link_or_copy(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if dst.exists() {
        fs::remove_file(dst)?;
    }
    if fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).map(|_| ())
}

/// Move `src` to `dst`: rename, or copy + delete across filesystems.
///
/// # Errors
/// I/O errors from creating the parent directory, copying or removing.
pub fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst)?;
    fs::remove_file(src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(id: u64, bytes: u64, age_secs: u64) -> CacheEntry {
        CacheEntry {
            id,
            path: PathBuf::from(format!("/c/t{id}")),
            bytes,
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - age_secs),
        }
    }

    #[test]
    fn evicts_oldest_until_under_cap() {
        let e = [entry(1, 40, 300), entry(2, 40, 200), entry(3, 40, 100)];
        assert_eq!(plan_eviction(&e, 100, &HashSet::new()), vec![1]);
        assert_eq!(plan_eviction(&e, 40, &HashSet::new()), vec![1, 2]);
        assert!(plan_eviction(&e, 120, &HashSet::new()).is_empty());
    }

    #[test]
    fn never_evicts_protected_even_if_over_cap() {
        let e = [entry(1, 40, 300), entry(2, 40, 200), entry(3, 40, 100)];
        let protected: HashSet<u64> = [1].into();
        assert_eq!(plan_eviction(&e, 50, &protected), vec![2, 3]);
        let all: HashSet<u64> = [1, 2, 3].into();
        assert!(plan_eviction(&e, 0, &all).is_empty());
    }

    #[test]
    fn scan_index_and_max_id_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let c = tmp.path();
        write_meta(&c.join("t3"), "alice", r"Music\A\01.flac").unwrap();
        fs::write(c.join("t3").join("01.flac"), b"abcd").unwrap();
        write_meta(&c.join("t9"), "bob", r"Music\B\02.flac").unwrap();
        fs::write(c.join("t9").join("02.flac.part"), b"ab").unwrap();
        fs::write(c.join("slsk.log"), b"log").unwrap();
        fs::create_dir(c.join("other")).unwrap();

        let mut s = scan(c);
        s.sort_by_key(|e| e.id);
        assert_eq!(s.iter().map(|e| e.id).collect::<Vec<_>>(), [3, 9]);
        assert!(s[0].bytes >= 4);
        let idx = index(c);
        assert_eq!(idx.len(), 1, "only complete files are reusable");
        assert_eq!(
            idx[&("alice".to_string(), r"Music\A\01.flac".to_string())],
            c.join("t3").join("01.flac")
        );
        assert_eq!(max_transfer_id(&[c, &c.join("missing")]), 9);
    }

    #[test]
    fn unique_path_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert_eq!(unique_path(d, "a.flac"), d.join("a.flac"));
        fs::write(d.join("a.flac"), b"x").unwrap();
        assert_eq!(unique_path(d, "a.flac"), d.join("a (2).flac"));
        fs::write(d.join("a (2).flac"), b"x").unwrap();
        assert_eq!(unique_path(d, "a.flac"), d.join("a (3).flac"));
        fs::write(d.join("noext"), b"x").unwrap();
        assert_eq!(unique_path(d, "noext"), d.join("noext (2)"));
    }

    #[test]
    fn link_or_copy_keeps_source_and_move_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.flac");
        fs::write(&src, b"data").unwrap();
        let linked = tmp.path().join("x/linked.flac");
        link_or_copy(&src, &linked).unwrap();
        assert!(src.exists());
        assert_eq!(fs::read(&linked).unwrap(), b"data");
        let moved = tmp.path().join("y/moved.flac");
        move_file(&src, &moved).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&moved).unwrap(), b"data");
    }
}
