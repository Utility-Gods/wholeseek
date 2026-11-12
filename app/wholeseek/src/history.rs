//! The transfers drawer's History view: its state and what its rows do. Rows come from
//! `Backend::history` (the core's download history), a page at a time, re-read when a ↓ ends,
//! when the search changes and when the view is shown.

use rune::TextEdit;
use wholeseek_core::{AudioAttrs, CoreEvent, HistoryEntry, HistoryQuery, Outcome, RemoteFile};

use crate::app::{App, Panel};

/// Rows per page ("Show more" adds another).
pub const PAGE: usize = 100;

/// The drawer's two views.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DrawerView {
    #[default]
    Active,
    History,
}

pub struct HistoryRow {
    pub entry: HistoryEntry,
    /// The file is still where it was saved (finished ones only; checked when read).
    pub on_disk: bool,
}

pub struct HistoryUi {
    pub view: DrawerView,
    pub find: TextEdit,
    pub rows: Vec<HistoryRow>,
    /// More rows than shown match.
    pub more: bool,
    pub limit: usize,
    /// "Clear history?" is showing.
    pub confirm_clear: bool,
    /// The history couldn't be read at start, or cleared (one muted line).
    pub error: Option<String>,
    stale: bool,
    /// The (search, limit) the rows were read for.
    read_for: Option<(String, usize)>,
}

impl Default for HistoryUi {
    fn default() -> Self {
        Self {
            view: DrawerView::Active,
            find: TextEdit::new(""),
            rows: Vec::new(),
            more: false,
            limit: PAGE,
            confirm_clear: false,
            error: None,
            stale: true,
            read_for: None,
        }
    }
}

/// The remote file a history entry was (for retry and its quality line).
pub fn entry_file(e: &HistoryEntry) -> RemoteFile {
    RemoteFile {
        username: e.username.clone(),
        path: e.remote_path.clone(),
        size: e.size,
        attrs: AudioAttrs {
            bitrate_kbps: e.bitrate_kbps,
            duration_secs: e.duration_secs,
            vbr: None,
            sample_rate_hz: e.sample_rate_hz,
            bit_depth: e.bit_depth,
        },
    }
}

impl App {
    pub fn set_drawer_view(&mut self, v: DrawerView) {
        self.hist.view = v;
        self.hist.confirm_clear = false;
        self.drawer.confirm_cancel = None;
        if v == DrawerView::History {
            self.hist.stale = true;
        }
    }

    /// Before an event is applied: a ↓ that ended changed the history; a history that
    /// couldn't be read gets its line in the view (not the banner).
    pub fn history_event(&mut self, e: &CoreEvent) {
        match e {
            CoreEvent::TransferDone { .. }
            | CoreEvent::TransferFailed { .. }
            | CoreEvent::TransferCancelled { .. } => self.hist.stale = true,
            CoreEvent::Error { context, message } if context == "history" => {
                self.hist.error = Some(message.clone());
            }
            _ => {}
        }
    }

    /// Each frame: re-read the shown page if it is out of date. Whether the rows changed.
    pub fn refresh_history(&mut self) -> bool {
        if self.panel != Panel::Transfers || self.hist.view != DrawerView::History {
            return false;
        }
        let key = (self.hist.find.text.trim().to_string(), self.hist.limit);
        if !self.hist.stale && self.hist.read_for.as_ref() == Some(&key) {
            return false;
        }
        let mut entries = self.backend.history(&HistoryQuery {
            text: key.0.clone(),
            limit: key.1 + 1,
            offset: 0,
        });
        self.hist.more = entries.len() > key.1;
        entries.truncate(key.1);
        self.hist.rows = entries
            .into_iter()
            .map(|entry| HistoryRow {
                on_disk: entry.outcome == Outcome::Done && self.backend.history_on_disk(&entry),
                entry,
            })
            .collect();
        self.hist.stale = false;
        self.hist.read_for = Some(key);
        true
    }

    pub fn history_more(&mut self) {
        self.hist.limit += PAGE;
    }

    pub fn history_remove(&mut self, id: u64) {
        self.backend.history_remove(id);
        self.hist.stale = true;
    }

    /// "Clear history" asks first; the second press clears.
    pub fn history_clear(&mut self) {
        if !self.hist.confirm_clear {
            self.hist.confirm_clear = true;
            return;
        }
        self.hist.confirm_clear = false;
        if let Err(e) = self.backend.history_clear() {
            self.hist.error = Some(e);
        }
        self.hist.limit = PAGE;
        self.hist.stale = true;
    }

    pub fn history_keep(&mut self) {
        self.hist.confirm_clear = false;
    }

    /// ↓ a failed or cancelled entry again (a new download of the same remote file).
    pub fn history_retry(&mut self, id: u64) {
        let Some(f) = self
            .hist
            .rows
            .iter()
            .find(|r| r.entry.id == id)
            .map(|r| entry_file(&r.entry))
        else {
            return;
        };
        self.download(&f);
    }
}

/// When it ended, relative: "just now", "5 min ago", "3 h ago", "2 d ago", else the date.
pub fn ago(finished_at: u64, now: u64) -> String {
    let d = now.saturating_sub(finished_at);
    match d {
        0..60 => "just now".into(),
        60..3_600 => format!("{} min ago", d / 60),
        3_600..86_400 => format!("{} h ago", d / 3_600),
        86_400..604_800 => format!("{} d ago", d / 86_400),
        _ => utc(finished_at)[..10].to_string(),
    }
}

/// "2026-11-12 14:02 UTC".
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_794_504_490), "2026-11-12 17:28 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        let now = 1_794_504_490;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 300, now), "5 min ago");
        assert_eq!(ago(now - 7_300, now), "2 h ago");
        assert_eq!(ago(now - 3 * 86_400, now), "3 d ago");
        assert_eq!(ago(now - 40 * 86_400, now), "2026-10-03");
    }
}
