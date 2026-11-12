//! Where transfer bytes land on disk.
//!
//! soulseek-rs-lib does not report the `.part` path or the final path of a
//! download (`part_path_of` is crate-private and `DownloadStatus::Completed`
//! carries no path). It is however deterministic
//! (`soulseek-rs-lib/src/peer/download_peer.rs:81-125`):
//! `final = expand_tilde(download_directory) / basename(remote_path)` and
//! `part = final + ".part"`. These helpers reproduce that rule so the UI gets
//! the path before the first byte arrives.

use crate::model::TransferId;
use std::path::{Path, PathBuf};

/// Suffix the library appends while a transfer is in flight
/// (`soulseek-rs-lib/src/utils/path.rs:7`).
pub const PART_SUFFIX: &str = ".part";

/// Last component of a remote virtual path; peers use `\` (and some `/`).
#[must_use]
pub fn remote_basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Parent of a remote virtual path, empty if there is none.
#[must_use]
pub fn remote_parent(path: &str) -> &str {
    path.rfind(['\\', '/']).map_or("", |i| &path[..i])
}

/// Final path the library will rename to, for a transfer into `dir`.
#[must_use]
pub fn final_path(dir: &Path, remote_path: &str) -> PathBuf {
    dir.join(remote_basename(remote_path))
}

/// `.part` path the library streams into while the transfer runs.
#[must_use]
pub fn part_path(dir: &Path, remote_path: &str) -> PathBuf {
    let mut p = final_path(dir, remote_path).into_os_string();
    p.push(PART_SUFFIX);
    PathBuf::from(p)
}

/// Directory for a play fetch: one sub-directory per transfer, so two tracks
/// sharing a basename (every `01 Intro.flac`) never share a `.part` — the
/// library would otherwise resume one into the other.
#[must_use]
pub fn cache_transfer_dir(cache_dir: &Path, id: TransferId) -> PathBuf {
    cache_dir.join(format!("t{}", id.0))
}

/// Hidden directory inside the downloads folder where a ↓ download streams
/// before it is moved to `downloads/<folder>/`. One sub-directory per
/// transfer (`t<id>`) for the same reason as the cache (G5), and so a final
/// name collision can be resolved with a unique name instead of overwriting.
pub const STAGING_DIR_NAME: &str = ".goalseek-partial";

#[must_use]
pub fn staging_transfer_dir(download_dir: &Path, id: TransferId) -> PathBuf {
    download_dir
        .join(STAGING_DIR_NAME)
        .join(format!("t{}", id.0))
}

/// Parse a `t<id>` directory name.
#[must_use]
pub fn parse_transfer_dir_name(name: &str) -> Option<u64> {
    let digits = name.strip_prefix('t')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Directory a downloaded file goes to: `downloads/<remote folder name>`, so
/// a whole-folder download keeps the folder name (MVP: "keeps folder name").
#[must_use]
pub fn download_dir_for(download_dir: &Path, remote_path: &str) -> PathBuf {
    let folder = remote_basename(remote_parent(remote_path));
    let folder = sanitize(folder);
    if folder.is_empty() {
        download_dir.to_path_buf()
    } else {
        download_dir.join(folder)
    }
}

/// Make a remote name safe as a single local path component.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c == '/' || c == '\0' { '_' } else { c })
        .collect();
    match s.trim() {
        "." | ".." => String::new(),
        t => t.to_string(),
    }
}

/// Files `Core::download_folder` leaves out: OS/file-manager junk, matched
/// case-insensitively against the basename. `*x` = ends with `x`, `x*` =
/// starts with `x`, otherwise the whole name. Cue sheets, rip logs, cover
/// art and `.nfo` files are kept.
pub const DOWNLOAD_SKIP: &[&str] = &["thumbs.db", "desktop.ini", ".ds_store", "*.url", "._*"];

/// True when `remote_path`'s basename matches [`DOWNLOAD_SKIP`].
#[must_use]
pub fn is_download_junk(remote_path: &str) -> bool {
    let name = remote_basename(remote_path).to_lowercase();
    DOWNLOAD_SKIP.iter().any(|p| {
        if let Some(suffix) = p.strip_prefix('*') {
            name.ends_with(suffix)
        } else if let Some(prefix) = p.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            name == *p
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_and_parent_handle_both_separators() {
        assert_eq!(remote_basename(r"@@x\Music\Album\01 a.flac"), "01 a.flac");
        assert_eq!(
            remote_parent(r"@@x\Music\Album\01 a.flac"),
            r"@@x\Music\Album"
        );
        assert_eq!(remote_basename("a/b/c.mp3"), "c.mp3");
        assert_eq!(remote_basename("c.mp3"), "c.mp3");
        assert_eq!(remote_parent("c.mp3"), "");
    }

    #[test]
    fn part_path_matches_library_rule() {
        let dir = Path::new("/tmp/cache/t7");
        assert_eq!(
            part_path(dir, r"Music\Album\01 a.flac"),
            PathBuf::from("/tmp/cache/t7/01 a.flac.part")
        );
        assert_eq!(
            final_path(dir, r"Music\Album\01 a.flac"),
            PathBuf::from("/tmp/cache/t7/01 a.flac")
        );
    }

    #[test]
    fn transfer_dir_names_round_trip() {
        let d = cache_transfer_dir(Path::new("/c"), TransferId(42));
        let name = d.file_name().unwrap().to_str().unwrap();
        assert_eq!(parse_transfer_dir_name(name), Some(42));
        assert_eq!(parse_transfer_dir_name("t"), None);
        assert_eq!(parse_transfer_dir_name("tx1"), None);
        assert_eq!(parse_transfer_dir_name("slsk.log"), None);
        assert!(
            staging_transfer_dir(Path::new("/dl"), TransferId(3))
                .starts_with("/dl/.goalseek-partial")
        );
    }

    #[test]
    fn cache_dirs_are_per_transfer() {
        let c = Path::new("/c");
        assert_ne!(
            cache_transfer_dir(c, TransferId(1)),
            cache_transfer_dir(c, TransferId(2))
        );
    }

    #[test]
    fn download_keeps_folder_name_and_is_not_escapable() {
        let d = Path::new("/dl");
        assert_eq!(
            download_dir_for(d, r"Music\Album (2019)\01.flac"),
            PathBuf::from("/dl/Album (2019)")
        );
        assert_eq!(download_dir_for(d, "01.flac"), PathBuf::from("/dl"));
        assert_eq!(download_dir_for(d, r"..\01.flac"), PathBuf::from("/dl"));
        assert_eq!(
            download_dir_for(d, r"x\a/b\01.flac"),
            PathBuf::from("/dl/b")
        );
    }

    #[test]
    fn download_junk_is_matched_case_insensitively_and_music_extras_kept() {
        for junk in [
            r"A\Thumbs.db",
            r"A\THUMBS.DB",
            r"A\desktop.ini",
            r"A\Desktop.INI",
            "A/.DS_Store",
            r"A\Album on Discogs.url",
            r"A\LINK.URL",
            r"A\._01 Track.flac",
        ] {
            assert!(is_download_junk(junk), "{junk}");
        }
        for keep in [
            r"A\01 Track.flac",
            r"A\album.cue",
            r"A\rip.log",
            r"A\cover.jpg",
            r"A\folder.png",
            r"A\info.nfo",
            r"A\thumbs.db.flac",
            r"A\my.url.txt",
            r"A\_01 underscore.flac",
            r"._dir\01.flac",
        ] {
            assert!(!is_download_junk(keep), "{keep}");
        }
    }
}
