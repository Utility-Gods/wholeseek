//! A peer's profile (user info) as the UI keeps it, per username, so each user view (a future
//! user tab) reads its own entry. Updated from `CoreEvent::UserProfile` / `UserProfileFailed`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use goalseek_core::UserProfile;
use rune::{Image, ImageCache, Waker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileStatus {
    /// Requested; nothing back yet.
    Loading,
    Known,
    Failed(String),
}

/// One user's profile as the UI holds it.
pub struct ProfileEntry {
    pub status: ProfileStatus,
    pub profile: UserProfile,
    /// The decoded picture handle; `None` without a picture. A picture Rune can't decode keeps
    /// its handle with `error()` set, and the header skips it.
    pub picture: Option<Image>,
    /// "more" pressed on the description.
    pub expanded: bool,
}

impl ProfileEntry {
    fn loading() -> Self {
        Self {
            status: ProfileStatus::Loading,
            profile: UserProfile::default(),
            picture: None,
            expanded: false,
        }
    }

    /// A picture that decoded (or is still decoding) — not one Rune rejected.
    pub fn shown_picture(&self) -> Option<&Image> {
        self.picture.as_ref().filter(|i| i.error().is_none())
    }
}

/// Profiles by username, and the image cache that decodes their pictures off the UI thread.
pub struct Profiles {
    entries: HashMap<String, ProfileEntry>,
    images: ImageCache,
    /// The window's waker, set in `setup`; the cache wakes the app through it after a decode.
    wake: Arc<Mutex<Option<Waker>>>,
}

impl Default for Profiles {
    fn default() -> Self {
        let wake: Arc<Mutex<Option<Waker>>> = Arc::default();
        let forward = wake.clone();
        let images = ImageCache::new(Arc::new(move || {
            if let Some(w) = forward.lock().ok().and_then(|w| w.clone()) {
                w();
            }
        }));
        Self {
            entries: HashMap::new(),
            images,
            wake,
        }
    }
}

impl Profiles {
    pub fn set_waker(&self, wake: Waker) {
        if let Ok(mut w) = self.wake.lock() {
            *w = Some(wake);
        }
    }

    pub fn get(&self, username: &str) -> Option<&ProfileEntry> {
        self.entries.get(username)
    }

    /// A fresh request: forget what we had and wait for the first event.
    pub fn requested(&mut self, username: &str) {
        self.entries
            .insert(username.to_string(), ProfileEntry::loading());
    }

    /// Drop a user's entry (their view closed).
    pub fn forget(&mut self, username: &str) {
        self.entries.remove(username);
    }

    /// Everything known so far about `username`. Ignored unless a request for that user is
    /// open (a stale answer for a view already closed). Returns true if an entry changed.
    pub fn loaded(&mut self, username: &str, profile: UserProfile) -> bool {
        let Some(entry) = self.entries.get_mut(username) else {
            return false;
        };
        // Same bytes = same handle (the cache is content-addressed): a repeat event doesn't
        // redecode.
        entry.picture = profile
            .picture
            .as_ref()
            .map(|bytes| self.images.load(bytes.clone()));
        entry.profile = profile;
        entry.status = ProfileStatus::Known;
        true
    }

    /// Nothing could be fetched. A failure after fields arrived keeps them.
    pub fn failed(&mut self, username: &str, reason: String) -> bool {
        let Some(entry) = self.entries.get_mut(username) else {
            return false;
        };
        if entry.status == ProfileStatus::Loading {
            entry.status = ProfileStatus::Failed(reason);
        }
        true
    }

    pub fn toggle_expanded(&mut self, username: &str) {
        if let Some(e) = self.entries.get_mut(username) {
            e.expanded = !e.expanded;
        }
    }
}

/// "2.1 MB/s · 27,814 files · 2,852 folders · free slot" — only the known parts.
pub fn stats_line(p: &UserProfile) -> String {
    let mut parts = Vec::new();
    if let Some(s) = p.avg_speed {
        parts.push(crate::fmt::speed(f64::from(s)));
    }
    if let Some(n) = p.shared_files {
        parts.push(format!(
            "{} {}",
            grouped(n),
            if n == 1 { "file" } else { "files" }
        ));
    }
    if let Some(n) = p.shared_dirs {
        parts.push(format!(
            "{} {}",
            grouped(n),
            if n == 1 { "folder" } else { "folders" }
        ));
    }
    match (p.free_slot, p.queue_size) {
        (Some(true), _) => parts.push("free slot".into()),
        (_, Some(q)) => parts.push(format!("queue {}", grouped(q))),
        (Some(false), None) => parts.push("no free slot".into()),
        (None, None) => {}
    }
    parts.join(" · ")
}

/// 27814 → "27,814".
pub fn grouped(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
