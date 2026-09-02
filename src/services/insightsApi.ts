/**
 * Insights API Service
 *
 * Thin wrappers around the `archive_db::insights` Tauri commands backing
 * the Insights surface's Problems/Agents tabs (spec §10/§17) and the
 * Home surface (spec §9).
 */

import { api } from "@/services/api";
import type {
  AgentRunDetail,
  AgentRunTree,
  ErrorOccurrence,
  InsightCard,
  LargeAgentRunCard,
  PersonalBaseline,
  RepeatedCommandFailureCard,
  RepeatedErrorCard,
  SessionListItem,
  SimilarErrorResolution,
  SinceLastVisitSummary,
  ThisWeekSummary,
} from "../types";

export async function fetchRepeatedCommandFailures(
  projectKey?: string,
  minFailures = 2,
  limit = 20
): Promise<RepeatedCommandFailureCard[]> {
  return api<RepeatedCommandFailureCard[]>("get_repeated_command_failures", {
    windowStart: undefined,
    projectKey,
    minFailures,
    limit,
  });
}

export async function fetchRepeatedErrors(
  projectKey?: string,
  minSessions = 2,
  limit = 20
): Promise<RepeatedErrorCard[]> {
  return api<RepeatedErrorCard[]>("get_repeated_errors", {
    windowStart: undefined,
    projectKey,
    minSessions,
    limit,
  });
}

/** Selecting an error shows its occurrences. */
export async function fetchErrorOccurrences(
  errorSignature: string,
  projectKey?: string,
  limit = 20
): Promise<ErrorOccurrence[]> {
  return api<ErrorOccurrence[]>("get_error_occurrences", {
    errorSignature,
    projectKey,
    limit,
  });
}

/** Cross-session reusable-solution retrieval. On-demand only -- never
 * call this eagerly for every error card, only when a user asks.
 * `excludeProjectKey`, when set,
 * omits that project's own occurrences so results are genuinely
 * evidence from another project. */
export async function fetchSimilarErrorResolutions(
  errorSignature: string,
  excludeProjectKey?: string,
  limit = 5
): Promise<SimilarErrorResolution[]> {
  return api<SimilarErrorResolution[]>("get_similar_error_resolutions", {
    errorSignature,
    excludeProjectKey,
    limit,
  });
}

/** Personal-baseline anomaly explanations. `excludeSessionId` should be
 * the currently-open session's own file path so it doesn't skew the
 * baseline it's being measured against. */
export async function fetchPersonalBaseline(
  providerKey: string,
  excludeSessionId?: string
): Promise<PersonalBaseline> {
  return api<PersonalBaseline>("get_personal_baseline", {
    providerKey,
    excludeSessionId,
  });
}

/** Marks a repeated-command-failure or repeated-error card as handled
 * so it stops resurfacing. `kind` is
 * `"command_failure"` or `"error"`; `signature` is the card's own
 * `template`/`error_signature`. */
export async function dismissProblem(
  kind: "command_failure" | "error",
  signature: string
): Promise<void> {
  return api<void>("dismiss_problem", { kind, signature });
}

export async function fetchLargeAgentRuns(
  projectKey?: string,
  minSubagents = 2,
  limit = 20
): Promise<LargeAgentRunCard[]> {
  return api<LargeAgentRunCard[]>("get_large_agent_runs", {
    windowStart: undefined,
    projectKey,
    minSubagents,
    limit,
  });
}

export async function fetchAgentRunTree(sessionId: string): Promise<AgentRunTree> {
  return api<AgentRunTree>("get_agent_run_tree", { sessionId });
}

/**
 * Full detail for one agent-run node in the agent-run topology: purpose,
 * model, tokens, tools, files, errors, and the correlated child
 * transcript's `session_id`, when one exists.
 */
export async function fetchAgentRunDetail(agentRunId: number): Promise<AgentRunDetail> {
  return api<AgentRunDetail>("get_agent_run_detail", { agentRunId });
}

// ============================================================================
// Home surface (spec §9)
// ============================================================================

/** `null` on a fresh install where no visit has ever been recorded. */
export async function fetchSinceLastVisitSummary(): Promise<SinceLastVisitSummary | null> {
  return api<SinceLastVisitSummary | null>("get_since_last_visit_summary");
}

/**
 * Marks "now" as the last-visited timestamp. Call AFTER
 * `fetchSinceLastVisitSummary` (e.g. once Home has rendered), never
 * before -- see the backend command's own doc for why.
 */
export async function recordVisit(): Promise<void> {
  return api<void>("record_visit");
}

export async function fetchThisWeekSummary(
  windowStart: string,
  windowEnd: string
): Promise<ThisWeekSummary> {
  return api<ThisWeekSummary>("get_this_week_summary", {
    windowStart,
    windowEnd,
  });
}

export async function fetchSessionsInWindow(
  start: string,
  end: string,
  limit = 50
): Promise<SessionListItem[]> {
  return api<SessionListItem[]>("list_sessions_in_window", { start, end, limit });
}

/**
 * `projectKey` is accepted for completeness (mirrors the other Problems/
 * Agents fetchers) but Home's own caller deliberately never passes it --
 * "Things worth looking at" stays global there, matching spec §10.
 */
export async function fetchThingsWorthLookingAt(projectKey?: string): Promise<InsightCard[]> {
  return api<InsightCard[]>("get_things_worth_looking_at", {
    windowStart: undefined,
    projectKey,
  });
}
