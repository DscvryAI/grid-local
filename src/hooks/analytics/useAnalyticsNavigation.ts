import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useAppStore } from "../../store/useAppStore";
import type { MetricMode, StatsMode } from "../../types";

export function useAnalyticsNavigation() {
  const { t } = useTranslation();
  const {
    analytics,
    setAnalyticsCurrentView,
    setAnalyticsStatsMode,
    setAnalyticsMetricMode,
    setAnalyticsProjectSummary,
    setAnalyticsProjectConversationSummary,
    setAnalyticsSessionComparison,
    setAnalyticsLoadingProjectSummary,
    setAnalyticsLoadingSessionComparison,
    setAnalyticsProjectSummaryError,
    setAnalyticsSessionComparisonError,
    resetAnalytics,
    clearAnalyticsErrors,
    loadProjectStatsSummary,
    loadSessionComparison,
    loadSessionTokenStats,
    loadGlobalStats,
    clearTokenStats,
    setPrimarySurface,
  } = useAppStore();

  const switchToMessages = useCallback(() => {
    setAnalyticsCurrentView("messages");
    setPrimarySurface("history");
    clearAnalyticsErrors();
  }, [setAnalyticsCurrentView, setPrimarySurface, clearAnalyticsErrors]);

  const switchToAnalytics = useCallback(async () => {
    const project = useAppStore.getState().selectedProject;
    if (!project) {
      throw new Error(t("common.hooks.noProjectSelected"));
    }

    setAnalyticsCurrentView("analytics");
    setPrimarySurface("insights");
    clearAnalyticsErrors();

    try {
      // Project summary and session comparison scan independent data (one
      // project-wide, one session-specific) -- load them concurrently
      // instead of making the session comparison wait for the project
      // summary to finish first.
      const projectSummaryTask = (async () => {
        setAnalyticsLoadingProjectSummary(true);
        try {
          const summary = await loadProjectStatsSummary(project.path);
          setAnalyticsProjectSummary(summary);
        } catch (error) {
          const errorMessage =
            error instanceof Error
              ? error.message
              : t("common.hooks.projectSummaryLoadFailed");
          setAnalyticsProjectSummaryError(errorMessage);
          throw error;
        } finally {
          setAnalyticsLoadingProjectSummary(false);
        }
      })();

      // Read selectedSession fresh here, not from this hook's closure --
      // this function can run right after selectProject() clears the
      // session (a project switch), and a closure value captured before
      // that clear would still point at the *previous* project's session.
      const session = useAppStore.getState().selectedSession;
      const sessionComparisonTask = session
        ? (async () => {
            setAnalyticsLoadingSessionComparison(true);
            try {
              const [comparison] = await Promise.all([
                loadSessionComparison(session.actual_session_id, project.path),
                loadSessionTokenStats(session.file_path),
              ]);
              setAnalyticsSessionComparison(comparison);
              setAnalyticsSessionComparisonError(null);
            } catch (error) {
              const errorMessage =
                error instanceof Error
                  ? error.message
                  : t("common.hooks.sessionComparisonLoadFailed");
              setAnalyticsSessionComparisonError(errorMessage);
            } finally {
              setAnalyticsLoadingSessionComparison(false);
            }
          })()
        : Promise.resolve();

      await Promise.all([projectSummaryTask, sessionComparisonTask]);
    } catch (error) {
      console.error("Failed to load analytics:", error);
      throw error;
    }
  }, [
    t,
    setAnalyticsCurrentView,
    setPrimarySurface,
    clearAnalyticsErrors,
    setAnalyticsLoadingProjectSummary,
    setAnalyticsLoadingSessionComparison,
    setAnalyticsProjectSummary,
    setAnalyticsSessionComparison,
    setAnalyticsProjectSummaryError,
    setAnalyticsSessionComparisonError,
    loadProjectStatsSummary,
    loadSessionComparison,
    loadSessionTokenStats,
  ]);

  const setStatsMode = useCallback(
    async (
      mode: StatsMode,
      options?: { isViewingGlobalStats?: boolean }
    ) => {
      const currentMode = useAppStore.getState().analytics.statsMode;
      if (currentMode === mode) {
        return;
      }

      setAnalyticsStatsMode(mode);
      clearTokenStats();
      setAnalyticsProjectSummary(null);
      setAnalyticsProjectConversationSummary(null);
      setAnalyticsSessionComparison(null);
      setAnalyticsProjectSummaryError(null);
      setAnalyticsSessionComparisonError(null);

      const state = useAppStore.getState();
      const project = state.selectedProject;
      const session = state.selectedSession;
      const currentView = state.analytics.currentView;
      const isGlobalScope =
        options?.isViewingGlobalStats ??
        (!project && currentView === "analytics");

      try {
        if (isGlobalScope) {
          await loadGlobalStats();
          return;
        }

        if (!project) {
          return;
        }

        if (currentView === "analytics") {
          setAnalyticsLoadingProjectSummary(true);
          try {
            const summary = await loadProjectStatsSummary(project.path);
            setAnalyticsProjectSummary(summary);
          } finally {
            setAnalyticsLoadingProjectSummary(false);
          }

          if (session) {
            setAnalyticsLoadingSessionComparison(true);
            try {
              const [comparison] = await Promise.all([
                loadSessionComparison(
                  session.actual_session_id,
                  project.path
                ),
                loadSessionTokenStats(session.file_path),
              ]);
              setAnalyticsSessionComparison(comparison);
            } finally {
              setAnalyticsLoadingSessionComparison(false);
            }
          }
        }
      } catch (error) {
        const errorMessage =
          error instanceof Error
            ? error.message
            : t("common.hooks.projectSummaryLoadFailed");
        toast.error(errorMessage);
        if (currentView === "analytics") {
          setAnalyticsProjectSummaryError(errorMessage);
          if (session != null) {
            setAnalyticsSessionComparisonError(errorMessage);
          }
          return;
        }

        setAnalyticsProjectSummaryError(errorMessage);
      }
    },
    [
      clearTokenStats,
      loadGlobalStats,
      loadProjectStatsSummary,
      loadSessionComparison,
      loadSessionTokenStats,
      setAnalyticsLoadingProjectSummary,
      setAnalyticsLoadingSessionComparison,
      setAnalyticsProjectSummary,
      setAnalyticsProjectConversationSummary,
      setAnalyticsProjectSummaryError,
      setAnalyticsSessionComparison,
      setAnalyticsSessionComparisonError,
      setAnalyticsStatsMode,
      t,
    ]
  );

  const setMetricMode = useCallback(
    (mode: MetricMode) => {
      setAnalyticsMetricMode(mode);
    },
    [setAnalyticsMetricMode]
  );

  const refreshAnalytics = useCallback(async () => {
    switch (analytics.currentView) {
      case "analytics":
        setAnalyticsProjectSummary(null);
        setAnalyticsProjectConversationSummary(null);
        setAnalyticsSessionComparison(null);
        await switchToAnalytics();
        break;
      case "messages":
        break;
      default:
        console.warn("Unknown analytics view:", analytics.currentView);
    }
  }, [
    analytics.currentView,
    switchToAnalytics,
    setAnalyticsProjectSummary,
    setAnalyticsProjectConversationSummary,
    setAnalyticsSessionComparison,
  ]);

  const clearAll = useCallback(() => {
    resetAnalytics();
    clearTokenStats();
  }, [resetAnalytics, clearTokenStats]);

  return {
    switchToMessages,
    switchToAnalytics,
    setStatsMode,
    setMetricMode,
    refreshAnalytics,
    clearAll,
  };
}
