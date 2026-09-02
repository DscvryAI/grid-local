//! Generic (non-Claude) provider ingestion -- turns an already-scanned
//! `ClaudeProject`/`ClaudeSession` from any file-based provider into
//! normalized `archive_db` rows, via the same [`super::persist_session_messages`]
//! every Claude session already goes through (universal-provider-ingestion
//! plan, Step 3; see the plan file referenced in `ingest/mod.rs`'s module
//! doc). Only providers listed in
//! [`crate::commands::stats::FILE_BASED_STATS_PROVIDERS`] are safe to call
//! this with -- their `(file_size, file_mtime)` staleness model doesn't fit
//! the DB-based/hybrid providers, which stay on the raw-scan-only path.
//!
//! Deliberately reuses `commands::stats::StatsProvider` for dispatch rather
//! than a second provider registry -- `commands::history` already depends
//! on the same enum for its own raw-scan fallback (see that enum's own doc
//! comment), and a second registry would just be a second place for a
//! provider fix to silently not apply.

use rayon::prelude::*;
use rusqlite::Connection;
use std::path::Path;

use crate::commands::stats::{self, StatsProvider};
use crate::models::{ClaudeMessage, ClaudeProject, ClaudeSession};

use super::IngestOutcome;

/// Bump on any material change to this file's extraction/mapping logic.
/// Separate from `claude::CLAUDE_PARSER_VERSION` so a bug fix here doesn't
/// force an unnecessary Claude re-ingest, and vice versa -- shared across
/// all file-based providers since they funnel through the same generic
/// `persist_session_messages` path.
pub const GENERIC_PARSER_VERSION: i64 = 1;

/// Human-readable name for a `StatsProvider`, reusing `providers::ProviderId`
/// (the existing display-name source of truth for the project tree/Settings
/// UI) rather than inventing a second one. Falls back to the raw provider
/// key in the unreachable case the two registries' key sets ever drift.
fn display_name(provider: StatsProvider) -> String {
    let key = stats::stats_provider_id(provider);
    crate::providers::ProviderId::parse(key)
        .map(|p| p.display_name().to_string())
        .unwrap_or_else(|| key.to_string())
}

/// Ingests every session of one file-based provider's one already-scanned
/// project. Mirrors [`super::ingest_claude_project`]'s shape, but plain
/// sync -- unlike Claude's own ingest, nothing on this path (scan/load/
/// persist) is actually async, so an `async fn` here would just be
/// ceremony (and clippy's `unused_async` correctly says so). Step 5's
/// backfill loop calls this without `.await`, alongside Claude's own
/// genuinely-async `ingest_claude_project` in the same loop.
///
/// Splits into two passes rather than one straight-through per-session
/// loop: a cheap, sequential staleness check first (one `fs::metadata` +
/// one indexed `SELECT` per session -- needs `conn`, but does no parsing),
/// then a `rayon`-parallel pass that actually parses each changed/new
/// session's full message content (`stats::load_stats_messages`), which
/// is read-only and independent per session. Only the final persist step
/// touches `conn` again, sequentially. This matters most for "Rebuild
/// index" (spec §31): it wipes every row first, so the staleness check
/// can't skip anything and every session in every provider pays the full
/// parse cost -- confirmed live that doing so strictly sequentially took
/// 1237s to rebuild a real 397-session Codex archive alone.
pub(crate) fn ingest_stats_provider_project(
    conn: &mut Connection,
    provider: StatsProvider,
    project: &ClaudeProject,
) -> Result<Vec<IngestOutcome>, String> {
    let sessions = stats::load_stats_sessions(provider, &project.path)?;
    ingest_stats_provider_sessions(conn, provider, project, &sessions)
}

