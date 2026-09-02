import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  Loader2,
  Terminal,
  Sparkles,
  TrendingUp,
  TrendingDown,
  X,
} from "lucide-react";
import {
  dismissProblem,
  fetchErrorOccurrences,
  fetchRepeatedCommandFailures,
  fetchRepeatedErrors,
  fetchSimilarErrorResolutions,
} from "@/services/insightsApi";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import { formatDateCompact } from "@/utils/time";
import { useAppStore } from "@/store/useAppStore";
import { ArchiveEmptyState, ArchiveHealthEmptyState } from "@/components/common/ArchiveEmptyState";
import { isProviderSupportedByArchiveIndex } from "@/utils/archiveSupport";
import { cn } from "@/lib/utils";
import type { ArchiveIndexHealth } from "@/hooks/useArchiveIndexHealth";
import type {
  ClaudeSession,
  ErrorOccurrence,
  ProblemTrend,
  RepeatedCommandFailureCard,
  RepeatedErrorCard,
  SimilarErrorResolution,
} from "@/types";
import { toClaudeSessionStub } from "../helpers/toClaudeSessionStub";

/**
 * Selecting an error shows its real occurrences (session/project/timestamp).
 * Correlating "surrounding commands" per occurrence (nearby `command` rows
 * by timestamp) is separate, not-yet-attempted work -- this shows only
 * confirmed occurrences. Fetched on demand, only when a card is expanded,
 * not eagerly for every card in the list.
 */
const ErrorOccurrencesPanel: React.FC<{
  errorSignature: string;
  projectKey?: string;
  onSessionSelect: (sessionId: string) => void;
}> = ({ errorSignature, projectKey, onSessionSelect }) => {
  const { t } = useTranslation();
  const [occurrences, setOccurrences] = useState<ErrorOccurrence[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchErrorOccurrences(errorSignature, projectKey)
      .then((result) => {
        if (!cancelled) setOccurrences(result);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [errorSignature, projectKey]);

  if (error) {
    return <p className="px-2 py-2 text-2xs text-destructive">{error}</p>;
  }
  if (occurrences === null) {
    return <div className="px-2 py-2 text-2xs text-muted-foreground">…</div>;
  }

  return (
    <ul className="space-y-1 rounded-md border border-border/50 bg-muted/30 px-3 py-2">
      {occurrences.map((occ, index) => (
        <li key={index}>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onSessionSelect(occ.session_id);
            }}
            className="flex w-full items-center gap-2 text-left text-xs text-foreground hover:underline"
          >
            <span className="min-w-0 flex-1 truncate">{occ.project_name}</span>
            {occ.occurred_at && (
              <span className="shrink-0 text-2xs text-muted-foreground">
                {formatDateCompact(occ.occurred_at)}
              </span>
            )}
          </button>
        </li>
      ))}
      {occurrences.length === 0 && (
        <li className="text-2xs text-muted-foreground">{t("insights.problems.noOccurrences")}</li>
      )}
    </ul>
  );
};

/**
 * Cross-session reusable-solution retrieval. Deliberately on-demand -- a
 * button, not a background fetch -- since this is more expensive than
 * `ErrorOccurrencesPanel` (a per-candidate-session query) and could
 * otherwise fire eagerly for every error card whether or not a user
 * cares. `excludeProjectKey` omits the project already being viewed so
 * results read as genuinely "another project." Copy is deliberately
 * evidentiary ("later followed by a passing verification"), never
 * "resolved" or "the fix" -- a passing test afterward doesn't prove
 * causation, see the backend's own doc comment for the full reasoning.
 */
