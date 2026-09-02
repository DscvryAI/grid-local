/**
 * Search API Service
 *
 * Thin wrapper around `archive_db::search`'s Tauri command -- the
 * FTS5-backed primary global-search path. `useGlobalSearch` calls this
 * first and falls back to the
 * pre-existing raw-file-walk `search_messages`/`search_all_providers`
 * commands when it returns no hits (covers a not-yet-backfilled archive
 * or a provider `archive_db` doesn't ingest).
 */

import { api } from "@/services/api";
import type { SearchResult, SearchResultKind } from "../types";

export async function searchArchiveFts(
  query: string,
  kinds?: SearchResultKind[],
  providerKey?: string,
  projectKey?: string,
  limit = 50
): Promise<SearchResult[]> {
  return api<SearchResult[]>("search_archive_fts", {
    query,
    kinds,
    providerKey,
    projectKey,
    limit,
  });
}
