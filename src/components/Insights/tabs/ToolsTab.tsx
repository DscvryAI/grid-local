import React from "react";
import { useTranslation } from "react-i18next";
import { ToolIcon } from "@/components/ToolIcon";
import { getToolDisplayName } from "@/components/AnalyticsDashboard/utils/toolNames";
import { useAppStore } from "@/store/useAppStore";
import type { ToolUsageStats } from "@/types";

interface ToolsTabProps {
  isViewingGlobalStats: boolean;
}

const ToolUsageList: React.FC<{ title: string; stats: ToolUsageStats[] }> = ({ title, stats }) => {
  const { t } = useTranslation();
  if (stats.length === 0) return null;
  const maxCount = stats[0]?.usage_count ?? 1;

  return (
    <section>
      <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      <div className="space-y-1">
        {stats.map((stat) => (
          <div key={stat.tool_name} className="flex items-center gap-3 rounded-md px-2 py-1.5">
            <ToolIcon toolName={stat.tool_name} colored size="default" />
            <span className="w-40 shrink-0 truncate text-sm text-foreground">
              {getToolDisplayName(stat.tool_name, t)}
            </span>
            <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary/60"
                style={{ width: `${Math.max((stat.usage_count / maxCount) * 100, 4)}%` }}
              />
            </div>
            <span className="w-16 shrink-0 text-right text-2xs text-muted-foreground">
              {/* success_rate is already 0-100 (see build_tool_usage_stats
                  in stats.rs) -- NOT a 0-1 fraction, don't multiply by 100 again. */}
              {t("insights.tools.successRate", { rate: Math.round(stat.success_rate) })}
            </span>
            <span className="w-10 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
              {stat.usage_count}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
};

/**
 * Tool-usage leaderboard (spec §16's Tools tab) -- reuses
 * `GlobalStatsSummary`/`ProjectStatsSummary.most_used_tools/skills/
 * subagents`, the SAME data `AnalyticsDashboard`'s existing
 * `ToolUsageChart` already fetches, just rendered as ranked lists instead
 * of bars (spec §28: "no chart unless materially faster than a number/
 * table"). No new backend query needed. Respects the current project
 * selection the same way the Usage tab does (project-scoped when a
 * project is selected, global otherwise).
 */
export const ToolsTab: React.FC<ToolsTabProps> = ({ isViewingGlobalStats }) => {
  const { t } = useTranslation();
  const globalSummary = useAppStore((s) => s.globalSummary);
  const projectSummary = useAppStore((s) => s.analytics.projectSummary);

  const summary = isViewingGlobalStats ? globalSummary : projectSummary;

  if (!summary) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("insights.tools.noData")}
      </div>
    );
  }

  const hasAny =
    summary.most_used_tools.length > 0 ||
    summary.most_used_skills.length > 0 ||
    summary.most_used_subagents.length > 0;

  if (!hasAny) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("insights.tools.empty")}
      </div>
    );
  }

  return (
    <div className="flex-1 space-y-6 overflow-y-auto px-4 py-3">
      <ToolUsageList title={t("insights.tools.mostUsedTools")} stats={summary.most_used_tools} />
      <ToolUsageList title={t("insights.tools.mostUsedSkills")} stats={summary.most_used_skills} />
      <ToolUsageList title={t("insights.tools.mostUsedSubagents")} stats={summary.most_used_subagents} />
    </div>
  );
};
