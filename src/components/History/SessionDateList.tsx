import React, { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { useHistorySessions } from "../../hooks/useHistorySessions";
import {
  bucketSessionsByDate,
  SESSION_DATE_BUCKET_ORDER,
  type SessionDateBucket,
} from "../../utils/historyDateBuckets";
import { formatRelativeTime } from "../../utils/time";
import { SessionItem } from "../SessionItem/SessionItem";
import { HistoryFilterBar } from "./HistoryFilterBar";
import { toClaudeSessionFromHistoryItem } from "../../utils/historySessionItem";
import { useArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ClaudeSession, HistorySessionItem } from "../../types";

interface SessionDateListProps {
  selectedSession: ClaudeSession | null;
  onSessionSelect: (session: ClaudeSession) => void;
}

const BUCKET_LABEL_KEYS: Record<SessionDateBucket, string> = {
  today: "history.bucket.today",
  yesterday: "history.bucket.yesterday",
  earlierThisWeek: "history.bucket.earlierThisWeek",
  older: "history.bucket.older",
};

export const SessionDateList: React.FC<SessionDateListProps> = ({
  selectedSession,
  onSessionSelect,
}) => {
  const { t } = useTranslation();
  // History's Claude-provider share is archive_db-backed
  // (query_claude_history_sessions), so a not-yet-indexed archive would
  // otherwise make Claude sessions silently vanish from this merged list
  // -- indistinguishable from "you genuinely have no Claude history."
  // RecentWorkSection.tsx (Home's own smaller teaser, same underlying
  // fetchHistorySessions call) gates on this the same way.
  const archiveHealth = useArchiveIndexHealth();
  const {
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
  } = useHistorySessions();

  const buckets = useMemo(
    () => bucketSessionsByDate(sessions, (item) => item.recency_time),
    [sessions]
  );

  const handleSelect = useCallback(
    (item: HistorySessionItem) => {
      onSessionSelect(toClaudeSessionFromHistoryItem(item));
    },
    [onSessionSelect]
  );

  const filterBar = (
    <HistoryFilterBar
      availableProjects={availableProjects}
      availableProviders={availableProviders}
      availableModels={availableModels}
      projectKeys={projectKeys}
      onProjectKeysChange={setProjectKeys}
      dateFilter={dateFilter}
      onDateFilterChange={setDateFilter}
      models={models}
      onModelsChange={setModels}
    />
  );

  // Any fresh (non-append) fetch -- first load OR a filter change --
  // clears visible results immediately rather than leaving the previous
  // filter's stale list on screen while the new one loads. Showing the
  // old results during a refetch reads as "the filter didn't do
  // anything" (reported directly: fast filters were mistaken for broken
  // ones because nothing visibly changed until the new page landed).
  if (isLoading) {
    return (
      <div className="flex h-full flex-col overflow-hidden">
        {filterBar}
        <div className="flex flex-1 items-center justify-center">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex h-full flex-col overflow-hidden">
        {filterBar}
        <div className="flex flex-1 items-center justify-center px-4 text-center text-sm text-destructive">
          {error}
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {filterBar}
      {customClaudeDirsOmitted && (
        <div className="border-b bg-muted/50 px-4 py-2 text-xs text-muted-foreground">
          {t("history.customDirsOmitted")}
        </div>
      )}
      {sessions.length === 0 ? (
        archiveHealth.state !== "ready" ? (
          <div className="flex flex-1 items-center justify-center px-4">
            <ArchiveHealthEmptyState health={archiveHealth} />
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center px-4 text-center text-sm text-muted-foreground">
            {t("history.empty")}
          </div>
        )
      ) : (
        <div className="flex-1 space-y-6 overflow-y-auto px-4 py-3">
          {SESSION_DATE_BUCKET_ORDER.filter((bucket) => buckets[bucket].length > 0).map(
            (bucket) => (
              <section key={bucket}>
                <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {t(BUCKET_LABEL_KEYS[bucket])}
                </h3>
                <div className="space-y-1">
                  {buckets[bucket].map((item) => (
                    <SessionItem
                      key={item.session_id}
                      session={toClaudeSessionFromHistoryItem(item)}
                      isSelected={selectedSession?.session_id === item.session_id}
                      onSelect={() => handleSelect(item)}
                      formatTimeAgo={formatRelativeTime}
                    />
                  ))}
                </div>
              </section>
            )
          )}
          {hasMore && (
            <div className="flex justify-center py-2">
              <button
                type="button"
                onClick={loadMore}
                disabled={isLoadingMore}
                className="text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
              >
                {isLoadingMore
                  ? t("common.loading")
                  : t("history.loadMore", { count: totalCount - sessions.length })}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
