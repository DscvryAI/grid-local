import React from "react";
import { useTranslation } from "react-i18next";
import { BarChart3, Wrench, Bot, AlertTriangle, Sparkles } from "lucide-react";
import { cn } from "@/lib/utils";
import type { InsightsTab } from "@/store/slices/navigationSlice";

interface InsightsTabsProps {
  activeTab: InsightsTab;
  onTabChange: (tab: InsightsTab) => void;
}

const TAB_ICONS: Record<InsightsTab, React.ComponentType<{ className?: string }>> = {
  questions: Sparkles,
  usage: BarChart3,
  tools: Wrench,
  agents: Bot,
  problems: AlertTriangle,
};

/** Tab strip led by "questions" ahead of Usage | Tools | Agents | Problems. */
export const InsightsTabs: React.FC<InsightsTabsProps> = ({ activeTab, onTabChange }) => {
  const { t } = useTranslation();
  const tabs: InsightsTab[] = ["questions", "usage", "tools", "agents", "problems"];

  return (
    <div className="flex items-center gap-1 border-b border-border bg-muted/20 px-4">
      {tabs.map((tab) => {
        const Icon = TAB_ICONS[tab];
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
            {t(`insights.tabs.${tab}`)}
          </button>
        );
      })}
    </div>
  );
};
