import { describe, it, expect } from "vitest";
import type { ClaudeMessage } from "../../../types";
import {
  calculateTokenTotal,
  calculateTokenBreakdown,
  getAllToolUseBlocks,
  summarizeToolUsage,
  extractFileEvents,
  deriveGoal,
  deriveVerificationStatus,
  endedOnToolError,
  deriveBaselineAnomalies,
  deriveUnresolvedItems,
} from "./sessionIntelligence";
import type { SessionDecisionBrief } from "./sessionIntelligence";

function assistantMessage(
  overrides: Partial<ClaudeMessage> & { uuid: string }
): ClaudeMessage {
  return {
    type: "assistant",
    role: "assistant",
    sessionId: "session-1",
    timestamp: "2026-08-01T00:00:00.000Z",
    content: [],
    ...overrides,
  } as ClaudeMessage;
}

function userMessage(
  overrides: Partial<ClaudeMessage> & { uuid: string }
): ClaudeMessage {
  return {
    type: "user",
    role: "user",
    sessionId: "session-1",
    timestamp: "2026-08-01T00:00:00.000Z",
    content: "",
    ...overrides,
  } as ClaudeMessage;
}

/** A Bash tool_use paired with a later message carrying its tool_result. */
function bashRun(
  id: string,
  command: string,
  timestamp: string,
  isError: boolean
): ClaudeMessage[] {
  return [
    assistantMessage({
      uuid: `${id}-call`,
      timestamp,
      content: [{ type: "tool_use", id, name: "Bash", input: { command } }],
    }),
    userMessage({
      uuid: `${id}-result`,
      timestamp,
      content: [{ type: "tool_result", tool_use_id: id, is_error: isError, content: "" }],
    }),
  ];
}

describe("calculateTokenTotal", () => {
  it("sums usage across distinct assistant messages", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        messageId: "msg-1",
        usage: { input_tokens: 10, output_tokens: 5 },
      }),
      assistantMessage({
        uuid: "b",
        messageId: "msg-2",
        usage: { input_tokens: 20, output_tokens: 8 },
      }),
    ];
    expect(calculateTokenTotal(messages)).toBe(43);
  });

  it("dedups repeated rows sharing the same messageId (#283)", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        messageId: "msg-1",
        usage: { input_tokens: 100, output_tokens: 50 },
      }),
      assistantMessage({
        uuid: "b",
        messageId: "msg-1",
        usage: { input_tokens: 100, output_tokens: 50 },
      }),
    ];
    expect(calculateTokenTotal(messages)).toBe(150);
  });

  it("falls back to uuid when messageId is absent, counting each row", () => {
    const messages = [
      assistantMessage({ uuid: "a", usage: { input_tokens: 10 } }),
      assistantMessage({ uuid: "b", usage: { input_tokens: 10 } }),
    ];
    expect(calculateTokenTotal(messages)).toBe(20);
  });

  it("ignores user messages and assistant messages with no usage", () => {
    const messages = [
      { type: "user", role: "user", sessionId: "s", timestamp: "t", uuid: "u1", content: "hi" } as ClaudeMessage,
      assistantMessage({ uuid: "a" }),
    ];
    expect(calculateTokenTotal(messages)).toBe(0);
  });
});

describe("getAllToolUseBlocks / summarizeToolUsage", () => {
  it("collects every tool_use block, including multiple in one message", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        content: [
          { type: "tool_use", id: "1", name: "Read", input: {} },
          { type: "tool_use", id: "2", name: "Bash", input: {} },
        ],
      }),
      assistantMessage({
        uuid: "b",
        content: [{ type: "tool_use", id: "3", name: "Read", input: {} }],
      }),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    expect(occurrences).toHaveLength(3);
    expect(summarizeToolUsage(occurrences)).toEqual([
      { name: "Read", count: 2 },
      { name: "Bash", count: 1 },
    ]);
  });

  it("falls back to the legacy top-level toolUse field", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        content: "no array here",
        toolUse: { id: "1", name: "Grep", input: { pattern: "x" } },
      } as Partial<ClaudeMessage> & { uuid: string }),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    expect(occurrences).toHaveLength(1);
    expect(occurrences[0].tool.name).toBe("Grep");
  });
});

