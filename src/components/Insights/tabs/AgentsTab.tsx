import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, ChevronRight, Loader2 } from "lucide-react";
import { fetchLargeAgentRuns } from "@/services/insightsApi";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import { formatDateCompact } from "@/utils/time";
import { AgentRunTreeView } from "@/components/AgentRunTree/AgentRunTreeView";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveEmptyState, ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import { isProviderSupportedByArchiveIndex } from "@/utils/archiveSupport";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ClaudeSession, LargeAgentRunCard } from "@/types";
import { toClaudeSessionStub } from "../helpers/toClaudeSessionStub";

interface AgentsTabProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Sessions with the most subagent launches, backed by `get_large_agent_runs`.
 * Each row expands in place to the session's own agent-run tree
 * (`AgentRunTreeView`) via a dedicated chevron, kept separate from the
 * row's own click (which opens the session) so both actions stay
 * unambiguous. Scoped to the current project (via its `archive_db`
 * `project_key`) when one is selected, global otherwise -- same
 * convention as Problems/Usage/Tools. An empty result distinguishes a
 * selected project whose provider `archive_db` doesn't ingest, the
 * archive index itself not being ready, and a genuine "no large agent
 * runs" result.
 */
export const AgentsTab: React.FC<AgentsTabProps> = ({ health, onSessionSelect }) => {
  const { t } = useTranslation();
  const selectedProject = useAppStore((s) => s.selectedProject);
  const projectKey = selectedProject?.path;
  const [runs, setRuns] = useState<LargeAgentRunCard[] | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [expandedSessionId, setExpandedSessionId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setLoadError(null);
    fetchLargeAgentRuns(projectKey)
      .then((results) => {
        if (!cancelled) setRuns(results);
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
  }, [projectKey]);

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (loadError) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
        {loadError}
      </div>
    );
  }

  if (!runs || runs.length === 0) {
    if (selectedProject && !isProviderSupportedByArchiveIndex(selectedProject.provider)) {
      return (
        <div className="flex h-full items-center justify-center">
          <ArchiveEmptyState reason="unsupported-provider" provider={selectedProject.provider} />
        </div>
      );
    }
    if (health.state !== "ready") {
      return (
        <div className="flex h-full items-center justify-center">
          <ArchiveHealthEmptyState health={health} />
        </div>
      );
    }
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("insights.agents.empty")}
      </div>
    );
  }

  return (
    <div className="flex-1 space-y-1 overflow-y-auto px-4 py-3">
      {runs.map((card) => {
        const isExpanded = expandedSessionId === card.session_id;
        return (
          <div key={card.session_id} className="rounded-md hover:bg-muted/50">
            <div className="flex w-full items-center gap-2 px-2 py-2">
              <button
                type="button"
                onClick={() =>
                  setExpandedSessionId(isExpanded ? null : card.session_id)
                }
                aria-label={t("insights.agents.toggleTree")}
                className="shrink-0 rounded p-0.5 hover:bg-muted"
              >
                <ChevronRight
                  className={cn("h-3.5 w-3.5 text-muted-foreground transition-transform", isExpanded && "rotate-90")}
                />
              </button>
              <button
                type="button"
                onClick={() => {
                  // Records both an agent-run open and the first evidence
                  // drill-down.
                  void recordDiagnosticsEvent({ kind: "agentRunOpened" });
                  void recordDiagnosticsEvent({ kind: "evidenceDrilldownOpened" });
                  onSessionSelect(
                    toClaudeSessionStub(card.session_id, card.project_name, card.session_summary)
                  );
                }}
                className="flex flex-1 items-center gap-3 text-left"
              >
                <Bot className="h-4 w-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm text-foreground">
                    {card.session_summary || card.project_name}
                  </div>
                  <div className="truncate text-2xs text-muted-foreground">{card.project_name}</div>
                </div>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {t("insights.agents.subagentCount", { count: card.subagent_count })}
                </span>
                {card.session_started_at && (
                  <span className="w-24 shrink-0 text-right text-2xs text-muted-foreground">
                    {formatDateCompact(card.session_started_at)}
                  </span>
                )}
              </button>
            </div>
            {isExpanded && (
              <div className="border-t border-border/50 py-2">
                <AgentRunTreeView
                  sessionId={card.session_id}
                  onOpenTranscript={(childSessionId) => {
                    void recordDiagnosticsEvent({ kind: "agentRunOpened" });
                    onSessionSelect(
                      toClaudeSessionStub(childSessionId, t("insights.agents.detail.subagentTranscript"))
                    );
                  }}
                />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};
