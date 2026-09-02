import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, ChevronRight } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { formatDateCompact, formatRelativeTime } from "@/utils/time";
import { describeVerificationStatus } from "../helpers/verificationStatusDisplay";
import type { FileEvent, VerificationStatus } from "../helpers/sessionIntelligence";

interface FilesTabProps {
  fileEvents: FileEvent[];
  /** The session's own verification status -- session-level, not
   * per-file: no per-file verification concept exists (a file isn't
   * individually "tested"), so the honest, real answer for "related
   * validation" is the same verification signal the Overview tab's
   * decision brief already shows, surfaced again here in the context a
   * file's own history is being reviewed. */
  verification: VerificationStatus;
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * Distinct files touched by a write-capable tool (spec §14's Files
 * tab). Clicking a row expands its full touch sequence -- a disclosed,
 * narrower slice of "contextual evidence rail": selecting from this
 * EXISTING list, not from raw inline transcript content, which would
 * need much deeper MessageViewer instrumentation -- plus the session's
 * own verification status for "related validation" context.
 */
export const FilesTab: React.FC<FilesTabProps> = ({ fileEvents, verification }) => {
  const { t } = useTranslation();
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const verificationDisplay = describeVerificationStatus(verification, t);

  if (fileEvents.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("session.tabs.filesEmpty")}
      </div>
    );
  }

  return (
    <div className="flex-1 space-y-1 overflow-y-auto px-4 py-3">
      {fileEvents.map((event) => {
        const isExpanded = expandedPath === event.filePath;
        return (
          <div key={event.filePath}>
            <button
              type="button"
              onClick={() => setExpandedPath((prev) => (prev === event.filePath ? null : event.filePath))}
              className="flex w-full items-center gap-3 rounded-md px-2 py-1.5 text-left hover:bg-muted/50"
            >
              <ChevronRight
                className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${isExpanded ? "rotate-90" : ""}`}
              />
              <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm text-foreground" title={event.filePath}>
                  {basename(event.filePath)}
                </div>
                <div className="truncate text-2xs text-muted-foreground">{event.filePath}</div>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                {event.tools.map((tool) => (
                  <Badge key={tool} size="sm" className="rounded px-1 py-0 text-2xs">
                    {tool}
                  </Badge>
                ))}
              </div>
              <span className="shrink-0 text-2xs text-muted-foreground">
                {t("session.tabs.filesTouchCount", { count: event.count })}
              </span>
              <span className="w-32 shrink-0 text-right text-2xs text-muted-foreground">
                {formatDateCompact(event.lastTouched)}
              </span>
            </button>
            {isExpanded && (
              <div className="ml-9 mb-1 space-y-1.5 rounded-md border border-border/50 bg-muted/30 px-3 py-2">
                <p className="text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {t("session.tabs.filesChangeLifecycle", "Change lifecycle")}
                </p>
                <ul className="space-y-0.5">
                  {event.touches.map((touch, index) => (
                    <li key={index} className="flex items-center gap-2 text-xs text-foreground">
                      <span className="text-muted-foreground">{formatRelativeTime(touch.timestamp)}</span>
                      <Badge size="sm" className="rounded px-1 py-0 text-2xs">
                        {touch.tool}
                      </Badge>
                    </li>
                  ))}
                </ul>
                {verificationDisplay && (
                  <p className="border-t border-border/50 pt-1.5">
                    <span className="text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                      {t("session.tabs.filesRelatedValidation", "Related validation")}:
                    </span>{" "}
                    <span className={`text-xs ${verificationDisplay.className}`}>
                      {verificationDisplay.text}
                    </span>
                  </p>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};
