import { useEffect, useState } from "react";
import {
  getArchiveDbStatus,
  type ArchiveDbStatus,
  type FirstIndexProgressEvent,
} from "@/services/archiveSyncApi";
import { useAppStore } from "@/store/useAppStore";

export type ArchiveIndexHealth =
  | { state: "building"; progress: FirstIndexProgressEvent }
  | { state: "failed"; error: string }
  | { state: "never-built" }
  | { state: "ready"; status: ArchiveDbStatus };

/**
 * Shared empty-state signal for every Home/Insights surface backed by
 * `archive_db`: distinguishes "index actively building," "the last
 * index/sync attempt failed," "the index has never been populated" (e.g.
 * the mandatory first index was cancelled mid-run -- see
 * `AppLayout.tsx`'s cancel button), and "ready" (has real archive data;
 * the calling surface still decides its own "no data" copy for a
 * genuinely empty result). Without this, a surface can't tell "nothing
 * happened" apart from "nothing exists."
 *
 * Deliberately scoped to surfaces whose data comes ENTIRELY from
 * `archive_db` (Home's 4 sections, Insights' Problems/Agents tabs) -- Usage
 * and Tools mix in raw per-provider file scanning that works whether or not
 * `archive_db` has been built, so applying this taxonomy there would
 * misreport a real (if incomplete) result as an index problem.
 */
export function useArchiveIndexHealth(): ArchiveIndexHealth {
  const firstIndexProgress = useAppStore((s) => s.firstIndexProgress);
  const archiveIndexError = useAppStore((s) => s.archiveIndexError);
  const [status, setStatus] = useState<ArchiveDbStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    getArchiveDbStatus()
      .then((result) => {
        if (!cancelled) setStatus(result);
      })
      .catch((err) => {
        console.error("Failed to read archive_db status:", err);
      });
    return () => {
      cancelled = true;
    };
    // Re-check once an in-flight index finishes or a failure is recorded,
    // so a state that started "never-built"/"failed" can resolve to "ready"
    // without requiring the surface to remount.
  }, [firstIndexProgress, archiveIndexError]);

  if (firstIndexProgress) return { state: "building", progress: firstIndexProgress };
  if (archiveIndexError) return { state: "failed", error: archiveIndexError };
  if (!status || status.sessionCount === 0) return { state: "never-built" };
  return { state: "ready", status };
}
