//! Claude-side query backing the History surface (spec §13, Phase 1
//! Step 5). Returns every ingested Claude session, optionally bounded by a
//! date range pushed into SQL (the one dimension cheap to index and worth
//! pushing down); Project/Provider/Model filtering happens once, uniformly,
//! across the full cross-provider merged list in `commands::history` --
//! keeping filter semantics identical for every provider rather than
//! splitting "SQL filters for Claude, Rust filters for everyone else."
//!
//! Timestamp choice: `last_message_time` (falling back to `last_modified`
//! when absent) is what History sorts and buckets Today/Yesterday on --
//! "when did I last touch this session," not "when did it start"
//! (`first_message_time` would misclassify a session started last week but
//! still active today) and not a bare filesystem mtime alone (touchable by
//! sync tools/the watcher/ingest itself without real user activity).

use crate::models::HistorySessionItem;
use rusqlite::Connection;

/// All Claude sessions, most-recent-first, optionally bounded by
/// `[start_date, end_date]` (RFC3339 strings, inclusive) applied to the
/// same `COALESCE(last_message_time, last_modified)` expression used for
/// sorting -- a session with no timestamp at all sorts last and is only
/// excluded by a date filter, never silently dropped otherwise.
pub fn query_claude_history_sessions(
    conn: &Connection,
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<Vec<HistorySessionItem>, String> {
    let mut sql = String::from(
        "SELECT s.file_path, p.project_key, p.display_name, s.actual_session_id,
                s.first_message_time, s.last_message_time, s.last_modified,
                s.message_count, s.has_tool_use, s.has_errors, s.summary,
                s.dominant_model,
                COALESCE(s.last_message_time, s.last_modified) AS recency_time
         FROM session s
         JOIN project p ON p.id = s.project_id
         JOIN provider pr ON pr.id = p.provider_id
         WHERE pr.provider_key = 'claude'
           AND s.is_subagent = 0",
    );
    // Owned so the borrows handed to `query_map` below outlive this
    // function call -- pushing `&start_date`-style temporaries directly
    // would be dropped at the end of their `if let` block while `params`
    // still needed them.
    let mut date_params: Vec<String> = Vec::new();
    if let Some(start) = start_date {
        sql.push_str(" AND COALESCE(s.last_message_time, s.last_modified) >= ?");
        date_params.push(start.to_string());
    }
    if let Some(end) = end_date {
        sql.push_str(" AND COALESCE(s.last_message_time, s.last_modified) <= ?");
        date_params.push(end.to_string());
    }
    sql.push_str(" ORDER BY recency_time DESC, s.id DESC");

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to prepare Claude history query: {e}"))?;
    let params: Vec<&dyn rusqlite::ToSql> = date_params
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let rows = stmt
        .query_map(params.as_slice(), |row| {
            let has_tool_use: i64 = row.get(8)?;
            let has_errors: i64 = row.get(9)?;
            let recency_time: Option<String> = row.get(12)?;
            Ok(HistorySessionItem {
                session_id: row.get(0)?,
                actual_session_id: row.get(3)?,
                provider_id: "claude".to_string(),
                project_key: row.get(1)?,
                project_name: row.get(2)?,
                file_path: row.get(0)?,
                recency_time: recency_time.unwrap_or_default(),
                first_message_time: row.get(4)?,
                last_message_time: row.get(5)?,
                message_count: {
                    let count: i64 = row.get(7)?;
                    usize::try_from(count).unwrap_or(0)
                },
                has_tool_use: has_tool_use != 0,
                has_errors: has_errors != 0,
                summary: row.get(10)?,
                model: row.get(11)?,
            })
        })
        .map_err(|e| format!("Failed to run Claude history query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read Claude history row: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_db::backfill::run_full_backfill;
    use crate::archive_db::migrate::migrate;
    use std::fs;
    use tempfile::TempDir;

    fn write_fixture_project(
        claude_base: &std::path::Path,
        project_dir_name: &str,
        session_file_name: &str,
        session_lines: &str,
    ) -> String {
        let project_dir = claude_base.join("projects").join(project_dir_name);
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join(session_file_name), session_lines).unwrap();
        claude_base.to_string_lossy().to_string()
    }

    fn message_line(uuid: &str, timestamp: &str, model: Option<&str>) -> String {
        let model_field = model
            .map(|m| format!(r#","model":"{m}""#))
            .unwrap_or_default();
        format!(
            r#"{{"uuid":"{uuid}","sessionId":"s1","timestamp":"{timestamp}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}]{model_field}}}}}"#
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn returns_dominant_model_and_orders_by_recency() {
        // query_claude_history_sessions is already filtered to
        // provider_key = 'claude', so real non-Claude provider data on
        // this machine can't affect correctness here -- this guard is
        // purely to skip re-parsing hundreds of real Codex sessions on
        // every run_full_backfill call (see archive_db::test_support's
        // own doc comment), matching this codebase's established pattern.
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let lines = format!(
            "{}\n{}\n{}\n",
            message_line("u1", "2026-01-01T00:00:00Z", Some("claude-a")),
            message_line("u2", "2026-01-01T00:00:01Z", Some("claude-a")),
            message_line("u3", "2026-01-01T00:00:02Z", Some("claude-b")),
        );
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &lines);

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let results = query_claude_history_sessions(&conn, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].model.as_deref(),
            Some("claude-a"),
            "claude-a appears twice, claude-b once -- most-frequent wins"
        );
        assert_eq!(results[0].provider_id, "claude");
        assert_eq!(results[0].message_count, 3);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn dominant_model_is_none_when_no_message_carries_one() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let lines = format!("{}\n", message_line("u1", "2026-01-01T00:00:00Z", None));
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &lines);

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let results = query_claude_history_sessions(&conn, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].model, None);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn date_filter_excludes_sessions_outside_the_range() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let old_lines = format!("{}\n", message_line("u1", "2026-01-01T00:00:00Z", None));
        let new_lines = format!("{}\n", message_line("u2", "2026-06-01T00:00:00Z", None));
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-old",
            "session-old.jsonl",
            &old_lines,
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-new",
            "session-new.jsonl",
            &new_lines,
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let results =
            query_claude_history_sessions(&conn, Some("2026-03-01T00:00:00Z"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].recency_time.starts_with("2026-06-01"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// A session at 23:59:59 one day vs 00:00:01 the next must sort in
    /// real chronological order, not be conflated by a coarse date-only
    /// comparison -- History's Today/Yesterday bucketing depends on this.
    async fn orders_sessions_across_a_day_boundary_correctly() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let late_lines = format!(
            "{}\n",
            message_line("u1", "2026-01-01T23:59:59Z", None)
        );
        let early_lines = format!(
            "{}\n",
            message_line("u2", "2026-01-02T00:00:01Z", None)
        );
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-late",
            "session-late.jsonl",
            &late_lines,
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-early",
            "session-early.jsonl",
            &early_lines,
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let results = query_claude_history_sessions(&conn, None, None).unwrap();
        assert_eq!(results.len(), 2);
        // Most recent first: the Jan 2 session comes before the Jan 1 one.
        assert!(results[0].recency_time.starts_with("2026-01-02T00:00:01"));
        assert!(results[1].recency_time.starts_with("2026-01-01T23:59:59"));
    }

    /// A correlated subagent transcript lands as its own `session` row
    /// (`is_subagent = 1`) once ingested -- History must never list it as
    /// if it were a real top-level session.
    #[tokio::test]
    #[serial_test::serial]
    async fn excludes_subagent_sessions() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let parent_lines = [
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{"subagent_type":"general-purpose"}}],"model":"claude-x","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"uuid":"p2","parentUuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:05:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"done","is_error":false}]}}"#,
        ];
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &(parent_lines.join("\n") + "\n"),
        );

        let subagents_dir = std::path::Path::new(&claude_base)
            .join("projects")
            .join("-fixture-project-a")
            .join("session1")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        let sub_lines = format!("{}\n", message_line("s1", "2026-01-01T00:01:00Z", Some("claude-x")));
        fs::write(subagents_dir.join("agent-sub1.jsonl"), &sub_lines).unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_agent1"}"#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        // Sanity: the subagent really did land as its own flagged row.
        let subagent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session WHERE is_subagent = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(subagent_count, 1);

        let results = query_claude_history_sessions(&conn, None, None).unwrap();
        assert_eq!(results.len(), 1, "the subagent session must not appear in History");
        assert!(results[0].session_id.contains("session1.jsonl"));
    }
}
