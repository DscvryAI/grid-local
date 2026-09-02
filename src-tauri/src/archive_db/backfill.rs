//! Full backfill (walk every discovered Claude project, plus every
//! file-based provider's projects, and ingest them) and "Rebuild index"
//! (spec §31 Settings action -- wipe Grid's own rows first, then backfill
//! fresh). Universal-provider-ingestion plan, Step 5.

use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

use super::ingest;
use crate::commands::stats::{self, StatsProvider, FILE_BASED_STATS_PROVIDERS};
use crate::models::ClaudeSession;
use crate::providers;

/// Outcome of a full backfill run.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillSummary {
    pub projects_scanned: usize,
    pub sessions_ingested: usize,
    pub sessions_skipped_unchanged: usize,
    pub messages_ingested: u64,
    /// True if a caller-supplied [`BackfillProgressHooks::should_cancel`]
    /// returned true partway through -- the providers processed before
    /// that point are still fully ingested (this never rolls back), only
    /// the remaining providers were skipped.
    pub cancelled: bool,
    /// Wall-clock time of the whole call -- feeds diagnostics export's
    /// "index duration" measure.
    pub duration_ms: u64,
    /// Count of per-project ingest failures across every file-based
    /// provider (the `Err(e)` arm below) -- previously only logged via
    /// `log::warn!` and never surfaced to any caller. Does NOT include a
    /// failure in the Claude-native path (`ingest::ingest_claude_project`
    /// propagates via `?` instead of catching per-project, a larger,
    /// separate behavioral change not made here).
    pub parser_failures: u64,
}

/// One phase of a backfill: "claude" itself counts as the first phase,
/// then each `FILE_BASED_STATS_PROVIDERS` entry is its own phase.
#[derive(Debug, Clone)]
pub struct BackfillPhase<'a> {
    pub provider_key: &'a str,
    pub phases_done: usize,
    pub phases_total: usize,
}

/// Optional progress/cancellation hooks for an interactive (first-run)
/// backfill. Every field defaults to `None` -- the plain
/// [`run_full_backfill`]/[`rebuild_index`] entry points pass
/// `&BackfillProgressHooks::default()`, so they behave exactly as before
/// this was added and every existing call site (tests included) needs no
/// change at all.
#[derive(Default)]
pub struct BackfillProgressHooks<'a> {
    /// Called once at the start of each phase (before that provider's
    /// projects are scanned/ingested).
    pub on_phase_start: Option<&'a (dyn Fn(BackfillPhase) + Send + Sync)>,
    /// Checked between phases (never mid-phase, so a single provider's
    /// ingestion is never interrupted partway). Returning true stops the
    /// backfill before the next phase starts.
    pub should_cancel: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}

/// Walks every discovered Claude project under `claude_base` (the
/// directory containing `projects/`, e.g. `~/.claude`) and ingests its
/// sessions. Idempotent by construction (Step 2's per-file signature
/// check) -- repeat backfills after the first run are nearly free, since
/// an unchanged session is a single `SELECT` with no reparse.
///
/// Takes `claude_base` explicitly rather than resolving it internally via
/// `providers::claude::get_base_path()` -- that resolution depends on
/// `dirs::home_dir()`, which does not reliably honor a test-mocked `$HOME`
/// on Windows. Keeping this function pure w.r.t. the environment makes it
/// safely testable on every platform; the Tauri command wrapper is where
/// the real path gets resolved for production use.
pub async fn run_full_backfill(
    conn: &mut Connection,
    claude_base: &str,
) -> Result<BackfillSummary, String> {
    run_full_backfill_with_hooks(conn, claude_base, &BackfillProgressHooks::default()).await
}

