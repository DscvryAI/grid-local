import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Bot, Coins, FlaskConical, Sparkles, Terminal } from "lucide-react";
import { fetchThingsWorthLookingAt } from "@/services/insightsApi";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import { formatDateCompact } from "@/utils/time";
import { formatNumber } from "@/utils/formatters";
import { toClaudeSessionStub } from "../../Insights/helpers/toClaudeSessionStub";
import { ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import { useAppStore } from "@/store/useAppStore";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type { ClaudeSession, InsightCard } from "@/types";

const HOME_TEASER_LIMIT = 3;

/** The action verb Home's "View" affordance should read, per card kind --
 * distinct verbs per attention type ("Review"/"Inspect"/"Refresh"), not one
 * generic "View". */
function actionVerbKey(kind: InsightCard["kind"]): string {
  switch (kind) {
    case "RepeatedCommandFailure":
    case "RepeatedError":
      return "home.thingsWorthLookingAt.actionReview";
    case "LargeAgentRun":
      return "home.thingsWorthLookingAt.actionInspect";
    case "HighTokenSession":
      return "home.thingsWorthLookingAt.actionExplain";
    case "VerificationGap":
      return "home.thingsWorthLookingAt.actionReview";
  }
}

interface ThingsWorthLookingAtSectionProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/** A card's own sample/associated session, and a compact one-line summary + timestamp. */
function cardDetail(card: InsightCard): {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  detail: string;
  timestamp?: string;
  sessionId: string;
  projectName: string;
  summary?: string;
} {
  switch (card.kind) {
    case "RepeatedCommandFailure":
      return {
        icon: Terminal,
        label: card.data.shell_command,
        detail: `${card.data.failure_count}×`,
        timestamp: card.data.last_occurred_at,
        sessionId: card.data.sample_session_id,
        projectName: "",
      };
    case "RepeatedError":
      return {
        icon: AlertTriangle,
        label: card.data.error_signature,
        detail: `${card.data.occurrence_count}×`,
        timestamp: card.data.last_occurred_at,
        sessionId: card.data.sample_session_id,
        projectName: "",
      };
    case "LargeAgentRun":
      return {
        icon: Bot,
        label: card.data.session_summary || card.data.project_name,
        detail: `${card.data.subagent_count} agents`,
        timestamp: card.data.session_started_at,
        sessionId: card.data.session_id,
        projectName: card.data.project_name,
        summary: card.data.session_summary,
      };
    case "HighTokenSession":
      return {
        icon: Coins,
        label: card.data.session_summary || card.data.project_name,
        detail: formatNumber(card.data.total_tokens),
        timestamp: card.data.last_message_time,
        sessionId: card.data.session_id,
        projectName: card.data.project_name,
        summary: card.data.session_summary,
      };
    case "VerificationGap":
      return {
        icon: FlaskConical,
        label: card.data.session_summary || card.data.project_name,
        detail: `${card.data.files_changed_since} files`,
        timestamp: card.data.last_verified_at,
        sessionId: card.data.session_id,
        projectName: card.data.project_name,
        summary: card.data.session_summary,
      };
  }
}

/**
 * Spec §10, Home's highlight version: up to 3 of the highest-ranked
 * "things worth looking at" cards (the merged, already-ranked
 * `things_worth_looking_at` list). The full lists live in Insights'
 * Problems/Agents tabs -- this is a teaser, not a duplicate.
 */
export const ThingsWorthLookingAtSection: React.FC<ThingsWorthLookingAtSectionProps> = ({
  health,
  onSessionSelect,
}) => {
  const { t } = useTranslation();
  const setPrimarySurface = useAppStore((s) => s.setPrimarySurface);
  const setInsightsTab = useAppStore((s) => s.setInsightsTab);
  const [cards, setCards] = useState<InsightCard[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchThingsWorthLookingAt()
      .then((results) => {
        if (!cancelled) setCards(results);
      })
      .catch((err) => {
        console.error("Failed to load things-worth-looking-at:", err);
        if (!cancelled) {
          setLoadError(err instanceof Error ? err.message : String(err));
          setCards([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (cards === null) {
    return <div className="h-24 animate-pulse rounded-lg bg-muted/30" />;
  }
  if (loadError) {
    return (
      <div className="px-1 text-sm text-destructive" role="alert">
        {loadError}
      </div>
    );
  }
  if (cards.length === 0) {
    if (health.state !== "ready") {
      return <ArchiveHealthEmptyState health={health} />;
    }
    return null;
  }

  return (
    <section>
      <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold text-foreground">
        <Sparkles className="h-4 w-4 text-muted-foreground" />
        {t("home.thingsWorthLookingAt.title")}
      </h2>
      <div className="space-y-1">
        {cards.slice(0, HOME_TEASER_LIMIT).map((card, index) => {
          const { icon: Icon, label, detail, timestamp, sessionId, projectName, summary } =
            cardDetail(card);
          return (
            <button
              key={`${card.kind}-${index}`}
              type="button"
              onClick={() => {
                // Every card here is an evidence-driven session open;
                // RepeatedCommandFailure/RepeatedError count as a "problem
                // open", LargeAgentRun as an "agent-run open" --
                // HighTokenSession isn't either named category in the
                // diagnostics schema, so only the shared "evidence
                // drill-down" fires for it.
                if (card.kind === "RepeatedCommandFailure" || card.kind === "RepeatedError") {
                  void recordDiagnosticsEvent({ kind: "problemOpened" });
                } else if (card.kind === "LargeAgentRun") {
                  void recordDiagnosticsEvent({ kind: "agentRunOpened" });
                }
                void recordDiagnosticsEvent({ kind: "evidenceDrilldownOpened" });
                onSessionSelect(toClaudeSessionStub(sessionId, projectName, summary));
              }}
              className="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-muted/50"
            >
              <span className="w-4 shrink-0 text-xs tabular-nums text-muted-foreground/70">
                {index + 1}
              </span>
              <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate text-sm text-foreground" title={label}>
                {label}
              </span>
              <span className="shrink-0 text-xs text-muted-foreground">{detail}</span>
              {timestamp && (
                <span className="w-24 shrink-0 text-right text-2xs text-muted-foreground">
                  {formatDateCompact(timestamp)}
                </span>
              )}
              <span className="shrink-0 text-xs font-medium text-accent">
                {t(actionVerbKey(card.kind))}
              </span>
            </button>
          );
        })}
      </div>
      {cards.length > HOME_TEASER_LIMIT && (
        <button
          type="button"
          onClick={() => {
            setPrimarySurface("insights");
            setInsightsTab("problems");
          }}
          className="mt-2 text-xs text-muted-foreground hover:text-foreground hover:underline"
        >
          {t("home.thingsWorthLookingAt.viewAll")}
        </button>
      )}
    </section>
  );
};
