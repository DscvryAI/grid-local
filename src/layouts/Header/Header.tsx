import {
  Loader2,
  RefreshCw,
  BarChart3,
  Home,
  History,
  Search,
} from "lucide-react";

import { TooltipButton } from "@/shared/TooltipButton";
import { useAppStore } from "@/store/useAppStore";
import type { UseAnalyticsReturn } from "@/types/analytics";
import type { UseUpdaterReturn } from "@/hooks/useUpdater";
import { useModal } from "@/contexts/modal";

import { cn } from "@/lib/utils";
import { useTranslation } from "react-i18next";
import { isMacOS, isTauri } from "@/utils/platform";
import { SettingDropdown } from "./SettingDropdown";
import { SessionCopyMenu } from "./SessionCopyMenu";

interface HeaderProps {
  analyticsActions: UseAnalyticsReturn["actions"];
  analyticsComputed: UseAnalyticsReturn["computed"];
  updater: UseUpdaterReturn;
  handleGlobalStatsClick: () => void;
}

const SHORTCUT_LABEL = isMacOS() ? "⌘+K" : "Ctrl+K";

// macOS traffic-light buttons overlap the header in Tauri's Overlay
// titleBarStyle. Reserve space for them only when running in the desktop
// shell — the WebUI build has no overlay controls.
const HAS_MACOS_TRAFFIC_LIGHTS = isTauri() && isMacOS();

