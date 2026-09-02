import React from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Ban, Database, Loader2, RefreshCw } from "lucide-react";
import type { FirstIndexProgressEvent } from "@/services/archiveSyncApi";
import { getProviderLabel } from "@/utils/providers";
import { useAppStore } from "@/store/useAppStore";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ProviderId } from "@/types";

export type EmptyStateReason =
  | "no-data"
  | "unsupported-provider"
  | "building"
  | "failed"
  | "never-built";

interface ArchiveEmptyStateProps {
  reason: EmptyStateReason;
  /** Required for "no-data" -- each surface knows its own genuinely-empty
   * copy, this component doesn't guess it. Ignored for the other reasons,
   * which use shared, consistent copy across every surface. */
  title?: string;
  description?: string;
  /** Required for "unsupported-provider". */
  provider?: ProviderId | string;
  /** Shown only for "building". */
  progress?: FirstIndexProgressEvent | null;
  /** Shown only for "failed" -- retries whichever of the first index or a
   * background sync is appropriate for the archive's current state. */
  onRetry?: () => void;
  className?: string;
}

/**
 * Shared empty-state primitive for the 4-reason taxonomy: no data /
 * unsupported provider / index not yet built / index build failed. One
 * visual shape so a user sees the same
 * pattern everywhere `archive_db`-backed data can be silently absent,
 * rather than each surface inventing its own ad hoc wording -- see
 * `useArchiveIndexHealth`'s doc comment for which surfaces this applies to
 * and why some (Usage/Tools) are deliberately excluded.
 */
export const ArchiveEmptyState: React.FC<ArchiveEmptyStateProps> = ({
  reason,
  title,
  description,
  provider,
  progress,
  onRetry,
  className,
}) => {
  const { t } = useTranslation();

  const content = (() => {
    switch (reason) {
      case "building":
        return {
          icon: <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />,
          title: t("emptyState.building.title", "Grid is still building your index"),
          description: progress
            ? t("emptyState.building.progress", "{{done}} of {{total}} sources scanned", {
                done: progress.phasesDone + 1,
                total: progress.phasesTotal,
              })
            : t(
                "emptyState.building.description",
                "This happens once -- check back in a moment."
              ),
        };
      case "failed":
        return {
          icon: <AlertTriangle className="h-5 w-5 text-destructive" />,
          title: t("emptyState.failed.title", "Grid's local index couldn't be built"),
          description: t(
            "emptyState.failed.description",
            "Something went wrong while reading your history. You can try again."
          ),
        };
      case "never-built":
        return {
          icon: <Database className="h-5 w-5 text-muted-foreground" />,
          title: t("emptyState.neverBuilt.title", "Your local index hasn't been built yet"),
          description: t(
            "emptyState.neverBuilt.description",
            "Grid needs to scan your history once before it can show this."
          ),
        };
      case "unsupported-provider":
        return {
          icon: <Ban className="h-5 w-5 text-muted-foreground" />,
          title:
            title ??
            t("emptyState.unsupportedProvider.title", "{{provider}} isn't fully supported yet", {
              provider: getProviderLabel(t, provider),
            }),
          description:
            description ??
            t(
              "emptyState.unsupportedProvider.description",
              "Grid can browse this history but doesn't compute this insight for it yet."
            ),
        };
      case "no-data":
      default:
        return {
          icon: <Database className="h-5 w-5 text-muted-foreground" />,
          title: title ?? t("emptyState.noData.title", "Nothing here yet"),
          description,
        };
    }
  })();

  return (
    <div
      className={
        "flex flex-col items-center justify-center gap-2 px-4 py-6 text-center " +
        (className ?? "")
      }
    >
      {content.icon}
      <p className="text-sm font-medium text-foreground">{content.title}</p>
      {content.description && (
        <p className="max-w-sm text-xs text-muted-foreground">{content.description}</p>
      )}
      {reason === "failed" && onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="mt-1 inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1 text-xs font-medium text-foreground hover:bg-muted/50"
        >
          <RefreshCw className="h-3 w-3" />
          {t("emptyState.failed.retry", "Try again")}
        </button>
      )}
    </div>
  );
};

/**
 * Convenience wrapper for the common case: a surface already holds a
 * non-"ready" {@link ArchiveIndexHealth} value (building/failed/never-built)
 * and just needs it rendered, with "Try again" wired to the store's own
 * {@link retryArchiveIndex} action -- avoids repeating that plumbing in
 * every one of the 6 consuming surfaces.
 */
export const ArchiveHealthEmptyState: React.FC<{
  health: Exclude<ArchiveIndexHealth, { state: "ready" }>;
  className?: string;
}> = ({ health, className }) => {
  const retryArchiveIndex = useAppStore((s) => s.retryArchiveIndex);

  if (health.state === "building") {
    return <ArchiveEmptyState reason="building" progress={health.progress} className={className} />;
  }
  if (health.state === "failed") {
    return (
      <ArchiveEmptyState
        reason="failed"
        onRetry={() => void retryArchiveIndex()}
        className={className}
      />
    );
  }
  return <ArchiveEmptyState reason="never-built" className={className} />;
};
