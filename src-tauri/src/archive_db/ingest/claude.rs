//! Claude-specific ingestion: turns an already-parsed `ClaudeSession` +
//! its `ClaudeMessage`s (reusing the existing mmap/SIMD parse pipeline in
//! `commands::session::load`) into normalized rows.
//!
//! Ground truth for the extraction logic below was read directly from a
//! real Claude Code session file, not assumed from the Anthropic API docs:
//! an assistant `message.content` array carries `tool_use` blocks
//! (`{"type":"tool_use","id","name","input"}`); the corresponding result
//! arrives on a LATER message as a `tool_result` block in ITS
//! `message.content` array (`{"type":"tool_result","tool_use_id","content",
//! "is_error"}`), correlated back to the `tool_use` purely by
//! `tool_use_id == id`. The richer, per-tool `toolUseResult` top-level
//! field (surfaced on `ClaudeMessage` nowhere -- it isn't a typed field)
//! varies too much shape-by-shape across tools to be a reliable generic
//! extraction source, so it is deliberately not used here.

use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;

use crate::commands::session::{get_session_subagents, SubagentSession};
use crate::models::{ClaudeMessage, ClaudeSession};

use super::{existing_session_signature, IngestOutcome};

/// Bump whenever this file's extraction logic changes materially -- forces
/// re-ingestion of every already-ingested Claude session on the next
/// backfill, mirroring `CACHE_VERSION`'s role for `.session_cache.json`.
///
/// Bumped 1 -> 2: now stores `message.message_id` (needed for stats
/// dedup) and normalizes `cache_creation_tokens` via
/// `commands::stats::normalize_token_usage` before storing it (previously
/// only the `_5m`/`_1h` sub-fields were normalized, silently undercounting
/// the aggregate `cache_creation_tokens` column for messages using the
/// newer nested-only `cache_creation` cache-usage shape).
/// Bumped 2->3: adds `session.dominant_model` (see `MIGRATION_0003` in
/// `archive_db::schema`). The existing idempotency check below already
/// forces every already-archived session to fully re-ingest the next time
/// it's touched by backfill/rebuild-index/the watcher when its stored
/// `parser_version` is behind this constant -- no separate one-time
/// backfill needed, sessions self-heal.
/// Bumped 3->4: subagent transcripts are now ingested as their own
/// `session` rows and correlated into `agent_run` (see
/// `ingest_subagent_tree` below) -- every already-archived Claude session
/// needs to re-run through the now-subagent-aware ingest path at least
/// once to pick this up.
pub const CLAUDE_PARSER_VERSION: i64 = 4;

/// Maximum subagent-tree recursion depth (`ingest_subagent_tree`) -- a
/// safety net against pathological/corrupted data (e.g. a subagent
/// transcript that somehow references itself), not an expected real-world
/// limit. Real Claude Code subagent nesting is rarely more than 1-2 levels
/// deep in practice.
const MAX_SUBAGENT_DEPTH: u32 = 5;

/// Result text is stored as a bounded preview, not full fidelity (schema
/// doc comment on `message.content_text`) -- long tool output/transcript
/// text is still reachable via `source_record`/the original provider file.
const TEXT_PREVIEW_MAX_LEN: usize = 4000;

/// Ingests a single Claude session file. Idempotent: if the file's
/// `(size, mtime)` and this parser's version match what's already stored,
/// this is a cheap no-op (one `SELECT`, no reparse). Otherwise, does a
/// full re-ingest of that one session (delete + reinsert), which is
/// correct but not the most efficient possible incremental update -- an
/// intentional simplification.
pub async fn ingest_claude_session_file(
    conn: &mut Connection,
    provider_id: i64,
    project_id: i64,
    session: &ClaudeSession,
) -> Result<IngestOutcome, String> {
    ingest_claude_session_file_with_snapshot(conn, provider_id, project_id, session, None).await
}

/// Same as [`ingest_claude_session_file`], but takes an already-computed
/// project snapshot instead of letting `load_session_messages` recompute
/// one internally on every call.
///
/// `ingest_claude_project`'s per-session loop is the one caller that needs
/// this: it calls this function once per session in the SAME project, and
/// the plain version's per-call directory scan is a real, confirmed O(n²)
/// cost at scale -- see
/// `commands::session::load_session_messages_with_snapshot`'s own doc
/// comment for the full story. `ingest_subagent_tree`'s own recursive call
/// (a DIFFERENT directory -- a session's own `subagents/` subfolder, not
/// the parent project root) deliberately keeps using the plain,
/// snapshot-less version below: subagent counts per session are small, so
/// there's no hot loop to protect there.
pub(super) async fn ingest_claude_session_file_with_snapshot(
    conn: &mut Connection,
    // Reserved for future per-provider extraction variations -- the
    // `session` row only needs `project_id` (which transitively carries
    // the provider), not `provider_id` directly.
    _provider_id: i64,
    project_id: i64,
    session: &ClaudeSession,
    snapshot: Option<&crate::commands::session::ProjectSnapshot>,
) -> Result<IngestOutcome, String> {
    let path = Path::new(&session.file_path);
    let (file_size, file_mtime) = super::stat_signature(path)?;

    if let Some((_, existing_size, existing_mtime, existing_parser_version)) =
        existing_session_signature(conn, project_id, &session.session_id)?
    {
        if existing_size == file_size
            && existing_mtime == file_mtime
            && existing_parser_version == CLAUDE_PARSER_VERSION
        {
            return Ok(IngestOutcome {
                messages_ingested: 0,
                skipped_unchanged: true,
            });
        }
    }

    let messages = crate::commands::session::load_session_messages_with_snapshot(
        session.file_path.clone(),
        snapshot,
    )
    .await?;

    super::persist_session_messages(
        conn,
        project_id,
        session,
        &messages,
        file_size,
        file_mtime,
        CLAUDE_PARSER_VERSION,
    )
}

