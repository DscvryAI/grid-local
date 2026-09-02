import { FolderOpen, MessageSquare, Search, BarChart3 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import type { PrimarySurface } from "@/store/slices/navigationSlice";

type SwitchableSurface = Exclude<PrimarySurface, "home">;

interface BottomTabBarProps {
  activeSurface: PrimarySurface;
  onOpenSidebar: () => void;
  onSwitchSurface: (surface: SwitchableSurface) => void;
}

const surfaceTabs: Array<{
  id: SwitchableSurface;
  icon: typeof MessageSquare;
  labelKey: string;
}> = [
  { id: "history", icon: MessageSquare, labelKey: "common.mobile.tab.history" },
  { id: "search", icon: Search, labelKey: "common.mobile.tab.search" },
  { id: "insights", icon: BarChart3, labelKey: "common.mobile.tab.insights" },
];

export function BottomTabBar({ activeSurface, onOpenSidebar, onSwitchSurface }: BottomTabBarProps) {
  const { t } = useTranslation();

  return (
    <nav
      className="fixed bottom-0 inset-x-0 z-40 md:hidden h-14 bg-sidebar border-t border-border/50 pb-[env(safe-area-inset-bottom)]"
      aria-label="Navigation"
    >
      <div className="flex items-center justify-around h-full px-2">
        <button
          onClick={onOpenSidebar}
          className={cn(
            "relative flex flex-col items-center justify-center gap-0.5 flex-1 h-full transition-colors",
            "min-w-0 min-h-[var(--mobile-touch-target)] text-muted-foreground"
          )}
          aria-label={t("common.mobile.tab.projects")}
        >
          <FolderOpen className="w-5 h-5" />
          <span className="text-3xs font-medium truncate max-w-full">
            {t("common.mobile.tab.projects")}
          </span>
        </button>

        {surfaceTabs.map(({ id, icon: Icon, labelKey }) => {
          // Home has no tab of its own here yet -- this bar is deliberately
          // scoped to 3 items for mobile real estate; reachable from
          // desktop's nav only for now. `activeSurface === "home"` simply
          // shows no tab as active, which is honest -- none of these 3
          // represent it.
          const isActive = activeSurface === id;

          return (
            <button
              key={id}
              onClick={() => onSwitchSurface(id)}
              className={cn(
                "relative flex flex-col items-center justify-center gap-0.5 flex-1 h-full transition-colors",
                "min-w-0 min-h-[var(--mobile-touch-target)]",
                isActive ? "text-accent" : "text-muted-foreground"
              )}
              aria-label={t(labelKey)}
              aria-current={isActive ? "page" : undefined}
            >
              <Icon className="w-5 h-5" />
              <span className="text-3xs font-medium truncate max-w-full">{t(labelKey)}</span>
              {isActive && (
                <span className="absolute bottom-1 w-1 h-1 rounded-full bg-accent" />
              )}
            </button>
          );
        })}
      </div>
    </nav>
  );
}
