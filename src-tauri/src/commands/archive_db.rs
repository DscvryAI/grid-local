//! Tauri commands exposing Grid's own normalized archive (`archive_db`) to
//! the frontend: triggering a backfill/rebuild, reporting basic status, and
//! deleting Grid's own copy (spec §31's Data section). No persistent
//! connection is held in app state -- each command opens its own
//! short-lived connection (a local `SQLite` file open/close is cheap, and
//! this avoids the Send/Sync complexity of holding a `rusqlite::Connection`
//! across `.await` points in shared app state).

use crate::archive_db::{
    self,
    backfill::{BackfillPhase, BackfillProgressHooks, BackfillSummary},
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Emitter;

/// Runs (or re-runs) a full backfill of Claude Code history into Grid's
/// normalized archive. Idempotent -- safe to call on every app launch.
#[tauri::command]
pub async fn sync_grid_index() -> Result<BackfillSummary, String> {
    let claude_base = crate::providers::claude::get_base_path()
        .ok_or("Could not resolve the Claude Code base directory")?;
    let mut conn = archive_db::open_connection()?;
    archive_db::backfill::run_full_backfill(&mut conn, &claude_base).await
}

/// Shared cancellation flag for an in-progress [`run_first_index`] call.
/// Managed Tauri state -- one flag app-wide is enough since only one
/// first-run index can ever be in flight (it runs once, during
/// `initializeApp`, before the rest of the UI is usable).
#[derive(Default)]
pub struct FirstIndexCancelFlag(pub Arc<AtomicBool>);

/// Progress payload emitted on the `"first-index-progress"` event while
/// [`run_first_index`] is running.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FirstIndexProgressEvent {
    provider_key: String,
    phases_done: usize,
    phases_total: usize,
}

/// The mandatory, interactive first-run index: unlike
/// [`sync_grid_index`], this reports per-provider
/// progress via the `"first-index-progress"` event and can be stopped
/// early via [`cancel_first_index`]. Only ever called once per app
/// launch, when `archive_db` has never been populated -- see
/// `initializeApp` on the frontend for the gating logic.
#[tauri::command]
pub async fn run_first_index(
    app: tauri::AppHandle,
    cancel_flag: tauri::State<'_, FirstIndexCancelFlag>,
) -> Result<BackfillSummary, String> {
    let claude_base = crate::providers::claude::get_base_path()
        .ok_or("Could not resolve the Claude Code base directory")?;
    let mut conn = archive_db::open_connection()?;

    // Reset before starting: a stale `true` left over from a previous
    // (already-finished) call must never cancel a fresh one.
    cancel_flag.0.store(false, Ordering::SeqCst);

    let on_phase_start = |phase: BackfillPhase| {
        let _ = app.emit(
            "first-index-progress",
            FirstIndexProgressEvent {
                provider_key: phase.provider_key.to_string(),
                phases_done: phase.phases_done,
                phases_total: phase.phases_total,
            },
        );
    };
    let cancel_flag_ref = cancel_flag.0.clone();
    let should_cancel = move || cancel_flag_ref.load(Ordering::SeqCst);
    let hooks = BackfillProgressHooks {
        on_phase_start: Some(&on_phase_start),
        should_cancel: Some(&should_cancel),
    };

    archive_db::backfill::run_full_backfill_with_hooks(&mut conn, &claude_base, &hooks).await
}

/// Signals an in-progress [`run_first_index`] call to stop before its next
/// provider phase. Cooperative, not preemptive -- the current phase always
/// finishes first, so no provider is ever left half-ingested.
#[tauri::command]
pub fn cancel_first_index(cancel_flag: tauri::State<'_, FirstIndexCancelFlag>) {
    cancel_flag.0.store(true, Ordering::SeqCst);
}

/// Wipes and fully re-ingests Grid's own Claude archive data. This is the
/// user-facing "Rebuild index" Settings action (spec §31) -- never
/// touches Claude Code's own files, only Grid's own database.
#[tauri::command]
pub async fn rebuild_grid_index() -> Result<BackfillSummary, String> {
    let claude_base = crate::providers::claude::get_base_path()
        .ok_or("Could not resolve the Claude Code base directory")?;
    let mut conn = archive_db::open_connection()?;
    archive_db::backfill::rebuild_index(&mut conn, &claude_base).await
}

/// Lightweight counts for a future Settings/status panel.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveDbStatus {
    pub provider_count: i64,
    pub project_count: i64,
    pub session_count: i64,
    pub message_count: i64,
}