const SimilarFixesLookup: React.FC<{
  errorSignature: string;
  excludeProjectKey?: string;
  onSessionSelect: (sessionId: string) => void;
}> = ({ errorSignature, excludeProjectKey, onSessionSelect }) => {
  const { t } = useTranslation();
  const [results, setResults] = useState<SimilarErrorResolution[] | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (results === null) {
    return (
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          setIsLoading(true);
          setError(null);
          fetchSimilarErrorResolutions(errorSignature, excludeProjectKey)
            .then(setResults)
            .catch((err) => setError(err instanceof Error ? err.message : String(err)))
            .finally(() => setIsLoading(false));
        }}
        disabled={isLoading}
        className="text-2xs text-muted-foreground hover:text-foreground hover:underline disabled:opacity-60"
      >
        {isLoading ? "…" : t("insights.problems.lookForSimilarFix")}
      </button>
    );
  }

  if (error) {
    return <p className="text-2xs text-destructive">{error}</p>;
  }

  if (results.length === 0) {
    return <p className="text-2xs text-muted-foreground">{t("insights.problems.noSimilarFixes")}</p>;
  }

  return (
    <ul className="space-y-1">
      {results.map((resolution, index) => (
        <li key={index}>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onSessionSelect(resolution.session_id);
            }}
            className="text-left text-2xs text-foreground hover:underline"
          >
            {t("insights.problems.similarFixEvidence", {
              project: resolution.project_name,
              command: resolution.verification_command,
              date: formatDateCompact(resolution.verification_occurred_at),
            })}
          </button>
        </li>
      ))}
    </ul>
  );
};

const TREND_ICON: Record<ProblemTrend, React.ComponentType<{ className?: string }> | null> = {
  new: Sparkles,
  increasing: TrendingUp,
  decreasing: TrendingDown,
  steady: null,
};

const TrendBadge: React.FC<{ trend: ProblemTrend }> = ({ trend }) => {
  const { t } = useTranslation();
  const Icon = TREND_ICON[trend];
  if (!Icon) return null;
  return (
    <span
      className={cn(
        "shrink-0",
        trend === "increasing" && "text-destructive",
        trend === "new" && "text-info",
        trend === "decreasing" && "text-success"
      )}
      title={t(`insights.problems.trend.${trend}`)}
    >
      <Icon className="h-3.5 w-3.5" />
    </span>
  );
};

interface ProblemsTabProps {
  health: ArchiveIndexHealth;
  onSessionSelect: (session: ClaudeSession) => void;
}

/**
 * Repeated-failure/repeated-error cards, backed by
 * `get_repeated_command_failures`/`get_repeated_errors`. Scoped to the
 * current project's own `archive_db` `project_key` (its Claude
 * session-storage path, same value History's own query filters on) when
 * one is selected, global otherwise -- matching Usage/Tools' existing
 * scoping convention. `archive_db` ingests every
 * `FILE_BASED_STATS_PROVIDERS` entry, not just Claude -- so an empty
 * result for a selected project distinguishes an unsupported provider,
 * the archive index not being ready, and a genuine "no repeated problems"
 * result, rather than conflating all three.
 */
