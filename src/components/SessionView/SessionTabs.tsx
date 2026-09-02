import React from "react";
import { useTranslation } from "react-i18next";
import { LayoutDashboard, MessageSquare, Wrench, FileText, Bot } from "lucide-react";
import { cn } from "@/lib/utils";
import type { SessionTab } from "@/store/slices/navigationSlice";

interface SessionTabsProps {
  activeTab: SessionTab;
  onTabChange: (tab: SessionTab) => void;
  toolCallCount: number;
  fileCount: number;
  agentCount: number;
}

const TAB_ICONS: Record<SessionTab, React.ComponentType<{ className?: string }>> = {
  overview: LayoutDashboard,
  conversation: MessageSquare,
  tools: Wrench,
  files: FileText,
  agents: Bot,
};

/** Spec §14's Overview | Conversation | Tools | Files | Agents tab strip. */
export const SessionTabs: React.FC<SessionTabsProps> = ({
  activeTab,
  onTabChange,
  toolCallCount,
  fileCount,
  agentCount,
}) => {
  const { t } = useTranslation();

  const counts: Partial<Record<SessionTab, number>> = {
    tools: toolCallCount,
    files: fileCount,
    agents: agentCount,
  };

  const tabs: SessionTab[] = ["overview", "conversation", "tools", "files", "agents"];

  return (
    <div className="flex items-center gap-1 border-b border-border bg-muted/20 px-4">
      {tabs.map((tab) => {
        const Icon = TAB_ICONS[tab];
        const count = counts[tab];
        const isActive = tab === activeTab;
        return (
          <button
            key={tab}
            type="button"
            onClick={() => onTabChange(tab)}
            aria-current={isActive ? "true" : undefined}
            className={cn(
              "flex items-center gap-1.5 border-b-2 px-2.5 py-2 text-xs font-medium transition-colors",
              isActive
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground"
            )}
          >
            <Icon className="h-3.5 w-3.5" />
            {t(`session.tabs.${tab}`)}
            {count !== undefined && count > 0 && (
              <span className="rounded-full bg-muted px-1.5 text-2xs tabular-nums text-muted-foreground">
                {count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
};
