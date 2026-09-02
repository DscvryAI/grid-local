import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, XCircle, ClipboardList } from "lucide-react";
import { ToolIcon } from "@/components/ToolIcon";
import { getToolDisplayName } from "@/components/AnalyticsDashboard/utils/toolNames";
import { calculateGlobalCostSummary, formatNumber, formatCurrency } from "@/components/AnalyticsDashboard/utils";
import { describeVerificationStatus } from "../helpers/verificationStatusDisplay";
import { deriveBaselineAnomalies, type BaselineAnomaly } from "../helpers/sessionIntelligence";
import { buildHandoffPreviewMarkdown } from "../helpers/handoffPreview";
import { fetchPersonalBaseline } from "@/services/insightsApi";
import { useCopyButton } from "@/hooks/useCopyButton";
import type { SessionTab } from "@/store/slices/navigationSlice";
import type { SessionIntelligence, SessionDecisionBrief } from "../helpers/sessionIntelligence";
import type { ClaudeSession } from "@/types";

interface OverviewTabProps {
  intelligence: SessionIntelligence;
  decisionBrief: SessionDecisionBrief;
  session: ClaudeSession;
  onNavigateToTab: (tab: SessionTab) => void;
}

/**
 * Session decision brief: goal, verification status, and whether the
 * session ended on an error -- every field derived deterministically in
 * `sessionIntelligence.ts` from data already loaded with the session,
 * never AI-generated, per the standing rule that Grid Local never
 * generates AI text. "What changed" isn't repeated here -- the Files
 * section immediately below already covers it in full.
 *
 * Also fetches this user's own historical average tokens/duration for
 * THIS session's provider (personal-baseline anomaly explanations), on
 * mount only (not eagerly for every session in a list) -- scoped
 * per-provider since providers report token usage on incomparable
 * scales, and the currently-open session is excluded from the average so
 * it can't skew the baseline it's being measured against. Renders
 * nothing below `MIN_BASELINE_SESSIONS`/`BASELINE_ANOMALY_THRESHOLD` --
 * no forced "insight" when there isn't a real one. Anomaly state is
 * lifted into this component (not a separate child) specifically so the
 * section's own "is there anything to show" check below can account for
 * it -- otherwise a session with no goal/verification/error but a real
 * token or duration outlier would render nothing at all.
 */
const DecisionBriefSection: React.FC<{
  brief: SessionDecisionBrief;
  session: ClaudeSession;
  totalTokens: number;
  onNavigateToTab: (tab: SessionTab) => void;
}> = ({ brief, session, totalTokens, onNavigateToTab }) => {
  const { t } = useTranslation();
  const { verification } = brief;
  const [anomalies, setAnomalies] = useState<BaselineAnomaly[]>([]);

  const verificationDisplay = useMemo(
    () => describeVerificationStatus(verification, t),
    [verification, t]
  );

  const durationMinutes =
    session.first_message_time && session.last_message_time
      ? (new Date(session.last_message_time).getTime() -
          new Date(session.first_message_time).getTime()) /
        60000
      : 0;

  useEffect(() => {
    if (!session.provider) return;
    let cancelled = false;
    fetchPersonalBaseline(session.provider, session.file_path)
      .then((baseline) => {
        if (!cancelled) setAnomalies(deriveBaselineAnomalies(totalTokens, durationMinutes, baseline));
      })
      .catch(() => {
        if (!cancelled) setAnomalies([]);
      });
    return () => {
      cancelled = true;
    };
  }, [session.provider, session.file_path, totalTokens, durationMinutes]);

  if (!brief.goal && !verificationDisplay && !brief.endedOnError && anomalies.length === 0) {
    return null;
  }

  return (
    <section className="rounded-lg border border-border/60 px-3 py-2.5 space-y-2">
      {brief.goal && (
        <p className="text-sm text-foreground" title={brief.goal}>
          <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            {t("session.brief.goal", "Goal")}:
          </span>{" "}
          <span className="line-clamp-2">{brief.goal}</span>
        </p>
      )}
      {verificationDisplay && (
        <p className={`flex items-start gap-1.5 text-sm ${verificationDisplay.className}`}>
          <verificationDisplay.icon className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{verificationDisplay.text}</span>
        </p>
      )}
      {anomalies.map((anomaly) => (
        <p key={anomaly.dimension} className="text-sm text-foreground">
          {t(`session.brief.baseline${anomaly.dimension === "tokens" ? "Tokens" : "Duration"}`, {
            ratio: anomaly.ratio.toFixed(1),
          })}
        </p>
      ))}
      {brief.endedOnError && (
        <button
          type="button"
          onClick={() => onNavigateToTab("conversation")}
          className="flex items-center gap-1.5 text-sm text-destructive hover:underline"
        >
          <XCircle className="h-3.5 w-3.5 shrink-0" />
          {t("session.brief.endedOnError", "Session ended on a tool error")}
        </button>
      )}
    </section>
  );
};

