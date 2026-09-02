//! Global search over Grid's own normalized archive (search-on-FTS),
//! backed by the `search_fts` FTS5 virtual
//! table (migration 6, see `archive_db::schema`'s own doc comment for the
//! full design: why one unified table, what backs each facet, and how it
//! stays in sync via triggers). Replaces the raw-file-walk
//! `commands::session::search::search_messages` as the PRIMARY search
//! path -- that raw scan stays registered as a fallback (see
//! `commands::search::global_search`'s own doc), covering whatever a
//! provider not yet ingested into `archive_db` needs.
//!
//! Query safety: raw user input is never interpolated into the `MATCH`
//! string. [`sanitize_fts_query`] quotes every whitespace-separated term
//! individually (with a trailing `*` for prefix matching), so a query
//! containing FTS5 operator syntax (`AND`/`OR`/`NOT`/`NEAR`/parens/
//! unbalanced quotes) is always treated as literal text to match, never
//! as a query-syntax injection.

use crate::models::{SearchResult, SearchResultKind};
use rusqlite::Connection;

/// Quotes each term individually (doubling embedded `"` per SQL-string
/// escaping) and appends a prefix-match `*`, so `foo AND "bar` becomes
/// `"foo"* "AND"* """bar"*` -- always valid FTS5 syntax, never able to
/// inject an operator or unbalance a quote. Terms are implicitly `ANDed` by
/// FTS5's own default between space-separated phrases. Returns an empty
/// string for a query with no non-whitespace content, which callers must
/// treat as "no results," not run against `MATCH` (an empty/all-
/// whitespace `MATCH` argument is a genuine FTS5 syntax error).
fn sanitize_fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every ingested provider's message/command/error/file/tool-output/
/// agent-instruction rows, ranked by FTS5's own `bm25()` relevance score,
/// most-relevant first. `kinds` narrows to specific facets (`None` or
/// empty means every facet); `provider_key`/`project_key` narrow the same
/// way every other `archive_db` query does. Subagent sessions
/// (`is_subagent = 1`) are excluded, matching every other user-facing
/// query in this module -- a subagent transcript is not a result a user
/// searching their own history should land on directly.
pub fn search_archive(
    conn: &Connection,
    query: &str,
    kinds: Option<&[SearchResultKind]>,
    provider_key: Option<&str>,
    project_key: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let fts_query = sanitize_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

    // Resolves a real, owning message for EVERY kind, not just 'message'
    // ("Open in context" was previously silently broken for the other 5
    // kinds -- `sf.ref_id` points into
    // `command`/`tool_result`/`file_event`/`error`/`tool_call`
    // respectively, never resolved back to a message before this fix, so
    // `message_uuid` was always `None` for anything but a `message` hit).
    // `ref_id`'s exact meaning per kind is fixed by the migration's own
    // INSERT/trigger bodies (`schema.rs`'s `MIGRATION_0006_SEARCH_FTS`):
    // command/tool_result/file_event's `ref_id` is that table's own `id`,
    // one hop from `message` via `tool_call`; `error.ref_id` is `error.id`,
    // one direct hop via `error.message_id`; `agent_instruction.ref_id` IS
    // already `tool_call.id` (the trigger fires ON tool_call), one direct
    // hop via `tool_call.message_id`. Every join below is a simple
    // `id`-equality lookup (SQLite's implicit rowid index), and the
    // `AND sf.kind = '...'` guard means at most one of the 6 `LEFT JOIN`
    // chains actually matches any given row.
    // `sf.occurred_at` is only ever populated for 'message'/'error' kinds
    // (see the migration's own INSERT bodies -- command/tool_result/file
    // rows insert a literal `NULL`). That gap was invisible while only
    // 'message' hits were ever queried/rendered (pre-P1-item-4); now that
    // every kind is shown, falling back to the newly-resolved owning
    // message's OWN `timestamp` avoids a real, live-caught bug (every
    // command/tool-output/file result rendering "Invalid Date").
    let mut sql = String::from(
        "SELECT sf.kind,
                snippet(search_fts, 1, '\u{2039}', '\u{203a}', '\u{2026}', 12) AS snippet,
                s.file_path, p.display_name, pr.provider_key,
                COALESCE(sf.occurred_at, m_direct.timestamp, m_cmd.timestamp, m_tres.timestamp, m_fev.timestamp, m_err.timestamp, m_ai.timestamp) AS resolved_occurred_at,
                COALESCE(m_direct.uuid, m_cmd.uuid, m_tres.uuid, m_fev.uuid, m_err.uuid, m_ai.uuid) AS resolved_uuid,
                COALESCE(m_direct.role, m_cmd.role, m_tres.role, m_fev.role, m_err.role, m_ai.role) AS resolved_role
         FROM search_fts sf
         JOIN session s ON s.id = sf.session_id
         JOIN project p ON p.id = s.project_id
         JOIN provider pr ON pr.id = p.provider_id
         LEFT JOIN message m_direct ON m_direct.id = sf.ref_id AND sf.kind = 'message'
         LEFT JOIN command cmd ON cmd.id = sf.ref_id AND sf.kind = 'command'
         LEFT JOIN tool_result tres ON tres.id = sf.ref_id AND sf.kind = 'tool_result'
         LEFT JOIN file_event fev ON fev.id = sf.ref_id AND sf.kind = 'file'
         LEFT JOIN error err ON err.id = sf.ref_id AND sf.kind = 'error'
         LEFT JOIN tool_call tc_ai ON tc_ai.id = sf.ref_id AND sf.kind = 'agent_instruction'
         LEFT JOIN tool_call tc_cmd ON tc_cmd.id = cmd.tool_call_id
         LEFT JOIN tool_call tc_tres ON tc_tres.id = tres.tool_call_id
         LEFT JOIN tool_call tc_fev ON tc_fev.id = fev.tool_call_id
         LEFT JOIN message m_cmd ON m_cmd.id = tc_cmd.message_id
         LEFT JOIN message m_tres ON m_tres.id = tc_tres.message_id
         LEFT JOIN message m_fev ON m_fev.id = tc_fev.message_id
         LEFT JOIN message m_err ON m_err.id = err.message_id
         LEFT JOIN message m_ai ON m_ai.id = tc_ai.message_id
         WHERE search_fts MATCH ?1
           AND s.is_subagent = 0",
    );

    // Owned so the borrows handed to `query_map` below outlive this
    // function call -- same reasoning as `history::query_claude_history_
    // sessions`'s own `date_params` vec.
    let mut string_params: Vec<String> = vec![fts_query];

    if let Some(kinds) = kinds.filter(|k| !k.is_empty()) {
        let placeholders = vec!["?"; kinds.len()].join(", ");
        sql.push_str(&format!(" AND sf.kind IN ({placeholders})"));
        string_params.extend(kinds.iter().map(|k| k.as_sql_str().to_string()));
    }
    if let Some(provider_key) = provider_key {
        sql.push_str(&format!(" AND pr.provider_key = ?{}", string_params.len() + 1));
        string_params.push(provider_key.to_string());
    }
    if let Some(project_key) = project_key {
        sql.push_str(&format!(" AND p.project_key = ?{}", string_params.len() + 1));
        string_params.push(project_key.to_string());
    }
    sql.push_str(&format!(
        " ORDER BY bm25(search_fts) ASC LIMIT ?{}",
        string_params.len() + 1
    ));
    string_params.push(limit.to_string());

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to prepare search query: {e}"))?;
    let params: Vec<&dyn rusqlite::ToSql> =
        string_params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

    let rows = stmt
        .query_map(params.as_slice(), |row| {
            let kind_str: String = row.get(0)?;
            Ok(SearchResult {
                kind: parse_kind(&kind_str),
                snippet: row.get(1)?,
                session_id: row.get(2)?,
                project_name: row.get(3)?,
                provider_key: row.get(4)?,
                occurred_at: row.get(5)?,
                message_uuid: row.get(6)?,
                message_role: row.get(7)?,
            })
        })
        .map_err(|e| format!("Failed to run search query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read search result row: {e}"))
}

