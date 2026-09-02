/**
 * Global Search Hook
 *
 * Extracts `GlobalSearchModal`'s query/filter state, debounced search,
 * result grouping, keyboard navigation, and session-resolution logic into a
 * shared hook (spec §15) so the Search surface (full-page)
 * and the existing Cmd/Ctrl+K palette (`Dialog`-wrapped) render from ONE
 * implementation instead of forking into two search UIs.
 */

import { useState, useCallback, useEffect, useRef, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { api } from "@/services/api";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import { searchArchiveFts } from "@/services/searchApi";
import { useAppStore } from "@/store/useAppStore";
import type {
  ClaudeMessage,
  ClaudeSession,
  ContentItem,
  SearchResult,
  SearchResultKind,
} from "@/types";
import {
  getProviderLabel,
  getWslSearchableProviderIds,
  hasNonDefaultProvider,
} from "@/utils/providers";
import { toast } from "sonner";

/** `searchKind` is set only for archive-FTS-backed results (the "evidence
 * type" taxonomy) -- the raw-scan fallback path has no equivalent kind
 * information, so its results carry no `searchKind` and render exactly
 * as before (a role-based badge, no dedicated kind label). This is a
 * deliberately additive extension of the existing `ClaudeMessage` stub
 * shape, not a rewrite -- every existing consumer (`getPreviewText`/
 * `getMatchLocation`/`highlightText`/click-through) keeps working
 * unchanged. */
export type GlobalSearchResult = ClaudeMessage & { searchKind?: SearchResultKind };

export type MessageTypeFilter = "all" | "user" | "assistant";

export interface SearchDateFilter {
  startDate?: string;
  endDate?: string;
}

export type SearchResultGroup = {
  label: string;
  provider?: string;
  pathUnavailable: boolean;
  items: GlobalSearchResult[];
};

const MAX_RESULTS = 100;

/** Adapts an archive-FTS hit (any of the 6 kinds, not just `message`
 * anymore) into a `ClaudeMessage`-shaped stub so it flows through the existing rendering
 * functions (`getPreviewText`/`getMatchLocation`/`highlightText`) and
 * click-through (`handleSelectResult`) unchanged -- this stub is only
 * ever used for rendering the results list and reading `sessionId`/
 * `uuid` on select, it is never written into the app's real message
 * state. `content` is the FTS5 `snippet()` excerpt (bounded, with its own
 * `‹›…` markers), not the full message -- `getPreviewText`'s own
 * substring search against it still finds and highlights the query in
 * the common case, since the matched word itself is untouched, just
 * bracketed.
 *
 * `hit.message_uuid`/`hit.message_role` are now resolved for EVERY kind
 * (the backend fix in `search_archive` that made this item's "open in
 * context" actually work), so `uuid` no longer needs a synthetic
 * fallback for non-message kinds -- only a genuinely unresolvable hit
 * (which shouldn't occur post-fix, but isn't assumed impossible) still
 * falls back to a session+timestamp-derived key. */
function archiveHitToGlobalSearchResult(hit: SearchResult): GlobalSearchResult {
  return {
    uuid: hit.message_uuid ?? `${hit.session_id}:${hit.occurred_at ?? hit.snippet.slice(0, 32)}`,
    sessionId: hit.session_id,
    projectName: hit.project_name,
    provider: hit.provider_key,
    type: hit.message_role === "user" ? "user" : "assistant",
    content: hit.snippet,
    timestamp: hit.occurred_at ?? "",
    searchKind: hit.kind,
  } as unknown as GlobalSearchResult;
}

/** `<input type="date">` values -> the RFC3339 `[start, end]` pair the
 * backend's `dateRange` filter expects (`matches_filters` in
 * `commands/session/search.rs`, which requires BOTH bounds to parse).
 * Local calendar day, matching the History surface's own date-filter
 * convention -- not a rolling 24h window. */
function dateFilterToRange(filter: SearchDateFilter): [string, string] | null {
  if (!filter.startDate && !filter.endDate) return null;
  const start = filter.startDate
    ? new Date(`${filter.startDate}T00:00:00`)
    : new Date(0);
  const end = filter.endDate
    ? new Date(`${filter.endDate}T23:59:59.999`)
    : new Date();
  return [start.toISOString(), end.toISOString()];
}

export interface UseGlobalSearchOptions {
  /** Called after a result is successfully resolved and opened, or when
   * resolution fails/is not found -- lets the Dialog palette close itself
   * while the full-page Search surface can leave itself open (no-op). */
  onAfterSelect?: () => void;
  /** Called on Escape -- the Dialog palette closes; the full-page surface
   * can leave this unset. */
  onEscape?: () => void;
}

export interface UseGlobalSearchResult {
  query: string;
  results: GlobalSearchResult[];
  isSearching: boolean;
  selectedIndex: number;
  setSelectedIndex: (index: number) => void;
  messageTypeFilter: MessageTypeFilter;
  setMessageTypeFilter: (filter: MessageTypeFilter) => void;
  selectedProjectPath: string;
  setSelectedProjectPath: (path: string) => void;
  dateFilter: SearchDateFilter;
  setDateFilter: (filter: SearchDateFilter) => void;
  /** Non-null when `dateFilter.startDate` is after `dateFilter.endDate`.
   * `performSearch` skips the request entirely while this is set (rather
   * than sending an invalid range to the backend and surfacing a generic
   * "Search failed" toast) -- callers should show this inline near the
   * date inputs. */
  dateRangeError: string | null;
  groupedResults: Map<string, SearchResultGroup>;
  flattenedResults: GlobalSearchResult[];
  handleInputChange: (value: string) => void;
  clearQuery: () => void;
  handleSelectResult: (result: GlobalSearchResult) => Promise<void>;
  handleKeyDown: (key: "ArrowDown" | "ArrowUp" | "Enter" | "Escape") => void;
  getSessionName: (result: GlobalSearchResult) => string | undefined;
  getPreviewText: (message: GlobalSearchResult) => string;
  getMatchLocation: (message: GlobalSearchResult) => string;
  formatTimestamp: (timestamp: string) => string;
  highlightText: (text: string) => React.ReactNode;
  reset: () => void;
}

export function useGlobalSearch(
  options: UseGlobalSearchOptions = {}
): UseGlobalSearchResult {
  const { onAfterSelect, onEscape } = options;
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<GlobalSearchResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [messageTypeFilter, setMessageTypeFilter] = useState<MessageTypeFilter>("all");
  const [selectedProjectPath, setSelectedProjectPath] = useState<string>("all");
  const [dateFilter, setDateFilter] = useState<SearchDateFilter>({});
  const debounceTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Bumped on every result click and on reset -- cancels an in-flight
  // session-resolution sweep so it stops issuing project requests.
  const resolveTokenRef = useRef(0);
  // Bumped at the start of every performSearch call; a response is only
  // committed to state if it's still the most recently issued request.
  // Without this, a fast filter change (e.g. typing/picking a date) can
  // fire several overlapping searches, and a slower EARLIER response
  // (queried before the filter was applied) can resolve AFTER a faster
  // later one and silently overwrite it -- showing results that ignore
  // the filter the user just set, read as "the filter doesn't work"
  // (reported directly: a date range of 25-26 Aug showed results dated
  // 21/22 Aug). Mirrors useHistorySessions' `requestIdRef` guard.
  const requestIdRef = useRef(0);

  const {
    claudePath,
    projects,
    selectProject,
    selectSession,
    sessions,
    getSessionDisplayName,
    activeProviders,
    navigateToMessage,
    clearTargetMessage,
    setAnalyticsCurrentView,
    userMetadata,
  } = useAppStore();

  // String comparison is correct here: `<input type="date">` values are
  // always "YYYY-MM-DD", where lexicographic order equals chronological
  // order.
  const dateRangeError =
    dateFilter.startDate && dateFilter.endDate && dateFilter.startDate > dateFilter.endDate
      ? t("globalSearch.filter.dateRangeInvalid")
      : null;

  const groupedResults = useMemo(() => {
    const groups = new Map<string, SearchResultGroup>();

    for (const result of results) {
      const projectName = result.projectName || t("globalSearch.unknownProject");
      const resultProvider = result.provider ?? "claude";
      const matchingProject = projects.find(
        (project) =>
          (project.provider ?? "claude") === resultProvider && project.name === projectName
      );
      const providerLabel = getProviderLabel((key, fallback) => t(key, fallback), result.provider);
      const groupKey = `${resultProvider}::${projectName}`;
      const groupLabel = `${projectName} (${providerLabel})`;

      if (!groups.has(groupKey)) {
        groups.set(groupKey, {
          label: groupLabel,
          provider: result.provider,
          pathUnavailable: matchingProject?.path_status === "unavailable",
          items: [],
        });
      }
      groups.get(groupKey)!.items.push(result);
    }

    return groups;
  }, [projects, results, t]);

  const flattenedResults = useMemo(() => {
    const flat: GlobalSearchResult[] = [];
    for (const group of groupedResults.values()) {
      flat.push(...group.items);
    }
    return flat;
  }, [groupedResults]);

  const getSessionName = useCallback(
    (result: GlobalSearchResult): string | undefined => {
      if (!result.sessionId || result.sessionId === "unknown-session") return undefined;
      const name = getSessionDisplayName(result.sessionId);
      if (name) return name;
      return t("globalSearch.conversationId", { id: result.sessionId.slice(0, 8) });
    },
    [getSessionDisplayName, t]
  );

  const performSearch = useCallback(
    async (searchQuery: string) => {
      const requestId = ++requestIdRef.current;
      const trimmedQuery = searchQuery.trim();

      const hasNonClaudeProviders = hasNonDefaultProvider(activeProviders);
      const customClaudePaths = userMetadata?.settings?.customClaudePaths;
      const hasCustomPaths = (customClaudePaths?.length ?? 0) > 0;
      const wslEnabled = userMetadata?.settings?.wsl?.enabled ?? false;
      const hasAlternativeSource = hasNonClaudeProviders || hasCustomPaths || wslEnabled;
      const nativeClaudePath = claudePath || undefined;
      const wslProviders = wslEnabled ? getWslSearchableProviderIds(activeProviders) : undefined;

      if (
        trimmedQuery.length < 2 ||
        (!claudePath && !hasAlternativeSource) ||
        // An inverted date range would just fail the backend's
        // `validate_search_filters` check and surface a generic "Search
        // failed" toast -- `dateRangeError` already tells the user what's
        // wrong inline near the date inputs, so skip the request entirely
        // rather than round-tripping to fail.
        dateRangeError
      ) {
        if (requestIdRef.current === requestId) {
          setResults([]);
          setIsSearching(false);
        }
        return;
      }

      setIsSearching(true);
      try {
        const filters: Record<string, unknown> = {};
        if (selectedProjectPath !== "all") {
          const dirName = selectedProjectPath.split(/[\\/]/).pop() || selectedProjectPath;
          filters.projects = [dirName];
        }
        if (messageTypeFilter !== "all") {
          filters.messageType = messageTypeFilter;
        }
        const dateRange = dateFilterToRange(dateFilter);
        if (dateRange) {
          filters.dateRange = dateRange;
        }

        // Grid's own archive_db index is the PRIMARY path for the common
        // case -- an unfiltered content search with none of the
        // raw-scan-only filters active (project/message-type/date, which
        // `search_archive_fts` doesn't accept). Falls through to the
        // raw-file-walk scan below when the archive returns no hits (a
        // not-yet-backfilled archive, or a provider `archive_db` doesn't
        // ingest yet) or errors -- never silently drops a real query to "no
        // results." Queries every kind (`undefined` = no filter) rather
        // than hardcoding to `["message"]` only, so Command/ToolResult/
        // File/Error/AgentInstruction hits are searched too, since the
        // backend already supports them.
        const canUseArchiveSearch =
          selectedProjectPath === "all" && messageTypeFilter === "all" && !dateRange;
        if (canUseArchiveSearch) {
          try {
            const archiveHits = await searchArchiveFts(trimmedQuery, undefined, undefined, undefined, MAX_RESULTS);
            if (requestIdRef.current !== requestId) {
              return; // superseded by a newer request
            }
            if (archiveHits.length > 0) {
              const archiveResults = archiveHits.map(archiveHitToGlobalSearchResult);
              setResults(archiveResults);
              setSelectedIndex(0);
              void recordDiagnosticsEvent({
                kind: "searchExecuted",
                resultCount: archiveResults.length,
              });
              return;
            }
          } catch (archiveError) {
            // Archive search is a best-effort primary path -- any failure
            // (schema not migrated yet on an old install, etc.) falls
            // through to the always-available raw scan below rather than
            // surfacing an error for what the user experiences as one
            // search.
            console.error("Archive-backed search failed, falling back to raw scan:", archiveError);
          }
        }

        const wslExcludedDistros = userMetadata?.settings?.wsl?.excludedDistros ?? [];
        const useAllProvidersSearch = hasNonClaudeProviders || hasCustomPaths || wslEnabled;
        const searchResults = await api<GlobalSearchResult[]>(
          useAllProvidersSearch ? "search_all_providers" : "search_messages",
          useAllProvidersSearch
            ? {
                claudePath: nativeClaudePath,
                query: trimmedQuery,
                activeProviders,
                filters,
                limit: MAX_RESULTS,
                customClaudePaths: hasCustomPaths ? customClaudePaths : undefined,
                wslEnabled,
                wslProviders,
                wslExcludedDistros,
              }
            : { claudePath: nativeClaudePath, query: trimmedQuery, filters, limit: MAX_RESULTS }
        );
        if (requestIdRef.current !== requestId) {
          return; // superseded by a newer request
        }
        setResults(searchResults);
        setSelectedIndex(0);
        void recordDiagnosticsEvent({
          kind: "searchExecuted",
          resultCount: searchResults.length,
        });
      } catch (error) {
        if (requestIdRef.current !== requestId) {
          return;
        }
        console.error("Global search failed:", error);
        setResults([]);
        toast.error(t("globalSearch.searchFailed"));
      } finally {
        if (requestIdRef.current === requestId) {
          setIsSearching(false);
        }
      }
    },
    [
      claudePath,
      activeProviders,
      selectedProjectPath,
      messageTypeFilter,
      dateFilter,
      dateRangeError,
      userMetadata,
      t,
    ]
  );

  const handleInputChange = useCallback(
    (value: string) => {
      setQuery(value);

      if (debounceTimeoutRef.current) {
        clearTimeout(debounceTimeoutRef.current);
      }

      debounceTimeoutRef.current = setTimeout(() => {
        performSearch(value);
      }, 300);
    },
    [performSearch]
  );

  const clearQuery = useCallback(() => {
    setQuery("");
    setResults([]);
  }, []);

  const handleSelectResult = useCallback(
    async (result: GlobalSearchResult) => {
      try {
        const targetSession = sessions.find(
          (s) => s.session_id === result.sessionId || s.actual_session_id === result.sessionId
        );

        if (targetSession) {
          setAnalyticsCurrentView("messages");
          await selectSession(targetSession);
          if (result.uuid) {
            navigateToMessage(result.uuid);
          }
          void recordDiagnosticsEvent({ kind: "searchResultOpened" });
          onAfterSelect?.();
          return;
        }

        const { excludeSidechain } = useAppStore.getState();
        const token = ++resolveTokenRef.current;

        const resultProvider = result.provider ?? "claude";
        const rank = (project: (typeof projects)[number]): number => {
          const projectProvider = project.provider ?? "claude";
          if (projectProvider !== resultProvider) return 3;
          if (result.projectName && project.name === result.projectName) return 0;
          if (
            result.projectName &&
            (project.name.includes(result.projectName) ||
              project.actual_path?.endsWith(result.projectName))
          ) {
            return 1;
          }
          return 2;
        };
        const candidates = [...projects].sort((a, b) => rank(a) - rank(b));

        const findInProject = async (
          project: (typeof projects)[number]
        ): Promise<{ project: typeof project; session: ClaudeSession } | null> => {
          try {
            const projectProvider = project.provider ?? "claude";
            const projectSessions = await api<ClaudeSession[]>(
              projectProvider !== "claude" ? "load_provider_sessions" : "load_project_sessions",
              projectProvider !== "claude"
                ? { provider: projectProvider, projectPath: project.path, excludeSidechain }
                : { projectPath: project.path, excludeSidechain }
            );
            const session = projectSessions.find(
              (s) => s.session_id === result.sessionId || s.actual_session_id === result.sessionId
            );
            return session ? { project, session } : null;
          } catch (error) {
            console.error(`Failed to load sessions for project ${project.name}:`, error);
            return null;
          }
        };

        const BATCH_SIZE = 4;
        for (let i = 0; i < candidates.length; i += BATCH_SIZE) {
          if (token !== resolveTokenRef.current) return;
          const batch = candidates.slice(i, i + BATCH_SIZE);
          const found = (await Promise.all(batch.map(findInProject))).find(
            (hit): hit is NonNullable<typeof hit> => hit !== null
          );
          if (token !== resolveTokenRef.current) return;
          if (found) {
            setAnalyticsCurrentView("messages");
            await selectProject(found.project);
            await selectSession(found.session);
            if (result.uuid) {
              navigateToMessage(result.uuid);
            }
            void recordDiagnosticsEvent({ kind: "searchResultOpened" });
            onAfterSelect?.();
            return;
          }
        }

        clearTargetMessage();
        toast.error(t("globalSearch.sessionNotFound"));
        onAfterSelect?.();
      } catch (error) {
        clearTargetMessage();
        console.error("Failed to navigate to search result:", error);
        toast.error(t("globalSearch.navigationFailed"));
        onAfterSelect?.();
      }
    },
    [
      projects,
      sessions,
      selectProject,
      selectSession,
      navigateToMessage,
      clearTargetMessage,
      setAnalyticsCurrentView,
      onAfterSelect,
      t,
    ]
  );

  const handleKeyDown = useCallback(
    (key: "ArrowDown" | "ArrowUp" | "Enter" | "Escape") => {
      if (key === "Escape") {
        onEscape?.();
        return;
      }
      if (flattenedResults.length === 0) return;

      switch (key) {
        case "ArrowDown":
          setSelectedIndex((prev) => (prev < flattenedResults.length - 1 ? prev + 1 : 0));
          break;
        case "ArrowUp":
          setSelectedIndex((prev) => (prev > 0 ? prev - 1 : flattenedResults.length - 1));
          break;
        case "Enter":
          if (flattenedResults[selectedIndex]) {
            handleSelectResult(flattenedResults[selectedIndex]);
          }
          break;
      }
    },
    [flattenedResults, selectedIndex, handleSelectResult, onEscape]
  );

  // Re-search when filters change. `query` is intentionally omitted --
  // keystroke-driven searches go through handleInputChange's own
  // debounce below; this shares the same timer/ref so whichever fires
  // last wins, rather than racing two independent timers. Debounced
  // (not fired immediately) because a single date-range pick can emit
  // several distinct onChange values in quick succession (browsing the
  // calendar widget) -- without this, each one fired its own
  // uncached search, which read as slow/janky even with the requestId
  // guard above making the final result correct.
  useEffect(() => {
    if (debounceTimeoutRef.current) {
      clearTimeout(debounceTimeoutRef.current);
    }
    if (query.trim().length >= 2) {
      debounceTimeoutRef.current = setTimeout(() => {
        performSearch(query);
      }, 300);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [performSearch]);

  useEffect(() => {
    return () => {
      if (debounceTimeoutRef.current) {
        clearTimeout(debounceTimeoutRef.current);
      }
    };
  }, []);

  const getPreviewText = useCallback(
    (message: GlobalSearchResult): string => {
      if (!message.content) return t("globalSearch.noPreview");

      const content = message.content;
      let fullText = "";

      if (typeof content === "string") {
        fullText = content;
      } else if (Array.isArray(content)) {
        const texts: string[] = [];
        for (const item of content as ContentItem[]) {
          if (item.type === "text" && "text" in item) {
            texts.push(item.text as string);
          }
        }
        fullText = texts.join(" ");
      }

      if (!fullText) return t("globalSearch.noPreview");

      const trimmedQuery = query.trim().toLowerCase();
      if (trimmedQuery.length >= 2) {
        const lowerText = fullText.toLowerCase();
        const matchIndex = lowerText.indexOf(trimmedQuery);
        if (matchIndex !== -1) {
          const contextRadius = 60;
          const start = Math.max(0, matchIndex - contextRadius);
          const end = Math.min(fullText.length, matchIndex + trimmedQuery.length + contextRadius);
          const slice = fullText.slice(start, end);
          const prefix = start > 0 ? "..." : "";
          const suffix = end < fullText.length ? "..." : "";
          return prefix + slice + suffix;
        }
      }

      return fullText.slice(0, 150) + (fullText.length > 150 ? "..." : "");
    },
    [query, t]
  );

  // Best-effort label for spec §15's "Matched in: Assistant" / "Matched in:
  // Read" mockup. The backend (`search_in_file`) only reports THAT a
  // message matched, not WHERE -- so this re-checks the same surfaces it
  // does (content text, tool_use, tool_result, top-level toolUse/
  // toolUseResult) client-side, falling back to a role label when the
  // match can't be pinned to a specific block (e.g. it came from a
  // provider-specific field this check doesn't know about).
  //
  // Archive-FTS-backed results (`message.searchKind` set) skip this
  // heuristic entirely: the evidence-kind badge (`SearchResultsList`) already states
  // "why it matched" more precisely than this re-scan can (it comes from
  // the real backing table, not a guess), and the FTS `snippet()`'s own
  // `‹›` markers already show exactly where within the excerpt the match
  // is -- showing both would just repeat the same fact twice on one row.
  // This heuristic remains the ONLY source of that information for the
  // raw-scan fallback path, which has no kind data at all.
  const getMatchLocation = useCallback(
    (message: GlobalSearchResult): string => {
      if (message.searchKind) return "";
      const q = query.trim().toLowerCase();
      const roleLabel =
        message.type === "user"
          ? t("globalSearch.matchLocation.user")
          : t("globalSearch.matchLocation.assistant");
      if (!q) return roleLabel;

      const includesQuery = (s: string | undefined | null): boolean =>
        !!s && s.toLowerCase().includes(q);

      const content = message.content;
      if (typeof content === "string") {
        if (includesQuery(content)) return roleLabel;
      } else if (Array.isArray(content)) {
        for (const item of content as ContentItem[]) {
          if (item.type === "text" && "text" in item && includesQuery(item.text as string)) {
            return roleLabel;
          }
          if (item.type === "tool_use" && "name" in item) {
            const toolUse = item as { name: string; input?: Record<string, unknown> };
            if (includesQuery(toolUse.name) || includesQuery(JSON.stringify(toolUse.input ?? {}))) {
              return toolUse.name;
            }
          }
          if (item.type === "tool_result" && "content" in item) {
            const toolResult = item as { content?: unknown };
            const text =
              typeof toolResult.content === "string"
                ? toolResult.content
                : JSON.stringify(toolResult.content ?? "");
            if (includesQuery(text)) return t("globalSearch.matchLocation.toolResult");
          }
        }
      }

      const rawToolUse = (message as { toolUse?: Record<string, unknown> }).toolUse;
      if (rawToolUse && includesQuery(JSON.stringify(rawToolUse))) {
        const name = typeof rawToolUse.name === "string" ? rawToolUse.name : undefined;
        return name ?? t("globalSearch.matchLocation.toolResult");
      }
      const rawToolResult = (message as { toolUseResult?: Record<string, unknown> | string })
        .toolUseResult;
      if (rawToolResult) {
        const text = typeof rawToolResult === "string" ? rawToolResult : JSON.stringify(rawToolResult);
        if (includesQuery(text)) return t("globalSearch.matchLocation.toolResult");
      }

      return roleLabel;
    },
    [query, t]
  );

  const formatTimestamp = useCallback((timestamp: string): string => {
    try {
      const date = new Date(timestamp);
      return date.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return "";
    }
  }, []);

  const highlightRegex = useMemo(() => {
    const trimmed = query.trim();
    if (!trimmed) return null;
    return new RegExp(`(${trimmed.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "i");
  }, [query]);

  const highlightText = useCallback(
    (text: string): React.ReactNode => {
      if (!highlightRegex) return text;

      const parts = text.split(highlightRegex);
      return parts.map((part, index) =>
        highlightRegex.test(part) ? (
          <mark
            key={index}
            className="bg-yellow-300 dark:bg-yellow-500/40 text-foreground rounded-sm px-0.5"
          >
            {part}
          </mark>
        ) : (
          part
        )
      );
    },
    [highlightRegex]
  );

  const reset = useCallback(() => {
    resolveTokenRef.current++;
    setQuery("");
    setResults([]);
    setSelectedIndex(0);
    setSelectedProjectPath("all");
    setMessageTypeFilter("all");
    setDateFilter({});
  }, []);

  return {
    query,
    results,
    isSearching,
    selectedIndex,
    setSelectedIndex,
    messageTypeFilter,
    setMessageTypeFilter,
    selectedProjectPath,
    setSelectedProjectPath,
    dateFilter,
    setDateFilter,
    dateRangeError,
    groupedResults,
    flattenedResults,
    handleInputChange,
    clearQuery,
    handleSelectResult,
    handleKeyDown,
    getSessionName,
    getPreviewText,
    getMatchLocation,
    formatTimestamp,
    highlightText,
    reset,
  };
}
