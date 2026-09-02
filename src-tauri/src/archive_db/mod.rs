//! Grid's own local, normalized `SQLite` archive (spec §20-21).
//!
//! This is the permanent data layer behind Home/Insights aggregates
//! (repeated failures, large agent runs, usage stats) -- deliberately
//! separate from `commands::archive`, which is a different, pre-existing
//! feature (user-triggered manual session copy/export) that spec §23
//! flags for removal.
//!
//! Read-only guarantee: this module only ever reads provider session
//! files (via the existing, already-read-only provider parsers) and
//! writes exclusively to its own database file under `~/.grid-local/`,
//! never into a provider's own directory.

pub mod backfill;
pub mod hash;
pub mod history;
pub mod ingest;
pub mod insights;
pub mod migrate;
pub mod schema;
pub mod search;
#[cfg(test)]
pub(crate) mod test_support;

use rusqlite::Connection;
use std::path::PathBuf;

/// Path to Grid's own archive database: `~/.grid-local/archive.db`.
pub fn archive_db_path() -> Result<PathBuf, String> {
    let home = crate::utils::resolve_home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".grid-local").join("archive.db"))
}

/// Opens (creating if necessary) the archive database at
/// [`archive_db_path`], applies pragmas, and runs pending migrations.
pub fn open_connection() -> Result<Connection, String> {
    let path = archive_db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create Grid Local data directory: {e}"))?;
    }
    open_connection_at(&path)
}

/// Opens a connection at an explicit path, applies pragmas, and runs
/// pending migrations. Split out from [`open_connection`] so tests (and
/// `benches/performance.rs`) can point
/// at a temp-directory path without touching `~/.grid-local/`.
pub fn open_connection_at(path: &std::path::Path) -> Result<Connection, String> {
    let mut conn =
        Connection::open(path).map_err(|e| format!("Failed to open archive database: {e}"))?;
    // `synchronous = NORMAL` is SQLite's own documented pairing for WAL mode
    // (https://www.sqlite.org/pragma.html#pragma_synchronous): WAL already
    // makes the database crash-safe without an fsync on every commit, so
    // the only risk NORMAL adds over the (unset, so compile-time-default
    // FULL) prior behavior is losing the most recent commit(s) after an
    // OS crash or power loss -- never corruption. That's an acceptable
    // trade for archive_db specifically: every row here is a fully
    // rebuildable derived cache of the real provider files (never the
    // source of truth), recoverable with the existing "Rebuild index"
    // Settings action. Left unset, every one of `upsert_provider`/
    // `upsert_project`'s autocommitted statements (called once per
    // project) and every session's own commit in `persist_session_messages`
    // fsyncs individually -- confirmed live to be the dominant cost of a
    // full "Rebuild index" run, not the parsing itself.
    conn.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
    )
    .map_err(|e| format!("Failed to set archive database pragmas: {e}"))?;
    migrate::migrate(&mut conn).map_err(|e| format!("Failed to migrate archive database: {e}"))?;
    Ok(conn)
}

