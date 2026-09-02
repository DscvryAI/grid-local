//! Tauri commands exposing Grid's own `archive_db::insights` aggregations
//! to the frontend. No UI consumes these yet. Mirrors `commands::archive_db`'s
//! open-connection-then-delegate pattern exactly: no persistent
//! connection held in app state.

use crate::archive_db;
use crate::models::{
    AgentRunDetail, AgentRunTree, ErrorOccurrence, HighTokenSessionCard, InsightCard,
    LargeAgentRunCard, PersonalBaseline, RepeatedCommandFailureCard, RepeatedErrorCard,
    SessionListItem, SimilarErrorResolution, SinceLastVisitSummary, ThisWeekSummary,
};

/// `provider_key` is threaded through to `archive_db::insights` but
/// deliberately NOT exposed as a parameter here yet -- always passed
/// `None`. A future Insights "Provider" filter chip can wire it in once
/// there's a real signal that cross-provider mixing is noisy in
/// practice; building that UI speculatively ahead of the signal isn't
/// warranted yet.
#[tauri::command]
pub async fn get_repeated_command_failures(
    window_start: Option<String>,
    project_key: Option<String>,
    min_failures: usize,
    limit: usize,
) -> Result<Vec<RepeatedCommandFailureCard>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::repeated_command_failures(
        &conn,
        window_start.as_deref(),
        project_key.as_deref(),
        None,
        min_failures,
        limit,
    )
}

#[tauri::command]
pub async fn get_repeated_errors(
    window_start: Option<String>,
    project_key: Option<String>,
    min_sessions: usize,
    limit: usize,
) -> Result<Vec<RepeatedErrorCard>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::repeated_errors(
        &conn,
        window_start.as_deref(),
        project_key.as_deref(),
        None,
        min_sessions,
        limit,
    )
}

/// The real, unaggregated list of sessions where one specific
/// already-surfaced `RepeatedErrorCard.error_signature` occurred.
#[tauri::command]
pub async fn get_error_occurrences(
    error_signature: String,
    project_key: Option<String>,
    limit: usize,
) -> Result<Vec<ErrorOccurrence>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::error_occurrences(
        &conn,
        &error_signature,
        project_key.as_deref(),
        None,
        limit,
    )
}

/// On-demand, user-triggered lookup for whether this same
/// `error_signature` was later followed by a passing verification
/// command, optionally excluding the project currently being viewed so
/// results are genuinely "another project"'s evidence. See
/// [`archive_db::insights::similar_error_resolutions`]'s own doc comment
/// for the exact matching rule and its honest limits (exact-string
/// signature matching, evidentiary not causal).
#[tauri::command]
pub async fn get_similar_error_resolutions(
    error_signature: String,
    exclude_project_key: Option<String>,
    limit: usize,
) -> Result<Vec<SimilarErrorResolution>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::similar_error_resolutions(
        &conn,
        &error_signature,
        exclude_project_key.as_deref(),
        limit,
    )
}

/// This user's own historical average tokens/duration for one provider,
/// so a session view can show "Nx your normal usage"
/// instead of a raw, context-free total. See
/// [`archive_db::insights::personal_baseline`]'s own doc comment for why
/// this is scoped per-provider and how `exclude_session_id` avoids the
/// currently-open session skewing its own baseline.
#[tauri::command]
pub async fn get_personal_baseline(
    provider_key: String,
    exclude_session_id: Option<String>,
) -> Result<PersonalBaseline, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::personal_baseline(&conn, &provider_key, exclude_session_id.as_deref())
}

/// Marks a repeated-command-failure or repeated-error card as handled --
/// `kind` is `"command_failure"` or `"error"`, `signature` is the card's
/// own `template`/`error_signature`. Local-only, no sync.
#[tauri::command]
pub async fn dismiss_problem(kind: String, signature: String) -> Result<(), String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::dismiss_problem(&conn, &kind, &signature)
}

#[tauri::command]
pub async fn get_large_agent_runs(
    window_start: Option<String>,
    project_key: Option<String>,
    min_subagents: usize,
    limit: usize,
) -> Result<Vec<LargeAgentRunCard>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::large_agent_runs(
        &conn,
        window_start.as_deref(),
        project_key.as_deref(),
        None,
        min_subagents,
        limit,
    )
}

#[tauri::command]
pub async fn get_high_token_sessions(
    window_start: Option<String>,
    project_key: Option<String>,
    limit: usize,
) -> Result<Vec<HighTokenSessionCard>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::high_token_sessions(
        &conn,
        window_start.as_deref(),
        project_key.as_deref(),
        None,
        limit,
    )
}

#[tauri::command]
pub async fn get_things_worth_looking_at(
    window_start: Option<String>,
    project_key: Option<String>,
) -> Result<Vec<InsightCard>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::things_worth_looking_at(
        &conn,
        window_start.as_deref(),
        project_key.as_deref(),
        None,
    )
}

/// Reads the stored `last_visit_at` and summarizes activity since then.
/// `None` on a fresh install (no visit ever recorded) -- callers should
/// treat that as "nothing to summarize yet," not an error. Deliberately
/// does NOT also call `record_visit` -- the frontend controls exactly
/// when a visit is marked (see `record_visit`'s own doc).
#[tauri::command]
pub async fn get_since_last_visit_summary() -> Result<Option<SinceLastVisitSummary>, String> {
    let conn = archive_db::open_connection()?;
    let Some(since) = archive_db::insights::get_last_visit_at(&conn)? else {
        return Ok(None);
    };
    archive_db::insights::since_last_visit_summary(&conn, &since).map(Some)
}

/// Marks "now" as the last-visited timestamp. Call this AFTER reading
/// `get_since_last_visit_summary` (e.g. once Home has rendered), never
/// before -- calling first would overwrite the boundary that summary
/// needs to compare against.
#[tauri::command]
pub async fn record_visit() -> Result<(), String> {
    let conn = archive_db::open_connection()?;
    let now = chrono::Utc::now().to_rfc3339();
    archive_db::insights::record_visit(&conn, &now)
}

#[tauri::command]
pub async fn get_this_week_summary(
    window_start: String,
    window_end: String,
) -> Result<ThisWeekSummary, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::this_week_summary(&conn, &window_start, &window_end)
}

#[tauri::command]
pub async fn list_sessions_in_window(
    start: String,
    end: String,
    limit: usize,
) -> Result<Vec<SessionListItem>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::list_sessions_in_window(&conn, &start, &end, limit)
}

#[tauri::command]
pub async fn get_agent_run_tree(session_id: String) -> Result<AgentRunTree, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::get_agent_run_tree(&conn, &session_id)
}

#[tauri::command]
pub async fn get_agent_run_detail(agent_run_id: i64) -> Result<AgentRunDetail, String> {
    let conn = archive_db::open_connection()?;
    archive_db::insights::get_agent_run_detail(&conn, agent_run_id)
}
