//! The transfers drawer's model: the status filter, and
//! downloads grouped by (user, remote folder) with their aggregate progress.

use std::collections::HashSet;

use wholeseek_core::{TransferId, TransferKind};

use crate::app::{TState, Transfer};
use crate::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TFilter {
    #[default]
    All,
    Active,
    Queued,
    Done,
    Failed,
}

impl TFilter {
    pub const EVERY: [Self; 5] = [
        Self::All,
        Self::Active,
        Self::Queued,
        Self::Done,
        Self::Failed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Queued => "Queued",
            Self::Done => "Done",
            Self::Failed => "Failed",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::All => "filter-all",
            Self::Active => "filter-active",
            Self::Queued => "filter-queued",
            Self::Done => "filter-done",
            Self::Failed => "filter-failed",
        }
    }

    /// Failed includes cancelled: both are what ↻ retries.
    pub fn accepts(self, s: &TState) -> bool {
        match self {
            Self::All => true,
            Self::Active => matches!(s, TState::Active),
            Self::Queued => matches!(s, TState::Queued | TState::RemoteQueued(_)),
            Self::Done => matches!(s, TState::Done(_)),
            Self::Failed => matches!(s, TState::Failed(_) | TState::Cancelled),
        }
    }
}

/// Live: cancellable. Finished: what "Clear" removes. Retryable: failed or cancelled.
pub fn is_finished(s: &TState) -> bool {
    matches!(s, TState::Done(_) | TState::Failed(_) | TState::Cancelled)
}
pub fn is_retryable(s: &TState) -> bool {
    matches!(s, TState::Failed(_) | TState::Cancelled)
}

/// The downloads the drawer lists: not ▶ fetches, not cleared.
pub fn listed<'a>(
    transfers: &'a [Transfer],
    hidden: &'a HashSet<TransferId>,
) -> impl Iterator<Item = &'a Transfer> {
    transfers
        .iter()
        .filter(move |t| t.kind == TransferKind::Download && !hidden.contains(&t.id))
}

/// One (user, folder) of downloads, in the order they were first asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct TGroup {
    pub username: String,
    pub folder: String,
    /// Indices into `App::transfers`, in request order.
    pub rows: Vec<usize>,
}

impl TGroup {
    pub fn key(&self) -> (String, String) {
        (self.username.clone(), self.folder.clone())
    }
}

/// Listed downloads passing `filter`, grouped by (user, remote folder). Newest group first.
pub fn groups(
    transfers: &[Transfer],
    hidden: &HashSet<TransferId>,
    filter: TFilter,
) -> Vec<TGroup> {
    let mut out: Vec<TGroup> = Vec::new();
    for (i, t) in transfers.iter().enumerate() {
        if t.kind != TransferKind::Download || hidden.contains(&t.id) || !filter.accepts(&t.state) {
            continue;
        }
        let folder = t.file.folder().to_string();
        match out
            .iter_mut()
            .find(|g| g.username == t.file.username && g.folder == folder)
        {
            Some(g) => g.rows.push(i),
            None => out.push(TGroup {
                username: t.file.username.clone(),
                folder,
                rows: vec![i],
            }),
        }
    }
    out.reverse();
    out
}

/// Counts for the filter chips, over everything listed.
pub fn counts(transfers: &[Transfer], hidden: &HashSet<TransferId>) -> [usize; 5] {
    TFilter::EVERY.map(|f| {
        listed(transfers, hidden)
            .filter(|t| f.accepts(&t.state))
            .count()
    })
}

/// A group's line: "8 of 16 · 212 MB / 512 MB · queued at peer #7".
pub fn summary(rows: &[&Transfer]) -> String {
    let done = rows
        .iter()
        .filter(|t| matches!(t.state, TState::Done(_)))
        .count();
    let bytes: u64 = rows.iter().map(|t| t.bytes).sum();
    let total: u64 = rows.iter().map(|t| t.total).sum();
    let speed: f64 = rows
        .iter()
        .filter(|t| t.state == TState::Active)
        .map(|t| t.speed)
        .sum();
    let failed = rows.iter().filter(|t| is_retryable(&t.state)).count();
    let place = rows
        .iter()
        .filter_map(|t| match t.state {
            TState::RemoteQueued(p) => Some(p),
            _ => None,
        })
        .min_by_key(|p| p.unwrap_or(u32::MAX));
    let status = if rows.iter().any(|t| t.state == TState::Active) {
        fmt::speed(speed)
    } else if let Some(p) = place {
        p.map_or_else(
            || "queued at peer".into(),
            |n| format!("queued at peer #{n}"),
        )
    } else if rows.iter().any(|t| t.state == TState::Queued) {
        "queued".into()
    } else if failed > 0 {
        format!("{failed} failed")
    } else {
        "done".into()
    };
    format!(
        "{done} of {} · {} · {status}",
        rows.len(),
        of_size(bytes, total)
    )
}

/// "212 / 512 MB": the unit once when both sides share it.
pub fn of_size(part: u64, whole: u64) -> String {
    let (a, b) = (fmt::size(part), fmt::size(whole));
    match (a.rsplit_once(' '), b.rsplit_once(' ')) {
        (Some((n, ua)), Some((_, ub))) if ua == ub => format!("{n} / {b}"),
        _ => format!("{a} / {b}"),
    }
}
