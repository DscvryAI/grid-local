import React from "react";
import { useTranslation } from "react-i18next";
import { Coins, Wrench, FileText, Bot, Clock } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { getProviderLabel, getProviderBadgeStyle } from "@/utils/providers";
import { formatTimeShort, formatDuration } from "@/utils/time";
import { useAppStore } from "@/store/useAppStore";
import type { ClaudeSession } from "@/types";
import type { SessionIntelligence } from "./helpers/sessionIntelligence";

interface SessionIntelligenceHeaderProps {
  session: ClaudeSession;
  intelligence: SessionIntelligence;
}

function formatCount(value: number): string {
  return value.toLocaleString();
}

const StatChip: React.FC<{
  icon: React.ComponentType<{ className?: string }>;
  value: string;
  label: string;
}> = ({ icon: Icon, value, label }) => (
  <div className="flex items-center gap-1.5 text-xs" title={label}>
    <Icon className="h-3.5 w-3.5 text-muted-foreground" />
    <span className="font-mono tabular-nums font-medium text-foreground">{value}</span>
    <span className="text-muted-foreground">{label}</span>
  </div>
);

/**
 * Spec §14's intelligence header: title, project, provider, time range,
 * duration, and token/tool-call/file/agent counts -- all computed
 * client-side from the already-loaded session, with no new backend
 * command needed.
 */
export const SessionIntelligenceHeader: React.FC<SessionIntelligenceHeaderProps> = ({
  session,
  intelligence,
}) => {
  const { t } = useTranslation();
  const getSessionDisplayName = useAppStore((s) => s.getSessionDisplayName);

  const title =
    getSessionDisplayName(session.session_id) ||
    session.summary ||
    t("session.untitled");

  const durationMinutes =
    session.first_message_time && session.last_message_time
      ? Math.round(
          (new Date(session.last_message_time).getTime() -
            new Date(session.first_message_time).getTime()) /
            60000
        )
      : 0;

  return (
    <div className="border-b border-border bg-muted/10 px-4 py-3">
      <div className="flex items-start justify-between gap-3">
        <h2 className="truncate text-sm font-semibold text-foreground" title={title}>
          {title}
        </h2>
        <Badge
          size="sm"
          className={cn("shrink-0 rounded px-1.5 py-0 text-2xs", getProviderBadgeStyle(session.provider))}
        >
          {getProviderLabel((key, fallback) => t(key, fallback), session.provider)}
        </Badge>
      </div>

      <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span className="truncate">{session.project_name}</span>
        {session.first_message_time && session.last_message_time && (
          <span className="flex items-center gap-1">
            <Clock className="h-3 w-3" />
            {formatTimeShort(session.first_message_time)} – {formatTimeShort(session.last_message_time)}
            {durationMinutes > 0 && ` (${formatDuration(durationMinutes)})`}
          </span>
        )}
      </div>

      <div className="mt-2 flex flex-wrap items-center gap-4">
        <StatChip
          icon={Coins}
          value={formatCount(intelligence.tokenTotal)}
          label={t("session.intelligence.tokens")}
        />
        <StatChip
          icon={Wrench}
          value={formatCount(intelligence.toolCallCount)}
          label={t("session.intelligence.toolCalls")}
        />
        <StatChip
          icon={FileText}
          value={formatCount(intelligence.fileCount)}
          label={t("session.intelligence.files")}
        />
        <StatChip
          icon={Bot}
          value={formatCount(intelligence.agentCount)}
          label={t("session.intelligence.agents")}
        />
      </div>
    </div>
  );
};
