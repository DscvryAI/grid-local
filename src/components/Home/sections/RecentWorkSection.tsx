import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { History as HistoryIcon } from "lucide-react";
import { fetchHistorySessions } from "@/services/historyApi";
import { formatRelativeTime } from "@/utils/time";
import { toClaudeSessionFromHistoryItem } from "@/utils/historySessionItem";
import { SessionItem } from "../../SessionItem/SessionItem";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ClaudeSession, HistorySessionItem } from "@/types";

interface RecentWorkSectionProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

const RECENT_WORK_LIMIT = 5;

/**
 * Spec §11: a simple chronological slice of History's own cross-provider
 * session list (spec's own instruction: "a simple chronological list").
 * Reuses `SessionItem` (the same row component History/Search render)
 * rather than a bespoke card, so a session looks identical wherever it
 * appears. History's own backend merges Claude (via `archive_db`) with
 * every other provider (raw file scan, unaffected by `archive_db`'s own
 * state) -- so `health` only gates the case that's actually ambiguous: a
 * fully empty result while the index isn't confirmed ready, which could
 * mean "no history anywhere" or "Claude's share hasn't been indexed yet."
 * A non-empty result is shown as-is regardless of `health`.
 */
export const RecentWorkSection: React.FC<RecentWorkSectionProps> = ({ health, onSessionSelect }) => {
  const { t } = useTranslation();
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);
  const [items, setItems] = useState<HistorySessionItem[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchHistorySessions({ limit: RECENT_WORK_LIMIT })
      .then((page) => {
        if (!cancelled) setItems(page.items);
      })
      .catch((err) => {
        console.error("Failed to load recent work:", err);
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : String(err));
          setItems([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (items === null) {
    return <div className="h-32 animate-pulse rounded-lg bg-muted/30" />;
  }
  if (loadError) {
    return (
      <div className="px-1 text-sm text-destructive" role="alert">
        {loadError}
      </div>
    );
  }
  if (items.length === 0) {
    if (health.state !== "ready") {
      return <ArchiveHealthEmptyState health={health} />;
    }
    return null;
  }

  return (
    <section>
      <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold text-foreground">
        <HistoryIcon className="h-4 w-4 text-muted-foreground" />
        {t("home.recentWork.title")}
      </h2>
      <div className="divide-y divide-border/50 rounded-md border border-border/50">
        {items.map((item) => (
          <SessionItem
            key={item.session_id}
            session={toClaudeSessionFromHistoryItem(item)}
            isSelected={false}
            onSelect={() => onSessionSelect(toClaudeSessionFromHistoryItem(item))}
            formatTimeAgo={formatRelativeTime}
          />
        ))}
      </div>
      <button
        type="button"
        onClick={() => setPrimarySurface("history")}
        className="mt-2 text-xs text-muted-foreground hover:text-foreground hover:underline"
      >
        {t("home.recentWork.viewAll")}
      </button>
    </section>
  );
};
