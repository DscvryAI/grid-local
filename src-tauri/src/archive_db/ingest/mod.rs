//! Ingestion orchestration: resolves/creates `provider`/`project` rows and
//! walks a project's sessions, delegating per-session extraction to
//! [`claude`] (Claude) or [`provider`] (every other, file-based provider --
//! see the universal-provider-ingestion plan at
//! `C:\Users\sharad\.claude-work-3\plans\serialized-launching-hippo.md`).

pub mod claude;
pub mod provider;

use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use crate::models::{ClaudeMessage, ClaudeSession};

/// Outcome of ingesting a single session file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    pub messages_ingested: u64,
    pub skipped_unchanged: bool,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Inserts or touches a `provider` row, returning its id. `parser_version`
/// bump forces re-ingestion of every already-ingested session for this
/// provider on the next backfill (mirrors `CACHE_VERSION`'s role for the
/// pre-existing `.session_cache.json` cache).
pub fn upsert_provider(
    conn: &Connection,
    provider_key: &str,
    display_name: &str,
    tier: &str,
    parser_version: i64,
) -> Result<i64, String> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO provider (provider_key, display_name, tier, parser_version, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(provider_key) DO UPDATE SET
           display_name = excluded.display_name,
           tier = excluded.tier,
           parser_version = excluded.parser_version,
           last_seen_at = excluded.last_seen_at",
        rusqlite::params![provider_key, display_name, tier, parser_version, now],
    )
    .map_err(|e| format!("Failed to upsert provider `{provider_key}`: {e}"))?;

    conn.query_row(
        "SELECT id FROM provider WHERE provider_key = ?1",
        [provider_key],
        |row| row.get(0),
    )
    .map_err(|e| format!("Failed to read back provider `{provider_key}` id: {e}"))
}

/// Inserts or touches a `project` row, returning its id.
pub fn upsert_project(
    conn: &Connection,
    provider_id: i64,
    project_key: &str,
    display_name: &str,
    actual_path: Option<&str>,
) -> Result<i64, String> {
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO project (provider_id, project_key, display_name, actual_path, first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(provider_id, project_key) DO UPDATE SET
           display_name = excluded.display_name,
           actual_path = excluded.actual_path,
           last_seen_at = excluded.last_seen_at",
        rusqlite::params![provider_id, project_key, display_name, actual_path, now],
    )
    .map_err(|e| format!("Failed to upsert project `{project_key}`: {e}"))?;

    conn.query_row(
        "SELECT id FROM project WHERE provider_id = ?1 AND project_key = ?2",
        rusqlite::params![provider_id, project_key],
        |row| row.get(0),
    )
    .map_err(|e| format!("Failed to read back project `{project_key}` id: {e}"))
}

/// `(file_size, file_mtime)` for a session's own content file -- the
/// idempotency signal every file-based provider's ingest (Claude and,
/// per the universal-provider-ingestion plan, the 17 other file-based
/// providers) compares against `existing_session_signature`'s stored
/// value to decide whether a session needs re-parsing at all.
pub(super) fn stat_signature(path: &Path) -> Result<(u64, u64), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to stat session file {}: {e}", path.display()))?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok((metadata.len(), mtime))
}

