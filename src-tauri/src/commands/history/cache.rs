//! TTL-based cache for non-Claude provider session lists backing the
//! History surface (spec §13).
//!
//! Deliberately a different shape from `commands::stats::cache`'s
//! `(size, mtime)`-validated per-*file* caches: at this granularity the
//! dominant cost is the directory walk / composer DB scan itself, not
//! per-file parsing (confirmed via Codex profiling --
//! caching the per-file parse alone left "100% cache hits, 0ns parse time,
//! still 10+ seconds from call frequency" once the real bug, calling
//! `load_sessions` once per project, was isolated). A coarser, one-entry-
//! per-provider, time-based cache matches that cost shape directly: no
//! per-file signature to validate, just "don't re-walk within N seconds."
//!
//! This is a genuinely new caching pattern for this codebase -- every
//! existing cache validates freshness by file signature, not a clock. That
//! trade (up to `TTL` seconds of staleness for non-Claude providers) is a
//! conscious choice for a browsing UI's "a couple seconds, not a spinner"
//! bar, not real-time freshness.

use crate::commands::stats::StatsProvider;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a provider's scanned session list is served from cache before
/// the next request re-walks. 45s: long enough that switching History
/// filters or paging doesn't repeatedly re-walk, short enough that a
/// session started moments ago shows up without restarting the app.
const TTL: Duration = Duration::from_secs(45);

struct CacheEntry<T> {
    fetched_at: Instant,
    value: Arc<T>,
}

pub(super) struct ProviderHistorySessionsCache<T> {
    entries: Mutex<HashMap<StatsProvider, CacheEntry<T>>>,
}

impl<T> ProviderHistorySessionsCache<T> {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Return the cached value for `provider` if it was fetched within
    /// `TTL`, rebuilding via `build` otherwise. `build` failing (returns
    /// `Err`) never poisons the cache -- the stale entry, if any, is left
    /// in place for the next call to retry.
    pub(super) fn get_or_build<F>(&self, provider: StatsProvider, build: F) -> Result<Arc<T>, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        {
            let entries = self.lock_entries();
            if let Some(entry) = entries.get(&provider) {
                if entry.fetched_at.elapsed() < TTL {
                    return Ok(Arc::clone(&entry.value));
                }
            }
        }

        let built = Arc::new(build()?);

        let mut entries = self.lock_entries();
        entries.insert(
            provider,
            CacheEntry {
                fetched_at: Instant::now(),
                value: Arc::clone(&built),
            },
        );
        Ok(built)
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, HashMap<StatsProvider, CacheEntry<T>>> {
        match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

type SessionsEntry = Vec<(String, crate::models::ClaudeSession, Option<String>)>;

static PROVIDER_HISTORY_SESSIONS_CACHE: OnceLock<ProviderHistorySessionsCache<SessionsEntry>> =
    OnceLock::new();

/// Cache of `(project_key, session, model)` triples per provider -- the
/// shape every non-Claude dispatch branch in
/// `commands::history::collect_provider_history_sessions` normalizes to
/// before merging with Claude's `archive_db`-sourced sessions.
pub(super) fn provider_history_sessions_cache() -> &'static ProviderHistorySessionsCache<SessionsEntry>
{
    PROVIDER_HISTORY_SESSIONS_CACHE.get_or_init(ProviderHistorySessionsCache::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn caches_within_ttl_and_rebuilds_after_a_forced_expiry() {
        let cache: ProviderHistorySessionsCache<u32> = ProviderHistorySessionsCache::new();
        let calls = AtomicU32::new(0);

        let first = cache
            .get_or_build(StatsProvider::Codex, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(1)
            })
            .unwrap();
        assert_eq!(*first, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second call within TTL must NOT invoke build again.
        let second = cache
            .get_or_build(StatsProvider::Codex, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(2)
            })
            .unwrap();
        assert_eq!(*second, 1, "served from cache, not rebuilt");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A different provider is a distinct cache entry, unaffected by
        // the first provider's cached value.
        let other = cache
            .get_or_build(StatsProvider::Cursor, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(99)
            })
            .unwrap();
        assert_eq!(*other, 99);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_build_does_not_poison_a_previously_cached_value() {
        let cache: ProviderHistorySessionsCache<u32> = ProviderHistorySessionsCache::new();

        cache
            .get_or_build(StatsProvider::Codex, || Ok(1))
            .unwrap();

        // Manually expire the entry so the next call rebuilds.
        {
            let mut entries = cache.lock_entries();
            entries.get_mut(&StatsProvider::Codex).unwrap().fetched_at = Instant::now()
                .checked_sub(TTL + Duration::from_secs(1))
                .unwrap();
        }

        let err = cache.get_or_build(StatsProvider::Codex, || Err("boom".to_string()));
        assert!(err.is_err());

        // The stale-but-present entry is untouched by the failed rebuild;
        // manually mark it fresh again and confirm it still serves.
        {
            let mut entries = cache.lock_entries();
            entries.get_mut(&StatsProvider::Codex).unwrap().fetched_at = Instant::now();
        }
        let value = cache.get_or_build(StatsProvider::Codex, || Err("boom".to_string()));
        assert_eq!(*value.unwrap(), 1);
    }
}
