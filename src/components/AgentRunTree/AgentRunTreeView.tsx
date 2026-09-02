import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, CheckCircle2, AlertCircle, Loader2, ExternalLink } from "lucide-react";
import { cn } from "@/lib/utils";
import { fetchAgentRunDetail, fetchAgentRunTree } from "@/services/insightsApi";
import { formatDuration } from "@/utils/time";
import { pruneToExceptions } from "./helpers/pruneToExceptions";
import type { AgentRunDetail, AgentRunNode, AgentRunTree } from "@/types";

interface AgentRunTreeViewProps {
  sessionId: string;
  /**
   * Opens the correlated child transcript (a real session in its own
   * right). Omitted by `SessionView`'s own usage, where this tree is a
   * secondary section inside an already-open session and there's no
   * "switch to a different session" affordance to hook into -- the detail
   * panel still shows the transcript path as plain text in that case.
   */
  onOpenTranscript?: (childSessionId: string) => void;
}

function elapsedMinutes(startedAt?: string, endedAt?: string): number | null {
  if (!startedAt || !endedAt) return null;
  const start = Date.parse(startedAt);
  const end = Date.parse(endedAt);
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return null;
  return (end - start) / 60_000;
}

function countByStatus(nodes: AgentRunNode[], status: string): number {
  return nodes.reduce(
    (sum, node) =>
      sum + (node.status === status ? 1 : 0) + countByStatus(node.children, status),
    0
  );
}

const STATUS_ICON: Record<string, React.ComponentType<{ className?: string }>> = {
  completed: CheckCircle2,
  error: AlertCircle,
  running: Loader2,
  async_launched: Loader2,
};

const AgentRunNodeRow: React.FC<{
  node: AgentRunNode;
  depth: number;
  selectedId: number | null;
  onSelect: (agentRunId: number) => void;
  onOpenTranscript?: (childSessionId: string) => void;
}> = ({ node, depth, selectedId, onSelect, onOpenTranscript }) => {
  const { t } = useTranslation();
  const StatusIcon = (node.status && STATUS_ICON[node.status]) || Bot;
  const isError = node.status === "error";
  const isRunning = node.status === "running" || node.status === "async_launched";
  const minutes = elapsedMinutes(node.started_at, node.ended_at);

  return (
    <>
      <button
        type="button"
        onClick={() => onSelect(node.agent_run_id)}
        className={cn(
          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted/50",
          selectedId === node.agent_run_id && "bg-muted"
        )}
        style={{ paddingLeft: `${depth * 20 + 8}px` }}
      >
        <StatusIcon
          className={cn(
            "h-3.5 w-3.5 shrink-0",
            isError ? "text-destructive" : isRunning ? "text-info animate-spin" : "text-success"
          )}
        />
        <span className="flex-1 truncate text-foreground">
          {node.subagent_type || t("insights.agents.unknownType")}
        </span>
        {minutes !== null && (
          <span className="shrink-0 text-2xs text-muted-foreground">
            {formatDuration(minutes)}
          </span>
        )}
        {node.tool_call_count > 0 && (
          <span className="shrink-0 text-2xs text-muted-foreground">
            {t("insights.agents.toolCallCount", { count: node.tool_call_count })}
          </span>
        )}
      </button>
      {selectedId === node.agent_run_id && (
        <AgentRunDetailPanel
          agentRunId={node.agent_run_id}
          depth={depth}
          onOpenTranscript={onOpenTranscript}
        />
      )}
      {node.children.map((child) => (
        <AgentRunNodeRow
          key={child.agent_run_id}
          node={child}
          depth={depth + 1}
          selectedId={selectedId}
          onSelect={onSelect}
          onOpenTranscript={onOpenTranscript}
        />
      ))}
    </>
  );
};

