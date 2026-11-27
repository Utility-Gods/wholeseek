//! Cross-process ownership and restart-safe private state.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_STATE_BYTES: u64 = 32 * 1024 * 1024;
pub const HANDOVER_MAX_AGE: Duration = Duration::from_secs(3600);

/// Holding the file keeps the OS advisory lock; never unlink a lock file.
pub struct Lease {
    _file: File,
}

#[derive(Clone, Debug)]
pub struct Ownership {
    dir: PathBuf,
}

impl Ownership {
    pub fn new(dir: PathBuf) -> io::Result<Self> {
        ensure_private_dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    pub fn session(&self) -> io::Result<Option<Lease>> {
        self.lock("session.lock")
    }

    pub fn app_intent(&self) -> io::Result<Option<Lease>> {
        self.lock("app-intent.lock")
    }

    pub fn app_is_waiting(&self) -> io::Result<bool> {
        Ok(self.app_intent()?.is_none())
    }

    fn lock(&self, name: &str) -> io::Result<Option<Lease>> {
        let path = self.dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(m) if !m.is_file() => return Err(invalid("control lock is not a regular file")),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let file = private_options()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("control lock is not a regular file"));
        }
        match file.try_lock() {
            Ok(()) => Ok(Some(Lease { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    pub fn handover_path(&self) -> PathBuf {
        self.dir.join("handover-v1.json")
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(invalid("control state is not a directory"));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != super::unix::effective_uid() || meta.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control state directory is not private",
            ));
        }
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE, but not FILE_SHARE_DELETE.
        options.share_mode(0x3);
    }
    options
}

/// The session owner alone reads/writes state. Temporary files never become
/// handover input, even if the writer crashes before rename.
pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let temp = path.with_extension("json.pending");
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let result = (|| {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(invalid("control state exceeds size limit"));
        }
        let file = private_options().create_new(true).write(true).open(&temp)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&bytes)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn read_state<T: DeserializeOwned>(
    path: &Path,
    max_age: Option<Duration>,
) -> io::Result<Option<T>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let metadata = file.metadata()?;
    if let Some(max_age) = max_age
        && SystemTime::now()
            .duration_since(metadata.modified()?)
            .unwrap_or_default()
            > max_age
    {
        return Ok(None);
    }
    if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        return Err(invalid("invalid control state file"));
    }
    let mut contents = Vec::new();
    BufReader::new(file)
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("control state exceeds size limit"));
    }
    Ok(Some(serde_json::from_slice(&contents)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_intent_is_independent_and_crash_releases_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Ownership::new(dir.path().join("control")).unwrap();
        let daemon = owner.session().unwrap().unwrap();
        assert!(owner.session().unwrap().is_none());
        let intent = owner.app_intent().unwrap().unwrap();
        assert!(owner.app_is_waiting().unwrap());
        drop(daemon);
        let _app = owner.session().unwrap().unwrap();
        drop(intent);
        assert!(!owner.app_is_waiting().unwrap());
        assert!(
            read_state::<Vec<u32>>(&owner.handover_path(), Some(HANDOVER_MAX_AGE))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn atomic_handover_is_consumable_and_stale_input_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Ownership::new(dir.path().join("control")).unwrap();
        let path = owner.handover_path();
        fs::write(path.with_extension("json.pending"), b"uncommitted").unwrap();
        assert!(
            read_state::<Vec<u32>>(&path, Some(HANDOVER_MAX_AGE))
                .unwrap()
                .is_none()
        );
        write_atomic(&path, &vec![42u32]).unwrap();
        assert_eq!(
            read_state::<Vec<u32>>(&path, Some(HANDOVER_MAX_AGE)).unwrap(),
            Some(vec![42])
        );
        let stale = SystemTime::now() - HANDOVER_MAX_AGE - Duration::from_secs(1);
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(stale))
            .unwrap();
        assert_eq!(
            read_state::<Vec<u32>>(&path, Some(HANDOVER_MAX_AGE)).unwrap(),
            None
        );
    }
}
