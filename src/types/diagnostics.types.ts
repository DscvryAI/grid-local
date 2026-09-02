/**
 * Grid Local's local-only pilot diagnostics log. Mirrors
 * `src-tauri/src/models/diagnostics.rs::DiagnosticsLog` field-for-field --
 * keep both in sync. Deliberately excludes all conversation content,
 * prompts, commands, code, filenames, and raw paths.
 */
export interface DiagnosticsLog {
  version: number;
  installedAt: string | null;
  firstIndexCompletedAt: string | null;
  firstPopulatedHomeAt: string | null;
  firstEvidenceDrilldownAt: string | null;
  launchCount: number;
  /** Distinct `YYYY-MM-DD` (local time) days the app was launched on. */
  activeDays: string[];
  searchCount: number;
  searchZeroResultCount: number;
  searchResultOpenCount: number;
  /** Keyed by primary surface: "home" / "history" / "search" / "insights". */
  surfaceVisits: Record<string, number>;
  problemOpens: number;
  agentRunOpens: number;
  indexRuns: IndexRunRecord[];
  crashCount: number;
  /** Keyed by artifact type: "markdown" / "json" / "html" / "diagnostics". */
  exportCounts: Record<string, number>;
}

export interface IndexRunRecord {
  completedAt: string;
  durationMs: number;
  providerCount: number;
  sessionCount: number;
  parserFailures: number;
}

/** Mirrors `DiagnosticsEvent`'s Rust variants (`#[serde(tag = "kind", rename_all = "camelCase")]`). */
export type DiagnosticsEvent =
  | { kind: "appLaunched" }
  | { kind: "surfaceVisited"; surface: string }
  | { kind: "homePopulated" }
  | { kind: "evidenceDrilldownOpened" }
  | { kind: "searchExecuted"; resultCount: number }
  | { kind: "searchResultOpened" }
  | { kind: "problemOpened" }
  | { kind: "agentRunOpened" }
  | {
      kind: "indexCompleted";
      durationMs: number;
      providerCount: number;
      sessionCount: number;
      parserFailures: number;
    }
  | { kind: "crashed" }
  | { kind: "exported"; artifactType: string };