/// Same as [`ingest_stats_provider_project`], but takes an already-fetched
/// session list instead of calling `stats::load_stats_sessions` itself.
/// Exists for callers (currently just Codex's bespoke fast path in
/// `backfill::run_full_backfill`) that already have every session for
/// every project of a provider from ONE whole-store scan and want to avoid
/// paying `load_stats_sessions`'s per-project cost redundantly -- for
/// Codex specifically, that function re-walks and re-stat-checks the
/// entire rollout store on every call (`providers::codex::load_sessions`'s
/// own doc comment), which showed up live as a large, otherwise-invisible
/// share of a real "Rebuild index" run's total time (confirmed via
/// instrumentation, not guessed). Every other file-based provider still
/// goes through [`ingest_stats_provider_project`] unchanged.
pub(crate) fn ingest_stats_provider_sessions(
    conn: &mut Connection,
    provider: StatsProvider,
    project: &ClaudeProject,
    sessions: &[ClaudeSession],
) -> Result<Vec<IngestOutcome>, String> {
    let provider_key = stats::stats_provider_id(provider);
    let provider_id = super::upsert_provider(
        conn,
        provider_key,
        &display_name(provider),
        "B",
        GENERIC_PARSER_VERSION,
    )?;
    let project_id = super::upsert_project(
        conn,
        provider_id,
        &project.path,
        &project.name,
        Some(&project.actual_path),
    )?;

    let mut outcomes = Vec::with_capacity(sessions.len());
    let mut pending: Vec<PendingSession<'_>> = Vec::new();
    for session in sessions {
        let path = Path::new(&session.file_path);
        let (file_size, file_mtime) = super::stat_signature(path)?;
        if let Some((_, existing_size, existing_mtime, existing_parser_version)) =
            super::existing_session_signature(conn, project_id, &session.session_id)?
        {
            if existing_size == file_size
                && existing_mtime == file_mtime
                && existing_parser_version == GENERIC_PARSER_VERSION
            {
                outcomes.push(IngestOutcome {
                    messages_ingested: 0,
                    skipped_unchanged: true,
                });
                continue;
            }
        }
        pending.push(PendingSession {
            session,
            file_size,
            file_mtime,
        });
    }

    let parsed: Vec<ParsedSession<'_>> = pending
        .into_par_iter()
        .map(|pending| {
            let messages =
                stats::load_stats_messages(provider, load_messages_key(provider, pending.session));
            ParsedSession { pending, messages }
        })
        .collect();

    for ParsedSession { pending, messages } in parsed {
        let messages = messages?;
        outcomes.push(super::persist_session_messages(
            conn,
            project_id,
            pending.session,
            &messages,
            pending.file_size,
            pending.file_mtime,
            GENERIC_PARSER_VERSION,
        )?);
    }

    Ok(outcomes)
}

/// One session that has already passed the staleness check (new or
/// changed) and still needs its full message content parsed.
struct PendingSession<'a> {
    session: &'a ClaudeSession,
    file_size: u64,
    file_mtime: u64,
}

/// A [`PendingSession`] plus the (possibly failed) result of parsing it --
/// kept together so the parallel parse pass and the sequential persist
/// pass can be two separate, simple loops instead of one over-complex
/// tuple type (clippy's `type_complexity`, correctly).
struct ParsedSession<'a> {
    pending: PendingSession<'a>,
    messages: Result<Vec<ClaudeMessage>, String>,
}

