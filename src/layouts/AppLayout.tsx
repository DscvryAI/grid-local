import React, { useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  MessageSquare,
  Database,
  BarChart3,
} from "lucide-react";
import { useAppStore } from "@/store/useAppStore";
import { LoadingSpinner } from "@/components/ui/loading";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";
import { ProjectTree } from "@/components/ProjectTree";
import { SessionDateList } from "@/components/History/SessionDateList";
import { ProviderDiscoveryOnboarding } from "@/components/ProviderDiscoveryOnboarding";
import { HomeSurface } from "@/components/Home/HomeSurface";
import { ArchiveSyncPreferencePrompt } from "@/components/ArchiveSyncPreferencePrompt";
import { SessionView } from "@/components/SessionView/SessionView";
import { SessionErrorBoundary } from "@/components/SessionErrorBoundary";
import { SimpleUpdateManager } from "@/components/SimpleUpdateManager";
import { BottomTabBar } from "@/components/mobile/BottomTabBar";
import { MobileNavigatorSheet } from "@/components/mobile/MobileNavigatorSheet";
import { Header } from "@/layouts/Header/Header";
import { ModalContainer } from "@/layouts/Header/SettingDropdown/ModalContainer";
import { DesktopOnly } from "@/contexts/platform";
import {
  AppErrorType,
  type ClaudeMessage,
  type ClaudeProject,
  type ClaudeSession,
  type GroupingMode,
  type AppError,
} from "@/types";
import type { UseAnalyticsReturn } from "@/types/analytics";
import type { UseUpdaterReturn } from "@/hooks/useUpdater";
import type { SearchState, SearchFilterType } from "@/store/slices/types";
import type { WorktreeGroup, DirectoryGroup } from "@/utils/worktreeUtils";
import { getProviderLabel } from "@/utils/providers";
import { markColdStartReady } from "@/utils/coldStartTiming";

// Search and Insights are real, substantial surfaces not needed on the
// default "home" landing surface -- code-split so their
// chunks only load the first time a user actually navigates to either.
// Home/SessionView/History stay eager: they're the default landing
// surface and the two most-hit paths, so lazy-loading them would only
// slow down the common case.
const SearchSurface = React.lazy(() =>
  import("@/components/Search/SearchSurface").then((m) => ({
    default: m.SearchSurface,
  }))
);
const InsightsSurface = React.lazy(() =>
  import("@/components/Insights/InsightsSurface").then((m) => ({
    default: m.InsightsSurface,
  }))
);

export interface AppLayoutProps {
  // Store state
  projects: ClaudeProject[];
  sessions: ClaudeSession[];
  sessionsTotal: number;
  hasMoreSessions: boolean;
  selectedProject: ClaudeProject | null;
  selectedSession: ClaudeSession | null;
  messages: ClaudeMessage[];
  isLoading: boolean;
  isLoadingProjects: boolean;
  isLoadingSessions: boolean;
  isLoadingMoreSessions: boolean;
  isLoadingMessages: boolean;
  error: AppError | null;
  sessionSearch: SearchState;

  // Analytics
  analyticsActions: UseAnalyticsReturn["actions"];
  computed: UseAnalyticsReturn["computed"];

  // Updater
  updater: UseUpdaterReturn;
  appVersion: string;

  // Platform
  isDesktop: boolean;
  isMobile: boolean;

  // Local state
  isSidebarCollapsed: boolean;
  isMobileSidebarOpen: boolean;
  setIsMobileSidebarOpen: (open: boolean) => void;

  // Sidebar resize
  sidebarWidth: number;
  isSidebarResizing: boolean;
  handleSidebarResizeStart: (e: React.MouseEvent<HTMLElement>) => void;

  // Navigator resize
  navigatorWidth: number;
  isNavigatorResizing: boolean;
  handleNavigatorResizeStart: (e: React.MouseEvent<HTMLElement>) => void;
  isNavigatorOpen: boolean;
  toggleNavigator: () => void;

