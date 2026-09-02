import { useMemo } from "react";
import { useAppStore } from "../../store/useAppStore";

export function useAnalyticsComputed() {
  const analytics = useAppStore((s) => s.analytics);
  const isLoadingTokenStats = useAppStore((s) => s.isLoadingTokenStats);

  return useMemo(
    () => ({
      isAnalyticsView: analytics.currentView === "analytics",
      isMessagesView: analytics.currentView === "messages",
      hasAnyError: !!(
        analytics.projectSummaryError ||
        analytics.sessionComparisonError
      ),
      isLoadingAnalytics:
        analytics.isLoadingProjectSummary ||
        analytics.isLoadingSessionComparison,
      isAnyLoading:
        analytics.isLoadingProjectSummary ||
        analytics.isLoadingSessionComparison ||
        isLoadingTokenStats,
    }),
    [
      analytics.currentView,
      analytics.projectSummaryError,
      analytics.sessionComparisonError,
      analytics.isLoadingProjectSummary,
      analytics.isLoadingSessionComparison,
      isLoadingTokenStats,
    ]
  );
}
