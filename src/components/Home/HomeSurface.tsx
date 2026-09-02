import React, { useEffect, useState } from "react";
import { OverlayScrollbarsComponent } from "overlayscrollbars-react";
import { fetchSinceLastVisitSummary, recordVisit } from "@/services/insightsApi";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import { useArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import { ContinueSection } from "./sections/ContinueSection";
import { SinceLastVisitSection } from "./sections/SinceLastVisitSection";
import { ThisWeekSection } from "./sections/ThisWeekSection";
import { ThingsWorthLookingAtSection } from "./sections/ThingsWorthLookingAtSection";
import { RecentWorkSection } from "./sections/RecentWorkSection";
import type { ClaudeSession, SinceLastVisitSummary } from "@/types";

interface HomeSurfaceProps {
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Home: "What should I know about my AI coding activity right now?"
 * Max 5 sections, no charts, every number drills down. Owns the
 * since-last-visit fetch/record-visit sequencing at this
 * top level (not inside `SinceLastVisitSection`) because the backend's
 * own contract requires reading BEFORE recording -- doing that in one
 * place guarantees it only happens once per Home visit, regardless of
 * how the section below re-renders.
 */
export const HomeSurface: React.FC<HomeSurfaceProps> = ({ onSessionSelect }) => {
  const [sinceLastVisit, setSinceLastVisit] = useState<SinceLastVisitSummary | null>(null);
  const [isLoadingSinceLastVisit, setIsLoadingSinceLastVisit] = useState(true);
  // Computed once here (not per-section) so every section agrees on the
  // archive's health in the same render, matching this file's own existing
  // precedent of centralizing shared fetches at the top level.
  const archiveHealth = useArchiveIndexHealth();

  // "Time to first populated Home" is proxied by the archive index's own
  // "ready" transition, since that's
  // exactly the condition each section below already gates its own real
  // content on -- recorded once, backend-deduped (`apply_event` only sets
  // `firstPopulatedHomeAt` the first time), so re-visiting Home later is a
  // no-op here.
  useEffect(() => {
    if (archiveHealth.state === "ready") {
      void recordDiagnosticsEvent({ kind: "homePopulated" });
    }
  }, [archiveHealth.state]);

  useEffect(() => {
    let cancelled = false;
    fetchSinceLastVisitSummary()
      .then((summary) => {
        if (!cancelled) setSinceLastVisit(summary);
      })
      .catch((err) => {
        console.error("Failed to load since-last-visit summary:", err);
      })
      .finally(() => {
        if (cancelled) return;
        setIsLoadingSinceLastVisit(false);
        // Record AFTER reading, per record_visit's own contract -- an
        // error reading the summary above still shouldn't leave the
        // last-visit boundary stuck in the past forever.
        void recordVisit().catch((err) =>
          console.error("Failed to record visit:", err)
        );
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <OverlayScrollbarsComponent
      className="h-full"
      options={{ scrollbars: { theme: "os-theme-custom", autoHide: "leave" } }}
    >
      <div className="mx-auto max-w-3xl space-y-8 p-4 md:p-6">
        {/* Order: Continue (one dominant continuation item) -> Needs
            attention (things-worth-looking-at) -> Changed (since-last-visit,
            then this week) -> Recent work. SinceLastVisit/ThisWeek are kept
            as separate, independently-gated sections rather than merged
            under one heading -- each owns its own archive-index-health
            empty state ("never hide load failures, only genuinely-empty
            content" invariant), and merging their async loading states
            under one shared heading risked silently breaking that guarantee
            for one of the two. */}
        <ContinueSection health={archiveHealth} onSessionSelect={onSessionSelect} />
        <ThingsWorthLookingAtSection health={archiveHealth} onSessionSelect={onSessionSelect} />
        <SinceLastVisitSection
          summary={sinceLastVisit}
          isLoading={isLoadingSinceLastVisit}
          health={archiveHealth}
          onSessionSelect={onSessionSelect}
        />
        <ThisWeekSection health={archiveHealth} onSessionSelect={onSessionSelect} />
        <RecentWorkSection health={archiveHealth} onSessionSelect={onSessionSelect} />
      </div>
    </OverlayScrollbarsComponent>
  );
};
