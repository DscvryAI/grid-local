import type { ProviderId } from "@/types";

/**
 * Providers `archive_db` actually ingests today (Claude natively, plus the
 * universal-provider-ingestion plan's `FILE_BASED_STATS_PROVIDERS` --
 * `src-tauri/src/commands/stats.rs`, keep in sync with that const). Used to
 * distinguish empty-state item 4's "unsupported provider" reason (a project
 * whose provider archive_db doesn't ingest, so an Insights query correctly
 * finds nothing) from a genuine "no data" result for a supported provider.
 *
 * Deliberately an explicit allowlist mirroring the backend's own, not a
 * deny-list -- a newly-ingested provider must be added here on purpose.
 */
const ARCHIVE_SUPPORTED_PROVIDER_IDS: ReadonlySet<ProviderId> = new Set<ProviderId>([
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
]);

export function isProviderSupportedByArchiveIndex(
  provider: ProviderId | string | undefined | null
): boolean {
  if (!provider) return true; // no project selected -- global view, not a per-provider gap
  return ARCHIVE_SUPPORTED_PROVIDER_IDS.has(provider as ProviderId);
}
