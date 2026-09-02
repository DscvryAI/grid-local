import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Clock } from "lucide-react";
import { getLocale } from "@/utils/time";
import i18n from "@/i18n";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import { SessionWindowDrillDownDialog } from "../SessionWindowDrillDownDialog";
import type { ClaudeSession, SinceLastVisitSummary } from "@/types";

interface SinceLastVisitSectionProps {
  summary: SinceLastVisitSummary | null;
  isLoading: boolean;
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/** Within a week: the weekday name ("Friday"); otherwise a short date. */
function formatSinceLabel(since: string): string {
  const date = new Date(since);
  if (Number.isNaN(date.getTime())) return since;
  const diffDays = Math.floor((Date.now() - date.getTime()) / (1000 * 60 * 60 * 24));
  const locale = getLocale(i18n.language || "en");
  if (diffDays < 7) {
    return date.toLocaleDateString(locale, { weekday: "long" });
  }
  return date.toLocaleDateString(locale, { month: "short", day: "numeric" });
}

/**
 * "Since <day>" -- session/project counts plus up to 3 primary
 * projects for immediate orientation. Renders nothing once there's
 * genuinely nothing to report while the archive index is confirmed healthy
 * (`health.state === "ready"`) -- an empty section here would just be noise
 * on someone's very first Home visit. If the index ISN'T confirmed healthy
 * (still building, failed, or never built -- e.g. the mandatory first index
 * was cancelled), that's shown explicitly instead of collapsing to the same
 * silent blank.
 */
export const SinceLastVisitSection: React.FC<SinceLastVisitSectionProps> = ({
  summary,
  isLoading,
  health,
  onSessionSelect,
}) => {
  const { t } = useTranslation();
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);
  const setInsightsTab = useAppStore((s) => s.setInsightsTab);
  const [showDrillDown, setShowDrillDown] = useState(false);

  if (isLoading) {
    return <div className="h-24 animate-pulse rounded-lg bg-muted/30" />;
  }
  if (health.state !== "ready") {
    return <ArchiveHealthEmptyState health={health} />;
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
        <Clock className="h-4 w-4 text-muted-foreground" />
        {t("home.sinceLastVisit.title", { since: formatSinceLabel(summary.since) })}
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
            {t("home.sinceLastVisit.sessions", { count: summary.session_count })}
          </span>
        </button>
        <div>
          <span className="text-2xl font-semibold text-foreground">
            {summary.project_count}
          </span>{" "}
          <span className="text-sm text-muted-foreground">
            {t("home.sinceLastVisit.projects", { count: summary.project_count })}
          </span>
        </div>
        {summary.tool_call_count > 0 && (
          <div>
            <span className="text-2xl font-semibold text-foreground">
              {summary.tool_call_count}
            </span>{" "}
            <span className="text-sm text-muted-foreground">
              {t("home.sinceLastVisit.toolCalls", { count: summary.tool_call_count })}
            </span>
          </div>
        )}
        {summary.agent_run_count > 0 && (
          <button type="button" onClick={openAgentActivity} className="text-left hover:underline">
            <span className="text-2xl font-semibold text-foreground">
              {summary.agent_run_count}
            </span>{" "}
            <span className="text-sm text-muted-foreground">
              {t("home.sinceLastVisit.agentRuns", { count: summary.agent_run_count })}
            </span>
          </button>
        )}
      </div>
      {summary.primary_projects.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1.5">
          {summary.primary_projects.map((name) => (
            <span
              key={name}
              className="rounded-full bg-muted px-2.5 py-0.5 text-xs text-muted-foreground"
            >
              {name}
            </span>
          ))}
        </div>
      )}
      <SessionWindowDrillDownDialog
        isOpen={showDrillDown}
        onClose={() => setShowDrillDown(false)}
        title={t("home.sinceLastVisit.title", { since: formatSinceLabel(summary.since) })}
        start={summary.since}
        end={new Date().toISOString()}
        onSessionSelect={onSessionSelect}
      />
    </section>
  );
};