describe("extractFileEvents", () => {
  it("groups by file_path across write-capable tools, ignoring read-only tools", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [
          { type: "tool_use", id: "1", name: "Write", input: { file_path: "/a.ts" } },
          { type: "tool_use", id: "2", name: "Read", input: { file_path: "/a.ts" } },
        ],
      }),
      assistantMessage({
        uuid: "b",
        timestamp: "2026-08-02T00:00:00.000Z",
        content: [{ type: "tool_use", id: "3", name: "Edit", input: { file_path: "/a.ts" } }],
      }),
      assistantMessage({
        uuid: "c",
        timestamp: "2026-08-01T12:00:00.000Z",
        content: [{ type: "tool_use", id: "4", name: "MultiEdit", input: { file_path: "/b.ts" } }],
      }),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const events = extractFileEvents(occurrences);

    expect(events).toHaveLength(2);
    // Sorted by lastTouched descending.
    expect(events[0].filePath).toBe("/a.ts");
    expect(events[0].count).toBe(2);
    expect(events[0].tools).toEqual(["Write", "Edit"]);
    expect(events[0].lastTouched).toBe("2026-08-02T00:00:00.000Z");
    expect(events[1].filePath).toBe("/b.ts");
  });

  it("records the full ordered touch sequence behind the aggregate fields", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        timestamp: "2026-08-02T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "Edit", input: { file_path: "/a.ts" } }],
      }),
      assistantMessage({
        uuid: "b",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "2", name: "Write", input: { file_path: "/a.ts" } }],
      }),
    ];
    const events = extractFileEvents(getAllToolUseBlocks(messages));
    expect(events[0].touches).toEqual([
      { tool: "Write", timestamp: "2026-08-01T00:00:00.000Z" },
      { tool: "Edit", timestamp: "2026-08-02T00:00:00.000Z" },
    ]);
  });

  it("skips tool_use blocks with no file_path input", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        content: [{ type: "tool_use", id: "1", name: "Write", input: {} }],
      }),
    ];
    expect(extractFileEvents(getAllToolUseBlocks(messages))).toHaveLength(0);
  });

  it("parses every file path out of a Codex apply_patch call, including multi-file patches", () => {
    const patch = [
      "*** Begin Patch",
      "*** Update File: src/a.ts",
      "@@",
      "-old",
      "+new",
      "*** Add File: src/b.ts",
      "+content",
      "*** Delete File: src/c.ts",
      "*** End Patch",
    ].join("\n");
    const messages = [
      assistantMessage({
        uuid: "a",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "apply_patch", input: { patch } }],
      }),
    ];
    const events = extractFileEvents(getAllToolUseBlocks(messages));

    expect(events).toHaveLength(3);
    expect(events.map((e) => e.filePath).sort()).toEqual(["src/a.ts", "src/b.ts", "src/c.ts"]);
    expect(events[0].tools).toEqual(["apply_patch"]);
  });
});

describe("calculateTokenBreakdown", () => {
  it("sums per-category totals and groups by model, deduping by messageId", () => {
    const messages = [
      assistantMessage({
        uuid: "a",
        messageId: "msg-1",
        model: "claude-x",
        usage: {
          input_tokens: 10,
          output_tokens: 5,
          cache_creation_input_tokens: 2,
          cache_read_input_tokens: 1,
          reasoning_tokens: 3,
        },
      }),
      // Repeated row for the same messageId -- must not double-count.
      assistantMessage({
        uuid: "a-repeat",
        messageId: "msg-1",
        model: "claude-x",
        usage: { input_tokens: 10, output_tokens: 5 },
      }),
      assistantMessage({
        uuid: "b",
        messageId: "msg-2",
        model: "claude-y",
        usage: { input_tokens: 20, output_tokens: 8 },
      }),
    ];

    const { breakdown, modelDistribution } = calculateTokenBreakdown(messages);

    expect(breakdown).toEqual({
      input: 30,
      output: 13,
      cacheCreation: 2,
      cacheRead: 1,
      reasoning: 3,
    });
    expect(modelDistribution).toHaveLength(2);
    const claudeX = modelDistribution.find((m) => m.model_name === "claude-x");
    expect(claudeX).toMatchObject({
      input_tokens: 10,
      output_tokens: 5,
      cache_creation_tokens: 2,
      cache_read_tokens: 1,
      reasoning_tokens: 3,
      token_count: 21,
    });
    const claudeY = modelDistribution.find((m) => m.model_name === "claude-y");
    expect(claudeY).toMatchObject({
      input_tokens: 20,
      output_tokens: 8,
      token_count: 28,
    });
  });

  it("returns an all-zero breakdown and empty model list for no usage", () => {
    const { breakdown, modelDistribution } = calculateTokenBreakdown([]);
    expect(breakdown).toEqual({
      input: 0,
      output: 0,
      cacheCreation: 0,
      cacheRead: 0,
      reasoning: 0,
    });
    expect(modelDistribution).toEqual([]);
  });
});

