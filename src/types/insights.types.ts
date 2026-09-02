/**
 * Insights Types
 *
 * DTOs for `archive_db::insights` (spec §9/§10/§17). Mirror
 * `src-tauri/src/models/insights.rs` field-for-field -- plain
 * snake_case, no camelCase translation layer, matching
 * `history.types.ts`'s own established convention.
 */

/** Mirrors `src-tauri/src/models/insights.rs::ProblemTrend`, used across
 * the "things worth looking at" lifecycle. */
export type ProblemTrend = "new" | "increasing" | "decreasing" | "steady";

export interface RepeatedCommandFailureCard {
  /** The most recent RAW occurrence's own text -- for display. */
  shell_command: string;
  /** The normalized grouping key -- pass this to `dismissProblem`. */
  template: string;
  failure_count: number;
  session_count: number;
  first_occurred_at: string;
  last_occurred_at: string;
  sample_session_id: string;
  trend: ProblemTrend;
}

export interface RepeatedErrorCard {
  error_signature: string;
  occurrence_count: number;
  session_count: number;
  first_occurred_at: string;
  last_occurred_at: string;
  sample_session_id: string;
  trend: ProblemTrend;
}

/** One real occurrence of a `RepeatedErrorCard.error_signature`. */
export interface ErrorOccurrence {
  session_id: string;
  project_name: string;
  occurred_at?: string;
}

/** A real, deterministic candidate for "this same error was later
 * followed by a passing verification, possibly in a different project"
 * (cross-session reusable-solution retrieval). Evidentiary, not causal --
 * render as "later followed by a passing verification," never as
 * "resolved" or "the fix." See the backend's own doc comment
 * (`archive_db::insights::similar_error_resolutions`) for the exact
 * matching rule and its honest limits. */
export interface SimilarErrorResolution {
  session_id: string;
  project_name: string;
  error_occurred_at?: string;
  verification_occurred_at: string;
  verification_command: string;
}

/** This user's own historical average tokens/duration for one provider
 * (used for personal-baseline anomaly explanations). Scoped per-provider
 * -- never compare across providers, their token reporting isn't on the
 * same scale. */
export interface PersonalBaseline {
  average_total_tokens: number;
  average_duration_minutes: number;
  session_count: number;
}

export interface LargeAgentRunCard {
  session_id: string;
  project_name: string;
  session_summary?: string;
  subagent_count: number;
  /** The owning session's `first_message_time` -- see the backend's own
   * doc comment for why `agent_run.started_at` isn't used instead. */
  session_started_at?: string;
}

/**
 * One node in an agent-run tree (spec §17). `subagent_type`/`started_at`/
 * `ended_at`/`child_session_id`/`children` are populated whenever the
 * launched subagent's own transcript can be found and correlated (see
 * `archive_db::ingest::claude::ingest_subagent_tree`); they stay
 * `undefined`/empty only for an unlinked launch (no matching `.meta.json`/
 * `agentId`, or an older session predating that correlation key) or a
 * subagent that itself launched no further agents -- see
 * `archive_db::insights`'s own module doc for the full picture.
 */
export interface AgentRunNode {
  agent_run_id: number;
  subagent_type?: string;
  status?: string;
  started_at?: string;
  ended_at?: string;
  tool_call_count: number;
  child_session_id?: string;
  children: AgentRunNode[];
}

export interface AgentRunTree {
  session_id: string;
  roots: AgentRunNode[];
  total_count: number;
}

/** One tool's usage count within a single agent run's own child session. */
export interface AgentRunToolUsage {
  tool_name: string;
  count: number;
}

/**
 * A single agent run's full detail: purpose, duration, model, tokens,
 * tools, files, errors, and source transcript on selection. `purpose`/
 * `model`/`total_tokens`/`tools_used`/`files_touched`/`errors` are only
 * ever populated when `child_session_id` is set -- `undefined`/empty
 * otherwise, never fabricated.
 */
export interface AgentRunDetail {
  agent_run_id: number;
  /** The PARENT session's own `file_path`. */
  session_id: string;
  subagent_type?: string;
  status?: string;
  started_at?: string;
  ended_at?: string;
  tool_call_count: number;
  /** The launch prompt/description from the parent's own `Agent` tool_use. */
  purpose?: string;
  /** The child (subagent) session's own `file_path` -- the source transcript link. */
  child_session_id?: string;
  model?: string;
  total_tokens?: number;
  tools_used: AgentRunToolUsage[];
  files_touched: string[];
  error_count: number;
  /** Bounded preview (first few), not every error. */
  errors: string[];
}

export interface HighTokenSessionCard {
  session_id: string;
  project_name: string;
  session_summary?: string;
  total_tokens: number;
  last_message_time?: string;
}

/**
 * Normalizes errors, commands, and validation outcomes into deterministic
 * insight types -- the "Verification gap" row. `files_changed_since`/
 * `last_verified_*` are real, deterministic facts from
 * `archive_db::insights::verification_gaps` -- never AI-generated (a
 * standing product decision).
 */
export interface VerificationGapCard {
  session_id: string;
  project_name: string;
  session_summary?: string;
  files_changed_since: number;
  last_verified_command: string;
  last_verified_at: string;
}

/**
 * Merged, ranked view over the five "things worth looking at" card kinds
 * (spec §10) -- one discriminated union instead of interleaving five
 * separately-fetched lists on the frontend. Mirrors the Rust
 * `#[serde(tag = "kind", content = "data")]` enum exactly.
 */
export type InsightCard =
  | { kind: "RepeatedCommandFailure"; data: RepeatedCommandFailureCard }
  | { kind: "RepeatedError"; data: RepeatedErrorCard }
  | { kind: "LargeAgentRun"; data: LargeAgentRunCard }
  | { kind: "HighTokenSession"; data: HighTokenSessionCard }
  | { kind: "VerificationGap"; data: VerificationGapCard };

/**
 * Shared drill-down row (e.g. clicking Home's "N sessions" count) --
 * deliberately not `ClaudeSession`/`HistorySessionItem`: this is a
 * lighter DTO with only what a drill-down list needs to render and open a
 * session, not History's full filterable-surface shape.
 */
export interface SessionListItem {
  session_id: string;
  project_name: string;
  summary?: string;
  last_message_time?: string;
  total_tokens: number;
  message_count: number;
}

/** Home surface (spec §9.1). */
export interface SinceLastVisitSummary {
  since: string;
  session_count: number;
  message_count: number;
  total_tokens: number;
  error_count: number;
  project_count: number;
  tool_call_count: number;
  agent_run_count: number;
  primary_projects: string[];
}

/** Home surface (spec §9.2). */
export interface ThisWeekSummary {
  window_start: string;
  window_end: string;
  session_count: number;
  message_count: number;
  total_tokens: number;
  error_count: number;
  project_count: number;
  tool_call_count: number;
  agent_run_count: number;
  /** Most agent runs any single session in the window had ("Peak: 31 agents in one session"). 0 if none. */
  peak_agents_in_session: number;
  /** Token share per provider active in the window, most-tokens-first. Compute each percentage from `total_tokens / summary.total_tokens`. */
  provider_breakdown: ProviderTokenShare[];
}

export interface ProviderTokenShare {
  provider_key: string;
  display_name: string;
  total_tokens: number;
}
