/**
 * History API Service
 *
 * Thin wrapper around the `list_history_sessions` Tauri command backing
 * the History surface (spec §13).
 */

import { api } from "@/services/api";
import type { HistoryFilterParams, HistorySessionsPage } from "../types";

export interface FetchHistorySessionsOptions {
  activeProviders?: string[];
  customClaudePaths?: { path: string }[];
  filters?: HistoryFilterParams;
  offset?: number;
  limit?: number;
}

export async function fetchHistorySessions(
  options: FetchHistorySessionsOptions = {}
): Promise<HistorySessionsPage> {
  const {
    activeProviders,
    customClaudePaths,
    filters,
    offset = 0,
    limit = 50,
  } = options;

  return api<HistorySessionsPage>("list_history_sessions", {
    activeProviders:
      activeProviders && activeProviders.length > 0 ? activeProviders : undefined,
    customClaudePaths:
      customClaudePaths && customClaudePaths.length > 0 ? customClaudePaths : undefined,
    filters,
    offset,
    limit,
  });
}
