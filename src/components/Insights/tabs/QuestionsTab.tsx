import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  BarChart3,
  Bot,
  Loader2,
  Search,
  Sparkles,
  Terminal,
} from "lucide-react";
import { fetchThingsWorthLookingAt } from "@/services/insightsApi";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { InsightCard } from "@/types";
import type { InsightsTab } from "@/store/slices/navigationSlice";

interface QuestionsTabProps {
  health: ArchiveIndexHealth;
}

interface Question {
  key: string;
  icon: React.ComponentType<{ className?: string }>;
  onSelect: () => void;
}

/**
 * Insights' default landing tab: a question-led queue rather than a
 * colorful metric dashboard -- a metric card is not an insight merely
 * because the number is available. "Usage" (a thin wrapper around
 * `AnalyticsDashboard` + `TokenStatsViewer`) stays fully intact and
 * reachable, just no longer what a user lands on first. The other tabs
 * (Usage/Tools/Agents/Problems) are untouched.
 *
 * The "Needs attention" summary reuses `things_worth_looking_at` --
 * the SAME aggregated, already-ranked query Home's own
 * `ThingsWorthLookingAtSection` uses -- rather than a new backend
 * aggregation. Counts are grouped by real card kind; nothing here is
 * invented. A concept like "unverified change sets" that this codebase
 * has no tracked data for is deliberately not fabricated -- left out of
 * the summary rather than guessed at.
 *
 * Each question routes to the existing tab/surface that already answers
 * it -- no new views were built for them. "Which sources are incomplete
 * or stale?" answers inline instead of navigating, since it's really
 * asking about `health` itself, already available here. "What prior
 * work can I reuse?" routes to Search -- the closest honest existing
 * capability; this codebase has no separate similar-solution-retrieval
 * feature.
 */
export const QuestionsTab: React.FC<QuestionsTabProps> = ({ health }) => {
  const { t } = useTranslation();
  const selectedProject = useAppStore((s) => s.selectedProject);
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);
  const setInsightsTab = useAppStore((s) => s.setInsightsTab);
  const [cards, setCards] = useState<InsightCard[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchThingsWorthLookingAt(selectedProject?.path)
      .then((results) => {
        if (!cancelled) setCards(results);
      })
      .catch((err) => {
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : String(err));
          setCards([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selectedProject?.path]);

  const goTo = (tab: InsightsTab) => () => setInsightsTab(tab);

  const questions: Question[] = [
    { key: "failing", icon: Terminal, onSelect: goTo("problems") },
    { key: "agentReview", icon: Bot, onSelect: goTo("agents") },
    { key: "activity", icon: BarChart3, onSelect: goTo("usage") },
    { key: "reuse", icon: Search, onSelect: () => setPrimarySurface("search") },
  ];

  const attentionCounts = (cards ?? []).reduce<Record<string, number>>((acc, card) => {
    acc[card.kind] = (acc[card.kind] ?? 0) + 1;
    return acc;
  }, {});
  const attentionParts = [
    attentionCounts.RepeatedCommandFailure &&
      t("insights.questions.countFailures", { count: attentionCounts.RepeatedCommandFailure }),
    attentionCounts.RepeatedError &&
      t("insights.questions.countErrors", { count: attentionCounts.RepeatedError }),
    attentionCounts.LargeAgentRun &&
      t("insights.questions.countAgentRuns", { count: attentionCounts.LargeAgentRun }),
    attentionCounts.HighTokenSession &&
      t("insights.questions.countHighToken", { count: attentionCounts.HighTokenSession }),
    attentionCounts.VerificationGap &&
      t("insights.questions.countVerificationGaps", { count: attentionCounts.VerificationGap }),
  ].filter(Boolean);

  return (
    <div className="flex-1 space-y-8 overflow-y-auto px-4 py-4">
      <section>
        <h2 className="mb-2 flex items-center gap-2 text-sm font-semibold text-foreground">
          <AlertTriangle className="h-4 w-4 text-muted-foreground" />
          {t("insights.questions.needsAttention")}
        </h2>
        {cards === null ? (
          <div className="h-6 w-48 animate-pulse rounded bg-muted/30" />
        ) : loadError ? (
          <p className="text-sm text-destructive" role="alert">
            {loadError}
          </p>
        ) : attentionParts.length > 0 ? (
          <p className="text-sm text-muted-foreground">{attentionParts.join(" · ")}</p>
        ) : health.state !== "ready" ? (
          <ArchiveHealthEmptyState health={health} />
        ) : (
          <p className="text-sm text-muted-foreground">{t("insights.questions.allClear")}</p>
        )}
      </section>

      <section>
        <h2 className="mb-2 flex items-center gap-2 text-sm font-semibold text-foreground">
          <Sparkles className="h-4 w-4 text-muted-foreground" />
          {t("insights.questions.title")}
        </h2>
        <div className="space-y-1">
          {questions.map(({ key, icon: Icon, onSelect }) => (
            <button
              key={key}
              type="button"
              onClick={onSelect}
              className="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-muted/50"
            >
              <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="text-sm text-foreground">
                {t(`insights.questions.${key}`)}
              </span>
            </button>
          ))}
        </div>
        {/* "Which sources are incomplete or stale?" answers inline -- it's
            asking about `health` itself, not routing anywhere else. */}
        <div className="flex items-center gap-3 rounded-md px-2 py-2">
          <Loader2
            className={`h-4 w-4 shrink-0 text-muted-foreground ${
              health.state === "building" ? "animate-spin" : ""
            }`}
          />
          <span className="text-sm text-foreground">{t("insights.questions.sources")}</span>
          <span className="ml-auto shrink-0 text-xs text-muted-foreground">
            {t(`insights.questions.sourcesState.${health.state}`)}
          </span>
        </div>
      </section>

      <button
        type="button"
        onClick={goTo("usage")}
        className="text-xs text-muted-foreground hover:text-foreground hover:underline"
      >
        {t("insights.questions.usageAndCost")}
      </button>
    </div>
  );
};