/**
 * One-action handoff preview: a single compiled, read-only summary of
 * goal/changes/verification/unresolved items/source, for handing this
 * session off. Collapsed by default -- a "Preview handoff" toggle, same
 * pattern `OverviewSection` already uses -- since not every session view
 * visit needs this. "Copy as Markdown" keeps export a separate, explicit
 * action, done via clipboard (this app's existing "copy resume command"
 * precedent) rather than a new file write.
 */
const HandoffPreviewSection: React.FC<{
  session: ClaudeSession;
  decisionBrief: SessionDecisionBrief;
  fileEvents: SessionIntelligence["fileEvents"];
}> = ({ session, decisionBrief, fileEvents }) => {
  const { t } = useTranslation();
  const [isExpanded, setIsExpanded] = useState(false);
  const { renderCopyButton } = useCopyButton();

  const markdown = useMemo(
    () => buildHandoffPreviewMarkdown(session, decisionBrief, fileEvents, t),
    [session, decisionBrief, fileEvents, t]
  );

  return (
    <section className="rounded-lg border border-border/60">
      <button
        type="button"
        onClick={() => setIsExpanded((prev) => !prev)}
        className="flex w-full items-center justify-between px-3 py-2 text-left"
      >
        <span className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <ClipboardList className="h-3.5 w-3.5" />
          {t("session.handoff.button", "Preview handoff")}
        </span>
        <ChevronRight
          className={`h-3.5 w-3.5 text-muted-foreground transition-transform ${isExpanded ? "rotate-90" : ""}`}
        />
      </button>
      {isExpanded && (
        <div className="border-t border-border/60 px-3 py-2 space-y-2">
          <pre className="whitespace-pre-wrap break-words text-xs text-foreground">{markdown}</pre>
          {renderCopyButton(markdown, "handoff-preview", t("session.handoff.copyMarkdown", "Copy as Markdown"))}
        </div>
      )}
    </section>
  );
};

const TOP_N = 5;

const OverviewSection: React.FC<{
  title: string;
  count: number;
  emptyLabel: string;
  onViewAll: () => void;
  children: React.ReactNode;
}> = ({ title, count, emptyLabel, onViewAll, children }) => (
  <section className="rounded-lg border border-border/60">
    <button
      type="button"
      onClick={onViewAll}
      disabled={count === 0}
      className="flex w-full items-center justify-between px-3 py-2 text-left disabled:cursor-default"
    >
      <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {title} ({count})
      </span>
      {count > 0 && <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />}
    </button>
    <div className="border-t border-border/60 px-3 py-2">
      {count === 0 ? (
        <p className="text-sm text-muted-foreground">{emptyLabel}</p>
      ) : (
        children
      )}
    </div>
  </section>
);

const USAGE_CATEGORIES: Array<{
  key: keyof SessionIntelligence["tokenBreakdown"];
  labelKey: string;
  fallback: string;
}> = [
  { key: "input", labelKey: "analytics.inputTokens", fallback: "Input Tokens" },
  { key: "output", labelKey: "analytics.outputTokens", fallback: "Output Tokens" },
  { key: "cacheCreation", labelKey: "analytics.cacheCreation", fallback: "Cache Creation" },
  { key: "cacheRead", labelKey: "analytics.cacheRead", fallback: "Cache Read" },
  { key: "reasoning", labelKey: "analytics.reasoning", fallback: "Reasoning" },
];

