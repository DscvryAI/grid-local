import React, { useEffect, useMemo, useRef, useState } from "react";
import { CheckCheck, Copy, DownloadCloud, Loader2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/store/useAppStore";
import type { ClaudeSession } from "@/types";
import { useSessionBatchActions } from "../hooks/useSessionBatchActions";

interface SessionSelectionBarProps {
  /** All sessions loaded for the project (used to resolve selected IDs). */
  allSessions: ClaudeSession[];
  /** Sessions currently visible after search/filter (drives "Select all"). */
  visibleSessions: ClaudeSession[];
}

const actionButtonClass = cn(
  "flex items-center gap-1 rounded px-1.5 py-1 text-2xs font-medium transition-colors",
  "disabled:opacity-40 disabled:pointer-events-none"
);

export const SessionSelectionBar: React.FC<SessionSelectionBarProps> = ({
  allSessions,
  visibleSessions,
}) => {
  const { t } = useTranslation();

  const sessionSelectionIds = useAppStore((s) => s.sessionSelectionIds);
  const setSessionSelectionIds = useAppStore((s) => s.setSessionSelectionIds);
  const clearSessionSelection = useAppStore((s) => s.clearSessionSelection);
  const exitSessionSelectionMode = useAppStore((s) => s.exitSessionSelectionMode);
  // Session-list pagination state: the project may have more sessions on disk
  // than are loaded (250/page). "Select all" must not silently mean "select
  // the loaded page" when that's the case.
  const sessionsTotal = useAppStore((s) => s.sessionsTotal);
  const hasMoreSessions = useAppStore((s) => s.hasMoreSessions);
  const loadMoreSessions = useAppStore((s) => s.loadMoreSessions);

  const [isLoadingAll, setIsLoadingAll] = useState(false);
  // Fresh view of the visible sessions for the async load-all loop — the
  // closure would otherwise select from a stale, pre-load list.
  const visibleSessionsRef = useRef(visibleSessions);
  useEffect(() => {
    visibleSessionsRef.current = visibleSessions;
  });

  const { copyIds } = useSessionBatchActions();

  const idSet = useMemo(() => new Set(sessionSelectionIds), [sessionSelectionIds]);

  const selectedSessions = useMemo(
    () => allSessions.filter((s) => idSet.has(s.session_id)),
    [allSessions, idSet]
  );

  const selectedCount = selectedSessions.length;

  const allVisibleSelected =
    visibleSessions.length > 0 &&
    visibleSessions.every((s) => idSet.has(s.session_id));

  // Escape leaves selection mode.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") exitSessionSelectionMode();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [exitSessionSelectionMode]);

  const handleToggleAll = () => {
    if (allVisibleSelected) {
      clearSessionSelection();
    } else {
      setSessionSelectionIds(visibleSessions.map((s) => s.session_id));
    }
  };

  // Page in every remaining session, then select everything visible. Bounded
  // by a stall check (no growth between iterations aborts the loop) so a
  // backend that never flips hasMore cannot spin forever.
  const handleLoadAllAndSelect = async () => {
    if (isLoadingAll) return;
    setIsLoadingAll(true);
    try {
      let previousCount = -1;
      while (
        useAppStore.getState().hasMoreSessions &&
        useAppStore.getState().isSessionSelectionMode
      ) {
        const loaded = useAppStore.getState().sessions.length;
        if (loaded === previousCount) break;
        previousCount = loaded;
        await loadMoreSessions();
      }
      if (useAppStore.getState().isSessionSelectionMode) {
        setSessionSelectionIds(
          visibleSessionsRef.current.map((s) => s.session_id)
        );
      }
    } catch (error) {
      const description =
        error instanceof Error ? error.message : String(error);
      console.error("[session selection] load-all failed", error);
      toast.error(
        t("session.selection.loadAllError", "Failed to load all sessions"),
        { description }
      );
    } finally {
      setIsLoadingAll(false);
    }
  };

  return (
    <div className="sticky top-0 z-10 flex flex-col gap-1.5 border-b border-border/30 bg-sidebar/95 px-2 py-1.5 backdrop-blur-sm">
      <div className="flex items-center justify-between">
        <span className="text-2xs font-semibold text-foreground">
          {t("session.selection.count", {
            count: selectedCount,
            defaultValue: "{{count}} selected",
          })}
        </span>
        <button
          type="button"
          onClick={exitSessionSelectionMode}
          className="rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground"
          aria-label={t("session.selection.exit", "Exit selection")}
          title={t("session.selection.exit", "Exit selection")}
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {hasMoreSessions && (
        <div className="flex items-center gap-1 text-2xs text-amber-600 dark:text-amber-400">
          <span>
            {t("session.selection.partialNotice", {
              loaded: allSessions.length,
              total: sessionsTotal,
              defaultValue:
                "{{loaded}} of {{total}} sessions loaded — selection covers loaded sessions only",
            })}
          </span>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-1">
        <button
          type="button"
          onClick={handleToggleAll}
          className={cn(actionButtonClass, "text-muted-foreground hover:bg-muted/50 hover:text-foreground")}
        >
          <CheckCheck className="h-3 w-3" />
          <span>
            {allVisibleSelected
              ? t("session.selection.clear", "Clear")
              : t("session.selection.selectAllCount", {
                  count: visibleSessions.length,
                  defaultValue: "Select all ({{count}})",
                })}
          </span>
        </button>

        {hasMoreSessions && (
          <button
            type="button"
            onClick={() => void handleLoadAllAndSelect()}
            disabled={isLoadingAll}
            className={cn(actionButtonClass, "text-muted-foreground hover:bg-muted/50 hover:text-foreground")}
          >
            {isLoadingAll ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <DownloadCloud className="h-3 w-3" />
            )}
            <span>
              {isLoadingAll
                ? t("session.selection.loadingAll", {
                    loaded: allSessions.length,
                    total: sessionsTotal,
                    defaultValue: "Loading {{loaded}}/{{total}}...",
                  })
                : t("session.selection.loadAllAndSelect", {
                    count: sessionsTotal,
                    defaultValue: "Load & select all {{count}}",
                  })}
            </span>
          </button>
        )}

        <button
          type="button"
          onClick={() => copyIds(selectedSessions)}
          disabled={selectedCount === 0}
          className={cn(actionButtonClass, "ml-auto text-muted-foreground hover:bg-muted/50 hover:text-foreground")}
        >
          <Copy className="h-3 w-3" />
          <span>{t("session.selection.copyIds", "Copy IDs")}</span>
        </button>
      </div>
    </div>
  );
};