#[tauri::command]
pub async fn get_archive_db_status() -> Result<ArchiveDbStatus, String> {
    let conn = archive_db::open_connection()?;
    let count = |table: &str| -> Result<i64, String> {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("Failed to count `{table}`: {e}"))
    };
    Ok(ArchiveDbStatus {
        provider_count: count("provider")?,
        project_count: count("project")?,
        session_count: count("session")?,
        message_count: count("message")?,
    })
}

/// Deletes Grid's own REBUILDABLE archive data: `archive.db` (plus its
/// `SQLite` WAL/SHM sidecar files, if present) and everything under
/// `session-cache/`. This is the user-facing "Delete Grid's local data"
/// Settings action (spec §31) -- "deleting Grid data deletes Grid's copy
/// only, never provider history" holds by construction here (only
/// `~/.grid-local`-owned paths are touched, and every one of them is
/// naturally reconstructed by the existing backfill/watcher on next use).
///
/// Deliberately does NOT touch `user-data.json` (project/session metadata,
/// hidden-project patterns, the archive-sync preference itself, etc.) or
/// `presets/` (the separate, unrelated Settings-preset feature) -- spec's
/// own framing groups this action under "Data" alongside "Grid archive
/// location"/"Rebuild index", i.e. scoped to the archive/index specifically,
/// not a nuclear wipe of every file Grid has ever written. Named paths are
/// removed explicitly rather than recursively wiping the whole
/// `~/.grid-local` directory, so a future unrelated file living there can't
/// be silently swept up by accident.
#[tauri::command]
pub async fn delete_grid_local_data() -> Result<(), String> {
    let db_path = archive_db::archive_db_path()?;
    let cache_dir = crate::commands::session::get_session_cache_dir();
    delete_grid_local_data_at(&db_path, cache_dir.as_deref())
}

/// Pure, path-injectable core of [`delete_grid_local_data`] -- split out so
/// tests can point at a temp directory instead of the real `~/.grid-local`,
/// matching `archive_db::open_connection_at`'s own established convention
/// for this exact reason.
fn delete_grid_local_data_at(
    db_path: &std::path::Path,
    cache_dir: Option<&std::path::Path>,
) -> Result<(), String> {
    for path in [
        db_path.to_path_buf(),
        db_path.with_extension("db-wal"),
        db_path.with_extension("db-shm"),
    ] {
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
        }
    }

    if let Some(cache_dir) = cache_dir {
        if cache_dir.exists() {
            std::fs::remove_dir_all(cache_dir)
                .map_err(|e| format!("Failed to delete {}: {e}", cache_dir.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn delete_grid_local_data_at_removes_db_wal_shm_and_cache_dir_but_nothing_else() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("archive.db");
        let wal_path = dir.path().join("archive.db-wal");
        let shm_path = dir.path().join("archive.db-shm");
        let cache_dir = dir.path().join("session-cache");
        let user_data_path = dir.path().join("user-data.json");
        let presets_dir = dir.path().join("presets");

        fs::write(&db_path, b"db").unwrap();
        fs::write(&wal_path, b"wal").unwrap();
        fs::write(&shm_path, b"shm").unwrap();
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("some-project.json"), b"{}").unwrap();
        fs::write(&user_data_path, b"{}").unwrap();
        fs::create_dir_all(&presets_dir).unwrap();
        fs::write(presets_dir.join("my-preset.json"), b"{}").unwrap();

        delete_grid_local_data_at(&db_path, Some(&cache_dir)).unwrap();

        assert!(!db_path.exists(), "archive.db should be deleted");
        assert!(!wal_path.exists(), "archive.db-wal should be deleted");
        assert!(!shm_path.exists(), "archive.db-shm should be deleted");
        assert!(!cache_dir.exists(), "session-cache/ should be deleted");
        assert!(user_data_path.exists(), "user-data.json must be preserved");
        assert!(presets_dir.exists(), "presets/ must be preserved");
    }

    #[test]
    fn delete_grid_local_data_at_is_a_no_op_on_a_fresh_install_with_nothing_to_delete() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("archive.db");
        let cache_dir = dir.path().join("session-cache");

        // Nothing exists yet -- must not error just because there was
        // nothing to clean up.
        delete_grid_local_data_at(&db_path, Some(&cache_dir)).unwrap();
    }

    #[test]
    fn delete_grid_local_data_at_tolerates_no_cache_dir_resolved() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("archive.db");
        fs::write(&db_path, b"db").unwrap();

        delete_grid_local_data_at(&db_path, None).unwrap();
        assert!(!db_path.exists());
    }
}
