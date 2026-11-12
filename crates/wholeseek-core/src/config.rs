//! Persistence the app calls before starting [`crate::Core`]: settings as TOML
//! in `$XDG_CONFIG_HOME/wholeseek/config.toml`, the password in the OS keyring
//! (Secret Service on Linux). The password is never written to the config
//! file or logged; the file only remembers the username.
//!
//! Every function has an `*_in` variant taking the config directory (and,
//! where the keyring is involved, a [`SecretStore`]) for tests.

use crate::model::{Credentials, Settings};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The app's name in every on-disk/keyring location: config dir, default
/// download/cache dirs, log path, keyring service. The rename to Wholeseek
/// flips this (and [`LEGACY_APP_NAME`]) and nothing else.
pub const APP_NAME: &str = "wholeseek";
/// Name of the previous release whose config/keyring [`migrate_legacy`]
/// carries over. Equal to [`APP_NAME`] means "nothing to migrate".
pub const LEGACY_APP_NAME: &str = "goalseek";
/// Keyring service name; the account is the Soulseek username.
pub const KEYRING_SERVICE: &str = APP_NAME;
/// File name inside the config directory.
pub const CONFIG_FILE: &str = "config.toml";
/// How long to wait for the keyring before calling it unavailable (a Secret
/// Service that is on the bus but never answers would otherwise block for
/// D-Bus's activation timeout).
pub const KEYRING_TIMEOUT: Duration = Duration::from_secs(5);

/// What [`load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    /// Saved username + password, if both were found.
    pub credentials: Option<Credentials>,
    /// False when the keyring could not be reached; the UI should say the
    /// password will not be remembered.
    pub keyring_available: bool,
}

/// The keyring could not store or delete the password. The string is the
/// underlying error, safe to show (never contains the password).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyringUnavailable(pub String);

impl std::fmt::Display for KeyringUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "system keyring unavailable: {}", self.0)
    }
}

impl std::error::Error for KeyringUnavailable {}

/// What `config.toml` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    /// Last account saved with [`save_credentials`] (password lives in the keyring).
    pub username: Option<String>,
    pub settings: Settings,
}

/// The per-user base directories app dirs live under (`<base>/<APP_NAME>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseDirs {
    /// `$XDG_CONFIG_HOME` (else `~/.config`).
    pub config: PathBuf,
    /// `$XDG_DOWNLOAD_DIR` (else `~/Downloads`).
    pub downloads: PathBuf,
    /// `$XDG_CACHE_HOME` (else `~/.cache`).
    pub cache: PathBuf,
}

impl BaseDirs {
    /// The real per-user locations.
    #[must_use]
    pub fn system() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            config: dirs::config_dir().unwrap_or_else(|| home.join(".config")),
            downloads: dirs::download_dir().unwrap_or_else(|| home.join("Downloads")),
            cache: dirs::cache_dir().unwrap_or_else(|| home.join(".cache")),
        }
    }
}

/// `$XDG_CONFIG_HOME/<APP_NAME>` (else `~/.config/<APP_NAME>`).
#[must_use]
pub fn config_dir() -> PathBuf {
    BaseDirs::system().config.join(APP_NAME)
}

/// `$XDG_CACHE_HOME/<APP_NAME>/slsk.log` — where the protocol library logs (G13).
#[must_use]
pub fn default_log_path() -> PathBuf {
    BaseDirs::system().cache.join(APP_NAME).join("slsk.log")
}

// ---------------------------------------------------------------- public API

/// Load settings and saved credentials from the default locations.
///
/// # Errors
/// The config file exists but cannot be read or is not valid TOML.
pub fn load() -> io::Result<Loaded> {
    load_in(&config_dir(), &default_store())
}

/// Save settings (keeps the remembered username).
///
/// # Errors
/// I/O errors writing the file.
pub fn save_settings(settings: &Settings) -> io::Result<()> {
    save_settings_in(&config_dir(), settings)
}

/// Remember `credentials`: password into the keyring, username into the
/// config file. The username is saved even if the keyring fails.
///
/// # Errors
/// The keyring could not store the password.
pub fn save_credentials(credentials: &Credentials) -> Result<(), KeyringUnavailable> {
    save_credentials_in(&config_dir(), &default_store(), credentials)
}