const AgentRunDetailPanel: React.FC<{
  agentRunId: number;
  depth: number;
  onOpenTranscript?: (childSessionId: string) => void;
}> = ({ agentRunId, depth, onOpenTranscript }) => {
  const { t } = useTranslation();
  const [detail, setDetail] = useState<AgentRunDetail | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    setDetail(null);
    fetchAgentRunDetail(agentRunId)
      .then((result) => {
        if (!cancelled) setDetail(result);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agentRunId]);

  const paddingLeft = `${depth * 20 + 28}px`;

  if (isLoading) {
    return (
      <div className="flex items-center py-1.5" style={{ paddingLeft }}>
        <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (error || !detail) {
    return (
      <p className="py-1.5 text-2xs text-destructive" style={{ paddingLeft }}>
        {error}
      </p>
    );
  }

  return (
    <div
      className="mb-1 space-y-1 rounded-md border border-border/50 bg-muted/30 py-2 pr-2 text-2xs text-muted-foreground"
      style={{ paddingLeft }}
    >
      {detail.purpose && <p className="text-foreground">{detail.purpose}</p>}
      <div className="flex flex-wrap gap-x-4 gap-y-0.5">
        {detail.model && <span>{t("insights.agents.detail.model", { model: detail.model })}</span>}
        {typeof detail.total_tokens === "number" && (
          <span>{t("insights.agents.detail.tokens", { count: detail.total_tokens })}</span>
        )}
        {detail.error_count > 0 && (
          <span className="text-destructive">
            {t("insights.agents.detail.errorCount", { count: detail.error_count })}
          </span>
        )}
      </div>
      {detail.tools_used.length > 0 && (
        <p>
          {t("insights.agents.detail.tools")}{" "}
          {detail.tools_used.map((tool) => `${tool.tool_name} (${tool.count})`).join(", ")}
        </p>
      )}
      {detail.files_touched.length > 0 && (
        <p className="truncate">
          {t("insights.agents.detail.files")} {detail.files_touched.join(", ")}
        </p>
      )}
      {detail.child_session_id ? (
        onOpenTranscript ? (
          <button
            type="button"
            onClick={() => onOpenTranscript(detail.child_session_id!)}
            className="flex items-center gap-1 text-info hover:underline"
          >
            <ExternalLink className="h-3 w-3" />
            {t("insights.agents.detail.openTranscript")}
          </button>
        ) : (
          <p className="truncate">{detail.child_session_id}</p>
        )
      ) : (
        <p>{t("insights.agents.detail.noTranscript")}</p>
      )}
    </div>
  );
};

/**
 * Agent-run tree, consuming `get_agent_run_tree`. Real multi-level trees,
 * durations, and per-node detail (purpose, model, tokens, tools, files,
 * errors, source transcript) are populated whenever a launched subagent's
 * own transcript can be correlated (`archive_db::ingest::claude::ingest_subagent_tree`);
 * an unlinked launch still renders as a flat, depth-0 row with no duration --
 * see this module's types for the full breakdown of when each field is populated.
 */
export const AgentRunTreeView: React.FC<AgentRunTreeViewProps> = ({ sessionId, onOpenTranscript }) => {
  const { t } = useTranslation();
  const [tree, setTree] = useState<AgentRunTree | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  // "Exceptions first" defaults to true, but only actually filters
  // anything once a real exception exists (see the all-successful
  // early-return below, which never shows a silently empty tree just
  // because everything happened to succeed).
  const [showExceptionsOnly, setShowExceptionsOnly] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    setSelectedId(null);
    fetchAgentRunTree(sessionId)
      .then((result) => {
        if (!cancelled) setTree(result);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  const failedCount = useMemo(() => (tree ? countByStatus(tree.roots, "error") : 0), [tree]);
  // "Collapse successful branches by default": when nothing failed,
  // the default view shows NONE of them (just a quiet confirmation
  // below) rather than the full successful list -- toggling reveals it.
  const displayedRoots = useMemo(() => {
    if (!tree) return [];
    if (!showExceptionsOnly) return tree.roots;
    return failedCount > 0 ? pruneToExceptions(tree.roots) : [];
  }, [tree, showExceptionsOnly, failedCount]);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-4">
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (error) {
    return <p className="px-2 py-2 text-xs text-destructive">{error}</p>;
  }

  if (!tree || tree.total_count === 0) {
    return (
      <p className="px-2 py-2 text-xs text-muted-foreground">
        {t("insights.agents.treeEmpty")}
      </p>
    );
  }

  return (
    <div>
      <div className="mb-2 flex items-center gap-4 px-2 text-xs text-muted-foreground">
        <span>{t("insights.agents.totalCount", { count: tree.total_count })}</span>
        {failedCount > 0 && (
          <span className="text-destructive">
            {t("insights.agents.failedCount", { count: failedCount })}
          </span>
        )}
        <button
          type="button"
          onClick={() => setShowExceptionsOnly((prev) => !prev)}
          className="ml-auto text-muted-foreground hover:text-foreground hover:underline"
        >
          {showExceptionsOnly
            ? t("insights.agents.showAllBranches", "Show all branches")
            : t("insights.agents.showExceptionsOnly", "Show exceptions only")}
        </button>
      </div>
      {showExceptionsOnly && failedCount === 0 && (
        <p className="px-2 pb-2 text-xs text-muted-foreground">
          {t("insights.agents.allSucceeded", "All agent runs completed successfully.")}
        </p>
      )}
      <div className="space-y-0.5">
        {displayedRoots.map((node) => (
          <AgentRunNodeRow
            key={node.agent_run_id}
            node={node}
            depth={0}
            selectedId={selectedId}
            onSelect={(id) => setSelectedId((current) => (current === id ? null : id))}
            onOpenTranscript={onOpenTranscript}
          />
        ))}
      </div>
    </div>
  );
};
