//! Shared test-only helpers for `archive_db` tests that call
//! `backfill::run_full_backfill`/`rebuild_index`, needed since the
//! universal-provider-ingestion plan's Step 5 (`archive_db::backfill`'s
//! own commit) made those functions ALSO scan every file-based provider,
//! not just Claude.
//!
//! **Why this exists**: this specific dev machine has real, substantial
//! data for several providers under real `$HOME`-resolved directories --
//! notably hundreds of real Codex sessions under `~/.codex`, and a
//! handful of real Antigravity sessions under `~/.gemini/antigravity`
//! (resolved independently of any overridable env var, and `dirs::
//! home_dir()` does not reliably honor a test-mocked `$HOME` on Windows,
//! confirmed live this session). Every `archive_db` test that seeds a
//! `Connection` via `run_full_backfill`/`rebuild_index` -- across
//! `backfill.rs`, `mod.rs`, `insights.rs`, and `history.rs`'s own test
//! modules -- must therefore either (a) not assert exact GLOBAL row
//! counts (scope to `provider_key = 'claude'` instead, which this module
//! doesn't help with directly -- do it at each call site), or (b) guard
//! the loudest, most controllable source of incidental data
//! (`CODEX_HOME`) to an empty directory, which THIS module does provide.
//! Antigravity's small amount of real data cannot be neutralized this way
//! on Windows -- tests must still be written to tolerate it.

use std::path::Path;

/// Saves/restores one process-global env var around a test. Combine with
/// `#[serial_test::serial]` so tests touching the SAME env var don't race
/// each other -- same pattern already established independently in
/// `archive_db::ingest::provider`'s and several individual providers'
/// (`providers::pi`, `providers::codex`, ...) own test modules; this is
/// the shared copy for `archive_db`-level tests that need it.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    pub(crate) fn set(key: &'static str, path: &Path) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, path);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.original.as_ref() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Points `CODEX_HOME` at a fresh, empty `TempDir` for the lifetime of the
/// returned guard pair -- neutralizes the single largest source of
/// incidental real data this machine has (hundreds of real Codex
/// sessions), both for correctness (keeps exact-count assertions valid)
/// and for speed (avoids re-parsing that real data on every affected
/// test). Callers must hold BOTH returned values for the guard's
/// lifetime (the `TempDir` deletes on drop; dropping it early while the
/// `EnvVarGuard` is still live would point `CODEX_HOME` at a deleted
/// directory).
pub(crate) fn empty_codex_home_guard() -> (tempfile::TempDir, EnvVarGuard) {
    let dir = tempfile::TempDir::new().unwrap();
    let guard = EnvVarGuard::set("CODEX_HOME", dir.path());
    (dir, guard)
}