export const ProblemsTab: React.FC<ProblemsTabProps> = ({ health, onSessionSelect }) => {
  const { t } = useTranslation();
  const selectedProject = useAppStore((s) => s.selectedProject);
  const projectKey = selectedProject?.path;
  const [failures, setFailures] = useState<RepeatedCommandFailureCard[] | null>(null);
  const [errors, setErrors] = useState<RepeatedErrorCard[] | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [expandedSignature, setExpandedSignature] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setLoadError(null);
    Promise.all([fetchRepeatedCommandFailures(projectKey), fetchRepeatedErrors(projectKey)])
      .then(([failureResults, errorResults]) => {
        if (cancelled) return;
        setFailures(failureResults);
        setErrors(errorResults);
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectKey]);

  // Opening a problem card is a distinct diagnostics signal from a session
  // opened elsewhere (e.g. browsing History) -- it specifically means the
  // user drilled into evidence for a repeated problem.
  const openProblemCard = (sessionId: string) => {
    void recordDiagnosticsEvent({ kind: "problemOpened" });
    void recordDiagnosticsEvent({ kind: "evidenceDrilldownOpened" });
    onSessionSelect(toClaudeSessionStub(sessionId, ""));
  };

  // Local dismiss/resolve state removes the card from THIS view
  // immediately (optimistic; the backend call is fire-and-forget from the
  // UI's perspective, matching this tab's existing error handling for
  // non-critical actions) so a user who's already handled a failure
  // doesn't keep seeing it resurface.
  const handleDismissFailure = (e: React.MouseEvent, card: RepeatedCommandFailureCard) => {
    e.stopPropagation();
    setFailures((prev) => prev?.filter((f) => f.template !== card.template) ?? prev);
    void dismissProblem("command_failure", card.template).catch((err) => {
      console.error("Failed to dismiss command failure:", err);
    });
  };
  const handleDismissError = (e: React.MouseEvent, card: RepeatedErrorCard) => {
    e.stopPropagation();
    setErrors((prev) => prev?.filter((er) => er.error_signature !== card.error_signature) ?? prev);
    void dismissProblem("error", card.error_signature).catch((err) => {
      console.error("Failed to dismiss error:", err);
    });
  };

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (loadError) {
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-destructive">
        {loadError}
      </div>
    );
  }

  const hasFailures = (failures?.length ?? 0) > 0;
  const hasErrors = (errors?.length ?? 0) > 0;

  if (!hasFailures && !hasErrors) {
    if (selectedProject && !isProviderSupportedByArchiveIndex(selectedProject.provider)) {
      return (
        <div className="flex h-full items-center justify-center">
          <ArchiveEmptyState reason="unsupported-provider" provider={selectedProject.provider} />
        </div>
      );
    }
    if (health.state !== "ready") {
      return (
        <div className="flex h-full items-center justify-center">
          <ArchiveHealthEmptyState health={health} />
        </div>
      );
    }
    return (
      <div className="flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground">
        {t("insights.problems.empty")}
      </div>
    );
  }

  return (
    <div className="flex-1 space-y-6 overflow-y-auto px-4 py-3">
      {hasFailures && (
        <section>
          <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            <Terminal className="h-3.5 w-3.5" />
            {t("insights.problems.repeatedCommandFailures")}
          </h3>
          <div className="space-y-1">
            {failures!.map((card) => (
              <div
                key={card.template}
                role="button"
                tabIndex={0}
                onClick={() => openProblemCard(card.sample_session_id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") openProblemCard(card.sample_session_id);
                }}
                className="group flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-muted/50 cursor-pointer"
              >
                <TrendBadge trend={card.trend} />
                <code className="min-w-0 flex-1 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground">
                  {card.shell_command}
                </code>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {t("insights.problems.failureCount", {
                    count: card.failure_count,
                    sessions: card.session_count,
                  })}
                </span>
                <span className="w-24 shrink-0 text-right text-2xs text-muted-foreground">
                  {formatDateCompact(card.last_occurred_at)}
                </span>
                <button
                  type="button"
                  onClick={(e) => handleDismissFailure(e, card)}
                  aria-label={t("insights.problems.dismiss")}
                  title={t("insights.problems.dismiss")}
                  className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground group-hover:opacity-100 focus:opacity-100"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>
        </section>
      )}

      {hasErrors && (
        <section>
          <h3 className="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            <AlertTriangle className="h-3.5 w-3.5" />
            {t("insights.problems.repeatedErrors")}
          </h3>
          <div className="space-y-1">
            {errors!.map((card) => {
              const isExpanded = expandedSignature === card.error_signature;
              return (
                <div key={card.error_signature}>
                  <div
                    role="button"
                    tabIndex={0}
                    onClick={() => openProblemCard(card.sample_session_id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") openProblemCard(card.sample_session_id);
                    }}
                    className="group flex w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-muted/50 cursor-pointer"
                  >
                    <TrendBadge trend={card.trend} />
                    <span
                      className="min-w-0 flex-1 truncate text-sm text-foreground"
                      title={card.error_signature}
                    >
                      {card.error_signature}
                    </span>
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        setExpandedSignature((prev) =>
                          prev === card.error_signature ? null : card.error_signature
                        );
                      }}
                      className="shrink-0 text-xs text-muted-foreground hover:text-foreground hover:underline"
                    >
                      {t("insights.problems.errorCount", {
                        count: card.occurrence_count,
                        sessions: card.session_count,
                      })}
                    </button>
                    <span className="w-24 shrink-0 text-right text-2xs text-muted-foreground">
                      {formatDateCompact(card.last_occurred_at)}
                    </span>
                    <button
                      type="button"
                      onClick={(e) => handleDismissError(e, card)}
                      aria-label={t("insights.problems.dismiss")}
                      title={t("insights.problems.dismiss")}
                      className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 hover:bg-muted hover:text-foreground group-hover:opacity-100 focus:opacity-100"
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </div>
                  {isExpanded && (
                    <div className="space-y-2">
                      <ErrorOccurrencesPanel
                        errorSignature={card.error_signature}
                        projectKey={projectKey}
                        onSessionSelect={openProblemCard}
                      />
                      <div className="px-2">
                        <SimilarFixesLookup
                          errorSignature={card.error_signature}
                          excludeProjectKey={projectKey}
                          onSessionSelect={openProblemCard}
                        />
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
};
