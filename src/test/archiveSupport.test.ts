import { describe, expect, it } from "vitest";
import { isProviderSupportedByArchiveIndex } from "@/utils/archiveSupport";

describe("isProviderSupportedByArchiveIndex", () => {
  it("treats no selected project as supported (global view, not a per-provider gap)", () => {
    expect(isProviderSupportedByArchiveIndex(undefined)).toBe(true);
    expect(isProviderSupportedByArchiveIndex(null)).toBe(true);
  });

  it("marks every FILE_BASED_STATS_PROVIDERS-mirrored provider as supported", () => {
    // Mirrors src-tauri/src/commands/stats.rs's FILE_BASED_STATS_PROVIDERS
    // plus Claude itself -- keep this list in sync with that const.
    const supported = [
      "claude",
      "aider",
      "antigravity",
      "cline",
      "codebuddy",
      "codex",
      "continue",
      "copilot",
      "cursor-agent",
      "gemini",
      "grok",
      "kimi",
      "ompi",
      "openinterpreter",
      "pearai",
      "pi",
      "qwen",
      "vibe",
    ];
    for (const provider of supported) {
      expect(isProviderSupportedByArchiveIndex(provider)).toBe(true);
    }
  });

  it("marks providers not yet ingested by archive_db as unsupported", () => {
    const unsupported = [
      "amazonq",
      "crush",
      "cursor",
      "forgecode",
      "goose",
      "kiro",
      "llm",
      "opencode",
      "openhands",
      "trae",
      "zed",
    ];
    for (const provider of unsupported) {
      expect(isProviderSupportedByArchiveIndex(provider)).toBe(false);
    }
  });
});
