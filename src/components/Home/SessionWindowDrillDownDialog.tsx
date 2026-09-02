import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui";
import { fetchSessionsInWindow } from "@/services/insightsApi";
import { formatDateCompact } from "@/utils/time";
import { formatNumber } from "@/utils/formatters";
import { toClaudeSessionStub } from "../Insights/helpers/toClaudeSessionStub";
import type { ClaudeSession, SessionListItem } from "@/types";

interface SessionWindowDrillDownDialogProps {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  start: string;
  end: string;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Spec §9.2's "Numbers must drill down" -- clicking a Home session count
 * opens this dialog with the actual sessions behind it, fetched on demand
 * via `list_sessions_in_window` (Claude-only, matching every other
 * `archive_db` query today). Deliberately a lightweight, self-contained
 * dialog rather than routing to History: the window here is an exact
 * `[start, end]` boundary tied to Home's own summary, not one of
 * History's Today/Yesterday/date-range filters.
 */
export const SessionWindowDrillDownDialog: React.FC<SessionWindowDrillDownDialogProps> = ({
  isOpen,
  onClose,
  title,
  start,
  end,
  onSessionSelect,
}) => {
  const { t } = useTranslation();
  const [items, setItems] = useState<SessionListItem[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setItems(null);
    setLoadError(null);
    fetchSessionsInWindow(start, end)
      .then((results) => {
        if (!cancelled) setItems(results);
      })
      .catch((err) => {
        if (!cancelled) setLoadError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, start, end]);

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        {items === null && !loadError && (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        )}
        {loadError && (
          <div className="py-4 text-center text-sm text-destructive">{loadError}</div>
        )}
        {items !== null && items.length === 0 && (
          <div className="py-4 text-center text-sm text-muted-foreground">
            {t("home.drillDown.empty")}
          </div>
        )}
        {items !== null && items.length > 0 && (
          <ul className="max-h-[60vh] divide-y divide-border overflow-y-auto rounded-md border">
            {items.map((item) => (
              <li key={item.session_id}>
                <button
                  type="button"
                  onClick={() => {
                    onSessionSelect(
                      toClaudeSessionStub(item.session_id, item.project_name, item.summary)
                    );
                    onClose();
                  }}
                  className="flex w-full flex-col gap-1 p-3 text-left hover:bg-muted/50"
                >
                  <div className="truncate text-sm text-foreground">
                    {item.summary || item.project_name}
                  </div>
                  <div className="flex gap-2 text-xs text-muted-foreground">
                    <span className="truncate">{item.project_name}</span>
                    {item.last_message_time && (
                      <>
                        <span aria-hidden>·</span>
                        <span>{formatDateCompact(item.last_message_time)}</span>
                      </>
                    )}
                    <span aria-hidden>·</span>
                    <span>
                      {t("home.drillDown.tokenCount", {
                        tokens: formatNumber(item.total_tokens),
                      })}
                    </span>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </DialogContent>
    </Dialog>
  );
};

