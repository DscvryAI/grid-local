import React from "react";
import { useAppStore } from "@/store/useAppStore";
import { useArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ClaudeSession } from "@/types";
import { InsightsTabs } from "./InsightsTabs";
import { QuestionsTab } from "./tabs/QuestionsTab";
import { UsageTab } from "./tabs/UsageTab";
import { ToolsTab } from "./tabs/ToolsTab";
import { ProblemsTab } from "./tabs/ProblemsTab";
import { AgentsTab } from "./tabs/AgentsTab";

interface InsightsSurfaceProps {
  isViewingGlobalStats: boolean;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Insights surface's five-tab structure: Questions (default) | Usage |
 * Tools | Agents | Problems. Usage reuses today's `AnalyticsDashboard`
 * verbatim plus (project-scoped) the per-session token/cost grid folded
 * in from the old standalone "Token Stats" nav view -- every other tab
 * consumes the `archive_db::insights` commands.
 *
 * Tools respects the current project selection (project-scoped when a
 * project is selected, global otherwise, matching Usage's own existing
 * behavior) since its underlying data (`most_used_tools`/`skills`/
 * `subagents`) already exists per-project. Agents/Problems do too: their
 * queries take an optional `project_key` filter, so they scope to the
 * selected project's own `archive_db` key when one is chosen, global
 * otherwise -- the same convention as Usage/Tools.
 */
export const InsightsSurface: React.FC<InsightsSurfaceProps> = ({
  isViewingGlobalStats,
  onSessionSelect,
}) => {
  const insightsTab = useAppStore((s) => s.insightsTab);
  const setInsightsTab = useAppStore((s) => s.setInsightsTab);
  // Computed once here, not per-tab -- Agents/Problems are both entirely
  // `archive_db`-backed; Usage/Tools mix in raw per-provider scanning and
  // deliberately don't consume this (see `useArchiveIndexHealth`'s doc
  // comment).
  const archiveHealth = useArchiveIndexHealth();

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <InsightsTabs activeTab={insightsTab} onTabChange={setInsightsTab} />
      <div className="flex-1 overflow-hidden">
        {insightsTab === "questions" ? (
          <QuestionsTab health={archiveHealth} />
        ) : insightsTab === "usage" ? (
          <UsageTab isViewingGlobalStats={isViewingGlobalStats} onSessionSelect={onSessionSelect} />
        ) : insightsTab === "tools" ? (
          <ToolsTab isViewingGlobalStats={isViewingGlobalStats} />
        ) : insightsTab === "agents" ? (
          <AgentsTab health={archiveHealth} onSessionSelect={onSessionSelect} />
        ) : (
          <ProblemsTab health={archiveHealth} onSessionSelect={onSessionSelect} />
        )}
      </div>
    </div>
  );
};