/// Forget the saved account: delete the keyring entry and the username.
///
/// # Errors
/// I/O errors writing the config file. A keyring that cannot be reached is
/// not an error (nothing could have been stored there).
pub fn forget_credentials() -> io::Result<()> {
    forget_credentials_in(&config_dir(), &default_store())
}

// ------------------------------------------------------------ `_in` variants

/// [`load`] from `dir` with `secrets`.
///
/// # Errors
/// As [`load`].
pub fn load_in(dir: &Path, secrets: &Arc<dyn SecretStore>) -> io::Result<Loaded> {
    let file = read_file(&dir.join(CONFIG_FILE))?;
    let probe_user = file
        .username
        .clone()
        .unwrap_or_else(|| "__wholeseek_probe__".into());
    let (credentials, keyring_available) = match with_timeout(secrets, move |s| s.get(&probe_user))
    {
        Ok(Some(password)) => (
            file.username
                .clone()
                .map(|username| Credentials { username, password }),
            true,
        ),
        Ok(None) => (None, true),
        Err(_) => (None, false),
    };
    Ok(Loaded {
        settings: file.settings,
        credentials,
        keyring_available,
    })
}

/// [`save_settings`] into `dir`.
///
/// # Errors
/// As [`save_settings`].
pub fn save_settings_in(dir: &Path, settings: &Settings) -> io::Result<()> {
    let path = dir.join(CONFIG_FILE);
    let mut file = read_file(&path).unwrap_or_default();
    file.settings = settings.clone();
    write_file(&path, &file)
}

/// [`save_credentials`] into `dir` / `secrets`.
///
/// # Errors
/// As [`save_credentials`].
pub fn save_credentials_in(
    dir: &Path,
    secrets: &Arc<dyn SecretStore>,
    credentials: &Credentials,
) -> Result<(), KeyringUnavailable> {
    let path = dir.join(CONFIG_FILE);
    let mut file = read_file(&path).unwrap_or_default();
    file.username = Some(credentials.username.clone());
    let wrote = write_file(&path, &file);
    let c = credentials.clone();
    with_timeout(secrets, move |s| s.set(&c.username, &c.password)).map_err(KeyringUnavailable)?;
    wrote.map_err(|e| KeyringUnavailable(format!("password saved, but config not written: {e}")))
}

/// [`forget_credentials`] in `dir` / `secrets`.
///
/// # Errors
/// As [`forget_credentials`].
pub fn forget_credentials_in(dir: &Path, secrets: &Arc<dyn SecretStore>) -> io::Result<()> {
    let path = dir.join(CONFIG_FILE);
    let mut file = read_file(&path)?;
    if let Some(user) = file.username.take() {
        let _ = with_timeout(secrets, move |s| s.delete(&user));
        write_file(&path, &file)?;
    }
    Ok(())
}

/// Read `config.toml`; a missing file is the default config.
///
/// # Errors
/// I/O errors other than "not found", or invalid TOML.
pub fn read_file(path: &Path) -> io::Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(e) => Err(e),
    }
}

/// Write `config.toml` atomically (temp file + rename).
///
/// # Errors
/// I/O or serialisation errors.
pub fn write_file(path: &Path, config: &ConfigFile) -> io::Result<()> {
    let text = toml::to_string_pretty(config).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------ keyring

/// Where passwords are kept. The real one is the OS keyring ([`KeyringStore`]).
pub trait SecretStore: Send + Sync {
    /// # Errors
    /// The store is unavailable.
    fn get(&self, username: &str) -> Result<Option<String>, String>;
    /// # Errors
    /// The store is unavailable or refused the write.
    fn set(&self, username: &str, password: &str) -> Result<(), String>;
    /// Succeeds when there was nothing stored.
    ///
    /// # Errors
    /// The store is unavailable.
    fn delete(&self, username: &str) -> Result<(), String>;
}

/// OS keyring under service [`KEYRING_SERVICE`], account = username.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn get(&self, username: &str) -> Result<Option<String>, String> {
        NamedKeyringStore::new(KEYRING_SERVICE).get(username)
    }
    fn set(&self, username: &str, password: &str) -> Result<(), String> {
        NamedKeyringStore::new(KEYRING_SERVICE).set(username, password)
    }
    fn delete(&self, username: &str) -> Result<(), String> {
        NamedKeyringStore::new(KEYRING_SERVICE).delete(username)
    }
}