/// Builds a minimal `ClaudeSession` for a subagent transcript from what
/// `get_session_subagents` already extracted, so it can be run through the
/// exact same `ingest_claude_session_file` path (staleness check, parse,
/// persist) as any top-level session -- only the fields
/// `persist_session_messages` actually reads are populated; the rest have
/// no meaning for a subagent transcript and are left at safe defaults.
fn build_subagent_claude_session(subagent: &SubagentSession) -> ClaudeSession {
    ClaudeSession {
        session_id: subagent.agent_id.clone(),
        actual_session_id: subagent.agent_id.clone(),
        file_path: subagent.file_path.clone(),
        project_name: String::new(),
        message_count: subagent.message_count,
        first_message_time: subagent.first_message_time.clone().unwrap_or_default(),
        last_message_time: subagent.last_message_time.clone().unwrap_or_default(),
        last_modified: subagent.last_message_time.clone().unwrap_or_default(),
        has_tool_use: false,
        has_errors: false,
        summary: subagent.summary.clone(),
        is_renamed: false,
        provider: Some("claude".to_string()),
        storage_type: None,
        entrypoint: None,
    }
}

/// Ingests every subagent transcript nested under `parent_file_path`: each
/// becomes its own `session` row (`is_subagent = 1`, so it never pollutes
/// a user-facing count/list --
/// see the `is_subagent = 0` filters added throughout `archive_db::history`/
/// `archive_db::insights`), correlated back to the specific `agent_run` row
/// it belongs to via its own `tool_use_id` (read from a sibling
/// `agent-<id>.meta.json` by the already-existing
/// `commands::session::get_session_subagents` -- the SAME mechanism the
/// frontend's own Agents tab already uses, reused here rather than
/// reimplemented). Recurses into each subagent's own subagent files (an
/// agent can itself launch agents), bounded by `MAX_SUBAGENT_DEPTH`.
///
/// **Deliberately does NOT implement the frontend's separate `agentId`-
/// embedded-in-`toolUseResult` fallback correlation**
/// (`agentTaskHelpers.ts::extractAgentTask`) for subagent files with no
/// `.meta.json` (older sessions predating that file). This ingest pipeline
/// operates on already-typed `ClaudeMessage`s, and the raw `toolUseResult`
/// field that fallback needs is explicitly, deliberately NOT surfaced here
/// -- see `insert_tool_use`'s own module doc comment: "varies too much
/// shape-by-shape across tools to be a reliable generic extraction
/// source." A subagent file with no `.meta.json` is still ingested as an
/// `is_subagent = 1` session (so it never silently vanishes from the
/// archive or pollutes a count), but stays unlinked from any `agent_run`
/// row -- a disclosed, narrower-than-ideal scope, not a silent drop.
pub(super) async fn ingest_subagent_tree(
    conn: &mut Connection,
    provider_id: i64,
    project_id: i64,
    parent_file_path: &str,
    depth: u32,
) -> Result<(), String> {
    if depth >= MAX_SUBAGENT_DEPTH {
        return Ok(());
    }

    let subagents = get_session_subagents(parent_file_path.to_string()).await?;

    for sa in &subagents {
        // Look up the specific agent_run row this subagent belongs to (via
        // the parent's own tool_call.tool_use_id), plus the subagent_type
        // already captured there when the launch was ingested -- `None`
        // when there's no `.meta.json` to correlate with (see doc comment).
        let matched: Option<(i64, Option<String>)> = sa
            .tool_use_id
            .as_deref()
            .map(|tool_use_id| {
                conn.query_row(
                    "SELECT ar.id, tc.subagent_type FROM agent_run ar
                     JOIN tool_call tc ON tc.id = ar.parent_tool_call_id
                     JOIN session s ON s.id = tc.session_id
                     WHERE s.file_path = ?1 AND tc.tool_use_id = ?2",
                    rusqlite::params![parent_file_path, tool_use_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("Failed to look up matching agent_run row: {e}"))
            })
            .transpose()?
            .flatten();

        let subagent_session = build_subagent_claude_session(sa);
        ingest_claude_session_file(conn, provider_id, project_id, &subagent_session).await?;

        let Some((child_row_id, _, _, _)) =
            existing_session_signature(conn, project_id, &subagent_session.session_id)?
        else {
            // Just ingested it above -- this should always find a row.
            continue;
        };
        conn.execute(
            "UPDATE session SET is_subagent = 1 WHERE id = ?1",
            [child_row_id],
        )
        .map_err(|e| format!("Failed to mark subagent session: {e}"))?;

        if let Some((agent_run_id, subagent_type)) = matched {
            let tool_call_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tool_call WHERE session_id = ?1",
                    [child_row_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count subagent tool calls: {e}"))?;

            conn.execute(
                "UPDATE agent_run SET
                    child_session_id = ?1,
                    started_at = ?2,
                    ended_at = ?3,
                    tool_call_count = ?4,
                    subagent_type = COALESCE(subagent_type, ?5)
                 WHERE id = ?6",
                rusqlite::params![
                    child_row_id,
                    sa.first_message_time,
                    sa.last_message_time,
                    tool_call_count,
                    subagent_type,
                    agent_run_id,
                ],
            )
            .map_err(|e| format!("Failed to update agent_run linkage: {e}"))?;

            // Backfill parent_agent_run_id for any agent_run rows THIS
            // subagent's own transcript just produced (it launched a
            // sub-subagent) -- they were inserted with session_id = this
            // subagent's own new row id and a NULL parent_agent_run_id,
            // since `insert_tool_result` has no way to know its own
            // container agent_run row's id at insert time.
            conn.execute(
                "UPDATE agent_run SET parent_agent_run_id = ?1
                 WHERE session_id = ?2 AND parent_agent_run_id IS NULL",
                rusqlite::params![agent_run_id, child_row_id],
            )
            .map_err(|e| format!("Failed to set parent_agent_run_id: {e}"))?;
        }

        Box::pin(ingest_subagent_tree(
            conn,
            provider_id,
            project_id,
            &subagent_session.file_path,
            depth + 1,
        ))
        .await?;
    }

    Ok(())
}

pub(super) fn insert_message(
    conn: &Connection,
    session_row_id: i64,
    message: &ClaudeMessage,
) -> Result<i64, String> {
    let content_text = message
        .content
        .as_ref()
        .and_then(|c| flatten_text_preview(c, TEXT_PREVIEW_MAX_LEN));

    conn.execute(
        "INSERT INTO message (
            session_id, uuid, parent_uuid, role, message_type, timestamp,
            is_sidechain, model, stop_reason, cost_usd, duration_ms, content_text,
            message_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(session_id, uuid) DO NOTHING",
        rusqlite::params![
            session_row_id,
            message.uuid,
            message.parent_uuid,
            message.role,
            message.message_type,
            message.timestamp,
            message.is_sidechain.unwrap_or(false),
            message.model,
            message.stop_reason,
            message.cost_usd,
            message.duration_ms.map(|d| i64::try_from(d).unwrap_or(i64::MAX)),
            content_text,
            message.message_id,
        ],
    )
    .map_err(|e| format!("Failed to insert message row: {e}"))?;

    conn.query_row(
        "SELECT id FROM message WHERE session_id = ?1 AND uuid = ?2",
        rusqlite::params![session_row_id, message.uuid],
        |row| row.get(0),
    )
    .map_err(|e| format!("Failed to read back message row id: {e}"))
}

pub(super) fn insert_usage(
    conn: &Connection,
    message_row_id: i64,
    session_row_id: i64,
    message: &ClaudeMessage,
    usage: &crate::models::TokenUsage,
) -> Result<(), String> {
    // Normalize via the exact same function `commands::stats` uses, so the
    // stored `cache_creation_tokens` matches what the raw-file stats path
    // would compute -- the nested `cache_creation.ephemeral_*` shape must
    // fold into the flat aggregate field too, not just the `_5m`/`_1h`
    // sub-fields (a real undercounting bug in the initial version of this
    // function, caught by a parity test against the raw-file stats path).
    let usage = crate::commands::stats::normalize_token_usage(usage.clone());

    conn.execute(
        "INSERT INTO usage (
            message_id, session_id, model, service_tier, input_tokens, output_tokens,
            cache_creation_tokens, cache_creation_tokens_5m, cache_creation_tokens_1h,
            cache_read_tokens, reasoning_tokens, cost_usd
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            message_row_id,
            session_row_id,
            message.model,
            usage.service_tier,
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
            usage.cache_creation_input_tokens.unwrap_or(0),
            usage.cache_creation_input_tokens_5m.unwrap_or(0),
            usage.cache_creation_input_tokens_1h.unwrap_or(0),
            usage.cache_read_input_tokens.unwrap_or(0),
            usage.reasoning_tokens.unwrap_or(0),
            message.cost_usd,
        ],
    )
    .map_err(|e| format!("Failed to insert usage row: {e}"))?;
    Ok(())
}

/// Inserts a `tool_call` row for a `tool_use` content block, plus any
/// `command`/`file_event` rows its `input` implies. Returns the new row id
/// and the tool's name (needed by the caller to key the correlation map).
pub(super) fn insert_tool_use(
    conn: &Connection,
    message_row_id: i64,
    session_row_id: i64,
    block: &serde_json::Value,
) -> Result<Option<(i64, String)>, String> {
    let Some(tool_name) = block.get("name").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let tool_use_id = block.get("id").and_then(serde_json::Value::as_str);
    let input = block.get("input");
    let input_json = input.map(serde_json::Value::to_string);
    let subagent_type = input
        .and_then(|i| i.get("subagent_type"))
        .and_then(serde_json::Value::as_str);
    let skill_name = input
        .and_then(|i| i.get("skill"))
        .and_then(serde_json::Value::as_str);
    let is_subagent_task = tool_name == "Agent";

    conn.execute(
        "INSERT INTO tool_call (
            message_id, session_id, tool_use_id, tool_name, input_json,
            parent_tool_use_id, is_subagent_task, subagent_type, skill_name
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            message_row_id,
            session_row_id,
            tool_use_id,
            tool_name,
            input_json,
            Option::<String>::None,
            is_subagent_task,
            subagent_type,
            skill_name,
        ],
    )
    .map_err(|e| format!("Failed to insert tool_call row: {e}"))?;
    let tool_call_row_id = conn.last_insert_rowid();

    if tool_name == "Bash" {
        if let Some(command_text) = input
            .and_then(|i| i.get("command"))
            .and_then(serde_json::Value::as_str)
        {
            let description = input
                .and_then(|i| i.get("description"))
                .and_then(serde_json::Value::as_str);
            let timeout_ms = input
                .and_then(|i| i.get("timeout"))
                .and_then(serde_json::Value::as_i64);
            conn.execute(
                "INSERT INTO command (tool_call_id, session_id, shell_command, description, timeout_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![tool_call_row_id, session_row_id, command_text, description, timeout_ms],
            )
            .map_err(|e| format!("Failed to insert command row: {e}"))?;
        }
    }

    if let Some(file_path) = extract_file_path(tool_name, input) {
        let event_type = file_event_type(tool_name);
        conn.execute(
            "INSERT INTO file_event (tool_call_id, session_id, event_type, file_path)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tool_call_row_id, session_row_id, event_type, file_path],
        )
        .map_err(|e| format!("Failed to insert file_event row: {e}"))?;
    }

    Ok(Some((tool_call_row_id, tool_name.to_string())))
}