describe("deriveGoal", () => {
  it("returns the first real user message's text", () => {
    const messages = [
      userMessage({ uuid: "u1", content: "  Fix the auth bug  " }),
      assistantMessage({ uuid: "a1" }),
    ];
    expect(deriveGoal(messages)).toBe("Fix the auth bug");
  });

  it("extracts text from a content-array user message", () => {
    const messages = [
      userMessage({
        uuid: "u1",
        content: [{ type: "text", text: "Add retry logic" }],
      }),
    ];
    expect(deriveGoal(messages)).toBe("Add retry logic");
  });

  it("skips blank user messages and returns null when none have text", () => {
    const messages = [
      userMessage({ uuid: "u1", content: "   " }),
      assistantMessage({ uuid: "a1" }),
    ];
    expect(deriveGoal(messages)).toBeNull();
  });
});

describe("deriveVerificationStatus", () => {
  it("reports no-changes when no files were touched", () => {
    const occurrences = getAllToolUseBlocks([]);
    expect(deriveVerificationStatus([], occurrences, [])).toEqual({ kind: "no-changes" });
  });

  it("reports unverified when files changed but no test command ever ran", () => {
    const messages = [
      assistantMessage({
        uuid: "a1",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "Write", input: { file_path: "/a.ts" } }],
      }),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const fileEvents = extractFileEvents(occurrences);
    expect(deriveVerificationStatus(messages, occurrences, fileEvents)).toEqual({
      kind: "unverified",
      fileCount: 1,
    });
  });

  it("reports verified when a passing test command ran after the last file change", () => {
    const messages: ClaudeMessage[] = [
      assistantMessage({
        uuid: "a1",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "Write", input: { file_path: "/a.ts" } }],
      }),
      ...bashRun("2", "npm test", "2026-08-01T01:00:00.000Z", false),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const fileEvents = extractFileEvents(occurrences);
    expect(deriveVerificationStatus(messages, occurrences, fileEvents)).toEqual({
      kind: "verified",
      command: "npm test",
      timestamp: "2026-08-01T01:00:00.000Z",
    });
  });

  it("reports failed when the last test command's result was an error", () => {
    const messages: ClaudeMessage[] = [
      assistantMessage({
        uuid: "a1",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "Write", input: { file_path: "/a.ts" } }],
      }),
      ...bashRun("2", "cargo test", "2026-08-01T01:00:00.000Z", true),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const fileEvents = extractFileEvents(occurrences);
    expect(deriveVerificationStatus(messages, occurrences, fileEvents)).toEqual({
      kind: "failed",
      command: "cargo test",
      timestamp: "2026-08-01T01:00:00.000Z",
    });
  });

  it("reports stale when files changed after the last passing verification", () => {
    const messages: ClaudeMessage[] = [
      ...bashRun("1", "pytest", "2026-08-01T00:00:00.000Z", false),
      assistantMessage({
        uuid: "a2",
        timestamp: "2026-08-01T01:00:00.000Z",
        content: [{ type: "tool_use", id: "2", name: "Edit", input: { file_path: "/a.ts" } }],
      }),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const fileEvents = extractFileEvents(occurrences);
    expect(deriveVerificationStatus(messages, occurrences, fileEvents)).toEqual({
      kind: "stale",
      command: "pytest",
      timestamp: "2026-08-01T00:00:00.000Z",
      filesChangedSince: 1,
    });
  });

  it("does not treat an unrelated Bash command as verification", () => {
    const messages: ClaudeMessage[] = [
      assistantMessage({
        uuid: "a1",
        timestamp: "2026-08-01T00:00:00.000Z",
        content: [{ type: "tool_use", id: "1", name: "Write", input: { file_path: "/a.ts" } }],
      }),
      ...bashRun("2", "ls -la", "2026-08-01T01:00:00.000Z", false),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    const fileEvents = extractFileEvents(occurrences);
    expect(deriveVerificationStatus(messages, occurrences, fileEvents)).toEqual({
      kind: "unverified",
      fileCount: 1,
    });
  });
});