  // Grouping
  groupingMode: GroupingMode;
  worktreeGroups: WorktreeGroup[];
  directoryGroups: DirectoryGroup[];
  ungroupedProjects: ClaudeProject[];

  // Callbacks
  handleProjectSelect: (project: ClaudeProject) => void;
  loadMoreSessions: () => void;
  handleSessionSelect: (session: ClaudeSession) => void;
  handleGlobalStatsClick: () => void;
  handleToggleSidebar: () => void;
  handleGroupingModeChange: (mode: GroupingMode) => void;
  hideProject: (projectPath: string) => Promise<void>;
  unhideProject: (projectPath: string) => Promise<void>;
  isProjectHidden: (projectPath: string) => boolean;
  setSessionSearchQuery: (query: string) => void;
  setSearchFilterType: (type: SearchFilterType) => void;
  clearSessionSearch: () => void;
  goToNextMatch: () => void;
  goToPrevMatch: () => void;

  // Computed
  globalOverviewDescription: string;
  liveStatusMessage: string;
}

export const AppLayout: React.FC<AppLayoutProps> = (props) => {
  const { t } = useTranslation();
  // Loaded window may be partial under message pagination — used to render
  // the "+" suffix on the message count.
  const hasMoreMessages = useAppStore((s) => s.pagination.hasMore);
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
    analyticsActions,
    computed,
    updater,
    appVersion,
    isDesktop,
    isMobile,
    isSidebarCollapsed,
    isMobileSidebarOpen,
    setIsMobileSidebarOpen,
    sidebarWidth,
    isSidebarResizing,
    handleSidebarResizeStart,
    navigatorWidth,
    isNavigatorResizing,
    handleNavigatorResizeStart,
    isNavigatorOpen,
    toggleNavigator,
    groupingMode,
    worktreeGroups,
    directoryGroups,
    ungroupedProjects,
    handleProjectSelect,
    loadMoreSessions,
    handleSessionSelect,
    handleGlobalStatsClick,
    handleToggleSidebar,
    handleGroupingModeChange,
    hideProject,
    unhideProject,
    isProjectHidden,
    setSessionSearchQuery,
    setSearchFilterType,
    clearSessionSearch,
    goToNextMatch,
    goToPrevMatch,
    globalOverviewDescription,
    liveStatusMessage,
  } = props;

  const showProviderDiscoveryOnboarding = useAppStore(
    (s) => s.showProviderDiscoveryOnboarding
  );

  const firstIndexProgress = useAppStore((s) => s.firstIndexProgress);
  const cancelFirstIndex = useAppStore((s) => s.cancelFirstIndex);

  const primarySurface = useAppStore((s) => s.primarySurface);
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);

  // Derived from primarySurface rather than tracked as its own flag --
  // "viewing global stats" is just "on the Insights surface with no
  // project selected."
  const isViewingGlobalStats = primarySurface === "insights" && !selectedProject;

  // The moment this component has
  // something real to show (past both the provider-discovery gate below
  // and the full-screen "Building your Grid" loading state further
  // down), cold start is over. `markColdStartReady` is itself idempotent,
  // so re-renders after the first "ready" one are a cheap no-op.
  const hasContent =
    !showProviderDiscoveryOnboarding && !(isLoading && projects.length === 0);
  useEffect(() => {
    if (hasContent) {
      markColdStartReady();
    }
  }, [hasContent]);

  // First-run gate: ask before scanning for other AI tools (spec §6).
  if (showProviderDiscoveryOnboarding) {
    return <ProviderDiscoveryOnboarding />;
  }

  // Error State
  if (error && error.type !== AppErrorType.CLAUDE_FOLDER_NOT_FOUND) {
    return (
      <div className="h-screen flex items-center justify-center bg-background">
        <div className="text-center max-w-md mx-auto p-8">
          <div className="w-16 h-16 rounded-2xl bg-destructive/10 flex items-center justify-center mx-auto mb-6">
            <AlertTriangle className="w-8 h-8 text-destructive" />
          </div>
          <h1 className="text-xl font-semibold text-foreground mb-2">
            {t("common.errorOccurred")}
          </h1>
          <p className="text-sm text-muted-foreground mb-6">{error.message}</p>
          <button
            onClick={() => window.location.reload()}
            className="action-btn primary"
          >
            {t("common.retry")}
          </button>
        </div>
      </div>
    );
  }

  // Initial load: the very first scan (or the "Scan my coding history"
  // discovery run) has no content to show yet. A full-screen state here
  // keeps this visible instead of a small footer status line easy to miss
  // right after the first-run screen. When `firstIndexProgress` is set,
  // the mandatory first-run index is in flight -- show real per-provider
  // phase/count and a cancel affordance (spec §7 "Indexing experience")
  // instead of the generic spinner.
  if (isLoading && projects.length === 0) {
    return (
      <div className="h-screen flex items-center justify-center bg-background">
        <div className="text-center max-w-md mx-auto p-8">
          <LoadingSpinner size="lg" variant="accent" />
          <h1 className="text-lg font-semibold text-foreground mt-6 mb-1">
            {t("status.buildingGrid", "Building your Grid")}
          </h1>
          {firstIndexProgress ? (
            <>
              <p className="text-sm text-muted-foreground">
                {t("status.indexingProvider", {
                  provider: getProviderLabel(
                    (key, fallback) => t(key, fallback),
                    firstIndexProgress.providerKey
                  ),
                  done: firstIndexProgress.phasesDone + 1,
                  total: firstIndexProgress.phasesTotal,
                  defaultValue:
                    "Indexing {{provider}} ({{done}} of {{total}})",
                })}
              </p>
              <div
                className="mt-3 h-1.5 w-full max-w-xs mx-auto overflow-hidden rounded-full bg-muted"
                role="progressbar"
                aria-valuenow={firstIndexProgress.phasesDone + 1}
                aria-valuemin={0}
                aria-valuemax={firstIndexProgress.phasesTotal}
                aria-label={t("status.indexingProgress", "Indexing progress")}
              >
                <div
                  className="h-full bg-primary transition-all"
                  style={{
                    width: `${Math.min(
                      100,
                      ((firstIndexProgress.phasesDone + 1) /
                        Math.max(1, firstIndexProgress.phasesTotal)) *
                        100
                    )}%`,
                  }}
                />
              </div>
              <button
                type="button"
                onClick={cancelFirstIndex}
                className="mt-4 text-xs font-medium text-muted-foreground underline underline-offset-2 hover:text-foreground"
              >
                {t("status.cancelIndexing", "Cancel and continue with what's indexed so far")}
              </button>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">
              {liveStatusMessage || t("status.initializing")}
            </p>
          )}
        </div>
      </div>
    );
  }

  return (
    <TooltipProvider>
      <div className="h-screen flex flex-col bg-background">
        <nav
          aria-label={t("common.a11y.skipNavigation", {
            defaultValue: "Skip navigation",
          })}
        >
          <a
            href="#project-explorer"
            className="absolute left-2 top-[-40px] z-[700] rounded-md border border-border bg-card px-3 py-2 text-sm font-medium text-foreground transition-all focus:top-2"
          >
            {t("common.a11y.skipToProjects", {
              defaultValue: "Skip to project explorer",
            })}
          </a>
          <a
            href="#main-content"
            className="absolute left-52 top-[-40px] z-[700] rounded-md border border-border bg-card px-3 py-2 text-sm font-medium text-foreground transition-all focus:top-2"
          >
            {t("common.a11y.skipToMain", {
              defaultValue: "Skip to main content",
            })}
          </a>
          {!isMobile && isNavigatorOpen && selectedSession && (
            <a
              href="#message-navigator"
              className="absolute left-[23rem] top-[-40px] z-[700] rounded-md border border-border bg-card px-3 py-2 text-sm font-medium text-foreground transition-all focus:top-2"
            >
              {t("common.a11y.skipToNavigator", {
                defaultValue: "Skip to message navigator",
              })}
            </a>
          )}
          <a
            href="#app-settings-button"
            className="absolute right-2 top-[-40px] z-[700] rounded-md border border-border bg-card px-3 py-2 text-sm font-medium text-foreground transition-all focus:top-2"
          >
            {t("common.a11y.skipToSettings", {
              defaultValue: "Skip to settings",
            })}
          </a>
        </nav>

        {/* Header */}
        <Header
          analyticsActions={analyticsActions}
          analyticsComputed={computed}
          updater={updater}
          handleGlobalStatsClick={handleGlobalStatsClick}
        />

        {/* Mobile Sidebar Drawer */}
        {isMobile && (
          <Sheet
            open={isMobileSidebarOpen}
            onOpenChange={setIsMobileSidebarOpen}
          >
            <SheetContent
              side="left"
              className="w-[var(--mobile-drawer-width)] p-0"
              showCloseButton={false}
            >
              <SheetTitle className="sr-only">
                {t("common.mobile.openSidebar")}
              </SheetTitle>
              <ProjectTree
                projects={projects}
                sessions={sessions}
                sessionsTotal={sessionsTotal}
                hasMoreSessions={hasMoreSessions}
                selectedProject={selectedProject}
                selectedSession={selectedSession}
                onProjectSelect={handleProjectSelect}
                onSessionSelect={handleSessionSelect}
                onLoadMoreSessions={loadMoreSessions}
                onGlobalStatsClick={handleGlobalStatsClick}
                isLoading={isLoadingProjects || isLoadingSessions}
                isLoadingMoreSessions={isLoadingMoreSessions}
                isViewingGlobalStats={isViewingGlobalStats}
                groupingMode={groupingMode}
                worktreeGroups={worktreeGroups}
                directoryGroups={directoryGroups}
                ungroupedProjects={ungroupedProjects}
                onGroupingModeChange={handleGroupingModeChange}
                onHideProject={hideProject}
                onUnhideProject={unhideProject}
                isProjectHidden={isProjectHidden}
                onClose={() => setIsMobileSidebarOpen(false)}
                asideId="project-explorer"
              />
            </SheetContent>
          </Sheet>
        )}

        {/* Main Content */}
        <div className="flex-1 flex overflow-hidden">
          {/* Desktop Sidebar -- gated to History (which also covers an open
              Session, since selecting anything in the tree always flips
              primarySurface to "history" first: see handleProjectSelect/
              handleSessionSelect below). Previously permanent on every
              surface, which meant unwanted permanent chrome on
              Home/Search/Insights -- the project tree should not be
              permanent on every screen, only a contextual pane on History
              and Session. The mobile Sheet variant below was already
              correctly on-demand; only this desktop block needed the
              gate. */}
          {!isMobile && primarySurface === "history" && (
            <div className="hidden md:block">
              <ProjectTree
                projects={projects}
                sessions={sessions}
                sessionsTotal={sessionsTotal}
                hasMoreSessions={hasMoreSessions}
                selectedProject={selectedProject}
                selectedSession={selectedSession}
                onProjectSelect={handleProjectSelect}
                onSessionSelect={handleSessionSelect}
                onLoadMoreSessions={loadMoreSessions}
                onGlobalStatsClick={handleGlobalStatsClick}
                isLoading={isLoadingProjects || isLoadingSessions}
                isLoadingMoreSessions={isLoadingMoreSessions}
                isViewingGlobalStats={isViewingGlobalStats}
                width={isSidebarCollapsed ? undefined : sidebarWidth}
                isResizing={isSidebarResizing}
                onResizeStart={handleSidebarResizeStart}
                groupingMode={groupingMode}
                worktreeGroups={worktreeGroups}
                directoryGroups={directoryGroups}
                ungroupedProjects={ungroupedProjects}
                onGroupingModeChange={handleGroupingModeChange}
                onHideProject={hideProject}
                onUnhideProject={unhideProject}
                isProjectHidden={isProjectHidden}
                isCollapsed={isSidebarCollapsed}
                onToggleCollapse={handleToggleSidebar}
                asideId="project-explorer"
              />
            </div>
          )}

          {/* Main Content Area */}
          <main
            id="main-content"
            tabIndex={-1}
            className="flex-1 flex flex-col min-w-0 bg-background pb-14 md:pb-0"
          >
            {/* Content Header for non-message views */}
            {primarySurface !== "search" && primarySurface === "insights" && (
              <div className="px-4 py-3 md:px-6 md:py-4 border-b border-border/50 bg-card/50">
                <div className="flex items-center gap-3">
                  <div className="w-9 h-9 rounded-lg bg-accent/10 flex items-center justify-center">
                    {isViewingGlobalStats ? (
                      <Database className="w-5 h-5 text-accent" />
                    ) : (
                      <BarChart3 className="w-5 h-5 text-accent" />
                    )}
                  </div>
                  <div>
                    <h2 className="text-sm font-semibold text-foreground">
                      {isViewingGlobalStats
                        ? t("analytics.globalOverview")
                        : t("analytics.dashboard")}
                    </h2>
                    <p className="text-xs text-muted-foreground">
                      {isViewingGlobalStats
                        ? globalOverviewDescription
                        : selectedSession?.summary || t("session.summaryNotFound")}
                    </p>
                  </div>
                </div>
              </div>
            )}

            {/* Content */}
            <div className="flex-1 overflow-hidden">
              {primarySurface === "search" ? (
                // Full-page Search surface, consuming the
                // same useGlobalSearch hook + SearchFilterBar/
                // SearchResultsList as the Cmd/Ctrl+K palette
                // (GlobalSearchModal) -- one search implementation, two
                // renderings, not a forked second search UI.
                <React.Suspense
                  fallback={
                    <div className="h-full flex items-center justify-center">
                      <LoadingSpinner size="lg" variant="accent" />
                    </div>
                  }
                >
                  <SearchSurface />
                </React.Suspense>
              ) : primarySurface === "insights" ? (
                // The Insights surface's five-tab structure
                // (Usage/Tools/Agents/Problems/Files, spec §16, with Files
                // folded in). InsightsSurface owns its
                // own scroll handling per tab (the Usage tab keeps the
                // same OverlayScrollbarsComponent wrap AnalyticsDashboard
                // always had).
                <React.Suspense
                  fallback={
                    <div className="h-full flex items-center justify-center">
                      <LoadingSpinner size="lg" variant="accent" />
                    </div>
                  }
                >
                  <InsightsSurface
                    isViewingGlobalStats={isViewingGlobalStats}
                    onSessionSelect={handleSessionSelect}
                  />
                </React.Suspense>
              ) : selectedSession ? (
                // SessionView wraps the existing,
                // unchanged MessageViewer with the intelligence header +
                // Overview/Conversation/Tools/Files/Agents tab strip
                // (spec §14). MessageNavigator now lives inside SessionView
                // (Conversation tab only -- it's a minimap of the message
                // list, which the other tabs don't have).
                <SessionErrorBoundary key={selectedSession.file_path}>
                  <SessionView
                    messages={messages}
                    isLoading={isLoading}
                    selectedSession={selectedSession}
                    sessionSearch={sessionSearch}
                    onSearchChange={setSessionSearchQuery}
                    onFilterTypeChange={setSearchFilterType}
                    onClearSearch={clearSessionSearch}
                    onNextMatch={goToNextMatch}
                    onPrevMatch={goToPrevMatch}
                    navigatorWidth={navigatorWidth}
                    isNavigatorResizing={isNavigatorResizing}
                    onNavigatorResizeStart={handleNavigatorResizeStart}
                    isNavigatorOpen={isNavigatorOpen}
                    onToggleNavigator={toggleNavigator}
                  />
                </SessionErrorBoundary>
              ) : primarySurface === "home" ? (
                // Home's four sections (spec §9-11), consuming
                // archive_db::insights aggregations.
                // Reuses the same handleSessionSelect every other surface's
                // drill-down already uses.
                <HomeSurface onSessionSelect={handleSessionSelect} />
              ) : primarySurface === "history" ? (
                // History's default view (spec §13): a cross-project,
                // date-grouped session list. Reuses the
                // same handleSessionSelect the sidebar's ProjectTree already
                // uses, so opening a session behaves identically regardless
                // of where it was clicked from.
                <SessionDateList
                  selectedSession={selectedSession}
                  onSessionSelect={handleSessionSelect}
                />
              ) : (
                /* Empty State */
                <div className="h-full flex items-center justify-center">
                  <div className="text-center max-w-sm mx-auto">
                    <div className="w-20 h-20 rounded-2xl bg-muted/50 flex items-center justify-center mx-auto mb-6">
                      <MessageSquare className="w-10 h-10 text-muted-foreground/50" />
                    </div>
                    <h3 className="text-lg font-medium text-foreground mb-2">
                      {t("session.select")}
                    </h3>
                    <p className="text-sm text-muted-foreground">
                      {t("session.selectDescription")}
                    </p>
                  </div>
                </div>
              )}
            </div>
          </main>
        </div>

        {/* Status Bar (desktop only) */}
        <footer className="h-7 px-4 hidden md:flex items-center justify-between bg-sidebar border-t border-border/50 text-2xs text-muted-foreground">
          <div className="flex items-center gap-3 font-mono tabular-nums">
            <span>
              {isDesktop
                ? t("status.versionLabel", "v{{version}}", {
                    version: appVersion,
                  })
                : t("status.webMode", "Web")}
            </span>
            <span className="text-border">&bull;</span>
            <span>{t("project.count", { count: projects.length })}</span>
            <span className="text-border">&bull;</span>
            <span>{t("session.count", { count: sessions.length })}</span>
            {selectedSession && computed.isMessagesView && (
              <>
                <span className="text-border">&bull;</span>
                <span>
                  {t("message.count", { count: messages.length })}
                  {hasMoreMessages ? "+" : ""}
                </span>
              </>
            )}
          </div>

          {(isLoading ||
            isLoadingProjects ||
            isLoadingSessions ||
            isLoadingMessages ||
            computed.isAnyLoading) && (
            <div className="flex items-center gap-1.5">
              <LoadingSpinner size="xs" variant="muted" />
              <span>
                {isLoading
                  ? t("status.initializing")
                  : isLoadingProjects
                    ? t("status.scanning")
                    : isLoadingSessions
                      ? t("status.loadingSessions")
                      : isLoadingMessages
                        ? t("status.loadingMessages")
                        : computed.isAnyLoading
                          ? t("status.loadingStats")
                          : null}
              </span>
            </div>
          )}
        </footer>

        <div
          className="sr-only"
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          {liveStatusMessage}
        </div>

        {/* Update Manager (desktop only) */}
        <DesktopOnly>
          <SimpleUpdateManager updater={updater} />
        </DesktopOnly>

        {/* Mobile Bottom Tab Bar */}
        {isMobile && (
          <BottomTabBar
            activeSurface={primarySurface}
            onOpenSidebar={() => setIsMobileSidebarOpen(true)}
            onSwitchSurface={(surface) => {
              switch (surface) {
                case "history":
                  analyticsActions.switchToMessages();
                  break;
                case "search":
                  setPrimarySurface("search");
                  break;
                case "insights":
                  if (selectedProject) {
                    void analyticsActions.switchToAnalytics();
                  } else {
                    handleGlobalStatsClick();
                  }
                  break;
              }
            }}
          />
        )}

        {/* Mobile Navigator Sheet */}
        {isMobile && selectedSession && computed.isMessagesView && (
          <MobileNavigatorSheet messages={messages} />
        )}
      </div>

      {/* Modals */}
      <ModalContainer />
      <ArchiveSyncPreferencePrompt />
    </TooltipProvider>
  );
};