/// Inserts a `tool_result` row for a `tool_result` content block, matched
/// back to its `tool_call` via `tool_use_id`. Silently no-ops if no
/// matching `tool_call` was seen earlier in this session (e.g. the `tool_use`
/// lives in a predecessor file of a cross-file chain that wasn't part of
/// this pass) -- a result with no evidence trail is not worth failing the
/// whole ingest over.
pub(super) fn insert_tool_result(
    conn: &Connection,
    session_row_id: i64,
    message_row_id: i64,
    block: &serde_json::Value,
    tool_call_rows: &HashMap<String, (i64, String)>,
) -> Result<(), String> {
    let Some(tool_use_id) = block.get("tool_use_id").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let Some((tool_call_row_id, tool_name)) = tool_call_rows.get(tool_use_id) else {
        return Ok(());
    };
    let is_error = block
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let result_summary = block
        .get("content")
        .and_then(|c| flatten_text_preview(c, TEXT_PREVIEW_MAX_LEN));

    conn.execute(
        "INSERT INTO tool_result (tool_call_id, session_id, is_error, result_summary)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![tool_call_row_id, session_row_id, is_error, result_summary],
    )
    .map_err(|e| format!("Failed to insert tool_result row: {e}"))?;

    if tool_name == "Agent" {
        // `started_at`/`ended_at`/`tool_call_count`/`child_session_id`/
        // `subagent_type` are deliberately left unset here -- they need this
        // subagent's own transcript (a separate file under `subagents/`),
        // which isn't ingested until the whole parent session file has been
        // processed. `ingest_subagent_tree` (called right after this
        // session finishes ingesting, see `ingest/mod.rs`) finds and
        // correlates those files via `commands::session::load::
        // get_session_subagents` and backfills this row's remaining
        // columns then.
        let status = if is_error { "error" } else { "completed" };
        conn.execute(
            "INSERT INTO agent_run (session_id, parent_tool_call_id, status)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![session_row_id, tool_call_row_id, status],
        )
        .map_err(|e| format!("Failed to insert agent_run row: {e}"))?;
    }

    if is_error {
        let error_signature = result_summary
            .as_deref()
            .map(truncate_error_signature)
            .unwrap_or_else(|| "unknown error".to_string());
        conn.execute(
            "INSERT INTO error (session_id, tool_result_id, message_id, error_signature, raw_text)
             VALUES (?1, last_insert_rowid(), ?2, ?3, ?4)",
            rusqlite::params![session_row_id, message_row_id, error_signature, result_summary],
        )
        .map_err(|e| format!("Failed to insert error row: {e}"))?;
    }

    Ok(())
}