/// Same as [`run_full_backfill`], but reports per-phase progress and can
/// stop early between phases -- the interactive first-run index is the
/// only real caller; every other call site keeps using the plain,
/// hook-free `run_full_backfill`.
pub async fn run_full_backfill_with_hooks(
    conn: &mut Connection,
    claude_base: &str,
    hooks: &BackfillProgressHooks<'_>,
) -> Result<BackfillSummary, String> {
    let started_at = std::time::Instant::now();
    let phases_total = 1 + FILE_BASED_STATS_PROVIDERS.len();
    let mut phases_done = 0usize;

    let mut summary = BackfillSummary::default();

    if let Some(on_phase_start) = hooks.on_phase_start {
        on_phase_start(BackfillPhase {
            provider_key: "claude",
            phases_done,
            phases_total,
        });
    }

    let projects = crate::commands::project::scan_projects(claude_base.to_string()).await?;
    summary.projects_scanned = projects.len();

    for project in &projects {
        let outcomes = ingest::ingest_claude_project(conn, &project.path).await?;
        for outcome in outcomes {
            summary.messages_ingested += outcome.messages_ingested;
            if outcome.skipped_unchanged {
                summary.sessions_skipped_unchanged += 1;
            } else {
                summary.sessions_ingested += 1;
            }
        }
    }
    phases_done += 1;

    // Same idempotent treatment for every file-based provider (universal-
    // provider-ingestion plan, Step 5). One bad provider must not fail the
    // whole backfill -- a scan failure degrades to "no projects found" for
    // that provider (matches `commands::history`'s own raw-scan-fallback
    // graceful-degradation precedent), and a per-project ingest error is
    // logged and skipped rather than propagated.
    for &provider in FILE_BASED_STATS_PROVIDERS {
        if hooks.should_cancel.is_some_and(|f| f()) {
            summary.cancelled = true;
            summary.duration_ms = started_at.elapsed().as_millis() as u64;
            return Ok(summary);
        }
        if let Some(on_phase_start) = hooks.on_phase_start {
            on_phase_start(BackfillPhase {
                provider_key: stats::stats_provider_id(provider),
                phases_done,
                phases_total,
            });
        }

        let stats_projects = stats::scan_stats_projects(provider).unwrap_or_default();
        summary.projects_scanned += stats_projects.len();
        // Codex gets a bespoke fast path: the generic
        // `ingest_stats_provider_project` calls `load_stats_sessions` once
        // PER PROJECT, and for Codex that re-walks and re-stat-checks the
        // entire rollout store every time (same cost class already found
        // and fixed once for `get_global_stats_summary`, never applied
        // here -- see `providers::codex::scan_all_session_info`'s own doc
        // comment). Confirmed live via instrumentation that this was the
        // dominant cost of a real "Rebuild index" run, not the message
        // parsing itself. `collect_global_stats_sessions` walks the store
        // exactly once regardless of how many distinct projects (cwds)
        // exist.
        let codex_sessions_by_cwd: Option<HashMap<String, Vec<ClaudeSession>>> =
            (provider == StatsProvider::Codex).then(|| {
                let mut by_cwd: HashMap<String, Vec<ClaudeSession>> = HashMap::new();
                for (cwd, session) in providers::codex::collect_global_stats_sessions() {
                    by_cwd.entry(cwd).or_default().push(session);
                }
                by_cwd
            });

        for project in &stats_projects {
            let result = match &codex_sessions_by_cwd {
                Some(by_cwd) => {
                    let sessions = by_cwd.get(&project.actual_path).cloned().unwrap_or_default();
                    ingest::provider::ingest_stats_provider_sessions(
                        conn, provider, project, &sessions,
                    )
                }
                None => ingest::provider::ingest_stats_provider_project(conn, provider, project),
            };
            match result {
                Ok(outcomes) => {
                    for outcome in outcomes {
                        summary.messages_ingested += outcome.messages_ingested;
                        if outcome.skipped_unchanged {
                            summary.sessions_skipped_unchanged += 1;
                        } else {
                            summary.sessions_ingested += 1;
                        }
                    }
                }
                Err(e) => {
                    summary.parser_failures += 1;
                    log::warn!(
                        "Backfill: provider {provider:?} project {} failed: {e}",
                        project.path
                    );
                }
            }
        }
        phases_done += 1;
    }

    summary.duration_ms = started_at.elapsed().as_millis() as u64;
    Ok(summary)
}