describe("endedOnToolError", () => {
  it("is true when the chronologically last tool call's result was an error", () => {
    const messages: ClaudeMessage[] = [
      ...bashRun("1", "npm test", "2026-08-01T00:00:00.000Z", false),
      ...bashRun("2", "npm run build", "2026-08-01T01:00:00.000Z", true),
    ];
    const occurrences = getAllToolUseBlocks(messages);
    expect(endedOnToolError(messages, occurrences)).toBe(true);
  });

  it("is false when the last tool call succeeded", () => {
    const messages: ClaudeMessage[] = [...bashRun("1", "npm test", "2026-08-01T00:00:00.000Z", false)];
    const occurrences = getAllToolUseBlocks(messages);
    expect(endedOnToolError(messages, occurrences)).toBe(false);
  });

  it("is false when there are no tool calls at all", () => {
    expect(endedOnToolError([], [])).toBe(false);
  });
});

describe("deriveBaselineAnomalies", () => {
  const baseline = { average_total_tokens: 1000, average_duration_minutes: 10, session_count: 5 };

  it("flags tokens when the ratio crosses the threshold", () => {
    const anomalies = deriveBaselineAnomalies(2500, 10, baseline);
    expect(anomalies).toEqual([{ dimension: "tokens", ratio: 2.5 }]);
  });

  it("flags duration when the ratio crosses the threshold", () => {
    const anomalies = deriveBaselineAnomalies(1000, 25, baseline);
    expect(anomalies).toEqual([{ dimension: "duration", ratio: 2.5 }]);
  });

  it("flags both dimensions independently when both cross the threshold", () => {
    const anomalies = deriveBaselineAnomalies(3000, 30, baseline);
    expect(anomalies).toEqual([
      { dimension: "tokens", ratio: 3 },
      { dimension: "duration", ratio: 3 },
    ]);
  });

  it("flags nothing when the session is close to its own baseline", () => {
    expect(deriveBaselineAnomalies(1100, 11, baseline)).toEqual([]);
  });

  it("never fabricates a baseline from too small a sample", () => {
    const tinyBaseline = { ...baseline, session_count: 2 };
    expect(deriveBaselineAnomalies(5000, 50, tinyBaseline)).toEqual([]);
  });

  it("does not divide by a zero baseline average", () => {
    const zeroBaseline = { average_total_tokens: 0, average_duration_minutes: 0, session_count: 10 };
    expect(deriveBaselineAnomalies(5000, 50, zeroBaseline)).toEqual([]);
  });
});

describe("deriveUnresolvedItems", () => {
  function brief(overrides: Partial<SessionDecisionBrief>): SessionDecisionBrief {
    return {
      goal: null,
      verification: { kind: "no-changes" },
      endedOnError: false,
      ...overrides,
    };
  }

  it("is empty for a clean, verified session", () => {
    expect(
      deriveUnresolvedItems(brief({ verification: { kind: "verified", command: "npm test", timestamp: "t" } }))
    ).toEqual([]);
  });

  it("is empty for a session with no file changes", () => {
    expect(deriveUnresolvedItems(brief({}))).toEqual([]);
  });

  it("flags endedOnError", () => {
    expect(deriveUnresolvedItems(brief({ endedOnError: true }))).toEqual(["endedOnError"]);
  });

  it("flags an unverified verification status", () => {
    expect(deriveUnresolvedItems(brief({ verification: { kind: "unverified", fileCount: 3 } }))).toEqual([
      "unverified",
    ]);
  });

  it("flags a failed verification status", () => {
    expect(
      deriveUnresolvedItems(brief({ verification: { kind: "failed", command: "pytest", timestamp: "t" } }))
    ).toEqual(["failed"]);
  });

  it("flags a stale verification status", () => {
    expect(
      deriveUnresolvedItems(
        brief({ verification: { kind: "stale", command: "pytest", timestamp: "t", filesChangedSince: 2 } })
      )
    ).toEqual(["stale"]);
  });

  it("flags both endedOnError and a real verification gap together", () => {
    expect(
      deriveUnresolvedItems(brief({ endedOnError: true, verification: { kind: "unverified", fileCount: 1 } }))
    ).toEqual(["endedOnError", "unverified"]);
  });
});