/// First line only, capped -- a stable-ish grouping key for "occurred in N
/// sessions" aggregates (spec §10), not a full error message.
fn truncate_error_signature(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    first_line.chars().take(200).collect()
}

fn file_event_type(tool_name: &str) -> &'static str {
    match tool_name {
        "Read" => "read",
        "Write" => "write",
        "Edit" | "NotebookEdit" => "edit",
        "Glob" => "glob",
        "Grep" => "grep",
        _ => "other",
    }
}

fn extract_file_path(tool_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    if !matches!(
        tool_name,
        "Read" | "Write" | "Edit" | "NotebookEdit" | "Glob" | "Grep"
    ) {
        return None;
    }
    let input = input?;
    for key in ["file_path", "notebook_path", "path"] {
        if let Some(value) = input.get(key).and_then(serde_json::Value::as_str) {
            return Some(value.to_string());
        }
    }
    None
}

/// Flattens a message `content` value (a plain string, or an array of
/// content blocks) into a bounded preview string. Preview only -- not
/// full fidelity, per this table's schema doc comment.
fn flatten_text_preview(value: &serde_json::Value, max_len: usize) -> Option<String> {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            parts.join("\n")
        }
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(max_len).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_db::ingest::{upsert_project, upsert_provider};
    use crate::archive_db::migrate::migrate;
    use std::fs;
    use tempfile::TempDir;

    fn migrated_connection() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn write_session_fixture(dir: &TempDir, filename: &str, lines: &[&str]) -> ClaudeSession {
        let file_path = dir.path().join(filename);
        fs::write(&file_path, lines.join("\n") + "\n").unwrap();
        ClaudeSession {
            session_id: filename.to_string(),
            actual_session_id: "test-session".to_string(),
            file_path: file_path.to_string_lossy().to_string(),
            project_name: "test-project".to_string(),
            message_count: lines.len(),
            first_message_time: "2026-01-01T00:00:00Z".to_string(),
            last_message_time: "2026-01-01T00:01:00Z".to_string(),
            last_modified: "2026-01-01T00:01:00Z".to_string(),
            has_tool_use: true,
            has_errors: false,
            summary: Some("Test session".to_string()),
            is_renamed: false,
            provider: Some("claude".to_string()),
            storage_type: None,
            entrypoint: Some("cli".to_string()),
        }
    }

    const TOOL_USE_LINE: &str = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi","description":"say hi"}}],"model":"claude-x","usage":{"input_tokens":10,"output_tokens":5}}}"#;
    const TOOL_RESULT_LINE: &str = r#"{"uuid":"u2","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"hi","is_error":false}]}}"#;
    const ERROR_RESULT_LINE: &str = r#"{"uuid":"u3","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:02Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"boom: exit 1","is_error":true}]}}"#;

    #[tokio::test]
    async fn ingests_tool_use_and_matching_tool_result() {
        let dir = TempDir::new().unwrap();
        let session = write_session_fixture(&dir, "s1.jsonl", &[TOOL_USE_LINE, TOOL_RESULT_LINE]);

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        let outcome = ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();
        assert!(!outcome.skipped_unchanged);
        assert_eq!(outcome.messages_ingested, 2);

        let tool_call_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_call", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tool_call_count, 1);

        let (shell_command, is_error): (String, bool) = conn
            .query_row(
                "SELECT c.shell_command, r.is_error FROM command c
                 JOIN tool_result r ON r.tool_call_id = c.tool_call_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(shell_command, "echo hi");
        assert!(!is_error);

        let usage_input_tokens: i64 = conn
            .query_row("SELECT input_tokens FROM usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(usage_input_tokens, 10);
    }

    #[tokio::test]
    async fn records_an_error_row_for_a_failed_tool_result() {
        let dir = TempDir::new().unwrap();
        let session = write_session_fixture(&dir, "s1.jsonl", &[TOOL_USE_LINE, ERROR_RESULT_LINE]);

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();

        let error_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM error", [], |r| r.get(0))
            .unwrap();
        assert_eq!(error_count, 1);
    }

    #[tokio::test]
    async fn skips_unchanged_file_on_second_ingest() {
        let dir = TempDir::new().unwrap();
        let session = write_session_fixture(&dir, "s1.jsonl", &[TOOL_USE_LINE, TOOL_RESULT_LINE]);

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();
        let second = ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();

        assert!(second.skipped_unchanged);
        assert_eq!(second.messages_ingested, 0);

        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session_count, 1);
    }

    #[tokio::test]
    async fn re_ingests_fully_when_file_changes() {
        let dir = TempDir::new().unwrap();
        let session = write_session_fixture(&dir, "s1.jsonl", &[TOOL_USE_LINE]);

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();

        // Ensure a strictly later mtime so the (size, mtime) signature
        // actually differs -- appending content on the same filesystem
        // clock tick would otherwise look "unchanged".
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(
            &session.file_path,
            format!("{TOOL_USE_LINE}\n{TOOL_RESULT_LINE}\n"),
        )
        .unwrap();

        let outcome = ingest_claude_session_file(&mut conn, provider_id, project_id, &session)
            .await
            .unwrap();
        assert!(!outcome.skipped_unchanged);
        assert_eq!(outcome.messages_ingested, 2);

        let session_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(session_count, 1, "re-ingest should replace, not duplicate, the session row");

        let message_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(message_count, 2);
    }

    /// Regression guard for the universal-provider-ingestion refactor
    /// (Step 1): `persist_session_messages` must be fully disk/network-
    /// independent, taking already-loaded messages and touching only the
    /// DB connection -- no `TempDir`, no file I/O anywhere in this test.
    /// Every later step (feeding it messages loaded by a non-Claude
    /// provider's own loader) depends on this contract holding.
    #[tokio::test]
    async fn persist_session_messages_is_disk_independent() {
        use crate::archive_db::ingest::persist_session_messages;
        use crate::models::TokenUsage;

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        let session = ClaudeSession {
            session_id: "hand-built.jsonl".to_string(),
            actual_session_id: "test-session".to_string(),
            file_path: "/does/not/exist/on/disk.jsonl".to_string(),
            project_name: "test-project".to_string(),
            message_count: 1,
            first_message_time: "2026-01-01T00:00:00Z".to_string(),
            last_message_time: "2026-01-01T00:00:00Z".to_string(),
            last_modified: "2026-01-01T00:00:00Z".to_string(),
            has_tool_use: false,
            has_errors: false,
            summary: Some("Hand-built session".to_string()),
            is_renamed: false,
            provider: Some("claude".to_string()),
            storage_type: None,
            entrypoint: Some("cli".to_string()),
        };
        let messages = vec![ClaudeMessage {
            uuid: "u1".to_string(),
            session_id: "hand-built.jsonl".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            message_type: "assistant".to_string(),
            role: Some("assistant".to_string()),
            model: Some("claude-x".to_string()),
            content: Some(serde_json::json!([{"type": "text", "text": "hi"}])),
            usage: Some(TokenUsage {
                input_tokens: Some(7),
                output_tokens: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let outcome = persist_session_messages(
            &mut conn, project_id, &session, &messages, 123, 456, CLAUDE_PARSER_VERSION,
        )
        .unwrap();
        assert!(!outcome.skipped_unchanged);
        assert_eq!(outcome.messages_ingested, 1);

        let (dominant_model, total_input_tokens): (Option<String>, i64) = conn
            .query_row(
                "SELECT dominant_model, total_input_tokens FROM session WHERE id = last_insert_rowid()",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dominant_model.as_deref(), Some("claude-x"));
        assert_eq!(total_input_tokens, 7);
    }

    #[test]
    fn flatten_text_preview_handles_string_and_array_shapes() {
        assert_eq!(
            flatten_text_preview(&serde_json::json!("hello"), 100),
            Some("hello".to_string())
        );
        assert_eq!(
            flatten_text_preview(&serde_json::json!([{"type":"text","text":"a"},{"type":"text","text":"b"}]), 100),
            Some("a\nb".to_string())
        );
        assert_eq!(flatten_text_preview(&serde_json::json!(null), 100), None);
    }

    #[test]
    fn extract_file_path_only_applies_to_file_tools() {
        let input = serde_json::json!({"file_path": "/a/b.rs"});
        assert_eq!(extract_file_path("Read", Some(&input)), Some("/a/b.rs".to_string()));
        assert_eq!(extract_file_path("Bash", Some(&input)), None);
    }

    /// Real, 3-level parent -> subagent -> sub-subagent fixture, matching
    /// the exact on-disk layout `find_subagent_files`/`get_session_subagents` expect
    /// (`{parent-stem}/subagents/agent-<id>.jsonl` + a sibling
    /// `.meta.json` carrying the launching `tool_use`'s id) -- not a
    /// synthetic shortcut, the same directory convention a real Claude
    /// Code install uses.
    #[tokio::test]
    async fn ingests_a_real_two_level_subagent_tree_end_to_end() {
        let dir = TempDir::new().unwrap();

        let parent_lines = [
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{"subagent_type":"general-purpose","description":"do a task"}}],"model":"claude-x","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"uuid":"p2","parentUuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:05:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"done","is_error":false}]}}"#,
        ];
        let parent_session = write_session_fixture(&dir, "parent.jsonl", &parent_lines);

        // Subagent (child) transcript: `{parent-stem}/subagents/agent-sub1.jsonl`,
        // matching `find_subagent_files`'s native layout exactly.
        let subagents_dir = dir.path().join("parent").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        let sub1_lines = [
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent2","name":"Agent","input":{"subagent_type":"researcher"}}],"model":"claude-x","usage":{"input_tokens":3,"output_tokens":2}}}"#,
            r#"{"uuid":"s2","parentUuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:04:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent2","content":"sub done","is_error":false}]}}"#,
        ];
        fs::write(
            subagents_dir.join("agent-sub1.jsonl"),
            sub1_lines.join("\n") + "\n",
        )
        .unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_agent1"}"#,
        )
        .unwrap();

        // Sub-subagent (grandchild) transcript, one level deeper under the
        // child FILE's own stem (`agent-sub1`, not the derived `agent_id`
        // `sub1` -- `find_subagent_files` keys off the real file stem):
        // `parent/subagents/agent-sub1/subagents/agent-sub2.jsonl`.
        let grandchild_dir = subagents_dir.join("agent-sub1").join("subagents");
        fs::create_dir_all(&grandchild_dir).unwrap();
        let sub2_lines = [
            r#"{"uuid":"g1","sessionId":"sub2","timestamp":"2026-01-01T00:02:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}],"model":"claude-x"}}"#,
        ];
        fs::write(
            grandchild_dir.join("agent-sub2.jsonl"),
            sub2_lines.join("\n") + "\n",
        )
        .unwrap();
        fs::write(
            grandchild_dir.join("agent-sub2.meta.json"),
            r#"{"toolUseId":"toolu_agent2"}"#,
        )
        .unwrap();

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &parent_session)
            .await
            .unwrap();
        ingest_subagent_tree(&mut conn, provider_id, project_id, &parent_session.file_path, 0)
            .await
            .unwrap();

        // Every session (parent + 2 subagents) landed as its own row, and
        // only the 2 subagents are flagged.
        let is_subagent_by_key: Vec<(String, i64)> = {
            let mut stmt = conn
                .prepare("SELECT session_key, is_subagent FROM session ORDER BY session_key")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            is_subagent_by_key,
            vec![
                ("parent.jsonl".to_string(), 0),
                ("sub1".to_string(), 1),
                ("sub2".to_string(), 1),
            ]
        );

        // The parent's own agent_run row is correctly linked to sub1.
        let (child_session_key, started_at, ended_at, tool_call_count, subagent_type): (
            String,
            Option<String>,
            Option<String>,
            i64,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT cs.session_key, ar.started_at, ar.ended_at, ar.tool_call_count, ar.subagent_type
                 FROM agent_run ar
                 JOIN session ps ON ps.id = ar.session_id
                 JOIN session cs ON cs.id = ar.child_session_id
                 WHERE ps.session_key = 'parent.jsonl'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(child_session_key, "sub1");
        assert_eq!(started_at.as_deref(), Some("2026-01-01T00:01:00Z"));
        assert_eq!(ended_at.as_deref(), Some("2026-01-01T00:04:00Z"));
        assert_eq!(tool_call_count, 1, "sub1 itself made exactly 1 tool call (launching sub2)");
        assert_eq!(subagent_type.as_deref(), Some("general-purpose"));

        // sub1's own agent_run row (launching sub2) is linked to sub2 AND
        // has its parent_agent_run_id correctly backfilled to the row
        // asserted above -- this is the real multi-level tree link.
        let (grandchild_session_key, parent_agent_run_matches): (String, bool) = conn
            .query_row(
                "SELECT cs.session_key, ar.parent_agent_run_id = (
                    SELECT ar2.id FROM agent_run ar2
                    JOIN session ps2 ON ps2.id = ar2.session_id
                    WHERE ps2.session_key = 'parent.jsonl'
                 )
                 FROM agent_run ar
                 JOIN session ps ON ps.id = ar.session_id
                 JOIN session cs ON cs.id = ar.child_session_id
                 WHERE ps.session_key = 'sub1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(grandchild_session_key, "sub2");
        assert!(
            parent_agent_run_matches,
            "sub2's launching agent_run row must chain back to the root's own agent_run row"
        );
    }

    /// A subagent file with no sibling `.meta.json` (older sessions) is
    /// still ingested as a real, correctly-flagged session -- never
    /// silently dropped -- but stays unlinked from any `agent_run` row,
    /// per this module's own documented, disclosed scope.
    #[tokio::test]
    async fn ingests_an_unlinked_subagent_when_no_meta_json_exists() {
        let dir = TempDir::new().unwrap();
        let parent_lines = [
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{}}],"model":"claude-x"}}"#,
            r#"{"uuid":"p2","parentUuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:05:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"done","is_error":false}]}}"#,
        ];
        let parent_session = write_session_fixture(&dir, "parent.jsonl", &parent_lines);

        let subagents_dir = dir.path().join("parent").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(
            subagents_dir.join("agent-old1.jsonl"),
            r#"{"uuid":"s1","sessionId":"old1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"model":"claude-x"}}"#.to_string() + "\n",
        )
        .unwrap();
        // Deliberately no `agent-old1.meta.json`.

        let mut conn = migrated_connection();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &parent_session)
            .await
            .unwrap();
        ingest_subagent_tree(&mut conn, provider_id, project_id, &parent_session.file_path, 0)
            .await
            .unwrap();

        let is_subagent: i64 = conn
            .query_row(
                "SELECT is_subagent FROM session WHERE session_key = 'old1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(is_subagent, 1, "still ingested and flagged, not dropped");

        let linked_child_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run WHERE child_session_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked_child_count, 0, "no agent_run row could be matched without a tool_use_id");
    }

    /// A real write-path risk, exercised end-to-end (not just the
    /// synthetic unit test in `ingest::mod`'s own test module):
    /// re-ingesting a CHANGED subagent file deletes then reinserts its
    /// session row (`persist_session_messages`) -- whether or not `SQLite`
    /// happens to reuse the same numeric id for the new row (it can, for a
    /// non-`AUTOINCREMENT` `INTEGER PRIMARY KEY` -- never assumed either
    /// way, see `delete_session_rows`'s own doc comment), the parent's
    /// `agent_run.child_session_id` must end up pointing at the CURRENT
    /// row's real content, and re-running the whole tree must not raise a
    /// foreign-key error along the way.
    #[tokio::test]
    async fn re_ingesting_a_changed_subagent_follows_its_new_row_id() {
        let dir = TempDir::new().unwrap();
        let parent_lines = [
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{}}],"model":"claude-x"}}"#,
            r#"{"uuid":"p2","parentUuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:05:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"done","is_error":false}]}}"#,
        ];
        let parent_session = write_session_fixture(&dir, "parent.jsonl", &parent_lines);

        let subagents_dir = dir.path().join("parent").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        let sub_path = subagents_dir.join("agent-sub1.jsonl");
        fs::write(
            &sub_path,
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"v1"}],"model":"claude-x"}}"#.to_string() + "\n",
        )
        .unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_agent1"}"#,
        )
        .unwrap();

        let mut conn = migrated_connection();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let provider_id = upsert_provider(&conn, "claude", "Claude Code", "A", CLAUDE_PARSER_VERSION).unwrap();
        let project_id = upsert_project(&conn, provider_id, "/p", "p", None).unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &parent_session)
            .await
            .unwrap();
        ingest_subagent_tree(&mut conn, provider_id, project_id, &parent_session.file_path, 0)
            .await
            .unwrap();

        let first_child_id: i64 = conn
            .query_row(
                "SELECT child_session_id FROM agent_run WHERE child_session_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Force the next `session` insert to land at a genuinely NEW id --
        // without this, SQLite's non-`AUTOINCREMENT` `INTEGER PRIMARY KEY`
        // can (and, confirmed directly, sometimes does) reuse the just-
        // deleted row's own id when it happened to be the table's current
        // max, which would make this test pass even with a naive/buggy fix
        // that never actually re-resolves `child_session_id`. A dummy row
        // at a deliberately high id guarantees the next real insert can't
        // coincidentally land back on the original id, so this test
        // actually exercises the "id changed" case, not just the "id
        // happened to stay the same" one.
        conn.execute(
            "INSERT INTO session (id, project_id, session_key, file_path, file_size, file_mtime, parser_version, last_ingested_at)
             VALUES (1000, ?1, 'dummy-high-id', '/dummy', 0, 0, 1, '2026-01-01T00:00:00Z')",
            [project_id],
        )
        .unwrap();

        // Change the subagent's own content (strictly later mtime, same
        // pattern `re_ingests_fully_when_file_changes` already uses) and
        // re-run the whole tree -- must not raise an FK error.
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(
            &sub_path,
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"v2"}],"model":"claude-x"}}"#.to_string()
                + "\n"
                + r#"{"uuid":"s2","parentUuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:02:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"more"}],"model":"claude-x"}}"#
                + "\n",
        )
        .unwrap();

        ingest_claude_session_file(&mut conn, provider_id, project_id, &parent_session)
            .await
            .unwrap();
        ingest_subagent_tree(&mut conn, provider_id, project_id, &parent_session.file_path, 0)
            .await
            .expect("re-ingesting a changed subagent must not violate the child_session_id FK");

        let second_child_id: i64 = conn
            .query_row(
                "SELECT child_session_id FROM agent_run WHERE child_session_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(
            first_child_id, second_child_id,
            "the dummy high-id row should have forced a genuinely new id for this test"
        );

        let child_message_count: i64 = conn
            .query_row(
                "SELECT message_count FROM session WHERE id = ?1",
                [second_child_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(child_message_count, 2, "the parent must point at the CURRENT, re-ingested child row");
    }
}
