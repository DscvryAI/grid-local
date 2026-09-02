//! DTOs for `archive_db::insights` (spec §9/§10/§17).
//!
//! Every `session_id` field here is a `file_path`, matching
//! `HistorySessionItem`'s existing "unique ID based on file path"
//! convention -- the same field the existing open-session flow already
//! keys off, so a card's drill-down can reuse it unchanged.
//!
//! Wire format is plain `snake_case` (no `rename_all = "camelCase"`),
//! matching `models::history`'s own deliberate precedent -- these are
//! new DTOs with no legacy-naming pressure, so the field-for-field
//! mirror with zero translation layer is the simpler choice.
//!
//! **Home surface (spec §9): the v1 gap above is now filled in.**
//! `SinceLastVisitSummary`/`ThisWeekSummary` now carry
//! `tool_call_count`/`agent_run_count` (and `ThisWeekSummary` also
//! `peak_agents_in_session`/`provider_breakdown`), matching spec §9's
//! mockup ("17 agent runs", "382 tool calls", "Claude Code 71% / Codex
//! 24% / Cursor 5%"). The original blocker for provider-mix specifically
//! -- "`archive_db` is Claude-only today" -- no longer holds: `archive_db`
//! is now genuinely multi-provider, live-verified against real
//! Codex/Antigravity/Copilot data. `agent_run`'s sparsity is real but
//! handled at the UI layer instead of by omitting the field: the frontend
//! only renders an agent-run figure when the count is nonzero, the same
//! "render nothing rather than a misleading zero" convention these
//! sections already use for an empty summary as a whole.

use serde::{Deserialize, Serialize};

/// Whether a "worth looking at" card's own occurrences are trending up,
/// down, holding steady, or too recent to say. Computed by comparing occurrence
/// counts in the last 7 days against the 7 days before that -- a fixed
/// lookback independent of the card's own `min_failures`/`window_start`
/// filter, since trend is about recent trajectory, not about which cards
/// qualify to be shown at all.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProblemTrend {
    /// First ever occurrence falls within the last 7 days.
    New,
    Increasing,
    Decreasing,
    #[default]
    Steady,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RepeatedCommandFailureCard {
    /// The most recent RAW occurrence's own text -- a real, concrete
    /// example for display, not the abstract `template`.
    pub shell_command: String,
    /// The normalized grouping key (see `archive_db::insights::
    /// normalize_command_template`) -- e.g. `pytest tests/test_foo.py`
    /// and `pytest tests/test_bar.py` both normalize to `pytest <PATH>`
    /// and are counted as the SAME repeated failure. Exposed so the
    /// frontend's dismiss action has a stable key across different exact
    /// commands that are really "the same" failure.
    pub template: String,
    pub failure_count: usize,
    pub session_count: usize,
    pub first_occurred_at: String,
    pub last_occurred_at: String,
    pub sample_session_id: String,
    pub trend: ProblemTrend,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RepeatedErrorCard {
    pub error_signature: String,
    pub occurrence_count: usize,
    pub session_count: usize,
    pub first_occurred_at: String,
    pub last_occurred_at: String,
    pub sample_session_id: String,
    pub trend: ProblemTrend,
}

/// One real occurrence of a `RepeatedErrorCard`'s own `error_signature`.
/// Deliberately narrower than "surrounding commands" would imply --
/// correlating nearby `command` rows by timestamp per occurrence would
/// need a separate enhancement not attempted here; every field here is a
/// real, already-tracked fact, never fabricated.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ErrorOccurrence {
    pub session_id: String,
    pub project_name: String,
    pub occurred_at: Option<String>,
}

/// A real, deterministic candidate for "this same error was followed by
/// a passing verification elsewhere". Every field
/// is a real timestamp/command already in the data -- this deliberately
/// does NOT claim the later verification proves the error was actually
/// fixed by anything related; it's evidence, not a conclusion. See
/// [`crate::archive_db::insights::similar_error_resolutions`]'s own doc
/// comment for the exact matching rule and its honest limits.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SimilarErrorResolution {
    pub session_id: String,
    pub project_name: String,
    pub error_occurred_at: Option<String>,
    pub verification_occurred_at: String,
    pub verification_command: String,
}

/// This user's own historical average total tokens and session duration
/// for one provider. Deliberately scoped
/// per-provider, never global -- different providers report token usage
/// on incomparable scales, so a global average would produce a
/// misleading ratio. `session_count` is the real sample size behind the
/// averages; callers must require a minimum before treating a ratio as
/// meaningful (an average of 1-2 sessions isn't a "personal baseline").
/// See [`crate::archive_db::insights::personal_baseline`]'s own doc
/// comment for the exact query and its exclude-current-session logic.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PersonalBaseline {
    pub average_total_tokens: f64,
    pub average_duration_minutes: f64,
    pub session_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LargeAgentRunCard {
    pub session_id: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
    pub subagent_count: usize,
    /// The owning session's `first_message_time` -- NOT any single
    /// `agent_run.started_at` (which CAN be populated now, see
    /// `archive_db::ingest::claude::ingest_subagent_tree`): this card
    /// counts potentially many agent runs per
    /// session, so a session-level timestamp represents the group where a
    /// single run's own timing wouldn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HighTokenSessionCard {
    pub session_id: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_time: Option<String>,
}

