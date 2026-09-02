/**
 * Analytics Slice
 *
 * Handles analytics dashboard state.
 */

import type {
  ProjectStatsSummary,
  SessionComparison,
  MetricMode,
  StatsMode,
} from "../../types";
import type { AnalyticsState, AnalyticsViewType } from "../../types/analytics";
import { initialAnalyticsState } from "../../types/analytics";
import type { StateCreator } from "zustand";
import type { FullAppStore } from "./types";

// ============================================================================
// State Interface
// ============================================================================

export interface AnalyticsSliceState {
  analytics: AnalyticsState;
}

export interface AnalyticsSliceActions {
  setAnalyticsCurrentView: (view: AnalyticsViewType) => void;
  setAnalyticsStatsMode: (mode: StatsMode) => void;
  setAnalyticsMetricMode: (mode: MetricMode) => void;
  setAnalyticsProjectSummary: (summary: ProjectStatsSummary | null) => void;
  setAnalyticsProjectConversationSummary: (summary: ProjectStatsSummary | null) => void;
  setAnalyticsSessionComparison: (comparison: SessionComparison | null) => void;
  setAnalyticsLoadingProjectSummary: (loading: boolean) => void;
  setAnalyticsLoadingSessionComparison: (loading: boolean) => void;
  setAnalyticsProjectSummaryError: (error: string | null) => void;
  setAnalyticsSessionComparisonError: (error: string | null) => void;
  resetAnalytics: () => void;
  clearAnalyticsErrors: () => void;
}

export type AnalyticsSlice = AnalyticsSliceState & AnalyticsSliceActions;

// ============================================================================
// Initial State
// ============================================================================

const initialAnalyticsSliceState: AnalyticsSliceState = {
  analytics: initialAnalyticsState,
};

// ============================================================================
// Slice Creator
// ============================================================================

export const createAnalyticsSlice: StateCreator<
  FullAppStore,
  [],
  [],
  AnalyticsSlice
> = (set) => ({
  ...initialAnalyticsSliceState,

  setAnalyticsCurrentView: (view: AnalyticsViewType) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        currentView: view,
      },
    }));
  },

  setAnalyticsStatsMode: (mode: StatsMode) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        statsMode: mode,
      },
    }));
  },

  setAnalyticsMetricMode: (mode: MetricMode) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        metricMode: mode,
      },
    }));
  },

  setAnalyticsProjectSummary: (summary: ProjectStatsSummary | null) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        projectSummary: summary,
      },
    }));
  },

  setAnalyticsProjectConversationSummary: (summary: ProjectStatsSummary | null) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        projectConversationSummary: summary,
      },
    }));
  },

  setAnalyticsSessionComparison: (comparison: SessionComparison | null) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        sessionComparison: comparison,
      },
    }));
  },

  setAnalyticsLoadingProjectSummary: (loading: boolean) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        isLoadingProjectSummary: loading,
      },
    }));
  },

  setAnalyticsLoadingSessionComparison: (loading: boolean) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        isLoadingSessionComparison: loading,
      },
    }));
  },

  setAnalyticsProjectSummaryError: (error: string | null) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        projectSummaryError: error,
      },
    }));
  },

  setAnalyticsSessionComparisonError: (error: string | null) => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        sessionComparisonError: error,
      },
    }));
  },

  resetAnalytics: () => {
    set({ analytics: initialAnalyticsState });
  },

  clearAnalyticsErrors: () => {
    set((state) => ({
      analytics: {
        ...state.analytics,
        projectSummaryError: null,
        sessionComparisonError: null,
      },
    }));
  },
});
