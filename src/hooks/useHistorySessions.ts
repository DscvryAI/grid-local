/**
 * History Sessions Hook
 *
 * Fetches and paginates the cross-provider, date-sortable session list
 * backing the History surface (spec §13). Filter state
 * lives here (Project/Provider/Date/Model); the Provider dimension mirrors
 * the store's existing `activeProviders` (the same field ProjectTree's
 * provider tabs already drive) rather than duplicating it.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { fetchHistorySessions } from "../services/historyApi";
import { useAppStore } from "../store/useAppStore";
import type {
  HistoryFilterParams,
  HistoryProjectFacet,
  HistoryProviderFacet,
  HistorySessionItem,
} from "../types";

const PAGE_SIZE = 50;

export type HistoryDateFilter = {
  startDate?: string;
  endDate?: string;
};

export interface UseHistorySessionsResult {
  sessions: HistorySessionItem[];
  isLoading: boolean;
  isLoadingMore: boolean;
  error: string | null;
  hasMore: boolean;
  totalCount: number;
  availableProjects: HistoryProjectFacet[];
  availableProviders: HistoryProviderFacet[];
  availableModels: string[];
  customClaudeDirsOmitted: boolean;
  projectKeys: string[];
  setProjectKeys: (keys: string[]) => void;
  dateFilter: HistoryDateFilter;
  setDateFilter: (filter: HistoryDateFilter) => void;
  models: string[];
  setModels: (models: string[]) => void;
  loadMore: () => void;
  refresh: () => void;
}

export function useHistorySessions(): UseHistorySessionsResult {
  const activeProviders = useAppStore((s) => s.activeProviders);

  const [sessions, setSessions] = useState<HistorySessionItem[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [totalCount, setTotalCount] = useState(0);
  const [availableProjects, setAvailableProjects] = useState<HistoryProjectFacet[]>([]);
  const [availableProviders, setAvailableProviders] = useState<HistoryProviderFacet[]>([]);
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [customClaudeDirsOmitted, setCustomClaudeDirsOmitted] = useState(false);

  const [projectKeys, setProjectKeys] = useState<string[]>([]);
  const [dateFilter, setDateFilter] = useState<HistoryDateFilter>({});
  const [models, setModels] = useState<string[]>([]);

  // Guards against a slow, stale request overwriting a newer one's result
  // (e.g. rapidly toggling a filter) -- only the most recently issued
  // request is allowed to commit state.
  const requestIdRef = useRef(0);

  const buildFilters = useCallback((): HistoryFilterParams => {
    const filters: HistoryFilterParams = {};
    if (projectKeys.length > 0) filters.project_keys = projectKeys;
    if (activeProviders.length > 0) filters.provider_ids = activeProviders;
    if (dateFilter.startDate) filters.start_date = dateFilter.startDate;
    if (dateFilter.endDate) filters.end_date = dateFilter.endDate;
    if (models.length > 0) filters.models = models;
    return filters;
  }, [projectKeys, activeProviders, dateFilter, models]);

  const fetchPage = useCallback(
    async (offset: number, append: boolean) => {
      const requestId = ++requestIdRef.current;
      if (append) {
        setIsLoadingMore(true);
      } else {
        setIsLoading(true);
      }
      setError(null);

      try {
        const start = performance.now();
        const page = await fetchHistorySessions({
          filters: buildFilters(),
          offset,
          limit: PAGE_SIZE,
        });
        if (import.meta.env.DEV) {
          console.log(
            `[History] list_history_sessions: ${(performance.now() - start).toFixed(1)}ms, ${page.items.length} items, ${page.total_count} total`
          );
        }

        if (requestIdRef.current !== requestId) {
          return; // superseded by a newer request
        }

        setSessions((prev) => (append ? [...prev, ...page.items] : page.items));
        setHasMore(page.has_more);
        setTotalCount(page.total_count);
        setAvailableProjects(page.available_projects);
        setAvailableProviders(page.available_providers);
        setAvailableModels(page.available_models);
        setCustomClaudeDirsOmitted(page.custom_claude_dirs_omitted);
      } catch (err) {
        if (requestIdRef.current !== requestId) {
          return;
        }
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (requestIdRef.current === requestId) {
          setIsLoading(false);
          setIsLoadingMore(false);
        }
      }
    },
    [buildFilters]
  );

  useEffect(() => {
    void fetchPage(0, false);
  }, [fetchPage]);

  const loadMore = useCallback(() => {
    if (isLoadingMore || !hasMore) return;
    void fetchPage(sessions.length, true);
  }, [fetchPage, isLoadingMore, hasMore, sessions.length]);

  const refresh = useCallback(() => {
    void fetchPage(0, false);
  }, [fetchPage]);

  return {
    sessions,
    isLoading,
    isLoadingMore,
    error,
    hasMore,
    totalCount,
    availableProjects,
    availableProviders,
    availableModels,
    customClaudeDirsOmitted,
    projectKeys,
    setProjectKeys,
    dateFilter,
    setDateFilter,
    models,
    setModels,
    loadMore,
    refresh,
  };
}