export const Header = ({
  analyticsActions,
  analyticsComputed,
  updater,
  handleGlobalStatsClick,
}: HeaderProps) => {
  const { t } = useTranslation();
  const { openModal } = useModal();

  const {
    selectedProject,
    selectedSession,
    isLoadingProjects,
    isLoadingSessions,
    isLoadingMessages,
    isRefreshingAllConversations,
    refreshAllConversations,
    refreshCurrentSession,
    primarySurface,
    setPrimarySurface,
    setSelectedSession,
  } = useAppStore();

  const computed = analyticsComputed;
  const isRefreshingConversations =
    isRefreshingAllConversations ||
    isLoadingProjects ||
    isLoadingSessions ||
    isLoadingMessages;

  const handleLoadAnalytics = async () => {
    if (!selectedProject) return;
    try {
      await analyticsActions.switchToAnalytics();
    } catch (error) {
      console.error("Failed to load analytics:", error);
    }
  };

  // Insights is a top-level surface (spec §8/§16): scoped to the current
  // project when one is selected, falling back to the same global
  // overview the sidebar's "Global Stats" entry point shows otherwise.
  const handleToggleInsights = () => {
    if (primarySurface === "insights") {
      analyticsActions.switchToMessages();
      return;
    }
    if (selectedProject) {
      void handleLoadAnalytics();
    } else {
      handleGlobalStatsClick();
    }
  };

  // History didn't return to its LIST view when a session was already
  // open -- switchToMessages() alone leaves `selectedSession` set, and
  // AppLayout's content switch renders MessageViewer whenever a session
  // is selected, regardless of primarySurface. Clicking History while a
  // session is open must behave like clicking a top-level nav item
  // normally does: return to that surface's list, not no-op on an
  // already-open session. Search's own close handler intentionally keeps
  // the session open -- this is a separate, narrower handler, not a
  // change to switchToMessages() itself.
  const handleReturnToHistoryList = () => {
    analyticsActions.switchToMessages();
    setSelectedSession(null);
  };

  // Home is its own destination, not a redirect to
  // History -- can't reuse switchToMessages() here, since that hardcodes
  // primarySurface to "history". Still needs the same analytics.
  // currentView reset switchToMessages() does (via the store directly,
  // not the analyticsActions hook object, which is recreated every
  // render): without it, a stale "analytics" currentView would keep
  // AppLayout's content switch rendering whatever was showing before,
  // since that check runs before the `primarySurface === "home"` branch.
  const handleGoHome = () => {
    useAppStore.getState().setAnalyticsCurrentView("messages");
    setSelectedSession(null);
    setPrimarySurface("home");
  };

  const handleToggleSearch = () => {
    if (primarySurface === "search") {
      // switchToMessages resets analytics.currentView back to "messages"
      // too, not just primarySurface -- without it, the content switch
      // (still keyed off the older analytics.currentView) can keep
      // rendering whatever was showing before Search was opened.
      analyticsActions.switchToMessages();
    } else {
      setPrimarySurface("search");
    }
  };

  return (
    <header
      id="app-header"
      role="banner"
      className={cn(
        "relative h-12 flex items-center justify-between px-4 bg-sidebar border-b border-border/50",
        HAS_MACOS_TRAFFIC_LIGHTS && "pl-[72px]"
      )}
    >
      {/* Full-header drag region — sits behind all content so the
          entire header is draggable. Interactive children (right-side
          buttons) sit above with their own pointer events; non-interactive
          children (logo, title) use pointer-events-none so clicks fall
          through to this layer. */}
      <div data-tauri-drag-region className="absolute inset-0" />

      {/* Left: Logo & Title */}
      <div className="relative z-10 flex items-center gap-2.5 min-w-0 pointer-events-none">
        <img
          src="/app-icon.png"
          alt="Claude Code History"
          className="w-6 h-6 hidden md:block"
        />
        <div className="flex flex-col min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <h1 className="text-sm font-semibold text-foreground hidden md:block">
              {t('common.appName')}
            </h1>
            {selectedProject && (
              <>
                <span className="text-muted-foreground/40 hidden md:block">/</span>
                <span className="text-sm text-muted-foreground truncate max-w-[180px]">
                  {selectedProject.name}
                </span>
              </>
            )}
            {!selectedProject && (
              <h1 className="text-sm font-semibold text-foreground md:hidden">
                {t('common.appName')}
              </h1>
            )}
          </div>
          {selectedSession ? (
            <p className="text-2xs text-muted-foreground truncate max-w-[280px] md:max-w-sm">
              <span className="text-muted-foreground/60 hidden md:inline">Session:</span>{" "}
              {selectedSession.summary ||
                `${t("session.title")} ${selectedSession.session_id.slice(-8)}`}
            </p>
          ) : (
            <p className="text-2xs text-muted-foreground hidden md:block">{t('common.appDescription')}</p>
          )}
        </div>
      </div>

      {/* Center: Quick Stats (when session selected) */}
      {selectedSession && computed.isMessagesView && (
        <div className="relative z-10 hidden lg:flex items-center gap-2">
          <SessionCopyMenu project={selectedProject} session={selectedSession} />
        </div>
      )}

      {/* Right: Actions */}
      <div className="relative z-10 flex items-center gap-1">
        {selectedSession && computed.isMessagesView && (
          <div className="lg:hidden">
            <SessionCopyMenu compact project={selectedProject} session={selectedSession} />
          </div>
        )}

        {/* Search button with shortcut hint */}
        <button
          onClick={() => openModal("globalSearch")}
          className="hidden md:inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-muted transition-colors border border-border/50 text-xs"
          aria-label={t("common.commandPalette")}
        >
          <Search className="w-3.5 h-3.5" />
          <span>{t("globalSearch.placeholder")}</span>
          <kbd className="ml-1 px-1 py-0.5 text-px10 font-mono bg-muted rounded border border-border">
            {SHORTCUT_LABEL}
          </kbd>
        </button>
        <button
          onClick={() => openModal("globalSearch")}
          className="md:hidden p-2 rounded-md text-muted-foreground hover:text-foreground hover:bg-muted transition-colors"
          aria-label={t("common.commandPalette")}
        >
          <Search className="w-5 h-5" />
        </button>

        {/* Global refresh */}
        <TooltipButton
          onClick={() => {
            void refreshAllConversations();
          }}
          disabled={isRefreshingConversations}
          className={cn(
            "p-2 rounded-md transition-colors",
            "text-muted-foreground hover:text-foreground hover:bg-muted",
            isRefreshingConversations && "opacity-70 cursor-not-allowed"
          )}
          content={t(
            "session.refreshAllConversations",
            "Refresh all conversations"
          )}
        >
          <RefreshCw
            className={cn("w-4 h-4", isRefreshingConversations && "animate-spin")}
          />
        </TooltipButton>

        {/* Primary surface nav (spec §8): always visible, not gated on a
            project/session being selected -- Home/History/Search/Insights
            are the app's four top-level destinations. */}
        <div className="hidden md:flex items-center gap-1">
          <NavButton
            icon={Home}
            label={t("common.nav.home", "Home")}
            isActive={primarySurface === "home"}
            onClick={handleGoHome}
          />
          <NavButton
            icon={History}
            label={t("common.nav.history", "History")}
            isActive={primarySurface === "history"}
            onClick={handleReturnToHistoryList}
          />
          <NavButton
            icon={Search}
            label={t("common.nav.search", "Search")}
            isActive={primarySurface === "search"}
            onClick={handleToggleSearch}
          />
          <NavButton
            icon={computed.isLoadingAnalytics ? Loader2 : BarChart3}
            label={t("common.nav.insights", "Insights")}
            isActive={primarySurface === "insights"}
            isLoading={computed.isLoadingAnalytics}
            onClick={handleToggleInsights}
            disabled={computed.isLoadingAnalytics}
          />

          {selectedSession && (
            <>
              {/* Divider */}
              <div className="w-px h-6 bg-border mx-2" />

              {/* Refresh */}
              <TooltipButton
                onClick={() => refreshCurrentSession()}
                disabled={isLoadingMessages}
                className={cn(
                  "p-2 rounded-md transition-colors",
                  "text-muted-foreground hover:text-foreground hover:bg-muted"
                )}
                content={t("session.refresh")}
              >
                <RefreshCw
                  className={cn("w-4 h-4", isLoadingMessages && "animate-spin")}
                />
              </TooltipButton>
            </>
          )}

        </div>

        {/* Settings Dropdown (visible on all sizes) */}
        <SettingDropdown updater={updater} />
      </div>
    </header>
  );
};

/* Navigation Button Component
   Renders a persistent text label beside its icon rather than relying on
   a hover tooltip -- icon-only nav depending on tooltips is slower to
   recognize than reading, and fails to give icon-only controls persistent
   accessible names. Plain <button>, not TooltipButton -- a tooltip
   restating text that's already visible is redundant, and the visible
   label is itself the accessible name now. */
interface NavButtonProps {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  isActive?: boolean;
  isLoading?: boolean;
  onClick: () => void;
  disabled?: boolean;
}

const NavButton = ({
  icon: Icon,
  label,
  isActive,
  isLoading,
  onClick,
  disabled,
}: NavButtonProps) => {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-xs font-medium transition-colors",
        "text-muted-foreground",
        isActive
          ? "bg-accent/10 text-accent"
          : "hover:bg-muted hover:text-foreground",
        disabled && "opacity-50 cursor-not-allowed"
      )}
    >
      <Icon className={cn("w-4 h-4 shrink-0", isLoading && "animate-spin")} />
      <span className="whitespace-nowrap">{label}</span>
    </button>
  );
};