/// Normalizes errors, commands, and validation outcomes into a
/// deterministic insight type -- e.g. "Four files changed after the last
/// passing test." Deterministic per the standing no-AI-generation
/// policy: `last_verified_command`/`last_verified_at` are the real last
/// test/build command this session ran that matched
/// `archive_db::insights::VERIFICATION_COMMAND_PATTERN`, and
/// `files_changed_since` is a real count of `file_event` rows with a
/// later timestamp -- never a guessed or generated claim. Mirrors the
/// frontend's own per-session `deriveVerificationStatus`'s "stale" state
/// (`sessionIntelligence.ts`) at the cross-session/archive level instead
/// of requiring a session to already be open.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VerificationGapCard {
    pub session_id: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_summary: Option<String>,
    pub files_changed_since: usize,
    pub last_verified_command: String,
    pub last_verified_at: String,
}

/// One discriminated union so the frontend gets a single ranked list
/// instead of five it has to interleave itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data")]
pub enum InsightCard {
    RepeatedCommandFailure(RepeatedCommandFailureCard),
    RepeatedError(RepeatedErrorCard),
    LargeAgentRun(LargeAgentRunCard),
    HighTokenSession(HighTokenSessionCard),
    VerificationGap(VerificationGapCard),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SinceLastVisitSummary {
    pub since: String,
    pub session_count: usize,
    pub message_count: usize,
    pub total_tokens: i64,
    pub error_count: usize,
    pub project_count: usize,
    pub tool_call_count: usize,
    pub agent_run_count: usize,
    /// Up to 3 project display names, most-recently-active first (spec
    /// §9.1's "Primary projects" list). Not every project active in the
    /// window -- see `archive_db::insights::primary_projects_since`.
    pub primary_projects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ThisWeekSummary {
    pub window_start: String,
    pub window_end: String,
    pub session_count: usize,
    pub message_count: usize,
    pub total_tokens: i64,
    pub error_count: usize,
    pub project_count: usize,
    pub tool_call_count: usize,
    pub agent_run_count: usize,
    /// The most agent runs any single session in the window had (spec
    /// §9.2's "Peak: 31 agents in one session") -- 0 when no session in
    /// the window has any agent runs.
    pub peak_agents_in_session: usize,
    /// Token share per provider active in the window, sorted by
    /// `total_tokens` descending, providers with zero tokens omitted.
    /// The frontend derives each provider's percentage from
    /// `total_tokens / summary.total_tokens` rather than the backend
    /// pre-computing a percentage -- one fewer rounding-convention to
    /// keep in sync between two languages.
    pub provider_breakdown: Vec<ProviderTokenShare>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ProviderTokenShare {
    pub provider_key: String,
    pub display_name: String,
    pub total_tokens: i64,
}

/// Shared drill-down row (e.g. clicking "This week"'s session count) --
/// deliberately not `HistorySessionItem`: that DTO carries History-surface
/// fields (`has_tool_use`/`has_errors`/`model`/etc.) this drill-down list
/// doesn't need, and every field here comes straight off `session`, no
/// join-heavy per-provider dispatch the way History's does.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionListItem {
    pub session_id: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_time: Option<String>,
    pub total_tokens: i64,
    pub message_count: usize,
}

/// One node in an agent-run tree. `child_session_id` is the CHILD
/// session's `file_path` when Grid can resolve it -- real for any Claude
/// subagent correlated via its own `.meta.json`
/// (`archive_db::ingest::claude::ingest_subagent_tree`), `None` for a
/// subagent transcript with no correlatable key (an older
/// session predating `.meta.json`) or a run that never launched a
/// subagent at all. `children` can now be genuinely multi-level -- see
/// `archive_db::insights::get_agent_run_tree`'s recursive query.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AgentRunNode {
    pub agent_run_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub tool_call_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    pub children: Vec<AgentRunNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AgentRunTree {
    pub session_id: String,
    pub roots: Vec<AgentRunNode>,
    pub total_count: usize,
}

/// One tool's usage count within a single agent run's own child session --
/// `AgentRunDetail.tools_used`'s element type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunToolUsage {
    pub tool_name: String,
    pub count: usize,
}

/// A single agent run's full detail (F-06's own explicit ask: "show
/// purpose, duration, model, tokens, tools, files, errors, and source
/// transcript on selection").
/// `purpose`/`model`/`total_tokens`/`tools_used`/`files_touched`/`errors`
/// are only ever populated when `child_session_id` is `Some` (a real,
/// correlated subagent transcript exists to source them from) -- `None`/
/// empty otherwise, never fabricated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRunDetail {
    pub agent_run_id: i64,
    /// The PARENT session's own `file_path` (unchanged meaning from
    /// before this DTO was extended).
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub tool_call_count: usize,
    /// The launch instructions/description from the parent's own `Agent`
    /// `tool_use` input -- Claude Code's Task-tool schema's `prompt` field
    /// when present, else its shorter `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The child (subagent) session's own `file_path` -- the "source
    /// transcript" link F-06 asks for. `None` when unlinked (see struct
    /// doc comment).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i64>,
    #[serde(default)]
    pub tools_used: Vec<AgentRunToolUsage>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    #[serde(default)]
    pub error_count: usize,
    /// Bounded preview (first few), not every error -- a detail panel, not
    /// a full report.
    #[serde(default)]
    pub errors: Vec<String>,
}
