/**
 * Session Intelligence Calculations
 *
 * Client-side stats for the session view's intelligence header (spec §14):
 * token total, tool-call count, distinct files touched, agent-task count.
 * Computed directly from the already-loaded `ClaudeMessage[]` for the open
 * session -- no new backend command needed.
 */

import type { ClaudeMessage, ContentItem, ToolUseContent } from "../../../types";
import { extractTextContent, extractToolResultContent } from "@/utils/contentTypeGuards";

// ============================================================================
// Token total (dedup by messageId, mirroring the backend)
// ============================================================================

/**
 * Same identity rule as the backend's `dedup_usage_key` (#283,
 * `commands/stats.rs`): one assistant turn can produce multiple JSONL rows
 * sharing a `messageId`, and each repeats the SAME usage totals -- summing
 * every row would overstate tokens. Key by `messageId` when present,
 * falling back to the row's own `uuid` (always unique) when it isn't.
 */
function dedupUsageKey(messageId: string | undefined, uuid: string): string {
  return messageId ? `m:${messageId}` : `u:${uuid}`;
}

interface MessageUsage {
  input_tokens?: number;
  output_tokens?: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
  reasoning_tokens?: number;
}

/** Sums every usage field the backend's `token_usage_totals` also sums. */
function usageTotal(usage: MessageUsage): number {
  return (
    (usage.input_tokens ?? 0) +
    (usage.output_tokens ?? 0) +
    (usage.cache_creation_input_tokens ?? 0) +
    (usage.cache_read_input_tokens ?? 0) +
    (usage.reasoning_tokens ?? 0)
  );
}

export interface TokenBreakdown {
  input: number;
  output: number;
  cacheCreation: number;
  cacheRead: number;
  reasoning: number;
}

/** Matches `AnalyticsDashboard`'s `ModelUsageLike` shape (`globalCalculations.ts`) exactly, so `calculateGlobalCostSummary` can be reused without a translation layer. */
export interface ModelUsage {
  model_name: string;
  token_count: number;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
  reasoning_tokens: number;
}

/**
 * Per-category token totals plus a per-model breakdown, both deduped by
 * `messageId`/`uuid` the same way `calculateTokenTotal` is (a repeated
 * JSONL row for one assistant turn must not be double-counted). Feeds the
 * Overview tab's usage section (formerly the standalone "Token Stats" nav
 * view, folded in here to match the spec's exact 5-tab Session View shape).
 */
export function calculateTokenBreakdown(messages: ClaudeMessage[]): {
  breakdown: TokenBreakdown;
  modelDistribution: ModelUsage[];
} {
  const seen = new Set<string>();
  const breakdown: TokenBreakdown = {
    input: 0,
    output: 0,
    cacheCreation: 0,
    cacheRead: 0,
    reasoning: 0,
  };
  const byModel = new Map<string, ModelUsage>();

  for (const message of messages) {
    if (message.type !== "assistant" || !message.usage) continue;
    const key = dedupUsageKey(message.messageId, message.uuid);
    if (seen.has(key)) continue;
    seen.add(key);

    const usage = message.usage;
    const input = usage.input_tokens ?? 0;
    const output = usage.output_tokens ?? 0;
    const cacheCreation = usage.cache_creation_input_tokens ?? 0;
    const cacheRead = usage.cache_read_input_tokens ?? 0;
    const reasoning = usage.reasoning_tokens ?? 0;

    breakdown.input += input;
    breakdown.output += output;
    breakdown.cacheCreation += cacheCreation;
    breakdown.cacheRead += cacheRead;
    breakdown.reasoning += reasoning;

    const modelName = message.model ?? "unknown";
    const existing = byModel.get(modelName) ?? {
      model_name: modelName,
      token_count: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_creation_tokens: 0,
      cache_read_tokens: 0,
      reasoning_tokens: 0,
    };
    existing.token_count += input + output + cacheCreation + cacheRead + reasoning;
    existing.input_tokens += input;
    existing.output_tokens += output;
    existing.cache_creation_tokens += cacheCreation;
    existing.cache_read_tokens += cacheRead;
    existing.reasoning_tokens += reasoning;
    byModel.set(modelName, existing);
  }

  return { breakdown, modelDistribution: Array.from(byModel.values()) };
}

