import type { StateCreator } from "zustand";
import type { FullAppStore } from "./types";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";

/**
 * The four primary IA surfaces (spec §8). "home" is a placeholder that
 * AppLayout redirects to "history" immediately until the real Home
 * surface lands -- kept as a distinct value now so the
 * type shape doesn't need to change later.
 */
export type PrimarySurface = "home" | "history" | "search" | "insights";

/**
 * Session view's tab strip (spec §14). "conversation" is
 * the default -- opening a session must behave exactly as it always has
 * (today's unchanged `MessageViewer`), not land on a new tab first.
 */
export type SessionTab = "overview" | "conversation" | "tools" | "files" | "agents";

/**
 * Insights surface's tab strip (spec §16). "questions" is the default,
 * replacing the colorful metric-card/chart dashboard with a
 * question-led queue -- "usage," that dashboard, was the default before
 * this; it's unchanged and still fully reachable, just no longer what a
 * user lands on first.
 */
export type InsightsTab = "questions" | "usage" | "tools" | "agents" | "problems";

export interface NavigationSliceState {
    targetMessageUuid: string | null;
    shouldHighlightTarget: boolean;
    primarySurface: PrimarySurface;
    sessionTab: SessionTab;
    insightsTab: InsightsTab;
}

export interface NavigationSliceActions {
    navigateToMessage: (uuid: string) => void;
    clearTargetMessage: () => void;
    setPrimarySurface: (surface: PrimarySurface) => void;
    setSessionTab: (tab: SessionTab) => void;
    setInsightsTab: (tab: InsightsTab) => void;
}

export type NavigationSlice = NavigationSliceState & NavigationSliceActions;

export const createNavigationSlice: StateCreator<
    FullAppStore,
    [],
    [],
    NavigationSlice
> = (set, get) => ({
    targetMessageUuid: null,
    shouldHighlightTarget: false,
    primarySurface: "home",
    sessionTab: "conversation",
    insightsTab: "questions",

    navigateToMessage: (uuid) => {
        set({
            targetMessageUuid: uuid,
            shouldHighlightTarget: true
        });
    },

    clearTargetMessage: () => set({
        targetMessageUuid: null,
        shouldHighlightTarget: false
    }),

    setPrimarySurface: (surface) => {
        // Only count a genuine surface CHANGE for the per-surface visit
        // diagnostic -- re-selecting the already-active nav item, or a
        // redirect that lands back on the same surface, isn't a new visit.
        if (get().primarySurface !== surface) {
            void recordDiagnosticsEvent({ kind: "surfaceVisited", surface });
        }
        set({ primarySurface: surface });
    },

    setSessionTab: (tab) => set({ sessionTab: tab }),

    setInsightsTab: (tab) => set({ insightsTab: tab }),
});
