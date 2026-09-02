/**
 * Diagnostics API Service
 *
 * Thin wrappers around the `diagnostics` Tauri commands
 * (`src-tauri/src/commands/diagnostics.rs`). Local-only, privacy-safe
 * usage counters -- see `DiagnosticsLog`'s own doc comment for the exact
 * schema and what it deliberately excludes.
 */

import { api } from "@/services/api";
import type { DiagnosticsEvent, DiagnosticsLog } from "@/types/diagnostics.types";

/**
 * Records one diagnostics event. Never throws to the caller -- a failed
 * diagnostics write must not interrupt the real action the user was
 * taking (opening a session, running a search, etc.), so this swallows
 * and logs any error itself.
 */
export async function recordDiagnosticsEvent(event: DiagnosticsEvent): Promise<void> {
  try {
    await api<void>("record_diagnostics_event", { event });
  } catch (error) {
    console.error("Failed to record diagnostics event:", event.kind, error);
  }
}

/** Reads the current diagnostics log -- for the inspect-before-export dialog. Never writes. */
export async function getDiagnosticsSnapshot(): Promise<DiagnosticsLog> {
  return api<DiagnosticsLog>("get_diagnostics_snapshot");
}