export function calculateTokenTotal(messages: ClaudeMessage[]): number {
  const seen = new Set<string>();
  let total = 0;
  for (const message of messages) {
    if (message.type !== "assistant" || !message.usage) continue;
    const key = dedupUsageKey(message.messageId, message.uuid);
    if (seen.has(key)) continue;
    seen.add(key);
    total += usageTotal(message.usage);
  }
  return total;
}

// ============================================================================
// Tool calls
// ============================================================================

export interface ToolUseOccurrence {
  message: ClaudeMessage;
  tool: ToolUseContent;
}

/**
 * Every `tool_use` block across the session, from BOTH the content array
 * (standard shape) and the legacy top-level `toolUse` field some
 * processed messages carry -- unlike `messageUtils.ts`'s `getToolUseBlock`
 * (which returns only the first match per message, fine for its own
 * single-block UI use), this collects ALL of them: a turn can contain
 * several parallel tool calls, and undercounting here would understate
 * the Tools tab and the header's tool-call stat.
 */
export function getAllToolUseBlocks(messages: ClaudeMessage[]): ToolUseOccurrence[] {
  const occurrences: ToolUseOccurrence[] = [];
  for (const message of messages) {
    if (Array.isArray(message.content)) {
      for (const item of message.content as ContentItem[]) {
        if (item.type === "tool_use") {
          occurrences.push({ message, tool: item });
        }
      }
      continue;
    }
    const legacyToolUse = (message as { toolUse?: Record<string, unknown> }).toolUse;
    if (legacyToolUse && typeof legacyToolUse.name === "string") {
      occurrences.push({
        message,
        tool: {
          type: "tool_use",
          id: typeof legacyToolUse.id === "string" ? legacyToolUse.id : "",
          name: legacyToolUse.name,
          input: (legacyToolUse.input as Record<string, unknown>) ?? {},
        },
      });
    }
  }
  return occurrences;
}

export interface ToolUsageSummary {
  name: string;
  count: number;
}