/// Wipes every owned row (Claude AND every file-based provider -- session
/// and everything under it; project rows too) then runs a full fresh
/// backfill. For the user-facing "Rebuild index" action (spec §31) -- an
/// ordinary app-launch or watcher-triggered backfill should call
/// [`run_full_backfill`] directly instead, since the per-file signature
/// check already makes that cheap without throwing away correct existing
/// data.
pub async fn rebuild_index(conn: &mut Connection, claude_base: &str) -> Result<BackfillSummary, String> {
    rebuild_index_with_hooks(conn, claude_base, &BackfillProgressHooks::default()).await
}

/// Same as [`rebuild_index`], but reports per-phase progress and can stop
/// early -- see [`run_full_backfill_with_hooks`].
pub async fn rebuild_index_with_hooks(
    conn: &mut Connection,
    claude_base: &str,
    hooks: &BackfillProgressHooks<'_>,
) -> Result<BackfillSummary, String> {
    truncate_provider_data(conn, "claude")?;
    for &provider in FILE_BASED_STATS_PROVIDERS {
        truncate_provider_data(conn, stats::stats_provider_id(provider))?;
    }
    run_full_backfill_with_hooks(conn, claude_base, hooks).await
}

/// Deletes every `project`/`session`/child row for one provider, leaving
/// the `provider` row itself intact (so its `id` is stable across a
/// rebuild). No-ops if the provider was never ingested.
fn truncate_provider_data(conn: &Connection, provider_key: &str) -> Result<(), String> {
    let provider_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM provider WHERE provider_key = ?1",
            [provider_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to look up provider `{provider_key}`: {e}"))?;
    let Some(provider_id) = provider_id else {
        return Ok(());
    };

    let session_ids: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT s.id FROM session s
                 JOIN project p ON p.id = s.project_id
                 WHERE p.provider_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([provider_id], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>()
            .map_err(|e: rusqlite::Error| e.to_string())?
    };
    for session_id in session_ids {
        ingest::delete_session_rows(conn, session_id)?;
    }
    conn.execute("DELETE FROM project WHERE provider_id = ?1", [provider_id])
        .map_err(|e| format!("Failed to clear project rows for `{provider_key}`: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_db::migrate::migrate;
    use crate::archive_db::test_support::{empty_codex_home_guard, EnvVarGuard};
    use std::fs;
    use tempfile::TempDir;

    const TOOL_USE_LINE: &str = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":100,"output_tokens":20}}}"#;
    const ERROR_RESULT_LINE: &str = r#"{"uuid":"u2","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"FAILED","is_error":true}]}}"#;

    /// Builds a fixture `<claude_base>/projects/<project>/` directory with
    /// one session file so `scan_projects`/`load_project_sessions` (real,
    /// unmocked parsing) can discover and parse it exactly like a real
    /// install would. Returns `claude_base` as a string for convenience.
    fn write_fixture_project(
        claude_base: &std::path::Path,
        project_dir_name: &str,
        session_lines: &str,
    ) -> String {
        let project_dir = claude_base.join("projects").join(project_dir_name);
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("session1.jsonl"), session_lines).unwrap();
        claude_base.to_string_lossy().to_string()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn run_full_backfill_ingests_every_discovered_project() {
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        // Since Step 5, run_full_backfill also scans every file-based
        // provider -- this dev machine has a real, substantial ~/.codex
        // (hundreds of sessions), so every test in this file must guard
        // against incidentally scanning it (see the Step 5 tests' own
        // shared doc comment below for the full explanation, including
        // why global-total assertions were replaced with provider_key-
        // scoped ones here too).
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let (claude_sessions, _) = claude_and_gemini_session_counts(&conn);
        assert_eq!(claude_sessions, 1);

        // Sample aggregate query in the exact shape spec §10's "Things
        // worth looking at" needs: sessions ranked by total_tokens,
        // scoped to Claude's own session (a real machine may have other
        // providers' sessions with their own token totals too).
        let top_claude_session_tokens: i64 = conn
            .query_row(
                "SELECT s.total_tokens FROM session s
                 JOIN project p ON p.id = s.project_id
                 JOIN provider pr ON pr.id = p.provider_id
                 WHERE pr.provider_key = 'claude'
                 ORDER BY s.total_tokens DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(top_claude_session_tokens, 120);

        // And the repeated-command-failure shape: an errored Bash command
        // grouped by its text, scoped to our own fixture's exact command
        // text (unique enough that no incidental real data would collide).
        let failed_pytest_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM command c
                 JOIN tool_result r ON r.tool_call_id = c.tool_call_id
                 WHERE c.shell_command = 'pytest -q' AND r.is_error = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(failed_pytest_runs, 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeat_backfill_is_a_near_no_op_via_step_2_idempotency() {
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        let second = run_full_backfill(&mut conn, &claude_base).await.unwrap();

        // Robust against incidental real data on this machine: whatever
        // the first call found (our fixture plus any real sessions),
        // nothing changed on disk between the two calls, so the second
        // call must ingest exactly zero NEW sessions.
        assert_eq!(second.sessions_ingested, 0);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn rebuild_index_produces_the_same_result_as_a_fresh_backfill() {
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        rebuild_index(&mut conn, &claude_base).await.unwrap();

        let (claude_sessions, _) = claude_and_gemini_session_counts(&conn);
        assert_eq!(
            claude_sessions, 1,
            "rebuild must re-ingest Claude's session with no duplicates, not skip or drop it"
        );
    }

    fn write_gemini_fixture_project(gemini_home: &std::path::Path, project_hash: &str) {
        let chats_dir = gemini_home.join("tmp").join(project_hash).join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        let session = serde_json::json!({
            "sessionId": "s-1",
            "projectHash": project_hash,
            "startTime": "2026-01-01T00:00:00Z",
            "lastUpdated": "2026-01-01T00:01:00Z",
            "kind": "main",
            "messages": [
                {"id": "u1", "type": "user", "timestamp": "2026-01-01T00:00:00Z", "content": [{"text": "hi"}]},
                {"id": "g1", "type": "gemini", "timestamp": "2026-01-01T00:00:01Z", "content": [{"text": "hello"}]}
            ]
        });
        fs::write(chats_dir.join("session-1.json"), serde_json::to_string(&session).unwrap()).unwrap();
    }

    /// `FILE_BASED_STATS_PROVIDERS` includes several providers this
    /// specific dev machine has REAL, substantial data for (a genuine
    /// `~/.codex` with hundreds of sessions; real Antigravity data under
    /// `~/.gemini/antigravity`, resolved independently of `GEMINI_HOME`
    /// and un-overridable on Windows -- see this module's own Step 4
    /// notes on `dirs::home_dir()` not respecting a `HOME` override here).
    /// Every test below therefore (a) overrides `CODEX_HOME` to an empty
    /// dir to avoid scanning/parsing hundreds of real sessions, and (b)
    /// asserts against SPECIFIC `provider_key` rows (`'claude'`/
    /// `'gemini'`) rather than global summary totals, since a handful of
    /// real Antigravity/other HOME-based-provider sessions may still
    /// incidentally appear on this machine and must not make these tests
    /// flaky. A Mac/Linux/CI machine with no real provider installs would
    /// see identical global totals to what these scoped assertions check;
    /// the scoping is what keeps the test correct EVERYWHERE, not just
    /// there. `empty_codex_home_guard`/`EnvVarGuard` live in
    /// `archive_db::test_support` (shared with `mod.rs`/`insights.rs`/
    /// `history.rs`'s own test modules, all of which hit this exact same
    /// problem once `run_full_backfill` started scanning every provider).
    fn claude_and_gemini_session_counts(conn: &Connection) -> (i64, i64) {
        let claude: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session s
                 JOIN project p ON p.id = s.project_id
                 JOIN provider pr ON pr.id = p.provider_id
                 WHERE pr.provider_key = 'claude'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let gemini: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session s
                 JOIN project p ON p.id = s.project_id
                 JOIN provider pr ON pr.id = p.provider_id
                 WHERE pr.provider_key = 'gemini'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (claude, gemini)
    }

    /// `run_full_backfill` picks up Claude AND a file-based provider
    /// (Gemini) in one call -- the actual Step 5 requirement, not just
    /// Claude's own already-proven behavior.
    #[tokio::test]
    #[serial_test::serial]
    async fn run_full_backfill_ingests_claude_and_a_file_based_provider_together() {
        let claude_dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            claude_dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();
        let gemini_home = TempDir::new().unwrap();
        let _gemini_guard = EnvVarGuard::set("GEMINI_HOME", gemini_home.path());
        write_gemini_fixture_project(gemini_home.path(), "hash-1");

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        assert_eq!(claude_and_gemini_session_counts(&conn), (1, 1));

        // Second call: idempotency must engage for everything the first
        // call already ingested (our fixtures, and any incidental real
        // session this machine might additionally contribute) -- zero
        // NEW sessions on a repeat call with nothing changed on disk.
        let second = run_full_backfill(&mut conn, &claude_base).await.unwrap();
        assert_eq!(
            second.sessions_ingested, 0,
            "a repeat backfill must not re-ingest sessions it already has"
        );
        assert_eq!(
            claude_and_gemini_session_counts(&conn),
            (1, 1),
            "no duplicate rows on repeat backfill"
        );
    }

    /// `rebuild_index` must wipe and refresh EVERY provider, not just
    /// Claude -- confirms the Step 5 extension to `truncate_provider_data`.
    #[tokio::test]
    #[serial_test::serial]
    async fn rebuild_index_refreshes_every_provider_not_just_claude() {
        let claude_dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            claude_dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();
        let gemini_home = TempDir::new().unwrap();
        let _gemini_guard = EnvVarGuard::set("GEMINI_HOME", gemini_home.path());
        write_gemini_fixture_project(gemini_home.path(), "hash-1");

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        rebuild_index(&mut conn, &claude_base).await.unwrap();

        assert_eq!(
            claude_and_gemini_session_counts(&conn),
            (1, 1),
            "rebuild must re-ingest both providers' sessions with no duplicates"
        );
    }

    /// A provider whose `scan_stats_projects` finds zero projects (a
    /// valid, existing but empty override directory) must not disturb
    /// Claude's own ingestion in the same backfill call -- exercises the
    /// `.unwrap_or_default()` wrapping around `scan_stats_projects`.
    #[tokio::test]
    #[serial_test::serial]
    async fn an_empty_providers_scan_does_not_prevent_claudes_own_data_from_landing() {
        let claude_dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            claude_dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let (claude_sessions, _) = claude_and_gemini_session_counts(&conn);
        assert_eq!(claude_sessions, 1, "Claude's session must still land");
    }

    /// The interactive first-run index needs per-phase progress.
    /// `on_phase_start` must fire once per phase (1 for Claude + one per
    /// `FILE_BASED_STATS_PROVIDERS` entry), in order, with a correctly
    /// incrementing `phases_done`/stable `phases_total`.
    #[tokio::test]
    #[serial_test::serial]
    async fn run_full_backfill_with_hooks_reports_one_phase_per_provider_in_order() {
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        let seen: std::sync::Mutex<Vec<(String, usize, usize)>> = std::sync::Mutex::new(Vec::new());
        let on_phase_start = |phase: BackfillPhase| {
            seen.lock().unwrap().push((
                phase.provider_key.to_string(),
                phase.phases_done,
                phase.phases_total,
            ));
        };
        let hooks = BackfillProgressHooks {
            on_phase_start: Some(&on_phase_start),
            should_cancel: None,
        };

        run_full_backfill_with_hooks(&mut conn, &claude_base, &hooks)
            .await
            .unwrap();

        let seen = seen.into_inner().unwrap();
        let expected_total = 1 + FILE_BASED_STATS_PROVIDERS.len();
        assert_eq!(
            seen.len(),
            expected_total,
            "must fire exactly one phase-start per provider (claude + every file-based provider)"
        );
        assert_eq!(seen[0].0, "claude", "claude must always be the first phase");
        for (i, (_, phases_done, phases_total)) in seen.iter().enumerate() {
            assert_eq!(*phases_done, i, "phases_done must count up from 0");
            assert_eq!(*phases_total, expected_total, "phases_total must stay constant");
        }
    }

    /// Cancelling between phases must stop before any further provider is
    /// touched, while leaving already-ingested data intact and reporting
    /// `cancelled: true` -- never a silent partial result indistinguishable
    /// from a complete one.
    #[tokio::test]
    #[serial_test::serial]
    async fn run_full_backfill_with_hooks_stops_at_the_next_phase_boundary_when_cancelled() {
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            &format!("{TOOL_USE_LINE}\n{ERROR_RESULT_LINE}\n"),
        );
        let _codex_guard = empty_codex_home_guard();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        // Cancel immediately -- before even the first file-based provider
        // phase starts. Claude's own phase has no cancellation checkpoint
        // (it's the first phase and always runs), so its data must still
        // land; every file-based provider phase must be skipped.
        let should_cancel = || true;
        let hooks = BackfillProgressHooks {
            on_phase_start: None,
            should_cancel: Some(&should_cancel),
        };

        let summary = run_full_backfill_with_hooks(&mut conn, &claude_base, &hooks)
            .await
            .unwrap();

        assert!(summary.cancelled, "summary must report the cancellation");
        let (claude_sessions, _) = claude_and_gemini_session_counts(&conn);
        assert_eq!(
            claude_sessions, 1,
            "claude's phase already completed before the first cancellation check and must not be rolled back"
        );
    }

    /// "First index duration" at representative scale -- a genuine
    /// one-shot wall-clock cost, not a hot loop worth criterion's
    /// repeated-sampling model. `#[ignore]`d so normal `cargo test` stays
    /// fast; run explicitly with `cargo test --release -- --ignored
    /// first_index_duration_at_scale --nocapture` to see the real numbers
    /// `run_full_backfill`'s own `BackfillSummary::duration_ms` already
    /// computes (no new instrumentation needed).
    ///
    /// Was capped at 1,000 sessions during investigation -- `run_full_
    /// backfill` goes through `ingest_claude_project` ->
    /// `load_project_sessions` -> `superseded_chain_paths`, which had a
    /// real, now-fixed O(n²) bug (`resolve_session_chain` recomputing a
    /// full project directory snapshot per file; see
    /// `chain.rs::resolve_session_chain_with_snapshot`'s own doc comment
    /// and `load.rs`'s `load_project_sessions_stays_near_linear_at_scale`
    /// regression test). Restored to 10k/50k now that it's fixed.
    ///
    /// `home_override_guard` isolates every file-based provider's own
    /// home-relative scan root at once, not just Codex -- unlike the
    /// other tests in this file, this one doesn't also need
    /// `empty_codex_home_guard` on top of it.
    #[tokio::test]
    #[serial_test::serial]
    #[ignore = "slow -- run explicitly to measure at scale"]
    async fn first_index_duration_at_scale() {
        for session_count in [10_000usize, 50_000] {
            let home = TempDir::new().unwrap();
            let _home_guard = crate::utils::test_support::home_override_guard(home.path());
            let claude_base = home.path().join(".claude");
            let project_dir = claude_base.join("projects").join("bench-project");
            fs::create_dir_all(&project_dir).unwrap();
            for i in 0..session_count {
                let line = format!(
                    r#"{{"uuid":"u{i}","sessionId":"s{i}","timestamp":"2026-01-01T00:00:00Z","type":"user","message":{{"role":"user","content":"message number {i}"}}}}"#
                );
                fs::write(project_dir.join(format!("session_{i}.jsonl")), format!("{line}\n"))
                    .unwrap();
            }

            let mut conn = Connection::open_in_memory().unwrap();
            migrate(&mut conn).unwrap();

            let summary = run_full_backfill(&mut conn, &claude_base.to_string_lossy())
                .await
                .unwrap();
            println!(
                "[first-index-duration] {session_count} sessions: {}ms ({} ingested, {} skipped)",
                summary.duration_ms, summary.sessions_ingested, summary.sessions_skipped_unchanged
            );
        }
    }
}
