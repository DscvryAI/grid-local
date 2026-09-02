import React from "react";
import { useTranslation } from "react-i18next";
import { ToolIcon } from "@/components/ToolIcon";
import { getToolDisplayName } from "@/components/AnalyticsDashboard/utils/toolNames";
import type { ToolUsageSummary } from "../helpers/sessionIntelligence";

interface ToolsTabProps {
  toolUsage: ToolUsageSummary[];
}

/** Tool-name leaderboard for the session's Tools tab (spec §14). */
export const ToolsTab: React.FC<ToolsTabProps> = ({ toolUsage }) => {
  const { t } = useTranslation();

  if (toolUsage.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("session.tabs.toolsEmpty")}
      </div>
    );
  }

  const maxCount = toolUsage[0]?.count ?? 1;

  return (
    <div className="flex-1 space-y-1 overflow-y-auto px-4 py-3">
      {toolUsage.map(({ name, count }) => (
        <div key={name} className="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-muted/50">
          <ToolIcon toolName={name} colored size="default" />
          <span className="w-40 shrink-0 truncate text-sm text-foreground">
            {getToolDisplayName(name, t)}
          </span>
          <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-primary/60"
              style={{ width: `${Math.max((count / maxCount) * 100, 4)}%` }}
            />
          </div>
          <span className="w-10 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
            {count}
          </span>
        </div>
      ))}
    </div>
  );
};