/// Every provider `archive_db` has ever ingested, with the coverage tier
/// recorded at ingest time
/// -- see `ingest::upsert_provider`'s own `tier` argument for what sets
/// this ("A" for Claude's native parser, "B" for every generic file-based
/// provider today). A provider never ingested (no project scanned into
/// the archive yet) simply has no row here -- callers should treat an
/// absent `provider_key` as "coverage unknown," not "no coverage."
pub fn list_provider_tiers(conn: &Connection) -> Result<Vec<crate::models::ProviderTier>, String> {
    let mut stmt = conn
        .prepare("SELECT provider_key, tier FROM provider")
        .map_err(|e| format!("Failed to prepare provider tier query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(crate::models::ProviderTier {
                provider_key: row.get(0)?,
                tier: row.get(1)?,
            })
        })
        .map_err(|e| format!("Failed to run provider tier query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read provider tier row: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `search_fts` (migration 6) is a standard FTS5 virtual
    /// table with no `content=`/`contentless_delete` option, so `SQLite`
    /// creates its own shadow storage alongside the virtual table's own
    /// `sqlite_master` entry: `search_fts_data`, `search_fts_idx`,
    /// `search_fts_docsize`, `search_fts_config`, `search_fts_content` --
    /// all `type = 'table'`, confirmed empirically (a raw `sqlite_master`
    /// table count came back 5 higher than `TABLE_NAMES.len()`, which
    /// lists `search_fts` itself once, matching every other schema-
    /// defined name's convention).
    const FTS5_SHADOW_TABLE_COUNT: usize = 5;

    #[test]
    fn open_connection_at_creates_and_migrates_a_fresh_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("archive.db");

        let conn = open_connection_at(&db_path).unwrap();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table_count as usize,
            schema::TABLE_NAMES.len() + FTS5_SHADOW_TABLE_COUNT
        );
    }

    #[test]
    fn open_connection_at_is_idempotent_across_reopens() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("archive.db");

        {
            let _conn = open_connection_at(&db_path).unwrap();
        }
        // Reopening an already-migrated database must not error.
        let conn = open_connection_at(&db_path).unwrap();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table_count as usize,
            schema::TABLE_NAMES.len() + FTS5_SHADOW_TABLE_COUNT
        );
    }

    #[test]
    fn archive_db_path_lives_under_grid_local() {
        let path = archive_db_path().unwrap();
        assert!(
            path.ends_with(std::path::Path::new(".grid-local").join("archive.db")),
            "archive_db_path() should end with .grid-local/archive.db, got {}",
            path.display()
        );
    }

    /// A read-only-guarantee boundary test. `archive_db_path()`
    /// must never resolve under any provider's own storage root -- Grid's
    /// archive is a separate, Grid-owned file, never mixed into a provider's
    /// directory (spec §22/§46).
    #[test]
    fn archive_db_path_never_falls_under_a_known_provider_root() {
        let path = archive_db_path().unwrap();
        let home = dirs::home_dir().unwrap();
        let provider_roots = [
            home.join(".claude"),
            home.join(".codex"),
            home.join(".gemini"),
            home.join(".cursor"),
            home.join(".cline"),
            home.join(".continue"),
        ];
        for root in &provider_roots {
            assert!(
                !path.starts_with(root),
                "archive_db_path() {} must not live under provider root {}",
                path.display(),
                root.display()
            );
        }
    }

    /// "Rebuild index" (spec §31) must only ever rewrite Grid's own archive
    /// database -- never any provider-owned session file, even though it
    /// reads through the exact same discovery/parsing path as an ordinary
    /// backfill.
    #[tokio::test]
    #[serial_test::serial]
    async fn rebuild_index_truncates_only_grid_owned_rows() {
        const TOOL_USE_LINE: &str = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":100,"output_tokens":20}}}"#;
        const ERROR_RESULT_LINE: &str = r#"{"uuid":"u2","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"FAILED","is_error":true}]}}"#;

        // See archive_db::test_support's own doc comment: run_full_backfill
        // also scans every file-based provider since Step 5, and this
        // machine has real, substantial Codex data under ~/.codex.
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();

        let db_dir = tempfile::TempDir::new().unwrap();
        let claude_dir = tempfile::TempDir::new().unwrap();

        let project_dir = claude_dir.path().join("projects").join("proj");
        std::fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session1.jsonl");
        std::fs::write(
            &session_path,
            format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        )
        .unwrap();

        let mut conn = open_connection_at(&db_dir.path().join("archive.db")).unwrap();
        let claude_base = claude_dir.path().to_string_lossy().to_string();
        backfill::run_full_backfill(&mut conn, &claude_base)
            .await
            .unwrap();

        let before_mtime = std::fs::metadata(&session_path).unwrap().modified().unwrap();
        let before_contents = std::fs::read(&session_path).unwrap();

        backfill::rebuild_index(&mut conn, &claude_base).await.unwrap();

        let after_mtime = std::fs::metadata(&session_path).unwrap().modified().unwrap();
        let after_contents = std::fs::read(&session_path).unwrap();
        assert_eq!(
            before_mtime, after_mtime,
            "rebuild_index must not touch the provider's own session file's mtime"
        );
        assert_eq!(
            before_contents, after_contents,
            "rebuild_index must not modify the provider's own session file's contents"
        );

        // Scoped to Claude specifically -- a handful of real Antigravity
        // sessions may incidentally appear on this machine (see
        // archive_db::test_support's own doc comment for why that can't be
        // neutralized on Windows) and must not make this assertion flaky.
        let claude_session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session s
                 JOIN project p ON p.id = s.project_id
                 JOIN provider pr ON pr.id = p.provider_id
                 WHERE pr.provider_key = 'claude'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            claude_session_count, 1,
            "rebuild_index must still have genuinely re-populated Claude's own data in the DB"
        );
    }

    /// Ingesting a Claude project must only ever write Grid's own archive
    /// database (opened explicitly here as an in-memory connection, never
    /// via `open_connection()`) and Grid's own session-metadata cache
    /// (`~/.grid-local/session-cache/`, relocated off the provider directory
    /// in Step 0) -- never anything inside the provider's own project
    /// directory. Gated to unix, matching this codebase's established
    /// precedent for `$HOME`-mocked filesystem-location tests (see
    /// `commands::session::load`'s own `with_home`-based tests) --
    /// `dirs::home_dir()`'s `$HOME`-override behavior on Windows is not
    /// reliably exercised by this pattern.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn ingest_never_writes_outside_grid_local() {
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }

        fn file_mtimes(
            root: &std::path::Path,
        ) -> std::collections::HashMap<std::path::PathBuf, std::time::SystemTime> {
            walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_file())
                .map(|e| (e.path().to_path_buf(), e.metadata().unwrap().modified().unwrap()))
                .collect()
        }

        const TOOL_USE_LINE: &str = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":100,"output_tokens":20}}}"#;
        const ERROR_RESULT_LINE: &str = r#"{"uuid":"u2","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"FAILED","is_error":true}]}}"#;

        let home = tempfile::TempDir::new().unwrap();
        let project_dir = home.path().join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join("session1.jsonl"),
            format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        )
        .unwrap();

        let before = file_mtimes(home.path());

        let restore = RestoreHome(std::env::var_os("HOME"));
        std::env::set_var("HOME", home.path());

        let mut conn = Connection::open_in_memory().unwrap();
        migrate::migrate(&mut conn).unwrap();
        let result =
            ingest::ingest_claude_project(&mut conn, &project_dir.to_string_lossy()).await;
        drop(restore);
        result.unwrap();

        let after = file_mtimes(home.path());
        let grid_local = home.path().join(".grid-local");
        for (path, mtime) in &after {
            if path.starts_with(&grid_local) {
                continue;
            }
            match before.get(path) {
                Some(prev_mtime) => assert_eq!(
                    prev_mtime,
                    mtime,
                    "ingest modified a file outside .grid-local: {}",
                    path.display()
                ),
                None => panic!(
                    "ingest created a new file outside .grid-local: {}",
                    path.display()
                ),
            }
        }
        assert!(
            grid_local.join("session-cache").is_dir(),
            "expected the session-metadata cache to actually be exercised under .grid-local, \
             otherwise this test would pass vacuously"
        );
    }

    #[test]
    fn list_provider_tiers_returns_every_ingested_providers_own_tier() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate::migrate(&mut conn).unwrap();
        ingest::upsert_provider(&conn, "claude", "Claude Code", "A", 1).unwrap();
        ingest::upsert_provider(&conn, "codex", "Codex CLI", "B", 1).unwrap();

        let mut tiers = list_provider_tiers(&conn).unwrap();
        tiers.sort_by(|a, b| a.provider_key.cmp(&b.provider_key));
        assert_eq!(
            tiers,
            vec![
                crate::models::ProviderTier {
                    provider_key: "claude".to_string(),
                    tier: "A".to_string()
                },
                crate::models::ProviderTier {
                    provider_key: "codex".to_string(),
                    tier: "B".to_string()
                },
            ]
        );
    }
}