/** Tool-name leaderboard for the Tools tab, sorted by count descending. */
export function summarizeToolUsage(occurrences: ToolUseOccurrence[]): ToolUsageSummary[] {
  const counts = new Map<string, number>();
  for (const { tool } of occurrences) {
    counts.set(tool.name, (counts.get(tool.name) ?? 0) + 1);
  }
  return Array.from(counts.entries())
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

// ============================================================================
// Files touched
// ============================================================================

const FILE_WRITING_TOOLS = new Set(["Write", "Edit", "MultiEdit", "NotebookEdit", "apply_patch"]);

/** Matches a `*** Update/Add/Delete File: <path>` header line from Codex
 * CLI's `apply_patch` format, which can touch several files in one call. */
const APPLY_PATCH_FILE_HEADER = /^\*\*\* (?:Update|Add|Delete) File: (.+)$/gm;

function extractApplyPatchFilePaths(patch: string): string[] {
  return Array.from(patch.matchAll(APPLY_PATCH_FILE_HEADER), (m) => (m[1] ?? "").trim()).filter(
    (path) => path.length > 0
  );
}

/** One individual write-tool touch of a file -- the real, ordered change
 * lifecycle behind selecting a file, not just an aggregate count. */
export interface FileTouch {
  tool: string;
  timestamp: string;
}

export interface FileEvent {
  filePath: string;
  /** Every distinct tool that touched this file, in first-seen order. */
  tools: string[];
  count: number;
  lastTouched: string;
  /** Every individual touch, oldest first -- the full sequence behind
   * `count`/`tools`/`lastTouched` above. */
  touches: FileTouch[];
}

/**
 * Distinct files touched by a write-capable tool (`Write`/`Edit`/
 * `MultiEdit`/`NotebookEdit`, keyed on `input.file_path` -- the field name
 * every such tool's renderer already reads, e.g. `MultiEditToolRenderer.tsx`
 * -- plus Codex CLI's `apply_patch`, which keeps its native name and input
 * shape (a raw multi-file patch, no `file_path` field) so it can share the
 * dedicated diff-highlighted renderer; its file path(s) are parsed out of
 * the patch text instead. Same tool-name set the backend's
 * `has_file_changes` check uses.
 */
export function extractFileEvents(occurrences: ToolUseOccurrence[]): FileEvent[] {
  const events = new Map<string, FileEvent>();

  const recordTouch = (filePath: string, toolName: string, timestamp: string) => {
    const touch: FileTouch = { tool: toolName, timestamp };
    const existing = events.get(filePath);
    if (existing) {
      existing.count += 1;
      if (!existing.tools.includes(toolName)) existing.tools.push(toolName);
      if (timestamp > existing.lastTouched) existing.lastTouched = timestamp;
      existing.touches.push(touch);
    } else {
      events.set(filePath, {
        filePath,
        tools: [toolName],
        count: 1,
        lastTouched: timestamp,
        touches: [touch],
      });
    }
  };

  for (const { message, tool } of occurrences) {
    if (!FILE_WRITING_TOOLS.has(tool.name)) continue;

    if (tool.name === "apply_patch") {
      const patch = typeof tool.input.patch === "string" ? tool.input.patch : "";
      for (const filePath of extractApplyPatchFilePaths(patch)) {
        recordTouch(filePath, tool.name, message.timestamp);
      }
      continue;
    }

    const filePath = typeof tool.input.file_path === "string" ? tool.input.file_path : null;
    if (!filePath) continue;
    recordTouch(filePath, tool.name, message.timestamp);
  }
  for (const event of events.values()) {
    event.touches.sort((a, b) => (a.timestamp < b.timestamp ? -1 : 1));
  }
  return Array.from(events.values()).sort((a, b) => (a.lastTouched < b.lastTouched ? 1 : -1));
}

// ============================================================================
// Aggregate header stats
// ============================================================================

export interface SessionIntelligence {
  tokenTotal: number;
  tokenBreakdown: TokenBreakdown;
  modelDistribution: ModelUsage[];
  toolCallCount: number;
  fileCount: number;
  agentCount: number;
  toolUsage: ToolUsageSummary[];
  fileEvents: FileEvent[];
  toolOccurrences: ToolUseOccurrence[];
}

export function calculateSessionIntelligence(
  messages: ClaudeMessage[],
  agentTaskCount: number
): SessionIntelligence {
  const toolOccurrences = getAllToolUseBlocks(messages);
  const fileEvents = extractFileEvents(toolOccurrences);
  const { breakdown, modelDistribution } = calculateTokenBreakdown(messages);
  return {
    tokenTotal: calculateTokenTotal(messages),
    tokenBreakdown: breakdown,
    modelDistribution,
    toolCallCount: toolOccurrences.length,
    fileCount: fileEvents.length,
    agentCount: agentTaskCount,
    toolUsage: summarizeToolUsage(toolOccurrences),
    fileEvents,
    toolOccurrences,
  };
}

// ============================================================================
// Session decision brief
// ============================================================================
// Every field below is derived from real signals already loaded with the
// session -- no field is AI-generated text, per the standing rule that Grid
// Local never generates AI text: it never calls a remote API or bundles a
// local model, so every "insight" must be derivable from real data or
// omitted, never fabricated. A field with no honest real signal reports a
// real, named state (e.g. "unverified"), never a guessed one.

/** The session's own first real user message -- the closest honest,
 * non-fabricated stand-in for "goal or request" (never AI-summarized). */
export function deriveGoal(messages: ClaudeMessage[]): string | null {
  for (const message of messages) {
    if (message.type !== "user") continue;
    if (typeof message.content === "string") {
      const trimmed = message.content.trim();
      if (trimmed) return trimmed;
      continue;
    }
    if (Array.isArray(message.content)) {
      for (const item of message.content as ContentItem[]) {
        const text = extractTextContent(item);
        if (text && text.text.trim()) return text.text.trim();
      }
    }
  }
  return null;
}

/** Maps a `tool_use` block's `id` to its paired `tool_result`'s `is_error`,
 * by scanning every message's content array for a `tool_result` block
 * (the result of a tool call is always a later message, never the same
 * one as the `tool_use`). */
function buildToolResultErrorMap(messages: ClaudeMessage[]): Map<string, boolean> {
  const map = new Map<string, boolean>();
  for (const message of messages) {
    if (!Array.isArray(message.content)) continue;
    for (const item of message.content as ContentItem[]) {
      const result = extractToolResultContent(item);
      if (result) map.set(result.tool_use_id, Boolean(result.is_error));
    }
  }
  return map;
}

/** A small, honest allowlist of unambiguous test/build command shapes --
 * deliberately narrow. A command that doesn't match this is never assumed
 * to be a verification step; `deriveVerificationStatus` reports
 * "unverified" rather than guess at commands outside this list. */
const TEST_COMMAND_PATTERN =
  /\b(?:npm|pnpm|yarn)\s+(?:run\s+)?test\b|\bcargo\s+test\b|\bpytest\b|\bgo\s+test\b|\bmvn\s+test\b|\bgradle\s+test\b/i;

export type VerificationStatus =
  | { kind: "no-changes" }
  | { kind: "unverified"; fileCount: number }
  | { kind: "verified"; command: string; timestamp: string }
  | { kind: "failed"; command: string; timestamp: string }
  | { kind: "stale"; command: string; timestamp: string; filesChangedSince: number };

/**
 * "Changed after the last passing verification" -- built entirely from
 * data already loaded with the open session, no new backend query. Finds
 * the LAST `Bash` tool call whose
 * command matches `TEST_COMMAND_PATTERN` and compares its timestamp
 * against the most recent file-modifying tool call (`fileEvents`, already
 * sorted newest-first by `extractFileEvents`). If no such command ever
 * ran, or its result can't be found, reports "unverified" -- never guesses
 * pass/fail from missing data.
 */
export function deriveVerificationStatus(
  messages: ClaudeMessage[],
  toolOccurrences: ToolUseOccurrence[],
  fileEvents: FileEvent[]
): VerificationStatus {
  if (fileEvents.length === 0) return { kind: "no-changes" };

  const errorByToolUseId = buildToolResultErrorMap(messages);
  const testRuns = toolOccurrences
    .filter(
      (o): o is ToolUseOccurrence & { tool: ToolUseContent & { input: { command: string } } } =>
        o.tool.name === "Bash" && typeof o.tool.input.command === "string"
    )
    .map((o) => ({
      command: o.tool.input.command,
      timestamp: o.message.timestamp,
      isError: errorByToolUseId.get(o.tool.id),
    }))
    .filter((run) => TEST_COMMAND_PATTERN.test(run.command))
    .sort((a, b) => (a.timestamp < b.timestamp ? -1 : 1));

  const lastTestRun = testRuns[testRuns.length - 1];
  if (!lastTestRun || lastTestRun.isError === undefined) {
    return { kind: "unverified", fileCount: fileEvents.length };
  }

  const lastFileChangeTime = fileEvents[0].lastTouched;
  if (lastTestRun.timestamp > lastFileChangeTime) {
    return lastTestRun.isError
      ? { kind: "failed", command: lastTestRun.command, timestamp: lastTestRun.timestamp }
      : { kind: "verified", command: lastTestRun.command, timestamp: lastTestRun.timestamp };
  }

  const filesChangedSince = fileEvents.filter((f) => f.lastTouched > lastTestRun.timestamp).length;
  return {
    kind: "stale",
    command: lastTestRun.command,
    timestamp: lastTestRun.timestamp,
    filesChangedSince,
  };
}

/** Whether the session's chronologically LAST tool call's result was an
 * error -- deliberately simpler than proving an error was never later
 * fixed (which would need cross-referencing subsequent identical calls, a
 * much fuzzier and easier-to-get-wrong claim). "The session ended on an
 * error" is a real, literal, unambiguous fact instead. */
export function endedOnToolError(
  messages: ClaudeMessage[],
  toolOccurrences: ToolUseOccurrence[]
): boolean {
  if (toolOccurrences.length === 0) return false;
  const errorByToolUseId = buildToolResultErrorMap(messages);
  const last = toolOccurrences[toolOccurrences.length - 1];
  return errorByToolUseId.get(last.tool.id) === true;
}

export interface SessionDecisionBrief {
  goal: string | null;
  verification: VerificationStatus;
  endedOnError: boolean;
}

export function deriveSessionDecisionBrief(
  messages: ClaudeMessage[],
  toolOccurrences: ToolUseOccurrence[],
  fileEvents: FileEvent[]
): SessionDecisionBrief {
  return {
    goal: deriveGoal(messages),
    verification: deriveVerificationStatus(messages, toolOccurrences, fileEvents),
    endedOnError: endedOnToolError(messages, toolOccurrences),
  };
}

/** A dimension this session is a real outlier on, relative to this
 * user's OWN historical average for the same provider -- a personal-
 * baseline anomaly. `ratio` is always >= `BASELINE_ANOMALY_THRESHOLD`. */
export interface BaselineAnomaly {
  dimension: "tokens" | "duration";
  ratio: number;
}

/** Below this many historical sessions, an "average" isn't a meaningful
 * personal baseline yet -- comparing against 1-2 past sessions would be
 * noise dressed up as a pattern. */
export const MIN_BASELINE_SESSIONS = 5;

/** How far above baseline a session must be before it's worth calling
 * out -- avoids flagging routine variance as an "anomaly." */
export const BASELINE_ANOMALY_THRESHOLD = 2;

/** Pure ratio comparison -- fetching the actual baseline (a real backend
 * query over this user's own archive_db) happens in `OverviewTab`; this
 * function only decides, given both numbers, whether the difference is
 * worth surfacing. Never fabricates a baseline when the sample is too
 * small ([[MIN_BASELINE_SESSIONS]]) or the baseline itself is zero. */
export function deriveBaselineAnomalies(
  currentTotalTokens: number,
  currentDurationMinutes: number,
  baseline: { average_total_tokens: number; average_duration_minutes: number; session_count: number }
): BaselineAnomaly[] {
  if (baseline.session_count < MIN_BASELINE_SESSIONS) return [];

  const anomalies: BaselineAnomaly[] = [];
  if (baseline.average_total_tokens > 0) {
    const ratio = currentTotalTokens / baseline.average_total_tokens;
    if (ratio >= BASELINE_ANOMALY_THRESHOLD) anomalies.push({ dimension: "tokens", ratio });
  }
  if (baseline.average_duration_minutes > 0) {
    const ratio = currentDurationMinutes / baseline.average_duration_minutes;
    if (ratio >= BASELINE_ANOMALY_THRESHOLD) anomalies.push({ dimension: "duration", ratio });
  }
  return anomalies;
}

/** Which of `VerificationStatus`'s own real states represent open work --
 * "unresolved items" is one of the one-action handoff preview's required
 * fields. */
export type UnresolvedItemKind = "endedOnError" | "unverified" | "failed" | "stale";

/**
 * Composed entirely from already-real, already-displayed structured
 * signals -- deliberately NOT a free-text/keyword heuristic. A keyword-
 * based "unresolved items" detector would be too unreliable to fit either
 * a "Source-linked" or "Derived" reliability tier; this function stays on
 * the safe side of that line by only ever reusing `SessionDecisionBrief`'s
 * own fields (`endedOnError`, `verification.kind`), never scanning
 * message text.
 */
export function deriveUnresolvedItems(brief: SessionDecisionBrief): UnresolvedItemKind[] {
  const items: UnresolvedItemKind[] = [];
  if (brief.endedOnError) items.push("endedOnError");
  switch (brief.verification.kind) {
    case "unverified":
    case "failed":
    case "stale":
      items.push(brief.verification.kind);
      break;
    case "no-changes":
    case "verified":
      break;
  }
  return items;
}
