import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useAppStore } from "./store/useAppStore";
import { useAnalytics } from "./hooks/useAnalytics";
import { useUpdater } from "./hooks/useUpdater";
import { useResizablePanel } from "./hooks/useResizablePanel";
import { useAppKeyboard } from "./hooks/useAppKeyboard";
import { useAppInitialization } from "./hooks/useAppInitialization";
import { useLiveStatusMessage } from "./hooks/useLiveStatusMessage";
import { useExternalLinks } from "./hooks/useExternalLinks";
import { usePlatform } from "@/contexts/platform";
import { AppLayout } from "@/layouts/AppLayout";
import {
  type ClaudeSession,
  type ClaudeProject,
  type GroupingMode,
} from "./types";
import { getProviderLabel, normalizeProviderIds } from "./utils/providers";
import {
  fetchStartupSessionHint,
  preloadSessionFromCli,
  type SessionHint,
} from "./lib/preloadSession";

import "./App.css";

function App() {
  const {
    projects,
    sessions,
    sessionsTotal,
    hasMoreSessions,
    selectedProject,
    selectedSession,
    messages,
    isLoading,
    isLoadingProjects,
    isLoadingSessions,
    isLoadingMoreSessions,
    isLoadingMessages,
    error,
    sessionSearch,
    selectProject,
    loadMoreSessions,
    selectSession,
    clearProjectSelection,
    setSessionSearchQuery,
    setSearchFilterType,
    goToNextMatch,
    goToPrevMatch,
    clearSessionSearch,
    loadGlobalStats,
    setAnalyticsCurrentView,
    setPrimarySurface,
    updateUserSettings,
    getGroupedProjects,
    getDirectoryGroupedProjects,
    getEffectiveGroupingMode,
    hideProject,
    unhideProject,
    isProjectHidden,
    setDateFilter,
    isNavigatorOpen,
    toggleNavigator,
    activeProviders,
  } = useAppStore();

  const { actions: analyticsActions, computed } = useAnalytics();

  const { t } = useTranslation();
  const { isDesktop, isMobile } = usePlatform();
  const updater = useUpdater();
  const appVersion = updater.state.currentVersion || "—";

  // Side-effect hooks (no return value)
  useAppKeyboard();
  useExternalLinks();
  useAppInitialization({ isMessagesView: computed.isMessagesView });

  const liveStatusMessage = useLiveStatusMessage({
    isChecking: updater.state.isChecking,
    isLoading,
    isAnyLoading: computed.isAnyLoading,
    isLoadingMessages,
    isLoadingProjects,
    isLoadingSessions,
  });

  const globalOverviewDescription = useMemo(() => {
    const normalized = normalizeProviderIds(activeProviders);

    if (normalized.length === 0) {
      return t("analytics.globalOverviewDescription");
    }

    const labels = normalized.map((providerId) =>
      getProviderLabel((key, fallback) => t(key, fallback), providerId)
    );

    if (labels.length === 1) {
      return t(
        "analytics.globalOverviewDescriptionSingleProvider",
        "Aggregated statistics for {{provider}} projects on your machine",
        { provider: labels[0] }
      );
    }

    return t(
      "analytics.globalOverviewDescriptionMultiProvider",
      "Aggregated statistics for selected providers ({{providers}}) on your machine",
      { providers: labels.join(", ") }
    );
  }, [activeProviders, t]);

  // One-shot guard so the first-launch `--session` preload fires exactly once
  // per process, even if project loading renders multiple times.
  const cliPreloadAttempted = useRef(false);
  const openSessionPicker = useAppStore((s) => s.openSessionPicker);

  // Keep the latest projects list in a ref so the second-invocation event
  // listener (which is set up once and lives for the process lifetime) can
  // always see the current list without re-subscribing on every render.
  const projectsRef = useRef(projects);
  useEffect(() => {
    projectsRef.current = projects;
  }, [projects]);

  // Phase 3: second-invocation routing. If a `cli-session-hint` event arrives
  // before projects finish loading, stash the latest hint here so the first
  // load path can pick it up. Only the *latest* hint is kept — if the user
  // re-invokes the CLI twice in quick succession, the newer intent wins.
  const pendingHintRef = useRef<SessionHint | null>(null);
  const preloadGenerationRef = useRef(0);

  const invalidatePreload = useCallback(() => {
    preloadGenerationRef.current += 1;
  }, []);

  const runPreloadWithHint = useCallback(
    (hint: SessionHint, messageId: string | null = null) => {
      const generation = ++preloadGenerationRef.current;
      const isCurrent = () => preloadGenerationRef.current === generation;

      void preloadSessionFromCli({
        getStartupSessionHint: () => Promise.resolve(hint),
        projects: projectsRef.current,
        selectProject,
        selectSession: (session) => selectSession(session),
        openSessionPicker,
        t: (key, fallback) => t(key, fallback ?? key),
        isCurrent,
      }).then(({ matched }) => {
        if (isCurrent() && matched && messageId) {
          const state = useAppStore.getState();
          state.setAnalyticsCurrentView("messages");
          state.navigateToMessage(messageId);
        }
      }).catch((error) => {
        if (!isCurrent()) return;
        console.error("Failed to preload session:", error);
        toast.error(t("common.error.unexpected", "Failed to open session"));
      });
    },
    [selectProject, selectSession, openSessionPicker, t],
  );

  useEffect(() => {
    if (cliPreloadAttempted.current) return;
    if (isLoadingProjects || projects.length === 0) return;
    cliPreloadAttempted.current = true;
    // Drain a hint that arrived before projects loaded, if any. Otherwise use
    // the Tauri-managed first-launch hint.
    const queued = pendingHintRef.current;
    if (queued) {
      pendingHintRef.current = null;
      runPreloadWithHint(queued);
      return;
    }
    const startupGeneration = preloadGenerationRef.current;
    void fetchStartupSessionHint().then((hint) => {
      // A second invocation or deep link may have arrived while the startup
      // command was resolving. Its newer request owns navigation now.
      if (preloadGenerationRef.current !== startupGeneration) return;
      if (hint) {
        runPreloadWithHint(hint);
      }
    });
  }, [isLoadingProjects, projects, runPreloadWithHint]);

  // Second-invocation routing. `tauri-plugin-single-instance` (CLI re-exec)
  // and macOS `RunEvent::Opened` (Spotlight/Dock/Finder) both emit
  // `cli-session-hint`. We resolve each hint through `preloadSessionFromCli`
  // so uuid / path / folder / title behave identically regardless of entry
  // path.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    const subscribe = async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        if (cancelled) return;
        unlisten = await listen<SessionHint>("cli-session-hint", (event) => {
          const hint = event.payload;
          invalidatePreload();
          // Projects not yet loaded: stash and let the first-load effect
          // consume the latest hint. This handles the race where the user
          // re-invokes the CLI before the initial project scan finishes.
          if (!cliPreloadAttempted.current || projectsRef.current.length === 0) {
            pendingHintRef.current = hint;
            return;
          }
          runPreloadWithHint(hint);
        });
      } catch (error) {
        console.warn("cli-session-hint listener unavailable:", error);
      }
    };
    void subscribe();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [invalidatePreload, runPreloadWithHint]);

  // Local state
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState(false);
  const [isMobileSidebarOpen, setIsMobileSidebarOpen] = useState(false);

  // A selected session and the project-agnostic global-stats overview are
  // mutually exclusive — entering global stats clears the project/session
  // (clearProjectSelection). The sidebar resets this flag explicitly on
  // session select, but the global search modal selects sessions via store
  // actions and can't reach this local state, so it would otherwise stay in
  // the global-stats view and hide the navigated conversation. Guarantee the
  // exit here whenever a session becomes selected (issue #390).
  useEffect(() => {
    if (selectedSession) {
      setPrimarySurface("history");
    }
  }, [selectedSession, setPrimarySurface]);

  // Sidebar resize
  const {
    width: sidebarWidth,
    isResizing: isSidebarResizing,
    handleMouseDown: handleSidebarResizeStart,
  } = useResizablePanel({
    defaultWidth: 256,
    minWidth: 200,
    maxWidth: 800,
    storageKey: "sidebar-width",
  });

  // Navigator resize (right sidebar)
  const {
    width: navigatorWidth,
    isResizing: isNavigatorResizing,
    handleMouseDown: handleNavigatorResizeStart,
  } = useResizablePanel({
    defaultWidth: 280,
    minWidth: 200,
    maxWidth: 400,
    storageKey: "navigator-width",
    direction: "left",
  });

  const handleGlobalStatsClick = useCallback(() => {
    setPrimarySurface("insights");
    clearProjectSelection();
    setAnalyticsCurrentView("analytics");
    void loadGlobalStats();
  }, [
    clearProjectSelection,
    loadGlobalStats,
    setAnalyticsCurrentView,
    setPrimarySurface,
  ]);

  const handleToggleSidebar = useCallback(() => {
    setIsSidebarCollapsed((prev) => !prev);
  }, []);

  // Project grouping
  const groupingMode = getEffectiveGroupingMode();
  const { groups: worktreeGroups, ungrouped: ungroupedProjects } =
    getGroupedProjects();
  const { groups: directoryGroups } = getDirectoryGroupedProjects();

  const handleGroupingModeChange = useCallback(
    (newMode: GroupingMode) => {
      updateUserSettings({
        groupingMode: newMode,
        worktreeGrouping: newMode === "worktree",
        worktreeGroupingUserSet: true,
      });
    },
    [updateUserSettings]
  );

  const handleSessionSelect = useCallback(
    async (session: ClaudeSession) => {
      try {
        setPrimarySurface("history");
        setAnalyticsCurrentView("messages");

        // Find the project this session belongs to.
        //
        // Comparing `project.name === session.project_name` is unreliable
        // across providers: each provider picks its own way to turn a raw
        // on-disk directory (e.g. "Users-foo-Projects-bar") into a sidebar
        // display label, and `session.project_name` is set by the loader,
        // not by the sidebar. CodeBuddy in particular shortens
        // `dir_name.rsplit('-').next()` for its sidebar label while the
        // session loader keeps the encoded form — the two never match,
        // so the previous equality check failed to switch to the right
        // project when a session was selected.
        //
        // The path prefix is the one signal that's stable everywhere:
        // a session's `file_path` always lives under its project's `path`.
        // Match on that first, fall back to the name equality only when
        // `file_path` is unavailable.
        //
        // The prefix match must respect the path-segment boundary so
        // sibling projects sharing a parent dir don't collide — without
        // this, `/a/proj` would also match a session under `/a/proj2`.
        const findProjectForSession = (s: ClaudeSession) => {
          if (s.file_path) {
            const fp = s.file_path;
            const byPath = projects.find((p) => {
              if (!fp.startsWith(p.path)) return false;
              if (fp.length === p.path.length) return true;
              const next = fp.charAt(p.path.length);
              return next === "/" || next === "\\";
            });
            if (byPath) return byPath;
          }
          return projects.find((p) => p.name === s.project_name);
        };

        const targetProject = findProjectForSession(session);
        const currentProject = useAppStore.getState().selectedProject;

        // Temporary: History (Step 5b) reported "opens the view but shows
        // Insights tabs instead of the conversation" -- not reproducible
        // from reading handleSessionSelect/selectProject/selectSession in
        // isolation (setAnalyticsCurrentView("messages") already runs
        // synchronously above). Logging the actual state transition
        // instead of guessing further; remove once the real cause is
        // confirmed.
        if (import.meta.env.DEV) {
          console.log("[History] handleSessionSelect", {
            sessionFilePath: session.file_path,
            sessionProjectName: session.project_name,
            targetProjectFound: Boolean(targetProject),
            targetProjectPath: targetProject?.path,
            currentProjectPath: currentProject?.path,
            primarySurfaceBefore: useAppStore.getState().primarySurface,
            currentViewBefore: useAppStore.getState().analytics.currentView,
          });
        }

        if (
          targetProject &&
          (!currentProject || currentProject.path !== targetProject.path)
        ) {
          await selectProject(targetProject);
        }

        await selectSession(session);

        if (import.meta.env.DEV) {
          console.log("[History] handleSessionSelect done", {
            primarySurfaceAfter: useAppStore.getState().primarySurface,
            currentViewAfter: useAppStore.getState().analytics.currentView,
            selectedProjectAfter: useAppStore.getState().selectedProject?.path,
            selectedSessionAfter: useAppStore.getState().selectedSession?.file_path,
          });
        }
      } catch (error) {
        // Per CLAUDE.md "에러 처리": async failures need user-visible
        // feedback (toast/alert). console.error alone leaves the user
        // staring at a sidebar that didn't react to their click.
        console.error("Failed to select session:", error);
        const message = error instanceof Error ? error.message : String(error);
        toast.error(`${t("session.selectError")}: ${message}`);
      }
    },
    [
      projects,
      selectProject,
      selectSession,
      setAnalyticsCurrentView,
      setPrimarySurface,
      t,
    ]
  );

  const handleProjectSelect = useCallback(
    async (project: ClaudeProject) => {
      const currentProject = useAppStore.getState().selectedProject;

      if (currentProject?.path === project.path) {
        clearProjectSelection();
        return;
      }

      const activeView = useAppStore.getState().analytics.currentView;
      setPrimarySurface("history");

      analyticsActions.clearAll();
      setDateFilter({ start: null, end: null });

      await selectProject(project);

      try {
        if (activeView === "analytics") {
          await analyticsActions.switchToAnalytics();
        } else {
          analyticsActions.switchToMessages();
        }
      } catch (error) {
        console.error(`Failed to auto-load ${activeView} view:`, error);
      }
    },
    [
      clearProjectSelection,
      selectProject,
      analyticsActions,
      setDateFilter,
      setPrimarySurface,
    ]
  );

  return (
    <AppLayout
      projects={projects}
      sessions={sessions}
      sessionsTotal={sessionsTotal}
      hasMoreSessions={hasMoreSessions}
      selectedProject={selectedProject}
      selectedSession={selectedSession}
      messages={messages}
      isLoading={isLoading}
      isLoadingProjects={isLoadingProjects}
      isLoadingSessions={isLoadingSessions}
      isLoadingMoreSessions={isLoadingMoreSessions}
      isLoadingMessages={isLoadingMessages}
      error={error}
      sessionSearch={sessionSearch}
      analyticsActions={analyticsActions}
      computed={computed}
      updater={updater}
      appVersion={appVersion}
      isDesktop={isDesktop}
      isMobile={isMobile}
      isSidebarCollapsed={isSidebarCollapsed}
      isMobileSidebarOpen={isMobileSidebarOpen}
      setIsMobileSidebarOpen={setIsMobileSidebarOpen}
      sidebarWidth={sidebarWidth}
      isSidebarResizing={isSidebarResizing}
      handleSidebarResizeStart={handleSidebarResizeStart}
      navigatorWidth={navigatorWidth}
      isNavigatorResizing={isNavigatorResizing}
      handleNavigatorResizeStart={handleNavigatorResizeStart}
      isNavigatorOpen={isNavigatorOpen}
      toggleNavigator={toggleNavigator}
      groupingMode={groupingMode}
      worktreeGroups={worktreeGroups}
      directoryGroups={directoryGroups}
      ungroupedProjects={ungroupedProjects}
      handleProjectSelect={handleProjectSelect}
      loadMoreSessions={loadMoreSessions}
      handleSessionSelect={handleSessionSelect}
      handleGlobalStatsClick={handleGlobalStatsClick}
      handleToggleSidebar={handleToggleSidebar}
      handleGroupingModeChange={handleGroupingModeChange}
      hideProject={hideProject}
      unhideProject={unhideProject}
      isProjectHidden={isProjectHidden}
      setSessionSearchQuery={setSessionSearchQuery}
      setSearchFilterType={setSearchFilterType}
      clearSessionSearch={clearSessionSearch}
      goToNextMatch={goToNextMatch}
      goToPrevMatch={goToPrevMatch}
      globalOverviewDescription={globalOverviewDescription}
      liveStatusMessage={liveStatusMessage}
    />
  );
}

export default App;