/// Persists an already-loaded session's messages as normalized rows --
/// the provider-agnostic half of ingestion. Extracted out of what used to
/// be Claude-only `ingest_claude_session_file` (universal-provider-
/// ingestion plan, Step 1): everything here operates purely on the shared
/// `ClaudeMessage`/`ClaudeSession` DTOs and their already-normalized
/// content blocks (`{"type":"tool_use"|"tool_result",...}`, confirmed to
/// match Claude's own shape across every provider checked so far -- see
/// the plan file), with no Claude-specific logic. Callers own staleness
/// checking and message loading (both provider-specific) and pass the
/// already-loaded `messages` in.
pub(super) fn persist_session_messages(
    conn: &mut Connection,
    project_id: i64,
    session: &ClaudeSession,
    messages: &[ClaudeMessage],
    file_size: u64,
    file_mtime: u64,
    parser_version: i64,
) -> Result<IngestOutcome, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    if let Some((existing_row_id, _, _, _)) =
        existing_session_signature(&tx, project_id, &session.session_id)?
    {
        delete_session_rows(&tx, existing_row_id)?;
    }

    let now = now_rfc3339();
    // SQLite has no unsigned integer type; these are all realistically far
    // below i64::MAX (file sizes/mtimes/message counts), so a saturating
    // conversion is a safe, simple choice over threading `Result`s through.
    let message_count_i64 = i64::try_from(session.message_count).unwrap_or(i64::MAX);
    let file_size_i64 = i64::try_from(file_size).unwrap_or(i64::MAX);
    let file_mtime_i64 = i64::try_from(file_mtime).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO session (
            project_id, session_key, actual_session_id, file_path,
            first_message_time, last_message_time, last_modified,
            message_count, has_tool_use, has_errors, summary, entrypoint,
            file_size, file_mtime, last_byte_offset, parser_version, last_ingested_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        rusqlite::params![
            project_id,
            session.session_id,
            session.actual_session_id,
            session.file_path,
            session.first_message_time,
            session.last_message_time,
            session.last_modified,
            message_count_i64,
            session.has_tool_use,
            session.has_errors,
            session.summary,
            session.entrypoint,
            file_size_i64,
            file_mtime_i64,
            file_size_i64,
            parser_version,
            now,
        ],
    )
    .map_err(|e| format!("Failed to insert session row: {e}"))?;
    let session_row_id = tx.last_insert_rowid();

    // Maps a tool_use block's `id` to the `tool_call` row it produced, so a
    // later message's `tool_result` block (matched by `tool_use_id`) can be
    // linked back to it. Built incrementally in file order: a tool_use
    // always precedes its tool_result in real transcripts.
    let mut tool_call_rows: HashMap<String, (i64, String)> = HashMap::new();

    for message in messages {
        let message_row_id = claude::insert_message(&tx, session_row_id, message)?;

        if let Some(usage) = &message.usage {
            claude::insert_usage(&tx, message_row_id, session_row_id, message, usage)?;
        }

        let Some(content_array) = message
            .content
            .as_ref()
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };

        for block in content_array {
            match block.get("type").and_then(serde_json::Value::as_str) {
                Some("tool_use") => {
                    if let Some((tool_call_row_id, tool_name)) =
                        claude::insert_tool_use(&tx, message_row_id, session_row_id, block)?
                    {
                        if let Some(id) = block.get("id").and_then(serde_json::Value::as_str) {
                            tool_call_rows.insert(id.to_string(), (tool_call_row_id, tool_name));
                        }
                    }
                }
                Some("tool_result") => {
                    claude::insert_tool_result(
                        &tx,
                        session_row_id,
                        message_row_id,
                        block,
                        &tool_call_rows,
                    )?;
                }
                _ => {}
            }
        }
    }

    tx.execute(
        "UPDATE session SET
            total_input_tokens = (SELECT COALESCE(SUM(input_tokens), 0) FROM usage WHERE session_id = ?1),
            total_output_tokens = (SELECT COALESCE(SUM(output_tokens), 0) FROM usage WHERE session_id = ?1),
            total_cache_creation_tokens = (SELECT COALESCE(SUM(cache_creation_tokens), 0) FROM usage WHERE session_id = ?1),
            total_cache_read_tokens = (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM usage WHERE session_id = ?1),
            total_reasoning_tokens = (SELECT COALESCE(SUM(reasoning_tokens), 0) FROM usage WHERE session_id = ?1)
         WHERE id = ?1",
        [session_row_id],
    )
    .map_err(|e| format!("Failed to roll up session usage totals: {e}"))?;
    tx.execute(
        "UPDATE session SET total_tokens = total_input_tokens + total_output_tokens WHERE id = ?1",
        [session_row_id],
    )
    .map_err(|e| format!("Failed to roll up session total_tokens: {e}"))?;
    // Most-frequent model across this session's messages, denormalized the
    // same way total_tokens is above -- ties break alphabetically for a
    // deterministic result. NULL (no message carries a model) leaves
    // dominant_model NULL, read as "Unknown" by the History surface.
    tx.execute(
        "UPDATE session SET dominant_model = (
            SELECT model FROM message
            WHERE session_id = ?1 AND model IS NOT NULL
            GROUP BY model
            ORDER BY COUNT(*) DESC, model ASC
            LIMIT 1
         ) WHERE id = ?1",
        [session_row_id],
    )
    .map_err(|e| format!("Failed to roll up session dominant_model: {e}"))?;

    tx.commit().map_err(|e| e.to_string())?;

    Ok(IngestOutcome {
        messages_ingested: messages.len() as u64,
        skipped_unchanged: false,
    })
}