/**
 * Token-category breakdown + estimated cost -- folded in from the old
 * standalone "Token Stats" nav view, which violated spec's exact 5-tab
 * Session View shape. The header already shows the total; this section
 * is the detail behind it.
 */
const UsageSection: React.FC<{ intelligence: SessionIntelligence }> = ({ intelligence }) => {
  const { t } = useTranslation();
  const costSummary = useMemo(
    () => calculateGlobalCostSummary(intelligence.modelDistribution, intelligence.tokenTotal),
    [intelligence.modelDistribution, intelligence.tokenTotal]
  );

  if (intelligence.tokenTotal === 0) return null;

  return (
    <section className="rounded-lg border border-border/60">
      <div className="flex items-center justify-between px-3 py-2">
        <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {t("analytics.totalTokens")} ({formatNumber(intelligence.tokenTotal)})
        </span>
        {costSummary.pricedModels > 0 && (
          <span className="font-mono text-xs text-muted-foreground">
            {t("session.tabs.estimatedCostInline", "~{{cost}} est.", {
              cost: formatCurrency(costSummary.totalEstimatedCost),
            })}
          </span>
        )}
      </div>
      <div className="grid grid-cols-2 gap-2 border-t border-border/60 px-3 py-2 sm:grid-cols-3">
        {USAGE_CATEGORIES.map(({ key, labelKey, fallback }) => (
          <div key={key}>
            <p className="font-mono text-sm tabular-nums text-foreground">
              {formatNumber(intelligence.tokenBreakdown[key])}
            </p>
            <p className="text-2xs text-muted-foreground">{t(labelKey, fallback)}</p>
          </div>
        ))}
      </div>
    </section>
  );
};

/** Condensed cross-section of Usage/Tools/Files/Agents, each linking to its own tab (spec §14's Overview tab). */
export const OverviewTab: React.FC<OverviewTabProps> = ({
  intelligence,
  decisionBrief,
  session,
  onNavigateToTab,
}) => {
  const { t } = useTranslation();
  const topTools = intelligence.toolUsage.slice(0, TOP_N);
  const topFiles = intelligence.fileEvents.slice(0, TOP_N);

  return (
    <div className="flex-1 space-y-3 overflow-y-auto px-4 py-3">
      <DecisionBriefSection
        brief={decisionBrief}
        session={session}
        totalTokens={intelligence.tokenTotal}
        onNavigateToTab={onNavigateToTab}
      />
      <HandoffPreviewSection
        session={session}
        decisionBrief={decisionBrief}
        fileEvents={intelligence.fileEvents}
      />
      <UsageSection intelligence={intelligence} />

      <OverviewSection
        title={t("session.tabs.tools")}
        count={intelligence.toolUsage.length}
        emptyLabel={t("session.tabs.toolsEmpty")}
        onViewAll={() => onNavigateToTab("tools")}
      >
        <ul className="space-y-1.5">
          {topTools.map(({ name, count }) => (
            <li key={name} className="flex items-center gap-2 text-sm">
              <ToolIcon toolName={name} colored size="sm" />
              <span className="flex-1 truncate text-foreground">{getToolDisplayName(name, t)}</span>
              <span className="font-mono text-xs tabular-nums text-muted-foreground">{count}</span>
            </li>
          ))}
        </ul>
      </OverviewSection>

      <OverviewSection
        title={t("session.tabs.files")}
        count={intelligence.fileEvents.length}
        emptyLabel={t("session.tabs.filesEmpty")}
        onViewAll={() => onNavigateToTab("files")}
      >
        <ul className="space-y-1.5">
          {topFiles.map((event) => (
            <li key={event.filePath} className="truncate text-sm text-foreground" title={event.filePath}>
              {event.filePath.split(/[\\/]/).pop()}
            </li>
          ))}
        </ul>
      </OverviewSection>

      <OverviewSection
        title={t("session.tabs.agents")}
        count={intelligence.agentCount}
        emptyLabel={t("session.tabs.agentsEmpty")}
        onViewAll={() => onNavigateToTab("agents")}
      >
        <p className="text-sm text-foreground">
          {t("session.tabs.agentsSummary", { count: intelligence.agentCount })}
        </p>
      </OverviewSection>
    </div>
  );
};
