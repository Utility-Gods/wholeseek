//! The demo's download history: what `wholeseek_core::history` keeps, in memory, seeded with a
//! few downloads from "earlier runs" (one of them since deleted from disk).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use wholeseek_core::{HistoryEntry, HistoryQuery, Outcome, RemoteFile, TransferId};

/// Seeded entries whose file is gone from disk ("missing").
const GONE: &str = "Pale Harbour - Signals";

#[derive(Default)]
pub struct DemoHistory {
    /// Oldest first, like the core's.
    entries: Vec<HistoryEntry>,
    next_id: u64,
    by_transfer: HashMap<TransferId, u64>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl DemoHistory {
    pub fn seeded(download_dir: &std::path::Path) -> Self {
        let mut h = Self::default();
        let now = unix_now();
        let day = 86_400;
        let mut add = |ago: u64,
                       user: &str,
                       remote: &str,
                       size: u64,
                       outcome: Outcome,
                       reason: Option<&str>,
                       q: (Option<u32>, Option<u32>, Option<u32>)| {
            let name = remote.rsplit('\\').next().unwrap_or(remote);
            let album = remote.rsplit('\\').nth(1).unwrap_or_default().to_string();
            let local = (outcome == Outcome::Done).then(|| download_dir.join(&album).join(name));
            h.next_id += 1;
            h.entries.push(HistoryEntry {
                id: h.next_id,
                finished_at: now.saturating_sub(ago),
                username: user.into(),
                remote_path: remote.into(),
                local_path: local,
                size,
                format: name.rsplit('.').next().unwrap_or("").to_lowercase(),
                bitrate_kbps: q.0,
                sample_rate_hz: q.1,
                bit_depth: q.2,
                duration_secs: Some(240),
                album,
                outcome,
                reason: reason.map(str::to_string),
            });
        };
        let cd = (None, Some(44_100), Some(16));
        add(
            40 * day,
            "vinyl_rips",
            &format!(r"rips\{GONE} (2017)\A1 Signals.flac"),
            31_220_118,
            Outcome::Done,
            None,
            cd,
        );
        add(
            12 * day,
            "dj_ivo",
            r"mp3\H\Hollow Lights\Hollow Lights - Low Tide Radio.mp3",
            10_481_920,
            Outcome::Done,
            None,
            (Some(320), None, None),
        );
        add(
            3 * day,
            "marten_k",
            r"Music\Lantern Rooms\Early Hours (2016)\03 Tin Roof.flac",
            27_002_331,
            Outcome::Failed,
            Some("peer closed the connection"),
            cd,
        );
        add(
            day + 3_600,
            "nightbus",
            r"live\Hollow Lights 2022-05-14\04 Breakwater (live).mp3",
            9_870_002,
            Outcome::Cancelled,
            None,
            (Some(256), None, None),
        );
        for (i, t) in ["Paper Moons", "Wire Garden", "Quiet Engines"]
            .into_iter()
            .enumerate()
        {
            add(
                5_400 - 600 * i as u64,
                "alice",
                &format!(
                    r"Music\Lantern Rooms\Early Hours (2016)\0{} {t}.flac",
                    i + 1
                ),
                24_000_000 + 1_000_000 * i as u64,
                Outcome::Done,
                None,
                cd,
            );
        }
        h
    }

    /// How download `transfer` ended; a retry replaces its earlier row.
    pub fn record(
        &mut self,
        transfer: TransferId,
        file: &RemoteFile,
        outcome: Outcome,
        local_path: Option<PathBuf>,
        reason: Option<String>,
    ) {
        if let Some(old) = self.by_transfer.remove(&transfer) {
            self.entries.retain(|e| e.id != old);
        }
        self.next_id += 1;
        let a = file.attrs;
        self.entries.push(HistoryEntry {
            id: self.next_id,
            finished_at: unix_now(),
            username: file.username.clone(),
            remote_path: file.path.clone(),
            local_path,
            size: file.size,
            format: file.extension().to_lowercase(),
            bitrate_kbps: a.bitrate_kbps,
            sample_rate_hz: a.sample_rate_hz,
            bit_depth: a.bit_depth,
            duration_secs: a.duration_secs,
            album: file
                .folder()
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_string(),
            outcome,
            reason,
        });
        self.by_transfer.insert(transfer, self.next_id);
    }

    pub fn query(&self, q: &HistoryQuery) -> Vec<HistoryEntry> {
        let needle = q.text.trim().to_lowercase();
        let it = self
            .entries
            .iter()
            .rev()
            .filter(|e| {
                needle.is_empty()
                    || e.remote_path.to_lowercase().contains(&needle)
                    || e.username.to_lowercase().contains(&needle)
                    || e.local_path
                        .as_deref()
                        .is_some_and(|p| p.to_string_lossy().to_lowercase().contains(&needle))
            })
            .skip(q.offset);
        if q.limit == 0 {
            it.cloned().collect()
        } else {
            it.take(q.limit).cloned().collect()
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_transfer.clear();
    }

    /// `(user, remote path)` of completed downloads.
    pub fn keys(&self) -> HashSet<(String, String)> {
        self.entries
            .iter()
            .filter(|e| e.outcome == Outcome::Done)
            .map(|e| (e.username.clone(), e.remote_path.clone()))
            .collect()
    }

    /// Nothing is really on disk in the demo: every finished file is, except the deleted one.
    pub fn on_disk(e: &HistoryEntry) -> bool {
        e.outcome == Outcome::Done && !e.remote_path.contains(GONE)
    }
}