/// OS keyring under an explicit service name (used for migration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedKeyringStore {
    pub service: String,
}

impl NamedKeyringStore {
    #[must_use]
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }
}

impl SecretStore for NamedKeyringStore {
    fn get(&self, username: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(&self.service, username).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, username: &str, password: &str) -> Result<(), String> {
        keyring::Entry::new(&self.service, username)
            .and_then(|e| e.set_password(password))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, username: &str) -> Result<(), String> {
        match keyring::Entry::new(&self.service, username).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------- migration

/// What [`migrate_legacy`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Migration {
    /// Same name, no legacy config, or the new config dir already exists.
    NotNeeded,
    /// `config.toml` was copied (default dirs renamed).
    Migrated { password: PasswordMigration },
}

/// The keyring half of a migration. Never fatal: on failure the user signs
/// in again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordMigration {
    Copied,
    /// No saved username, or no password under the old service.
    NothingSaved,
    /// Keyring unavailable or refused (message is safe to show/log).
    Failed(String),
}

/// Carry the previous release's config and saved password over to
/// [`APP_NAME`]. Call once at startup, before [`load`]. Idempotent: does
/// nothing once the new config dir exists (and nothing at all while
/// [`LEGACY_APP_NAME`] equals [`APP_NAME`]).
///
/// # Errors
/// I/O errors reading the old or writing the new config file.
pub fn migrate_legacy() -> io::Result<Migration> {
    migrate_legacy_in(
        &BaseDirs::system(),
        LEGACY_APP_NAME,
        APP_NAME,
        &(Arc::new(NamedKeyringStore::new(LEGACY_APP_NAME)) as Arc<dyn SecretStore>),
        &(Arc::new(NamedKeyringStore::new(APP_NAME)) as Arc<dyn SecretStore>),
    )
}

/// [`migrate_legacy`] with explicit base dirs, names and secret stores.
///
/// - Copies `<config>/<old>/config.toml` to `<config>/<new>/config.toml`; a
///   `download_dir`/`cache_dir` equal to the old default
///   (`<downloads>/<old>`, `<cache>/<old>`) becomes the new default; any
///   other value is kept. The old files are left in place.
/// - Copies the saved username's password from `old_secrets` to
///   `new_secrets` (each call bounded by [`KEYRING_TIMEOUT`]); failure is
///   reported in the result, never an error.
///
/// # Errors
/// As [`migrate_legacy`].
pub fn migrate_legacy_in(
    base: &BaseDirs,
    old: &str,
    new: &str,
    old_secrets: &Arc<dyn SecretStore>,
    new_secrets: &Arc<dyn SecretStore>,
) -> io::Result<Migration> {
    let (old_dir, new_dir) = (base.config.join(old), base.config.join(new));
    let old_file = old_dir.join(CONFIG_FILE);
    if old == new || new_dir.exists() || !old_file.is_file() {
        return Ok(Migration::NotNeeded);
    }
    let mut cfg = read_file(&old_file)?;
    let s = &mut cfg.settings;
    if s.download_dir == base.downloads.join(old) {
        s.download_dir = base.downloads.join(new);
    }
    if s.cache_dir == base.cache.join(old) {
        s.cache_dir = base.cache.join(new);
    }
    write_file(&new_dir.join(CONFIG_FILE), &cfg)?;

    let password = match cfg.username {
        None => PasswordMigration::NothingSaved,
        Some(user) => {
            let u = user.clone();
            match with_timeout(old_secrets, move |s| s.get(&u)) {
                Ok(None) => PasswordMigration::NothingSaved,
                Err(e) => PasswordMigration::Failed(e),
                Ok(Some(pw)) => match with_timeout(new_secrets, move |s| s.set(&user, &pw)) {
                    Ok(()) => PasswordMigration::Copied,
                    Err(e) => PasswordMigration::Failed(e),
                },
            }
        }
    };
    if let PasswordMigration::Failed(e) = &password {
        soulseek_rs::warn!("[config] saved password not migrated from {old} to {new}: {e}");
    }
    Ok(Migration::Migrated { password })
}

