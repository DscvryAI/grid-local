/**
 * Archive Sync API Service
 *
 * Thin wrappers around the `archive_db` backfill/status Tauri commands
 * (`src-tauri/src/commands/archive_db.rs`). Not to be confused with
 * `commands/archive.rs` (the removed Session Board/Archive Manager
 * feature) -- this is the normalized SQLite layer.
 */

import { api } from "@/services/api";

export interface BackfillSummary {
  projectsScanned: number;
  sessionsIngested: number;
  sessionsSkippedUnchanged: number;
  messagesIngested: number;
  /** Optional: absent only in stale test fixtures written before these
   * fields were added on the Rust side. */
  cancelled?: boolean;
  durationMs?: number;
  parserFailures?: number;
}

export interface ArchiveDbStatus {
  providerCount: number;
  projectCount: number;
  sessionCount: number;
  messageCount: number;
}

/** Payload of the `"first-index-progress"` event emitted during {@link runFirstIndex}. */
export interface FirstIndexProgressEvent {
  providerKey: string;
  phasesDone: number;
  phasesTotal: number;
}

/** Idempotent full backfill -- safe to call on every app launch. */
export async function syncGridIndex(): Promise<BackfillSummary> {
  return api<BackfillSummary>("sync_grid_index");
}

/** Wipes and fully re-ingests Grid's own Claude archive data. */
export async function rebuildGridIndex(): Promise<BackfillSummary> {
  return api<BackfillSummary>("rebuild_grid_index");
}

/**
 * The mandatory, interactive first-run index -- unlike {@link syncGridIndex}, this reports per-provider
 * progress via the `"first-index-progress"` event (subscribe with
 * `@tauri-apps/api/event`'s `listen` before calling this) and can be
 * stopped early with {@link cancelFirstIndex}.
 */
export async function runFirstIndex(): Promise<BackfillSummary> {
  return api<BackfillSummary>("run_first_index");
}

/** Signals an in-progress {@link runFirstIndex} call to stop before its next provider phase. */
export async function cancelFirstIndex(): Promise<void> {
  return api<void>("cancel_first_index");
}

export async function getArchiveDbStatus(): Promise<ArchiveDbStatus> {
  return api<ArchiveDbStatus>("get_archive_db_status");
}

/**
 * Deletes Grid's own rebuildable archive data (`archive.db` + WAL/SHM
 * sidecars, `session-cache/`). Never touches provider history, user
 * metadata (`user-data.json`), or Settings presets -- see the backend
 * command's own doc comment for the exact, deliberately narrow scope.
 */
export async function deleteGridLocalData(): Promise<void> {
  return api<void>("delete_grid_local_data");
}
