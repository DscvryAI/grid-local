import { describe, it, expect } from "vitest";
import { buildHandoffPreviewMarkdown } from "./handoffPreview";
import type { SessionDecisionBrief, FileEvent } from "./sessionIntelligence";
import type { ClaudeSession } from "@/types";

// Mirrors real react-i18next's polymorphic `t(key, defaultValueOrOptions)`
// signature -- `handoffPreview.ts` (like the rest of this codebase) passes
// a plain string as the second arg for a literal default, and an options
// object only when interpolation/defaultValue is needed.
const t = (key: string, defaultValueOrOptions?: string | Record<string, unknown>): string => {
  const isOptions = typeof defaultValueOrOptions === "object" && defaultValueOrOptions !== null;
  const options = isOptions ? (defaultValueOrOptions as Record<string, unknown>) : undefined;
  const defaultValue =
    (options?.defaultValue as string) ?? (typeof defaultValueOrOptions === "string" ? defaultValueOrOptions : key);
  let text = defaultValue;
  if (options) {
    for (const [k, v] of Object.entries(options)) {
      text = text.replace(`{{${k}}}`, String(v));
    }
  }
  return text;
};

function session(overrides: Partial<ClaudeSession> = {}): ClaudeSession {
  return {
    session_id: "id-1",
    actual_session_id: "actual-1",
    file_path: "/Users/me/.claude/projects/foo/session1.jsonl",
    project_name: "foo",
    message_count: 10,
    first_message_time: "2026-08-01T00:00:00.000Z",
    last_message_time: "2026-08-01T01:00:00.000Z",
    last_modified: "2026-08-01T01:00:00.000Z",
    has_tool_use: true,
    has_errors: false,
    ...overrides,
  } as ClaudeSession;
}

function brief(overrides: Partial<SessionDecisionBrief> = {}): SessionDecisionBrief {
  return {
    goal: null,
    verification: { kind: "no-changes" },
    endedOnError: false,
    ...overrides,
  };
}

const fileEvent = (filePath: string): FileEvent => ({
  filePath,
  tools: ["Edit"],
  count: 1,
  lastTouched: "2026-08-01T00:30:00.000Z",
  touches: [{ tool: "Edit", timestamp: "2026-08-01T00:30:00.000Z" }],
});

describe("buildHandoffPreviewMarkdown", () => {
  it("includes the goal when present", () => {
    const md = buildHandoffPreviewMarkdown(session(), brief({ goal: "Fix the login bug" }), [], t);
    expect(md).toContain("Fix the login bug");
  });

  it("omits the goal line when there is none", () => {
    const md = buildHandoffPreviewMarkdown(session(), brief({ goal: null }), [], t);
    expect(md).not.toContain("Goal");
  });

  it("lists every changed file", () => {
    const md = buildHandoffPreviewMarkdown(
      session(),
      brief(),
      [fileEvent("/a.ts"), fileEvent("/b.ts")],
      t
    );
    expect(md).toContain("- /a.ts");
    expect(md).toContain("- /b.ts");
    expect(md).toContain("Changes (2)");
  });

  it("reports no files changed honestly when there are none", () => {
    const md = buildHandoffPreviewMarkdown(session(), brief(), [], t);
    expect(md).toContain("Changes (0)");
    expect(md).toContain("No files changed in this session");
  });

  it("reports 'None' for unresolved items on a clean session", () => {
    const md = buildHandoffPreviewMarkdown(
      session(),
      brief({ verification: { kind: "verified", command: "npm test", timestamp: "t" } }),
      [],
      t
    );
    expect(md).toMatch(/Unresolved items:\*\*\n- None/);
  });

  it("lists endedOnError as an unresolved item, reusing its own real copy", () => {
    const md = buildHandoffPreviewMarkdown(session(), brief({ endedOnError: true }), [], t);
    expect(md).toContain("Session ended on a tool error");
  });

  it("lists a verification gap as an unresolved item", () => {
    const md = buildHandoffPreviewMarkdown(
      session(),
      brief({ verification: { kind: "unverified", fileCount: 2 } }),
      [fileEvent("/a.ts"), fileEvent("/b.ts")],
      t
    );
    expect(md).toContain("2 files changed; no test/build command found to verify them");
  });

  it("always includes the session's real file path as the source", () => {
    const md = buildHandoffPreviewMarkdown(session({ file_path: "/real/path.jsonl" }), brief(), [], t);
    expect(md).toContain("/real/path.jsonl");
  });
});
