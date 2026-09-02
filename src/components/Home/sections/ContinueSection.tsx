import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { PlayCircle } from "lucide-react";
import { fetchHistorySessions } from "@/services/historyApi";
import { formatRelativeTime } from "@/utils/time";
import { toClaudeSessionFromHistoryItem } from "@/utils/historySessionItem";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ClaudeSession, HistorySessionItem } from "@/types";

interface ContinueSectionProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * "One dominant continuation item" -- the single most recently active
 * session across every provider, with a one-click way back in.
 * Deliberately does NOT synthesize an "outcome" sentence (e.g. "Tests
 * failed after auth middleware changes. 3 files touched.") -- no backend
 * query today derives pass/fail or a files-touched count as a single
 * verified fact, and fabricating one would violate this app's
 * evidence-linked design principle (every insight must trace to real
 * source data). Shows only real, already-available facts instead:
 * project, relative last-active time, and the session's own summary
 * when one exists.
 */
export const ContinueSection: React.FC<ContinueSectionProps> = ({
  health,
  onSessionSelect,
}) => {
  const { t } = useTranslation();
  const [item, setItem] = useState<HistorySessionItem | null | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchHistorySessions({ limit: 1 })
      .then((page) => {
        if (!cancelled) setItem(page.items[0] ?? null);
      })
      .catch((err) => {
        console.error("Failed to load the most recent session:", err);
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : String(err));
          setItem(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (item === undefined) {
    return <div className="h-20 animate-pulse rounded-lg bg-muted/30" />;
  }
  if (loadError) {
    return (
      <div className="px-1 text-sm text-destructive" role="alert">
        {loadError}
      </div>
    );
  }
  if (!item) {
    if (health.state !== "ready") {
      return <ArchiveHealthEmptyState health={health} />;
    }
    return null;
  }

  const session = toClaudeSessionFromHistoryItem(item);
  const lastActive = item.last_message_time
    ? formatRelativeTime(item.last_message_time)
    : undefined;

  return (
    <section>
      <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold text-foreground">
        <PlayCircle className="h-4 w-4 text-muted-foreground" />
        {t("home.continue.title")}
      </h2>
      <button
        type="button"
        onClick={() => onSessionSelect(session)}
        className="flex w-full flex-col items-start gap-1 rounded-md border border-border/50 px-3 py-2.5 text-left hover:bg-muted/50"
      >
        <span className="w-full truncate text-sm font-medium text-foreground">
          {item.summary || item.project_name}
        </span>
        <span className="text-xs text-muted-foreground">
          {lastActive
            ? t("home.continue.lastActive", { time: lastActive })
            : item.project_name}
        </span>
      </button>
    </section>
  );
};