/// Run a keyring call on a detached helper thread, giving up after
/// [`KEYRING_TIMEOUT`] so a hung Secret Service never blocks the caller.
fn with_timeout<T: Send + 'static>(
    secrets: &Arc<dyn SecretStore>,
    f: impl FnOnce(&dyn SecretStore) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let store = secrets.clone();
    std::thread::spawn(move || {
        let _ = tx.send(f(store.as_ref()));
    });
    rx.recv_timeout(KEYRING_TIMEOUT).unwrap_or_else(|_| {
        Err(format!(
            "keyring did not answer within {}s",
            KEYRING_TIMEOUT.as_secs()
        ))
    })
}

fn default_store() -> Arc<dyn SecretStore> {
    Arc::new(KeyringStore)
}

/// Point the protocol library's logger at `log_file` (append) at Info level (G13).
///
/// The library only reads its log destination from the `LOG_FILE`
/// environment variable, once, when the first client is created.
///
/// # Safety
/// Sets an environment variable: call it at the very start of `main`, before
/// any other thread exists (see [`std::env::set_var`]). Has no effect on the
/// destination if `LOG_FILE` is already set or a client was already created.
///
/// # Errors
/// The log directory cannot be created.
pub unsafe fn init_logging(log_file: &Path) -> io::Result<()> {
    if let Some(dir) = log_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::env::var_os("LOG_FILE").is_none() {
        // SAFETY: the caller guarantees no other thread is running.
        unsafe { std::env::set_var("LOG_FILE", log_file) };
    }
    soulseek_rs::utils::logger::init();
    soulseek_rs::utils::logger::set_log_level(soulseek_rs::utils::logger::LogLevel::Info);
    Ok(())
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory secret store; `fail` simulates an unavailable keyring.
    #[derive(Default)]
    pub(crate) struct MemoryStore {
        pub(crate) map: Mutex<std::collections::HashMap<String, String>>,
        pub(crate) fail: bool,
    }

    impl SecretStore for MemoryStore {
        fn get(&self, u: &str) -> Result<Option<String>, String> {
            if self.fail {
                return Err("no secret service".into());
            }
            Ok(self.map.lock().unwrap().get(u).cloned())
        }
        fn set(&self, u: &str, p: &str) -> Result<(), String> {
            if self.fail {
                return Err("no secret service".into());
            }
            self.map.lock().unwrap().insert(u.into(), p.into());
            Ok(())
        }
        fn delete(&self, u: &str) -> Result<(), String> {
            if self.fail {
                return Err("no secret service".into());
            }
            self.map.lock().unwrap().remove(u);
            Ok(())
        }
    }

    fn mem() -> Arc<dyn SecretStore> {
        Arc::new(MemoryStore::default())
    }

    fn sample() -> Settings {
        Settings {
            download_dir: "/dl".into(),
            cache_dir: "/cache".into(),
            cache_cap_bytes: 123,
            share_dirs: vec!["/music".into(), "/more music".into()],
            listen_port: 50_000,
            upload_slots: 3,
            port_mapping: false,
            play_start_timeout_secs: 5,
            wishlist: vec!["wasn't me".into()],
            recent_download_dirs: vec!["/elsewhere".into()],
        }
    }

    #[test]
    fn fresh_dir_loads_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let l = load_in(tmp.path(), &mem()).unwrap();
        assert_eq!(l.settings, Settings::default());
        assert_eq!(l.credentials, None);
        assert!(l.keyring_available);
    }

    #[test]
    fn settings_and_credentials_round_trip_without_password_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("goalseek");
        let mem_store = Arc::new(MemoryStore::default());
        let store: Arc<dyn SecretStore> = mem_store.clone();
        save_settings_in(&dir, &sample()).unwrap();
        let creds = Credentials {
            username: "alice".into(),
            password: "s3cret-pw".into(),
        };
        save_credentials_in(&dir, &store, &creds).unwrap();
        // Saving settings again keeps the username.
        save_settings_in(&dir, &sample()).unwrap();

        let l = load_in(&dir, &store).unwrap();
        assert_eq!(l.settings, sample());
        assert_eq!(l.credentials, Some(creds));
        assert!(l.keyring_available);
        let text = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(!text.contains("s3cret-pw"));
        assert!(text.contains("alice"));

        forget_credentials_in(&dir, &store).unwrap();
        let l = load_in(&dir, &store).unwrap();
        assert_eq!(l.credentials, None);
        assert!(mem_store.map.lock().unwrap().is_empty());
        assert_eq!(l.settings, sample(), "forgetting keeps settings");
    }

    #[test]
    fn unavailable_keyring_is_reported_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let bad: Arc<dyn SecretStore> = Arc::new(MemoryStore {
            fail: true,
            ..Default::default()
        });
        let creds = Credentials {
            username: "bob".into(),
            password: "pw".into(),
        };
        let err = save_credentials_in(tmp.path(), &bad, &creds).unwrap_err();
        assert!(!err.to_string().contains("pw\""), "{err}");
        let l = load_in(tmp.path(), &bad).unwrap();
        assert!(!l.keyring_available);
        assert_eq!(l.credentials, None);
        assert_eq!(
            read_file(&tmp.path().join(CONFIG_FILE))
                .unwrap()
                .username
                .as_deref(),
            Some("bob")
        );
        forget_credentials_in(tmp.path(), &bad).unwrap();
    }

    #[test]
    fn partial_or_bad_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        std::fs::write(&path, "[settings]\nlisten_port = 4000\n").unwrap();
        let l = load_in(tmp.path(), &mem()).unwrap();
        assert_eq!(l.settings.listen_port, 4000);
        assert_eq!(l.settings.upload_slots, Settings::default().upload_slots);
        assert_eq!(
            l.settings.play_start_timeout_secs, 8,
            "pre-CP1 configs get the default"
        );
        std::fs::write(&path, "not = [toml").unwrap();
        assert!(load_in(tmp.path(), &mem()).is_err());
    }

    #[test]
    fn config_from_before_wishlist_and_recent_dirs_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        // A complete config as written before these fields existed.
        std::fs::write(
            &path,
            "username = \"alice\"\n[settings]\ndownload_dir = \"/dl\"\ncache_dir = \"/c\"\n\
             cache_cap_bytes = 1\nshare_dirs = []\nlisten_port = 2240\nupload_slots = 2\n\
             port_mapping = false\nplay_start_timeout_secs = 8\n",
        )
        .unwrap();
        let l = load_in(tmp.path(), &mem()).unwrap();
        assert_eq!(l.settings.listen_port, 2240);
        assert!(l.settings.wishlist.is_empty());
        assert!(l.settings.recent_download_dirs.is_empty());
        // And they round-trip once set.
        let mut s = l.settings.clone();
        s.wishlist.push("wasn't me".into());
        s.remember_download_dir(Path::new("/usb"));
        save_settings_in(tmp.path(), &s).unwrap();
        assert_eq!(load_in(tmp.path(), &mem()).unwrap().settings, s);
    }

    fn bases(root: &Path) -> BaseDirs {
        BaseDirs {
            config: root.join("config"),
            downloads: root.join("Downloads"),
            cache: root.join("cache"),
        }
    }

    fn legacy(base: &BaseDirs, username: Option<&str>, settings: Settings) {
        let cfg = ConfigFile {
            username: username.map(Into::into),
            settings,
        };
        write_file(&base.config.join("goalseek").join(CONFIG_FILE), &cfg).unwrap();
    }

    fn stores(pw: Option<&str>) -> (Arc<MemoryStore>, Arc<MemoryStore>) {
        let old = Arc::new(MemoryStore::default());
        if let Some(pw) = pw {
            old.map.lock().unwrap().insert("alice".into(), pw.into());
        }
        (old, Arc::new(MemoryStore::default()))
    }

    fn migrate(base: &BaseDirs, old: &Arc<MemoryStore>, new: &Arc<MemoryStore>) -> Migration {
        let o: Arc<dyn SecretStore> = old.clone();
        let n: Arc<dyn SecretStore> = new.clone();
        migrate_legacy_in(base, "goalseek", "wholeseek", &o, &n).unwrap()
    }

    #[test]
    fn migration_renames_default_dirs_and_copies_password_once() {
        let tmp = tempfile::tempdir().unwrap();
        let base = bases(tmp.path());
        let mut old_settings = sample();
        old_settings.download_dir = base.downloads.join("goalseek");
        old_settings.cache_dir = base.cache.join("goalseek");
        legacy(&base, Some("alice"), old_settings.clone());
        let (old, new) = stores(Some("pw1"));

        let m = migrate(&base, &old, &new);
        assert_eq!(
            m,
            Migration::Migrated {
                password: PasswordMigration::Copied
            }
        );
        let n = read_file(&base.config.join("wholeseek").join(CONFIG_FILE)).unwrap();
        assert_eq!(n.username.as_deref(), Some("alice"));
        assert_eq!(n.settings.download_dir, base.downloads.join("wholeseek"));
        assert_eq!(n.settings.cache_dir, base.cache.join("wholeseek"));
        assert_eq!(
            n.settings.share_dirs, old_settings.share_dirs,
            "other settings kept"
        );
        assert_eq!(n.settings.listen_port, old_settings.listen_port);
        assert_eq!(
            new.map.lock().unwrap().get("alice").map(String::as_str),
            Some("pw1")
        );
        assert!(
            old.map.lock().unwrap().contains_key("alice"),
            "old entry left in place"
        );
        assert!(
            base.config.join("goalseek").join(CONFIG_FILE).is_file(),
            "old config left"
        );

        // Idempotent: a second run (even with a changed old password) does nothing.
        old.map.lock().unwrap().insert("alice".into(), "pw2".into());
        assert_eq!(migrate(&base, &old, &new), Migration::NotNeeded);
        assert_eq!(
            new.map.lock().unwrap().get("alice").map(String::as_str),
            Some("pw1")
        );
    }

    #[test]
    fn migration_keeps_custom_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = bases(tmp.path());
        legacy(&base, None, sample()); // "/dl", "/cache"
        let (old, new) = stores(None);
        assert_eq!(
            migrate(&base, &old, &new),
            Migration::Migrated {
                password: PasswordMigration::NothingSaved
            }
        );
        let n = read_file(&base.config.join("wholeseek").join(CONFIG_FILE)).unwrap();
        assert_eq!(n.settings, sample());
    }

    #[test]
    fn migration_survives_unavailable_keyring() {
        let tmp = tempfile::tempdir().unwrap();
        let base = bases(tmp.path());
        legacy(&base, Some("alice"), sample());
        let old = Arc::new(MemoryStore {
            fail: true,
            ..Default::default()
        });
        let new = Arc::new(MemoryStore::default());
        let m = migrate(&base, &old, &new);
        assert!(
            matches!(
                m,
                Migration::Migrated {
                    password: PasswordMigration::Failed(_)
                }
            ),
            "{m:?}"
        );
        assert!(base.config.join("wholeseek").join(CONFIG_FILE).is_file());
    }

    #[test]
    fn migration_not_needed_cases() {
        let tmp = tempfile::tempdir().unwrap();
        let base = bases(tmp.path());
        let (old, new) = stores(Some("pw"));
        // No legacy config.
        assert_eq!(migrate(&base, &old, &new), Migration::NotNeeded);
        assert!(!base.config.join("wholeseek").exists());
        // New dir already exists (user already ran the new app).
        legacy(&base, Some("alice"), sample());
        std::fs::create_dir_all(base.config.join("wholeseek")).unwrap();
        assert_eq!(migrate(&base, &old, &new), Migration::NotNeeded);
        assert!(new.map.lock().unwrap().is_empty());
        // Same name: no-op (today's build, APP_NAME == LEGACY_APP_NAME).
        let o: Arc<dyn SecretStore> = old.clone();
        let n: Arc<dyn SecretStore> = new.clone();
        assert_eq!(
            migrate_legacy_in(&base, "goalseek", "goalseek", &o, &n).unwrap(),
            Migration::NotNeeded
        );
        assert_ne!(APP_NAME, LEGACY_APP_NAME, "rename flipped: migration runs");
    }
}
