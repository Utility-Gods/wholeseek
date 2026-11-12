//! The slice of `soulseek_rs::Client` the worker uses, behind a trait so the
//! worker's logic is tested offline against a fake.

use soulseek_rs::UserInfo;
use soulseek_rs::message::peer::PeerInfo;
use soulseek_rs::{
    Client, ClientSettings, DownloadStatus, SearchResult, SessionLoss, SharedDirectory, SoulseekRs,
    UploadInfo,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// A logged-in session. Every method is `&self` and non-blocking except
/// `search_blocking` and `set_shares`, which the worker runs on helper threads.
pub(crate) trait Session: Send + Sync {
    fn listen_port(&self) -> Option<u16>;
    fn session_loss(&self) -> Option<SessionLoss>;
    /// Register and send `query`, then wait out `window` or `cancel`.
    fn search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>);
    /// Like `search_blocking`, sent as a wishlist search (server code 103).
    fn wishlist_search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>);
    /// Server's wishlist interval (code 104), or the library default.
    fn wishlist_interval(&self) -> Duration;
    fn search_count(&self, query: &str) -> usize;
    fn search_results(&self, query: &str) -> Vec<SearchResult>;
    fn forget_search(&self, query: &str);
    fn browse(&self, username: &str) -> Result<(), String>;
    fn take_browse(&self, username: &str) -> Option<Vec<SharedDirectory>>;
    fn request_folder(&self, username: &str, folder: &str) -> Result<(), String>;
    fn take_folder(&self, username: &str, folder: &str) -> Option<Vec<SharedDirectory>>;
    fn download(
        &self,
        username: &str,
        path: &str,
        size: u64,
        dir: &str,
    ) -> Result<Receiver<DownloadStatus>, String>;
    fn cancel_download(&self, username: &str, path: &str) -> bool;
    fn remove_download(&self, username: &str, path: &str);
    fn request_place_in_queue(&self, username: &str, path: &str);
    fn queue_place(&self, username: &str, path: &str) -> Option<u32>;
    /// Rescan `dirs` (blocking) and announce; returns `(folders, files)`.
    fn set_shares(&self, dirs: Vec<String>) -> Result<(u32, u32), String>;
    fn shared_counts(&self) -> (u32, u32);
    fn uploads(&self) -> Vec<UploadInfo>;
    fn cancel_upload(&self, username: &str, path: &str) -> bool;
    /// Server presence + stats; replies polled with `user_info`.
    fn request_user_info(&self, username: &str) -> Result<(), String>;
    fn user_info(&self, username: &str) -> Option<UserInfo>;
    /// Peer UserInfo (code 15/16); reply polled with `peer_info`.
    fn request_peer_info(&self, username: &str) -> Result<(), String>;
    fn peer_info(&self, username: &str) -> Option<PeerInfo>;
    fn set_upload_slots(&self, slots: usize);
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectParams {
    pub username: String,
    pub password: String,
    pub listen_port: u16,
    pub share_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginError {
    /// Server refused the credentials; do not retry.
    Rejected,
    /// Anything else (network, timeout); retry with backoff.
    Other(String),
}

/// Opens sessions. Called on a helper thread; may block (share scan, login ≤45 s).
pub(crate) trait Connector: Send + Sync {
    fn connect(&self, params: &ConnectParams) -> Result<Arc<dyn Session>, LoginError>;
}

pub(crate) struct RealConnector;

impl Connector for RealConnector {
    fn connect(&self, p: &ConnectParams) -> Result<Arc<dyn Session>, LoginError> {
        let settings = ClientSettings {
            listen_port: p.listen_port,
            shared_directories: p.share_dirs.clone(),
            version: crate::CLIENT_VERSION,
            ..ClientSettings::new(p.username.clone(), p.password.clone())
        };
        let mut client = Client::with_settings(settings);
        client
            .connect()
            .map_err(|e| LoginError::Other(e.to_string()))?;
        match client.login() {
            Ok(true) => Ok(Arc::new(client)),
            Ok(false) | Err(SoulseekRs::AuthenticationFailed) => Err(LoginError::Rejected),
            Err(e) => Err(LoginError::Other(e.to_string())),
        }
    }
}

impl Session for Client {
    fn listen_port(&self) -> Option<u16> {
        Client::listen_port(self)
    }
    fn session_loss(&self) -> Option<SessionLoss> {
        Client::session_loss(self)
    }
    fn search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>) {
        let _ = self.search_with_cancel(query, window, Some(cancel));
    }
    fn wishlist_search_blocking(&self, query: &str, window: Duration, cancel: Arc<AtomicBool>) {
        if self.start_wishlist_search(query).is_err() {
            return;
        }
        let end = std::time::Instant::now() + window;
        while std::time::Instant::now() < end && !cancel.load(std::sync::atomic::Ordering::Relaxed)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn wishlist_interval(&self) -> Duration {
        Client::wishlist_interval(self)
    }
    fn search_count(&self, query: &str) -> usize {
        self.get_search_results_count(query)
    }
    fn search_results(&self, query: &str) -> Vec<SearchResult> {
        self.get_search_results(query)
    }
    fn forget_search(&self, query: &str) {
        let _ = Client::forget_search(self, query);
    }
    fn browse(&self, username: &str) -> Result<(), String> {
        self.browse_user(username).map_err(|e| e.to_string())
    }
    fn take_browse(&self, username: &str) -> Option<Vec<SharedDirectory>> {
        self.take_browse_result(username)
    }
    fn request_folder(&self, username: &str, folder: &str) -> Result<(), String> {
        self.request_folder_contents(username, folder)
            .map_err(|e| e.to_string())
    }
    fn take_folder(&self, username: &str, folder: &str) -> Option<Vec<SharedDirectory>> {
        self.take_folder_contents(username, folder)
    }
    fn download(
        &self,
        username: &str,
        path: &str,
        size: u64,
        dir: &str,
    ) -> Result<Receiver<DownloadStatus>, String> {
        Client::download(
            self,
            path.to_string(),
            username.to_string(),
            size,
            dir.to_string(),
        )
        .map(|(_, rx)| rx)
        .map_err(|e| e.to_string())
    }
    fn cancel_download(&self, username: &str, path: &str) -> bool {
        Client::cancel_download(self, username, path)
    }
    fn remove_download(&self, username: &str, path: &str) {
        let _ = Client::remove_download(self, username, path);
    }
    fn request_place_in_queue(&self, username: &str, path: &str) {
        let _ = Client::request_place_in_queue(self, username, path);
    }
    fn queue_place(&self, username: &str, path: &str) -> Option<u32> {
        self.get_all_downloads()
            .into_iter()
            .find(|d| d.username == username && d.filename == path && !d.is_finished())
            .and_then(|d| d.queue_position)
    }
    fn set_shares(&self, dirs: Vec<String>) -> Result<(u32, u32), String> {
        // The index is swapped even when the announce fails (not connected).
        let res = self.set_shared_directories(dirs);
        let counts = self.shared_counts();
        res.map(|()| counts).map_err(|e| e.to_string())
    }
    fn shared_counts(&self) -> (u32, u32) {
        Client::shared_counts(self)
    }
    fn uploads(&self) -> Vec<UploadInfo> {
        Client::uploads(self)
    }
    fn set_upload_slots(&self, slots: usize) {
        Client::set_upload_slots(self, slots);
    }
    fn cancel_upload(&self, username: &str, path: &str) -> bool {
        Client::cancel_upload(self, username, path)
    }
    fn request_user_info(&self, username: &str) -> Result<(), String> {
        Client::request_user_info(self, username).map_err(|e| e.to_string())
    }
    fn user_info(&self, username: &str) -> Option<UserInfo> {
        Client::user_info(self, username)
    }
    fn request_peer_info(&self, username: &str) -> Result<(), String> {
        Client::request_peer_info(self, username).map_err(|e| e.to_string())
    }
    fn peer_info(&self, username: &str) -> Option<PeerInfo> {
        Client::peer_info(self, username)
    }
}

/// Count files/folders of `dirs` without a session (offline share setup).
pub(crate) fn scan_counts(dirs: &[std::path::PathBuf]) -> (u32, u32) {
    let roots: Vec<_> = dirs
        .iter()
        .filter(|d| !d.as_os_str().is_empty())
        .cloned()
        .collect();
    if roots.is_empty() {
        return (0, 0);
    }
    let s = soulseek_rs::shares::Shares::scan_many(&roots);
    (s.folder_count(), s.file_count())
}

/// Reconnect delays: 2, 5, 15, 30 s, then 60 s for every later attempt.
pub(crate) const BACKOFF_SECS: [u64; 5] = [2, 5, 15, 30, 60];

/// Reconnect delay after `attempt` consecutive failures (0-based).
#[must_use]
pub(crate) fn backoff(attempt: u32) -> Duration {
    let i = usize::try_from(attempt)
        .unwrap_or(usize::MAX)
        .min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_steps_and_caps_at_60s() {
        let secs: Vec<u64> = (0..8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, [2, 5, 15, 30, 60, 60, 60, 60]);
    }
}
