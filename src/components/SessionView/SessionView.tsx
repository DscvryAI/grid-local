import React, { useEffect, useMemo } from "react";
import { MessageViewer } from "@/components/MessageViewer/MessageViewer";
import type { MessageViewerProps } from "@/components/MessageViewer/types";
import { MessageNavigator } from "@/components/MessageNavigator";
import { useAppStore } from "@/store/useAppStore";
import { groupAgentTasks } from "@/components/MessageViewer/helpers/agentTaskHelpers";
import { SessionIntelligenceHeader } from "./SessionIntelligenceHeader";
import { SessionTabs } from "./SessionTabs";
import { OverviewTab } from "./tabs/OverviewTab";
import { ToolsTab } from "./tabs/ToolsTab";
import { FilesTab } from "./tabs/FilesTab";
import { AgentsTab } from "./tabs/AgentsTab";
import { calculateSessionIntelligence, deriveSessionDecisionBrief } from "./helpers/sessionIntelligence";

interface SessionViewProps extends MessageViewerProps {
  navigatorWidth: number;
  isNavigatorResizing: boolean;
  onNavigatorResizeStart: (e: React.MouseEvent<HTMLElement>) => void;
  isNavigatorOpen: boolean;
  onToggleNavigator: () => void;
}

/**
 * Wraps the existing, unchanged `MessageViewer` with spec §14's
 * intelligence header and Overview | Conversation | Tools | Files |
 * Agents tab strip. `Conversation` renders `MessageViewer` exactly as it
 * always has -- every other tab is new, client-side-only content computed
 * from the session already loaded into `messages`.
 */
export const SessionView: React.FC<SessionViewProps> = ({
  navigatorWidth,
  isNavigatorResizing,
  onNavigatorResizeStart,
  isNavigatorOpen,
  onToggleNavigator,
  ...messageViewerProps
}) => {
  const { messages, selectedSession } = messageViewerProps;
  const sessionTab = useAppStore((s) => s.sessionTab);
  const setSessionTab = useAppStore((s) => s.setSessionTab);

  // A freshly opened session always starts on Conversation -- landing on
  // whatever tab a PREVIOUS session was left on would be confusing, and
  // Conversation is the one guaranteed to render something useful.
  useEffect(() => {
    setSessionTab("conversation");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSession?.session_id]);

  const agentTaskGroups = useMemo(
    () => Array.from(groupAgentTasks(messages).values()),
    [messages]
  );
  const agentCount = useMemo(
    () => agentTaskGroups.reduce((sum, group) => sum + group.tasks.length, 0),
    [agentTaskGroups]
  );
  const intelligence = useMemo(
    () => calculateSessionIntelligence(messages, agentCount),
    [messages, agentCount]
  );
  const decisionBrief = useMemo(
    () => deriveSessionDecisionBrief(messages, intelligence.toolOccurrences, intelligence.fileEvents),
    [messages, intelligence.toolOccurrences, intelligence.fileEvents]
  );

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {selectedSession && (
        <>
          <SessionIntelligenceHeader session={selectedSession} intelligence={intelligence} />
          <SessionTabs
            activeTab={sessionTab}
            onTabChange={setSessionTab}
            toolCallCount={intelligence.toolCallCount}
            fileCount={intelligence.fileCount}
            agentCount={intelligence.agentCount}
          />
        </>
      )}

      <div className="flex flex-1 overflow-hidden">
        {sessionTab === "conversation" ? (
          <>
            <div className="min-w-0 flex-1 overflow-x-hidden">
              <MessageViewer {...messageViewerProps} />
            </div>
            <div className="hidden md:block">
              <MessageNavigator
                messages={messages}
                width={navigatorWidth}
                isResizing={isNavigatorResizing}
                onResizeStart={onNavigatorResizeStart}
                isCollapsed={!isNavigatorOpen}
                onToggleCollapse={onToggleNavigator}
                asideId="message-navigator"
              />
            </div>
          </>
        ) : sessionTab === "overview" ? (
          selectedSession && (
            <OverviewTab
              intelligence={intelligence}
              decisionBrief={decisionBrief}
              session={selectedSession}
              onNavigateToTab={setSessionTab}
            />
          )
        ) : sessionTab === "tools" ? (
          <ToolsTab toolUsage={intelligence.toolUsage} />
        ) : sessionTab === "files" ? (
          <FilesTab fileEvents={intelligence.fileEvents} verification={decisionBrief.verification} />
        ) : (
          selectedSession && (
            <AgentsTab sessionId={selectedSession.session_id} agentTaskGroups={agentTaskGroups} />
          )
        )}
      </div>
    </div>
  );
};
