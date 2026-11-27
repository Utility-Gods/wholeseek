use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

pub const RESULT_TTL_SECS: u64 = 6 * 3600;
pub const RESULT_SEARCHES: usize = 20;
pub const DEFAULT_SEARCH_INTERVAL_SECS: u64 = 15;
pub const DEFAULT_SEARCHES_PER_HOUR: usize = 40;
const HOUR: u64 = 3600;

/// Caller supplies epoch seconds so deadlines survive an owner restart. A
/// backwards clock step does not expire any existing entries early.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultCache<T> {
    next_search: u64,
    batches: VecDeque<Batch<T>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Batch<T> {
    id: u64,
    created_at: u64,
    groups: Vec<T>,
}

impl<T> Default for ResultCache<T> {
    fn default() -> Self {
        Self {
            next_search: 1,
            batches: VecDeque::new(),
        }
    }
}

impl<T> ResultCache<T> {
    pub fn insert(&mut self, now: u64, groups: Vec<T>) -> Vec<String> {
        self.batches
            .retain(|b| now.saturating_sub(b.created_at) < RESULT_TTL_SECS);
        let id = self.next_search;
        self.next_search = self
            .next_search
            .checked_add(1)
            .expect("search id space exhausted");
        let ids = (1..=groups.len()).map(|n| format!("{id}.{n}")).collect();
        self.batches.push_back(Batch {
            id,
            created_at: now,
            groups,
        });
        while self.batches.len() > RESULT_SEARCHES {
            self.batches.pop_front();
        }
        ids
    }

    pub fn get(&self, now: u64, id: &str) -> Option<&T> {
        let (batch, group) = id.split_once('.')?;
        let batch = batch.parse::<u64>().ok()?;
        let group = group.parse::<usize>().ok()?.checked_sub(1)?;
        self.batches
            .iter()
            .find(|b| b.id == batch && now.saturating_sub(b.created_at) < RESULT_TTL_SECS)?
            .groups
            .get(group)
    }
}

/// Timestamp buckets for a rolling hour. The map contains at most one bucket
/// per second; a restart does not give the CLI a fresh quota.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SearchRate {
    attempts: BTreeMap<u64, usize>,
}

impl SearchRate {
    /// Reserve a search or return the number of seconds until it can be tried.
    /// Rejected attempts do not consume additional quota.
    pub fn reserve(&mut self, now: u64, interval_secs: u64, per_hour: usize) -> Result<(), u64> {
        let interval_secs = interval_secs.max(1);
        let per_hour = per_hour.max(1);
        let last = self.attempts.last_key_value().map(|(&at, _)| at);
        self.attempts.retain(|&at, _| now.saturating_sub(at) < HOUR);
        let interval_ready = last.map_or(now, |at| at.saturating_add(interval_secs));
        let count: usize = self.attempts.values().sum();
        let mut hourly_ready = now;
        if count >= per_hour {
            let mut must_expire = count - per_hour + 1;
            for (&at, &n) in &self.attempts {
                if n >= must_expire {
                    hourly_ready = at.saturating_add(HOUR);
                    break;
                }
                must_expire -= n;
            }
        }
        let delay = interval_ready.max(hourly_ready).saturating_sub(now);
        if delay > 0 {
            return Err(delay);
        }
        *self.attempts.entry(now).or_default() += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ids_survive_reload_until_ttl() {
        let mut cache = ResultCache::default();
        let ids = cache.insert(100, vec!["a".to_string(), "b".to_string()]);
        let cache: ResultCache<String> =
            serde_json::from_str(&serde_json::to_string(&cache).unwrap()).unwrap();
        assert_eq!(ids, ["1.1", "1.2"]);
        assert_eq!(
            cache
                .get(100 + RESULT_TTL_SECS - 1, &ids[1])
                .map(String::as_str),
            Some("b")
        );
        assert_eq!(cache.get(100 + RESULT_TTL_SECS, &ids[1]), None);
        assert_eq!(cache.get(100, "1.0"), None);
    }

    #[test]
    fn oldest_search_is_evicted_without_reusing_id() {
        let mut cache = ResultCache::default();
        let oldest = cache.insert(100, vec![1]).remove(0);
        for _ in 0..RESULT_SEARCHES {
            let next = cache.insert(100, vec![2]).remove(0);
            assert_ne!(oldest, next);
        }
        assert_eq!(cache.get(100, &oldest), None);
    }

    #[test]
    fn rate_limit_checks_interval_and_rolling_quota_after_reload() {
        let mut rate = SearchRate::default();
        assert_eq!(rate.reserve(100, 15, 2), Ok(()));
        assert_eq!(rate.reserve(114, 15, 2), Err(1));
        assert_eq!(rate.reserve(115, 15, 2), Ok(()));
        let mut rate: SearchRate =
            serde_json::from_str(&serde_json::to_string(&rate).unwrap()).unwrap();
        assert_eq!(rate.reserve(116, 15, 2), Err(3584));
        assert_eq!(rate.reserve(3700, 15, 2), Ok(()));
        assert_eq!(rate.reserve(3701, 15, 2), Err(14));
        assert_eq!(rate.reserve(3715, 15, 2), Ok(()));
    }
}
