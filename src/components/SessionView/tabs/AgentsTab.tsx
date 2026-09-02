import React from "react";
import { useTranslation } from "react-i18next";
import { AgentTaskGroupRenderer } from "@/components/toolResultRenderer";
import { AgentRunTreeView } from "@/components/AgentRunTree/AgentRunTreeView";
import type { AgentTaskGroupResult } from "@/components/MessageViewer/types";

interface AgentsTabProps {
  sessionId: string;
  agentTaskGroups: AgentTaskGroupResult[];
}

/**
 * Flat list of every agent-task group in the session (spec §14's Agents
 * tab), reusing the same `AgentTaskGroupRenderer` already used inline in
 * the Conversation tab. This message-derived list stays the PRIMARY
 * content -- it has real task descriptions/prompts/output files, none of
 * which the backend's `agent_run` table captures today. The archive-backed
 * `AgentRunTreeView` is shown as a secondary, clearly-labeled section
 * below it -- real aggregate counts from Grid's own archive, not a
 * replacement for the richer per-task detail above.
 */
export const AgentsTab: React.FC<AgentsTabProps> = ({ sessionId, agentTaskGroups }) => {
  const { t } = useTranslation();

  return (
    <div className="flex-1 space-y-4 overflow-y-auto px-4 py-3">
      {agentTaskGroups.length === 0 ? (
        <div className="flex items-center justify-center px-4 py-8 text-center text-sm text-muted-foreground">
          {t("session.tabs.agentsEmpty")}
        </div>
      ) : (
        <div className="space-y-2">
          {agentTaskGroups.map((group, index) => (
            <AgentTaskGroupRenderer
              key={group.tasks[0]?.agentId ?? `agent-group-${index}`}
              tasks={group.tasks}
            />
          ))}
        </div>
      )}

      <section className="rounded-lg border border-border/60 px-2 py-2">
        <h3 className="mb-1 px-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
          {t("session.tabs.agentsFromArchive")}
        </h3>
        <AgentRunTreeView sessionId={sessionId} />
      </section>
    </div>
  );
};