/// Ingests every session in a Claude project directory. Reuses the existing
/// `load_project_sessions` (mmap/SIMD parse pipeline, chain resolution,
/// etc.) rather than re-walking or re-parsing anything.
pub async fn ingest_claude_project(
    conn: &mut Connection,
    project_path: &str,
) -> Result<Vec<IngestOutcome>, String> {
    let provider_id = upsert_provider(conn, "claude", "Claude Code", "A", claude::CLAUDE_PARSER_VERSION)?;

    let sessions =
        crate::commands::session::load_project_sessions(project_path.to_string(), None).await?;

    // `load_project_sessions` walks the WHOLE project tree, including
    // `subagents/` subdirectories -- unlike `commands::project::scan_projects`,
    // it applies no depth filter of its own. Without this, a subagent
    // transcript would be ingested TWICE: once here as a phantom top-level
    // session (never flagged `is_subagent`), and once correctly via
    // `ingest_subagent_tree` below -- whichever runs last wins on the same
    // `session_key`, silently able to strip the flag depending on directory
    // walk order. Keeping only direct children mirrors `scan_projects`'s own
    // `relative.components().count() == 1` convention, so subagent files are
    // ingested exactly once, only through the correlated path.
    let sessions: Vec<ClaudeSession> = sessions
        .into_iter()
        .filter(|s| {
            Path::new(&s.file_path)
                .strip_prefix(project_path)
                .is_ok_and(|relative| relative.components().count() == 1)
        })
        .collect();

    let project_display_name = sessions
        .first()
        .map(|s| s.project_name.clone())
        .unwrap_or_else(|| project_path.to_string());
    let project_id = upsert_project(
        conn,
        provider_id,
        project_path,
        &project_display_name,
        Some(project_path),
    )?;

    // Computed ONCE and shared across every session's message load below --
    // see `ingest_claude_session_file_with_snapshot`'s own doc comment for
    // the real, confirmed O(n²) cost this closes at scale.
    let snapshot = crate::commands::session::project_snapshot(Path::new(project_path));

    let mut outcomes = Vec::with_capacity(sessions.len());
    for session in &sessions {
        let outcome = claude::ingest_claude_session_file_with_snapshot(
            conn,
            provider_id,
            project_id,
            session,
            snapshot.as_ref(),
        )
        .await?;
        outcomes.push(outcome);

        // After the session's own tool_call/agent_run rows exist,
        // correlate and ingest any subagent transcripts it launched --
        // see `ingest_subagent_tree`'s own doc comment for the full
        // design.
        claude::ingest_subagent_tree(conn, provider_id, project_id, &session.file_path, 0).await?;
    }
    Ok(outcomes)
}

/// Reads back `(row_id, file_size, file_mtime, parser_version)` for an
/// already-ingested session, if one exists for this `(project_id,
/// session_key)`.
pub(super) fn existing_session_signature(
    conn: &Connection,
    project_id: i64,
    session_key: &str,
) -> Result<Option<(i64, u64, u64, i64)>, String> {
    conn.query_row(
        "SELECT id, file_size, file_mtime, parser_version FROM session
         WHERE project_id = ?1 AND session_key = ?2",
        rusqlite::params![project_id, session_key],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)?,
            ))
        },
    )
    .optional()
    .map_err(|e| format!("Failed to read existing session signature: {e}"))
}