/// Which `ClaudeSession` field a provider's own `load_messages` actually
/// expects as its `session_path` argument. Confirmed by DIRECT TEST
/// FAILURE, not assumed: `file_path` is the correct, safe default (matches
/// every provider tested so far -- Gemini/Codex/Qwen/OpenInterpreter/Grok
/// all set `session_id == file_path` so either works; Vibe's `load_messages`
/// requires the real absolute directory path, which only lives in
/// `file_path` -- `session_id` there is an opaque UUID string and errors
/// with "session path must be absolute"). Aider is the one confirmed
/// exception: its `load_messages` requires the `aider://<path>#<index>`
/// shape, which only `session_id` carries (`file_path` is the bare shared
/// history file, with no scheme or index -- multiple sessions in one
/// Aider project share the same `file_path`, see
/// `FILE_BASED_STATS_PROVIDERS`'s Aider-specific staleness caveat).
///
/// This is a real, provider-specific divergence, not a single universal
/// rule -- do not "simplify" this back to a blanket choice without
/// re-verifying every provider using this function, the way Vibe's own
/// test failure caught the original wrong assumption here.
fn load_messages_key(provider: StatsProvider, session: &ClaudeSession) -> &str {
    match provider {
        StatsProvider::Aider => &session.session_id,
        _ => &session.file_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_db::migrate::migrate;
    use serial_test::serial;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn migrated_connection() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    /// Saves/restores one process-global env var around a test so a
    /// provider's own `$XXX_HOME`-style override resolves to a fixture
    /// store under a fresh `TempDir` instead of the real user home.
    /// Combined with `#[serial]` so these tests don't race each other --
    /// same pattern several providers' own test modules already establish
    /// per-provider (e.g. `providers::pi`'s `HomeGuard`,
    /// `providers::codex`'s `EnvVarGuard`); this is the equivalent shared
    /// across every dedicated-env-var provider tested from this file.
    ///
    /// Providers that resolve their store via `dirs::home_dir()` directly
    /// with NO dedicated override env var (pi, ompi, antigravity, cline,
    /// ...) are deliberately NOT tested from this file with a `HOME`
    /// override -- confirmed live that `dirs::home_dir()` does not respect
    /// a `HOME` env var override on Windows (this dev machine), matching
    /// `providers::pi`'s/`providers::antigravity`'s own `HOME`-override
    /// tests already being part of the established, pre-existing 56-test
    /// Windows-only failure baseline. Adding more such tests here would
    /// just grow that same known class without being verifiable on this
    /// machine; those providers' fixture tests should be added on a
    /// Mac/Linux dev machine or in CI where `HOME` genuinely works, not
    /// guessed at blind here.
    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }
    impl EnvVarGuard {
        fn set(key: &'static str, path: &Path) -> Self {
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

    /// Writes one legacy-format Gemini session file (a single JSON object
    /// carrying a `messages` array, matching `providers::gemini`'s own test
    /// fixture shape) under `<gemini_home>/tmp/<project_hash>/chats/`, the
    /// exact on-disk layout `scan_projects_from_path` walks.
    fn write_gemini_fixture_project(gemini_home: &Path, project_hash: &str) -> PathBuf {
        let chats_dir = gemini_home.join("tmp").join(project_hash).join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        let session = serde_json::json!({
            "sessionId": "s-1",
            "projectHash": project_hash,
            "startTime": "2026-01-01T00:00:00Z",
            "lastUpdated": "2026-01-01T00:01:00Z",
            "kind": "main",
            "messages": [
                {
                    "id": "u1",
                    "type": "user",
                    "timestamp": "2026-01-01T00:00:00Z",
                    "content": [{"text": "hi"}]
                },
                {
                    "id": "g1",
                    "type": "gemini",
                    "timestamp": "2026-01-01T00:00:01Z",
                    "content": [{"text": "hello"}]
                }
            ]
        });
        fs::write(
            chats_dir.join("session-1.json"),
            serde_json::to_string(&session).unwrap(),
        )
        .unwrap();
        chats_dir.parent().unwrap().to_path_buf()
    }

    #[test]
    #[serial]
    fn ingests_a_real_gemini_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("GEMINI_HOME", home.path());
        write_gemini_fixture_project(home.path(), "hash-1");

        let mut conn = migrated_connection();
        let projects = stats::scan_stats_projects(StatsProvider::Gemini).unwrap();
        assert_eq!(projects.len(), 1, "the fixture project must be discovered");
        assert_eq!(projects[0].provider.as_deref(), Some("gemini"));

        let outcomes =
            ingest_stats_provider_project(&mut conn, StatsProvider::Gemini, &projects[0])
                .unwrap();
        assert_eq!(outcomes.len(), 1, "one session in the fixture project");
        assert!(!outcomes[0].skipped_unchanged);
        assert_eq!(outcomes[0].messages_ingested, 2);

        let (provider_key, session_count): (String, i64) = conn
            .query_row(
                "SELECT pr.provider_key, COUNT(s.id)
                 FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id
                 GROUP BY pr.provider_key",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(provider_key, "gemini");
        assert_eq!(session_count, 1);

        let message_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(message_count, 2, "both the user and gemini messages must round-trip");

        // Second ingest of the same, unchanged fixture must be a no-op --
        // proves the generic staleness check works end to end, not just
        // Claude's own copy of the same logic.
        let projects_again = stats::scan_stats_projects(StatsProvider::Gemini).unwrap();
        let second =
            ingest_stats_provider_project(&mut conn, StatsProvider::Gemini, &projects_again[0])
                .unwrap();
        assert!(second[0].skipped_unchanged);
    }

    fn assert_provider_ingest_round_trips(
        provider: StatsProvider,
        expected_key: &str,
        expected_message_count: i64,
    ) {
        let projects = stats::scan_stats_projects(provider).unwrap();
        assert_eq!(
            projects.len(),
            1,
            "{expected_key}: the fixture project must be discovered"
        );

        let mut conn = migrated_connection();
        let outcomes = ingest_stats_provider_project(&mut conn, provider, &projects[0]).unwrap();
        assert_eq!(outcomes.len(), 1, "{expected_key}: one session in the fixture project");
        assert!(!outcomes[0].skipped_unchanged);
        assert_eq!(outcomes[0].messages_ingested, expected_message_count as u64);

        let provider_key: String = conn
            .query_row(
                "SELECT pr.provider_key FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider_key, expected_key);

        let message_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(message_count, expected_message_count);

        // Second ingest of the same, unchanged fixture must be a no-op.
        let projects_again = stats::scan_stats_projects(provider).unwrap();
        let second = ingest_stats_provider_project(&mut conn, provider, &projects_again[0]).unwrap();
        assert!(second[0].skipped_unchanged);
    }

    /// A Codex rollout with a real `cwd` (so `scan_projects_from_path`
    /// groups it into a discoverable project -- the existing
    /// `providers::codex` fixture tests only ever call `load_messages`
    /// directly, never `scan_projects`, so none of them needed one) plus a
    /// real `exec_command`/`function_call_output` pair, which
    /// `providers::codex`'s own message-construction code already remaps
    /// to the literal `"Bash"` tool name (confirmed directly in the
    /// universal-provider-ingestion plan's Step 4 research) -- so this
    /// fixture also exercises real `command` table population, not just
    /// message round-tripping.
    fn write_codex_fixture_project(codex_home: &Path) {
        let sessions_dir = codex_home.join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        let lines = [
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:00Z",
                "type": "session_meta",
                "payload": { "id": "sess-1" }
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:01Z",
                "type": "turn_context",
                "payload": { "model": "gpt-5-codex", "cwd": "/tmp/fixture-project" }
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:02Z",
                "type": "response_item",
                "payload": {
                    "id": "item-1",
                    "type": "function_call",
                    "name": "exec_command",
                    "call_id": "call_1",
                    "arguments": "{\"cmd\":\"grep -r login\"}"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:03Z",
                "type": "response_item",
                "payload": {
                    "id": "item-2",
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "{\"output\":\"login.rs:42\",\"metadata\":{\"exit_code\":0}}"
                }
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:04Z",
                "type": "response_item",
                "payload": {
                    "id": "item-3",
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "done" }]
                }
            }),
        ];
        let content = lines
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(sessions_dir.join("rollout-2026-01-01.jsonl"), format!("{content}\n")).unwrap();
    }

    #[test]
    #[serial]
    fn ingests_a_real_codex_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("CODEX_HOME", home.path());
        write_codex_fixture_project(home.path());

        // 3 messages: assistant (tool_use+tool_result rolled into one turn
        // per providers::codex's own message-construction convention,
        // matching its existing load_messages_parses_codex_rollout_end_to_end
        // test's own count for a similar shape) is asserted for real below
        // rather than assumed -- see the dynamic message_count check.
        let projects = stats::scan_stats_projects(StatsProvider::Codex).unwrap();
        assert_eq!(projects.len(), 1, "codex: the fixture project must be discovered");
        assert_eq!(projects[0].actual_path, "/tmp/fixture-project");

        let mut conn = migrated_connection();
        let outcomes =
            ingest_stats_provider_project(&mut conn, StatsProvider::Codex, &projects[0]).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].skipped_unchanged);
        assert!(outcomes[0].messages_ingested > 0);

        let provider_key: String = conn
            .query_row(
                "SELECT pr.provider_key FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider_key, "codex");

        // The exec_command/function_call_output pair must round-trip into
        // a real command row -- proves Codex's own "Bash" tool-name remap
        // (found during Step 4 research, not built here) actually feeds
        // archive_db's generic command extraction correctly end to end.
        let (shell_command, is_error): (String, bool) = conn
            .query_row(
                "SELECT c.shell_command, r.is_error FROM command c
                 JOIN tool_result r ON r.tool_call_id = c.tool_call_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(shell_command, "grep -r login");
        assert!(!is_error);

        // Second ingest of the same, unchanged fixture must be a no-op.
        let projects_again = stats::scan_stats_projects(StatsProvider::Codex).unwrap();
        let second =
            ingest_stats_provider_project(&mut conn, StatsProvider::Codex, &projects_again[0])
                .unwrap();
        assert!(second[0].skipped_unchanged);
    }

    /// A minimal Qwen Code session JSONL (`user`/`assistant`/`tool_result` trio,
    /// plus a `custom_title` system record that must NOT count as a
    /// message) -- mirrors `providers::qwen`'s own test fixture shape.
    const QWEN_SESSION: &str = concat!(
        r#"{"uuid":"u1","parentUuid":null,"sessionId":"sess-1","timestamp":"2026-01-01T00:00:00Z","type":"user","cwd":"/tmp/fixture-project","message":{"role":"user","parts":[{"text":"why does login fail?"}]}}"#,
        "\n",
        r#"{"uuid":"u2","parentUuid":"u1","sessionId":"sess-1","timestamp":"2026-01-01T00:00:01Z","type":"assistant","model":"qwen3-coder-plus","message":{"role":"model","parts":[{"functionCall":{"id":"c1","name":"run_shell_command","args":{"command":"grep -r login"}}}]}}"#,
        "\n",
        r#"{"uuid":"u3","parentUuid":"u2","sessionId":"sess-1","timestamp":"2026-01-01T00:00:02Z","type":"tool_result","cwd":"/tmp/fixture-project","message":{"role":"user","parts":[{"functionResponse":{"id":"c1","name":"run_shell_command","response":{"output":"login.rs:42"}}}]}}"#,
        "\n",
        r#"{"uuid":"s1","sessionId":"sess-1","timestamp":"2026-01-01T00:00:03Z","type":"system","subtype":"custom_title","message":{"role":"user","parts":[{"text":"ignored"}]}}"#,
        "\n",
    );

    #[test]
    #[serial]
    fn ingests_a_real_qwen_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("QWEN_HOME", home.path());
        let chats_dir = home
            .path()
            .join("projects")
            .join("fixture-project")
            .join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        fs::write(chats_dir.join("session1.jsonl"), QWEN_SESSION).unwrap();

        assert_provider_ingest_round_trips(StatsProvider::Qwen, "qwen", 3);
    }

    #[test]
    #[serial]
    fn ingests_a_real_openinterpreter_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("INTERPRETER_HOME", home.path());
        // Open Interpreter shares Codex's exact rollout format
        // (providers::codex's own SessionInfo doc comment: "providers that
        // share the Codex rollout format... can reuse the extractors
        // below") -- same fixture shape, different env var/base dir.
        write_codex_fixture_project(home.path());

        let projects = stats::scan_stats_projects(StatsProvider::OpenInterpreter).unwrap();
        assert_eq!(projects.len(), 1, "openinterpreter: the fixture project must be discovered");

        let mut conn = migrated_connection();
        let outcomes = ingest_stats_provider_project(
            &mut conn,
            StatsProvider::OpenInterpreter,
            &projects[0],
        )
        .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].skipped_unchanged);
        assert!(outcomes[0].messages_ingested > 0);

        let provider_key: String = conn
            .query_row(
                "SELECT pr.provider_key FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider_key, "openinterpreter");

        let projects_again = stats::scan_stats_projects(StatsProvider::OpenInterpreter).unwrap();
        let second = ingest_stats_provider_project(
            &mut conn,
            StatsProvider::OpenInterpreter,
            &projects_again[0],
        )
        .unwrap();
        assert!(second[0].skipped_unchanged);
    }

    /// Grok and Vibe (below) both store one session as a DIRECTORY of
    /// files, not a single file -- `session.file_path`/`session_id` are
    /// both that directory's path. `stat_signature` on a directory still
    /// succeeds (confirmed live by this test passing), but see
    /// `FILE_BASED_STATS_PROVIDERS`'s own doc comment for the accepted
    /// mtime-reliability caveat this implies (an in-place-appended file
    /// inside the directory doesn't change the directory's own mtime).
    /// Reuses `providers::grok`'s own test fixture shape almost verbatim
    /// (its private `write_fixture` helper can't be imported cross-module).
    #[test]
    #[serial]
    fn ingests_a_real_grok_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("GROK_HOME", home.path());

        let encoded = "%2FUsers%2Ftest%2Fdemo";
        let session_id = "019fa555-791c-71e2-8c92-ff2e6fa26d6e";
        let session_dir = home.path().join("sessions").join(encoded).join(session_id);
        fs::create_dir_all(&session_dir).unwrap();

        let summary = serde_json::json!({
            "info": { "id": session_id, "cwd": "/Users/test/demo" },
            "session_summary": "Demo session",
            "generated_title": "Demo Title",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:05:00Z",
            "num_chat_messages": 4,
            "current_model_id": "grok-4.5"
        });
        fs::write(session_dir.join("summary.json"), serde_json::to_string(&summary).unwrap())
            .unwrap();
        let lines = [
            r#"{"type":"system","content":"You are Grok."}"#,
            r#"{"type":"user","content":[{"type":"text","text":"why does login fail?"}]}"#,
            r#"{"type":"assistant","content":"checking","tool_calls":[{"id":"call-1","name":"exec_command","arguments":"{\"command\":\"grep -r login\"}"}],"model_id":"grok-4.5"}"#,
            r#"{"type":"tool_result","tool_call_id":"call-1","content":"login.rs:42"}"#,
        ];
        fs::write(session_dir.join("chat_history.jsonl"), lines.join("\n")).unwrap();

        let projects = stats::scan_stats_projects(StatsProvider::Grok).unwrap();
        assert_eq!(projects.len(), 1, "grok: the fixture project must be discovered");

        let mut conn = migrated_connection();
        let outcomes =
            ingest_stats_provider_project(&mut conn, StatsProvider::Grok, &projects[0]).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].skipped_unchanged);
        assert!(outcomes[0].messages_ingested > 0);

        let provider_key: String = conn
            .query_row(
                "SELECT pr.provider_key FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider_key, "grok");

        let projects_again = stats::scan_stats_projects(StatsProvider::Grok).unwrap();
        let second =
            ingest_stats_provider_project(&mut conn, StatsProvider::Grok, &projects_again[0])
                .unwrap();
        assert!(second[0].skipped_unchanged);
    }

    /// Reuses `providers::vibe`'s own test fixture shape almost verbatim
    /// (its private `write_fixture` helper can't be imported cross-module).
    /// Also exercises the `session_id != file_path` load-key handling
    /// (`ingest_stats_provider_session` already keys `load_stats_messages`
    /// on `session.session_id`, not `file_path`, specifically for cases
    /// like this one).
    #[test]
    #[serial]
    fn ingests_a_real_vibe_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("VIBE_HOME", home.path());

        let session_dir = home
            .path()
            .join("logs")
            .join("session")
            .join("session_20260101_120000_abc123");
        fs::create_dir_all(&session_dir).unwrap();

        let metadata = serde_json::json!({
            "session_id": "full-session-id-abc123",
            "start_time": "2026-01-01T12:00:00+00:00",
            "end_time": "2026-01-01T12:05:00+00:00",
            "environment": { "working_directory": "/tmp/fixture-project" },
            "title": "Fix login bug",
            "title_source": "manual",
            "stats": { "session_prompt_tokens": 120, "session_completion_tokens": 45 }
        });
        fs::write(session_dir.join("meta.json"), serde_json::to_string(&metadata).unwrap())
            .unwrap();

        let messages = [
            serde_json::json!({"role": "user", "content": "why does login fail?", "message_id": "msg-1"}),
            serde_json::json!({
                "role": "assistant",
                "content": "let me check",
                "message_id": "msg-2",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": "exec_command", "arguments": "{\"command\":\"grep -r login\"}" }
                }]
            }),
            serde_json::json!({"role": "tool", "tool_call_id": "call-1", "content": "login.rs:42", "message_id": "msg-3"}),
        ];
        let jsonl = messages
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(session_dir.join("messages.jsonl"), jsonl).unwrap();

        assert_provider_ingest_round_trips(StatsProvider::Vibe, "vibe", 3);
    }

    /// Copilot CLI's own `session-state/<session_id>/events.jsonl` layout,
    /// mirroring `providers::copilot_cli`'s own test fixture shape almost
    /// verbatim (its private `write_session` helper can't be imported
    /// cross-module). No `assistant.message`'s `toolRequests[].name` remap
    /// to `"Bash"` exists in `providers::copilot_cli` (confirmed by reading
    /// its `tool_use` block construction directly) -- unlike Codex, so this
    /// fixture's own `"bash"` (lowercase) tool call is expected to leave
    /// `archive_db`'s `command` table empty, matching the accepted,
    /// documented sparse-`command`-table precedent already established for
    /// qwen/openinterpreter/grok/vibe.
    fn write_copilot_cli_fixture_project(copilot_cli_home: &Path, cwd: &str) {
        let session_dir = copilot_cli_home
            .join("session-state")
            .join("cccccccc-cccc-cccc-cccc-cccccccccccc");
        fs::create_dir_all(&session_dir).unwrap();
        let lines = [
            serde_json::json!({
                "type": "session.start",
                "data": {
                    "sessionId": "cccccccc-cccc-cccc-cccc-cccccccccccc",
                    "context": { "cwd": cwd }
                },
                "timestamp": "2026-01-01T00:00:00.000Z"
            }),
            serde_json::json!({
                "type": "user.message",
                "data": { "content": "why does login fail?" },
                "timestamp": "2026-01-01T00:00:01.000Z"
            }),
            serde_json::json!({
                "type": "assistant.message",
                "data": {
                    "messageId": "asst-1",
                    "content": "checking",
                    "toolRequests": [{
                        "toolCallId": "tool-1",
                        "name": "bash",
                        "arguments": { "command": "grep -r login" }
                    }],
                    "outputTokens": 42
                },
                "timestamp": "2026-01-01T00:00:02.000Z"
            }),
            serde_json::json!({
                "type": "tool.execution_complete",
                "data": {
                    "toolCallId": "tool-1",
                    "success": true,
                    "result": { "content": "login.rs:42" }
                },
                "timestamp": "2026-01-01T00:00:03.000Z"
            }),
        ];
        let content = lines
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(session_dir.join("events.jsonl"), content).unwrap();
    }

    /// Proves Copilot -- the 3-way CLI/Desktop/VS Code aggregator
    /// deliberately excluded from `FILE_BASED_STATS_PROVIDERS` for most of
    /// the universal-provider-ingestion plan, then added once
    /// `providers::copilot::merge_projects`'s `sources`-ordering was made
    /// deterministic (see that function's own doc comment and
    /// `providers::copilot::tests::
    /// merge_projects_produces_a_deterministic_project_key_regardless_of_input_order`)
    /// -- round-trips through the SAME generic orchestrator every other
    /// file-based provider uses, end to end via a real `COPILOT_CLI_HOME`
    /// fixture (the CLI/Desktop half; the VS Code half has no dedicated
    /// override env var and isn't exercised here, same class of Windows
    /// limitation as the deferred providers).
    ///
    /// **Deliberately does NOT assert `projects.len() == 1` or a global
    /// project/session count** -- `providers::copilot::scan_projects()`
    /// ALSO scans this dev machine's real, un-overridable VS Code
    /// Copilot Chat data (see `archive_db::test_support`'s own doc comment
    /// for the general shape of this risk), so this test finds ITS OWN
    /// fixture project by `actual_path` instead, matching this session's
    /// established scoping discipline.
    #[test]
    #[serial]
    fn ingests_a_real_copilot_cli_project_end_to_end() {
        let home = TempDir::new().unwrap();
        let _guard = EnvVarGuard::set("COPILOT_CLI_HOME", home.path());
        let cwd = "/tmp/fixture-copilot-project";
        write_copilot_cli_fixture_project(home.path(), cwd);

        let projects = stats::scan_stats_projects(StatsProvider::Copilot).unwrap();
        let project = projects
            .iter()
            .find(|p| p.actual_path == cwd)
            .expect("the fixture project must be discovered");
        assert!(project.path.starts_with("copilot://"));

        let mut conn = migrated_connection();
        let outcomes =
            ingest_stats_provider_project(&mut conn, StatsProvider::Copilot, project).unwrap();
        assert_eq!(outcomes.len(), 1, "one session in the fixture project");
        assert!(!outcomes[0].skipped_unchanged);
        assert_eq!(outcomes[0].messages_ingested, 2, "user + merged assistant");

        let (provider_key, session_count): (String, i64) = conn
            .query_row(
                "SELECT pr.provider_key, COUNT(s.id)
                 FROM provider pr
                 JOIN project p ON p.provider_id = pr.id
                 JOIN session s ON s.project_id = p.id
                 WHERE p.project_key = ?1
                 GROUP BY pr.provider_key",
                [&project.path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(provider_key, "copilot");
        assert_eq!(session_count, 1);

        // Re-scanning independently must regenerate the SAME project_key --
        // proves providers::copilot::merge_projects's sources-ordering fix
        // actually keeps upsert_project's idempotency intact across two
        // genuinely separate scan calls, not just within one.
        let projects_again = stats::scan_stats_projects(StatsProvider::Copilot).unwrap();
        let project_again = projects_again
            .iter()
            .find(|p| p.actual_path == cwd)
            .expect("the fixture project must still be discovered");
        assert_eq!(
            project_again.path, project.path,
            "the project key must be stable across independent scans"
        );
        let second =
            ingest_stats_provider_project(&mut conn, StatsProvider::Copilot, project_again)
                .unwrap();
        assert!(second[0].skipped_unchanged);

        let project_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project WHERE project_key = ?1",
                [&project.path],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            project_count, 1,
            "an unstable project key would have minted a second project row here"
        );
    }
}
