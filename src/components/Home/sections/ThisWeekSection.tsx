import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Calendar } from "lucide-react";
import { fetchThisWeekSummary } from "@/services/insightsApi";
import { formatNumber } from "@/utils/formatters";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import { SessionWindowDrillDownDialog } from "../SessionWindowDrillDownDialog";
import type { ClaudeSession, ThisWeekSummary } from "@/types";

interface ThisWeekSectionProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * "This week" is a rolling 7-day window ending now, matching
 * `historyDateBuckets.ts`'s own "earlier this week ⩽ 7 days" convention --
 * not a Monday-Sunday calendar week, so it stays consistent with how
 * History already buckets the same label.
 *
 * `health` distinguishes "genuinely nothing this week" (quietly renders
 * nothing, `health.state === "ready"`) from the archive index still
 * building/failed/never-built, which renders explicitly instead.
 */
function thisWeekWindow(): { start: string; end: string } {
  const end = new Date();
  const start = new Date(end.getTime() - 7 * 24 * 60 * 60 * 1000);
  return { start: start.toISOString(), end: end.toISOString() };
}

export const ThisWeekSection: React.FC<ThisWeekSectionProps> = ({ health, onSessionSelect }) => {
  const { t } = useTranslation();
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);
  const setInsightsTab = useAppStore((s) => s.setInsightsTab);
  const [summary, setSummary] = useState<ThisWeekSummary | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [showDrillDown, setShowDrillDown] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const { start, end } = thisWeekWindow();
    setIsLoading(true);
    fetchThisWeekSummary(start, end)
      .then((result) => {
        if (!cancelled) setSummary(result);
      })
      .catch((err) => {
        if (!cancelled) setLoadError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (isLoading) {
    return <div className="h-24 animate-pulse rounded-lg bg-muted/30" />;
  }
  if (health.state !== "ready") {
    return <ArchiveHealthEmptyState health={health} />;
  }
  if (loadError) {
    return (
      <div className="px-1 text-sm text-destructive" role="alert">
        {loadError}
      </div>
    );
  }
  if (!summary || summary.session_count === 0) {
    return null;
  }

  const openAgentActivity = () => {
    setPrimarySurface("insights");
    setInsightsTab("agents");
  };

  return (
    <section>
      <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold text-foreground">
        <Calendar className="h-4 w-4 text-muted-foreground" />
        {t("home.thisWeek.title")}
      </h2>
      <div className="flex flex-wrap items-baseline gap-x-6 gap-y-2">
        <button
          type="button"
          onClick={() => setShowDrillDown(true)}
          className="text-left hover:underline"
        >
          <span className="text-2xl font-semibold text-foreground">
            {summary.session_count}
          </span>{" "}
          <span className="text-sm text-muted-foreground">
            {t("home.thisWeek.sessions", { count: summary.session_count })}
          </span>
        </button>
        <div>
          <span className="text-2xl font-semibold text-foreground">
            {summary.project_count}
          </span>{" "}
          <span className="text-sm text-muted-foreground">
            {t("home.thisWeek.projects", { count: summary.project_count })}
          </span>
        </div>
        <div>
          <span className="text-2xl font-semibold text-foreground">
            {formatNumber(summary.total_tokens)}
          </span>{" "}
          <span className="text-sm text-muted-foreground">{t("home.thisWeek.tokens")}</span>
        </div>
        {summary.agent_run_count > 0 && (
          <button type="button" onClick={openAgentActivity} className="text-left hover:underline">
            <span className="text-2xl font-semibold text-foreground">
              {summary.agent_run_count}
            </span>{" "}
            <span className="text-sm text-muted-foreground">
              {t("home.thisWeek.agentRuns", { count: summary.agent_run_count })}
            </span>
          </button>
        )}
      </div>
      {summary.provider_breakdown.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1">
          {summary.provider_breakdown.map((provider) => (
            <span key={provider.provider_key} className="text-xs text-muted-foreground">
              {provider.display_name}{" "}
              <span className="font-medium text-foreground">
                {Math.round((provider.total_tokens / summary.total_tokens) * 100)}%
              </span>
            </span>
          ))}
        </div>
      )}
      {summary.peak_agents_in_session > 0 && (
        <p className="mt-2 text-xs text-muted-foreground">
          {t("home.thisWeek.peakAgents", { count: summary.peak_agents_in_session })}
        </p>
      )}
      <SessionWindowDrillDownDialog
        isOpen={showDrillDown}
        onClose={() => setShowDrillDown(false)}
        title={t("home.thisWeek.title")}
        start={summary.window_start}
        end={summary.window_end}
        onSessionSelect={onSessionSelect}
      />
    </section>
  );
};
