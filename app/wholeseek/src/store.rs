//! Saving settings and the account. The real store calls `wholeseek_core::config` on a
//! background thread (the keyring can block for seconds); the demo store only records calls.

use std::sync::mpsc::{self, Receiver, Sender};

use wholeseek_core::{Credentials, Settings, config};

/// What a save reported back, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreNote {
    /// The password could not go into the system keyring (username was still saved).
    KeyringUnavailable(String),
    /// Writing the config file failed.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCall {
    SaveSettings(Settings),
    SaveCredentials(String),
    ForgetCredentials,
}

pub trait Store {
    fn save_settings(&mut self, settings: &Settings);
    fn save_credentials(&mut self, credentials: &Credentials);
    fn forget_credentials(&mut self);
    /// Results of earlier saves. Never blocks.
    fn drain(&mut self) -> Vec<StoreNote>;
    fn set_waker(&mut self, _wake: rune::Waker) {}
    /// Calls so far (demo only; tests read it).
    fn calls(&self) -> &[StoreCall] {
        &[]
    }
}

pub struct RealStore {
    tx: Sender<StoreNote>,
    rx: Receiver<StoreNote>,
    wake: Option<rune::Waker>,
}

impl RealStore {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, wake: None }
    }

    fn spawn(&self, job: impl FnOnce() -> Option<StoreNote> + Send + 'static) {
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new()
            .name("wholeseek-store".into())
            .spawn(move || {
                if let Some(note) = job() {
                    let _ = tx.send(note);
                    if let Some(w) = wake {
                        w();
                    }
                }
            });
        if let Err(e) = spawned {
            let _ = self
                .tx
                .send(StoreNote::Failed(format!("could not save: {e}")));
        }
    }
}

impl Store for RealStore {
    fn save_settings(&mut self, settings: &Settings) {
        let settings = settings.clone();
        self.spawn(move || {
            config::save_settings(&settings)
                .err()
                .map(|e| StoreNote::Failed(format!("saving settings: {e}")))
        });
    }
    fn save_credentials(&mut self, credentials: &Credentials) {
        let credentials = credentials.clone();
        self.spawn(move || {
            config::save_credentials(&credentials)
                .err()
                .map(|e| StoreNote::KeyringUnavailable(e.0))
        });
    }
    fn forget_credentials(&mut self) {
        self.spawn(|| {
            config::forget_credentials()
                .err()
                .map(|e| StoreNote::Failed(format!("forgetting the account: {e}")))
        });
    }
    fn drain(&mut self) -> Vec<StoreNote> {
        self.rx.try_iter().collect()
    }
    fn set_waker(&mut self, wake: rune::Waker) {
        self.wake = Some(wake);
    }
}

/// Records saves in memory. `keyring` false makes credential saves report unavailable.
pub struct MemoryStore {
    pub keyring: bool,
    calls: Vec<StoreCall>,
    pending: Vec<StoreNote>,
}

impl MemoryStore {
    pub fn new(keyring: bool) -> Self {
        Self {
            keyring,
            calls: Vec::new(),
            pending: Vec::new(),
        }
    }
}

impl Store for MemoryStore {
    fn save_settings(&mut self, settings: &Settings) {
        self.calls.push(StoreCall::SaveSettings(settings.clone()));
    }
    fn save_credentials(&mut self, credentials: &Credentials) {
        self.calls
            .push(StoreCall::SaveCredentials(credentials.username.clone()));
        if !self.keyring {
            self.pending.push(StoreNote::KeyringUnavailable(
                "no Secret Service (demo)".into(),
            ));
        }
    }
    fn forget_credentials(&mut self) {
        self.calls.push(StoreCall::ForgetCredentials);
    }
    fn drain(&mut self) -> Vec<StoreNote> {
        std::mem::take(&mut self.pending)
    }
    fn calls(&self) -> &[StoreCall] {
        &self.calls
    }
}