/// `search_fts.kind` only ever holds one of the 6 literal strings this
/// module itself inserts (see the migration's own `INSERT`/trigger
/// bodies) -- an unrecognized value would mean the schema and this
/// function have drifted apart, not real user data, so this defaults to
/// `Message` rather than making every caller handle an `Option`/`Result`
/// for a case that should never occur.
fn parse_kind(raw: &str) -> SearchResultKind {
    match raw {
        "command" => SearchResultKind::Command,
        "tool_result" => SearchResultKind::ToolResult,
        "file" => SearchResultKind::File,
        "error" => SearchResultKind::Error,
        "agent_instruction" => SearchResultKind::AgentInstruction,
        _ => SearchResultKind::Message,
    }
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

    async fn seeded_conn(lines: &str) -> Connection {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", lines);
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        conn
    }

    #[test]
    fn sanitize_fts_query_neutralizes_operator_syntax_and_quotes() {
        assert_eq!(sanitize_fts_query("foo bar"), "\"foo\"* \"bar\"*");
        assert_eq!(
            sanitize_fts_query("foo AND \"bar"),
            "\"foo\"* \"AND\"* \"\"\"bar\"*"
        );
        assert_eq!(sanitize_fts_query("   "), "");
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn search_archive_finds_a_message_and_a_command_by_text() {
        let lines = format!(
            "{}\n{}\n{}\n",
            r#"{"uuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the quokka investigation continues"}]}}"#,
            r#"{"uuid":"m2","parentUuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"grep -r quokka ."}}]}}"#,
            r#"{"uuid":"m3","parentUuid":"m2","sessionId":"s1","timestamp":"2026-01-01T00:02:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"no matches","is_error":false}]}}"#,
        );
        let conn = seeded_conn(&lines).await;

        let message_hits =
            search_archive(&conn, "quokka investigation", None, None, None, 10).unwrap();
        let message_hit = message_hits
            .iter()
            .find(|r| r.kind == SearchResultKind::Message)
            .unwrap_or_else(|| panic!("expected a message hit, got {message_hits:?}"));
        assert_eq!(
            message_hit.message_uuid.as_deref(),
            Some("m1"),
            "message-kind hits must carry the exact message's own uuid for deep-linking"
        );

        let command_hits = search_archive(&conn, "grep quokka", None, None, None, 10).unwrap();
        let command_hit = command_hits
            .iter()
            .find(|r| r.kind == SearchResultKind::Command)
            .unwrap_or_else(|| panic!("expected a command hit, got {command_hits:?}"));
        assert_eq!(
            command_hit.message_uuid.as_deref(),
            Some("m2"),
            "a command hit must resolve back to its OWN owning message (the tool_use turn) \
             for 'open in context' to work -- this was previously silently broken"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn search_archive_resolves_a_message_for_every_non_message_kind() {
        let lines = format!(
            "{}\n{}\n{}\n",
            r#"{"uuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo quibblewomp"}}]}}"#,
            r#"{"uuid":"m2","parentUuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:01:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"quibblewomp","is_error":false}]}}"#,
            r#"{"uuid":"m3","parentUuid":"m2","sessionId":"s1","timestamp":"2026-01-01T00:02:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Agent","input":{"subagent_type":"general-purpose","prompt":"investigate quibblewomp"}}]}}"#,
        );
        let conn = seeded_conn(&lines).await;

        // tool_result kind -- resolves via tool_result -> tool_call ->
        // message. `tool_call.message_id` was set when the TOOL_USE was
        // ingested (m1), not when its result arrived (m2) -- one tool_call
        // row is shared by both, and it belongs to the use, not the
        // result. So a tool_result hit correctly opens at m1, the message
        // where the command was actually issued -- arguably the more
        // useful "open in context" target anyway (you see the call, not
        // just the bare result sitting alone).
        let tool_result_hits = search_archive(&conn, "quibblewomp", None, None, None, 10).unwrap();
        let tool_result_hit = tool_result_hits
            .iter()
            .find(|r| r.kind == SearchResultKind::ToolResult)
            .unwrap_or_else(|| panic!("expected a tool_result hit, got {tool_result_hits:?}"));
        assert_eq!(tool_result_hit.message_uuid.as_deref(), Some("m1"));
        assert_eq!(tool_result_hit.message_role.as_deref(), Some("assistant"));

        // agent_instruction kind -- resolves via tool_call -> message directly.
        let agent_hits = search_archive(&conn, "investigate", None, None, None, 10).unwrap();
        let agent_hit = agent_hits
            .iter()
            .find(|r| r.kind == SearchResultKind::AgentInstruction)
            .unwrap_or_else(|| panic!("expected an agent_instruction hit, got {agent_hits:?}"));
        assert_eq!(agent_hit.message_uuid.as_deref(), Some("m3"));
        assert_eq!(agent_hit.message_role.as_deref(), Some("assistant"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn search_archive_kind_filter_excludes_other_facets() {
        let lines = format!(
            "{}\n{}\n",
            r#"{"uuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"zzflorble everywhere"}]}}"#,
            r#"{"uuid":"m2","parentUuid":"m1","sessionId":"s1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo zzflorble"}}]}}"#,
        );
        let conn = seeded_conn(&lines).await;

        let all_hits = search_archive(&conn, "zzflorble", None, None, None, 10).unwrap();
        assert_eq!(all_hits.len(), 2, "one message hit + one command hit");

        let command_only = search_archive(
            &conn,
            "zzflorble",
            Some(&[SearchResultKind::Command]),
            None,
            None,
            10,
        )
        .unwrap();
        assert_eq!(command_only.len(), 1);
        assert_eq!(command_only[0].kind, SearchResultKind::Command);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn search_archive_excludes_subagent_sessions() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let parent_lines = format!(
            "{}\n",
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{"subagent_type":"general-purpose"}}],"model":"claude-x","usage":{"input_tokens":10,"output_tokens":5}}}"#
        );
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &parent_lines);

        let subagents_dir = std::path::Path::new(&claude_base)
            .join("projects")
            .join("-fixture-project-a")
            .join("session1")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.jsonl"),
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"gronkulator only in the subagent"}]}}"#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let hits = search_archive(&conn, "gronkulator", None, None, None, 10).unwrap();
        assert!(
            hits.is_empty(),
            "the only match lives in the excluded subagent session, got {hits:?}"
        );
    }

    #[test]
    fn search_archive_returns_empty_for_blank_query_without_erroring() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let hits = search_archive(&conn, "   ", None, None, None, 10).unwrap();
        assert!(hits.is_empty());
    }
}
