import React, { useEffect } from "react";
import { OverlayScrollbarsComponent } from "overlayscrollbars-react";
import { AnalyticsDashboard } from "@/components/AnalyticsDashboard";
import { TokenStatsViewer } from "@/components/TokenStatsViewer";
import { useAppStore } from "@/store/useAppStore";
import type { ClaudeSession, SessionTokenStats } from "@/types";

interface UsageTabProps {
  isViewingGlobalStats: boolean;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Usage tab: `AnalyticsDashboard` verbatim, plus -- when a project is
 * selected -- the per-session token/cost grid that used to live behind
 * the standalone "Token Stats" nav button (removed to keep the top nav
 * to its intended small, fixed set of items). `TokenStatsViewer` is
 * reused as-is with no `sessionStats` prop, so only its project-stats
 * half renders; the single-session half moved to Session View's own
 * Overview tab instead (`OverviewTab.tsx`'s usage section), since that's
 * about one session, not a project.
 */
export const UsageTab: React.FC<UsageTabProps> = ({ isViewingGlobalStats, onSessionSelect }) => {
  const selectedProject = useAppStore((s) => s.selectedProject);
  const sessions = useAppStore((s) => s.sessions);
  const isLoadingTokenStats = useAppStore((s) => s.isLoadingTokenStats);
  const projectTokenStats = useAppStore((s) => s.projectTokenStats);
  const projectConversationTokenStats = useAppStore((s) => s.projectConversationTokenStats);
  const projectTokenStatsSummary = useAppStore((s) => s.projectTokenStatsSummary);
  const projectConversationTokenStatsSummary = useAppStore(
    (s) => s.projectConversationTokenStatsSummary
  );
  const projectTokenStatsPagination = useAppStore((s) => s.projectTokenStatsPagination);
  const loadProjectTokenStats = useAppStore((s) => s.loadProjectTokenStats);
  const loadMoreProjectTokenStats = useAppStore((s) => s.loadMoreProjectTokenStats);

  useEffect(() => {
    if (selectedProject) {
      void loadProjectTokenStats(selectedProject.path);
    }
  }, [selectedProject, loadProjectTokenStats]);

  const handleSessionClick = (stats: SessionTokenStats) => {
    const session = sessions.find(
      (s) => s.actual_session_id === stats.session_id || s.session_id === stats.session_id
    );
    if (session) onSessionSelect(session);
  };

  return (
    <OverlayScrollbarsComponent
      className="h-full"
      options={{ scrollbars: { theme: "os-theme-custom", autoHide: "leave" } }}
    >
      <AnalyticsDashboard isViewingGlobalStats={isViewingGlobalStats} />
      {selectedProject && projectTokenStats.length > 0 && (
        <div className="p-3 md:p-6">
          <TokenStatsViewer
            projectStats={projectTokenStats}
            projectConversationStats={projectConversationTokenStats}
            projectStatsSummary={projectTokenStatsSummary}
            projectConversationStatsSummary={projectConversationTokenStatsSummary}
            providerId={selectedProject.provider ?? "claude"}
            pagination={projectTokenStatsPagination}
            onLoadMore={() => loadMoreProjectTokenStats(selectedProject.path)}
            isLoading={isLoadingTokenStats}
            onSessionClick={handleSessionClick}
          />
        </div>
      )}
    </OverlayScrollbarsComponent>
  );
};