/// Deletes all rows for a session (every child table, in FK-dependency
/// order, then the session row itself) so it can be cleanly re-ingested.
///
/// Once `agent_run.child_session_id` is populated, a re-ingest of a
/// CHANGED session deletes then reinserts
/// its row (`persist_session_messages`) -- `session.id` has no
/// `AUTOINCREMENT`, so `SQLite` may or may not reuse the same numeric rowid
/// for the new row (confirmed directly: it DOES get reused when the
/// deleted row happened to hold the table's current max id, e.g. in a
/// small/test database -- never assume either way). Regardless of which
/// happens, some OTHER session's `agent_run` row may still reference
/// THIS session's OLD id as its `child_session_id` at the moment of
/// deletion. That FK has no `ON DELETE` clause and `archive_db::open_connection` runs with
/// `PRAGMA foreign_keys = ON`, so deleting a still-referenced session would
/// raise a real constraint error, not silently orphan -- clearing any
/// incoming reference first (harmless no-op for a session nothing points
/// at, i.e. every session before this feature existed) closes that gap for
/// both a normal incremental re-ingest and `truncate_provider_data`'s own
/// per-session deletion loop (which calls this same function).
pub(super) fn delete_session_rows(conn: &Connection, session_row_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_run SET child_session_id = NULL WHERE child_session_id = ?1",
        [session_row_id],
    )
    .map_err(|e| format!("Failed to clear incoming agent_run references before delete: {e}"))?;

    let tables_referencing_session = [
        "agent_run",
        "error",
        "file_event",
        "tool_result",
        "command",
        "tool_call",
        "usage",
        "message",
    ];
    for table in tables_referencing_session {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?1"),
            [session_row_id],
        )
        .map_err(|e| format!("Failed to clear existing `{table}` rows for re-ingest: {e}"))?;
    }
    conn.execute("DELETE FROM session WHERE id = ?1", [session_row_id])
        .map_err(|e| format!("Failed to clear existing session row for re-ingest: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_connection() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();
        conn
    }

    /// Same as `migrated_connection`, but with `PRAGMA foreign_keys = ON` --
    /// `SQLite` does not enforce foreign keys by default, and the plain
    /// `migrated_connection()` above never turns it on, so a test relying
    /// on real FK enforcement (unlike every other test in this module) must
    /// opt in explicitly to actually exercise it, matching
    /// `archive_db::open_connection`'s own real-world pragma.
    fn migrated_connection_with_fk_enforcement() -> Connection {
        let conn = migrated_connection();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    /// Regression test for a real, confirmed bug: `ingest_claude_project`
    /// calls `load_session_messages` once per session (via
    /// `ingest_claude_session_file`), which in turn
    /// calls the plain `resolve_session_chain` -- and that function
    /// recomputed a full project directory snapshot on every single call,
    /// with no sharing across the loop. For N sessions in the same
    /// project, that's N calls each doing O(N) work: confirmed via a live
    /// repro that ingesting 10,000 sessions took ~10 minutes after the fix
    /// below, versus the OTHER bottleneck found the same day
    /// (`superseded_chain_paths`, fixed separately in `chain.rs`) which
    /// alone left this one still measured at 598 seconds total for 10,000
    /// sessions -- both numbers are a world away from the original,
    /// never-finished 2+ hour hang. Fixed with a short (2s) most-recent-
    /// snapshot cache inside `chain.rs::project_snapshot` itself (see its
    /// own doc comment) rather than threading a shared snapshot through
    /// every layer between here and `resolve_session_chain` -- this ONE
    /// call site is exactly the kind of same-directory burst the cache is
    /// for, and it also transparently protects any future caller with the
    /// same access pattern.
    ///
    /// Uses a modest N (this is real SQL work per session, not just a
    /// file stat) so the `#[ignore]`d run stays reasonably fast when
    /// someone actually runs it; the 10,000-session number above was
    /// confirmed manually during the fix, not re-asserted here.
    #[tokio::test]
    #[ignore = "slow -- run explicitly to guard the ingest-loop chain-resolution fix"]
    async fn ingest_claude_project_stays_near_linear_at_scale() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::utils::test_support::home_override_guard(home_dir.path());
        let temp_dir = tempfile::TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("scale-project");
        std::fs::create_dir_all(&project_dir).unwrap();
        const N: usize = 2_000;
        for i in 0..N {
            let line = format!(
                r#"{{"uuid":"u{i}","sessionId":"s{i}","timestamp":"2026-01-01T00:00:00Z","type":"user","message":{{"role":"user","content":"message number {i}"}}}}"#
            );
            std::fs::write(project_dir.join(format!("session_{i}.jsonl")), format!("{line}\n"))
                .unwrap();
        }
        let mut conn = migrated_connection();
        let start = std::time::Instant::now();
        let outcomes = ingest_claude_project(&mut conn, &project_dir.to_string_lossy())
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(outcomes.len(), N);
        assert!(
            elapsed.as_secs() < 180,
            "ingest_claude_project({N}) took {elapsed:?} -- the old per-call \
             chain-resolution bug would have blown through this generous bound long \
             before hitting a real timeout"
        );
    }

    #[test]
    fn upsert_provider_is_idempotent_and_returns_stable_id() {
        let conn = migrated_connection();
        let id1 = upsert_provider(&conn, "claude", "Claude Code", "A", 1).unwrap();
        let id2 = upsert_provider(&conn, "claude", "Claude Code", "A", 2).unwrap();
        assert_eq!(id1, id2);

        let tier: String = conn
            .query_row("SELECT tier FROM provider WHERE id = ?1", [id1], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tier, "A");
    }

    #[test]
    fn upsert_project_is_idempotent_and_scoped_per_provider() {
        let conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", 1).unwrap();
        let id1 = upsert_project(&conn, provider_id, "/some/project", "my-project", None).unwrap();
        let id2 = upsert_project(&conn, provider_id, "/some/project", "renamed", None).unwrap();
        assert_eq!(id1, id2);

        let display_name: String = conn
            .query_row("SELECT display_name FROM project WHERE id = ?1", [id1], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(display_name, "renamed");
    }

    #[test]
    fn existing_session_signature_is_none_when_absent() {
        let conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", 1).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();
        assert_eq!(
            existing_session_signature(&conn, project_id, "nonexistent").unwrap(),
            None
        );
    }

    fn insert_bare_session(conn: &Connection, project_id: i64, session_key: &str) -> i64 {
        conn.execute(
            "INSERT INTO session (project_id, session_key, file_path, file_size, file_mtime, parser_version, last_ingested_at)
             VALUES (?1, ?2, ?3, 0, 0, 1, '2026-01-01T00:00:00Z')",
            rusqlite::params![project_id, session_key, format!("/p/{session_key}")],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Regression guard for a real FK-constraint-violation risk -- once
    /// `agent_run.child_session_id` is populated, deleting
    /// the referenced (child/subagent) session must not fail even though
    /// the referencing `agent_run` row belongs to a DIFFERENT, still-live
    /// session.
    #[test]
    fn delete_session_rows_clears_incoming_agent_run_references_first() {
        let conn = migrated_connection_with_fk_enforcement();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", 1).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        let parent_session_id = insert_bare_session(&conn, project_id, "parent.jsonl");
        let child_session_id = insert_bare_session(&conn, project_id, "child.jsonl");

        conn.execute(
            "INSERT INTO agent_run (session_id, child_session_id, status) VALUES (?1, ?2, 'completed')",
            rusqlite::params![parent_session_id, child_session_id],
        )
        .unwrap();

        // Deleting the CHILD session (the referenced side) must not raise a
        // foreign-key-constraint error, even though the parent's agent_run
        // row (a different, still-live session) still points at it.
        delete_session_rows(&conn, child_session_id)
            .expect("deleting a still-referenced child session must not violate the FK");

        let remaining_child_session_id: Option<i64> = conn
            .query_row(
                "SELECT child_session_id FROM agent_run WHERE session_id = ?1",
                [parent_session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_child_session_id, None,
            "the dangling reference must be cleared, not left pointing at a deleted row"
        );
    }
}
