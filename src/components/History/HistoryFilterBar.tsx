import React, { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useAppStore } from "@/store/useAppStore";
import type { HistoryDateFilter } from "../../hooks/useHistorySessions";
import type { HistoryProjectFacet, HistoryProviderFacet, ProviderId } from "../../types";

const ALL_VALUE = "__all__";

interface HistoryFilterBarProps {
  availableProjects: HistoryProjectFacet[];
  availableProviders: HistoryProviderFacet[];
  availableModels: string[];
  projectKeys: string[];
  onProjectKeysChange: (keys: string[]) => void;
  dateFilter: HistoryDateFilter;
  onDateFilterChange: (filter: HistoryDateFilter) => void;
  models: string[];
  onModelsChange: (models: string[]) => void;
}

/**
 * Spec §13's four always-visible filters: Project / Provider / Date /
 * Model. Provider reuses the store's existing `activeProviders` (the same
 * field ProjectTree's own provider tabs drive) rather than a second,
 * competing filter state. Project and Model are single-select in this
 * first version -- the hook/backend already accept arrays (`project_keys`/
 * `models`), so upgrading to multi-select later is additive, not a
 * redesign; single-select was the pragmatic v1 scope call, not a backend
 * limitation.
 */
export const HistoryFilterBar: React.FC<HistoryFilterBarProps> = ({
  availableProjects,
  availableProviders,
  availableModels,
  projectKeys,
  onProjectKeysChange,
  dateFilter,
  onDateFilterChange,
  models,
  onModelsChange,
}) => {
  const { t } = useTranslation();
  const activeProviders = useAppStore((s) => s.activeProviders);
  const setActiveProviders = useAppStore((s) => s.setActiveProviders);

  // `HistoryProviderFacet.provider_id` is a plain `string` on the backend
  // DTO (any of the ~29 provider ids), while the store's `activeProviders`
  // is the closed `ProviderId` union -- these are the same real ids
  // (`stats_provider_id()` on the backend produces exactly this set), so a
  // cast here is a type-system technicality, not a real narrowing.
  const allProviderIds = availableProviders.map((p) => p.provider_id as ProviderId);
  const isAllProvidersSelected =
    activeProviders.length === 0 ||
    allProviderIds.every((id) => activeProviders.includes(id));

  const handleProviderToggle = useCallback(
    (providerId: string) => {
      if (providerId === ALL_VALUE) {
        // NOT `[]` -- `ProjectTree`'s own `isAllProvidersSelected`/
        // `matchesProviderFilter` (the sidebar's authoritative filter)
        // requires every selectable provider EXPLICITLY listed to mean
        // "all"; an empty array instead means "match nothing" there,
        // which silently emptied the whole sidebar (#: found via live
        // UI testing). `applyProviderSelection`'s own `normalized.length
        // === 0` guard confirms this is the established invariant.
        setActiveProviders(allProviderIds);
        return;
      }
      const id = providerId as ProviderId;
      if (isAllProvidersSelected) {
        setActiveProviders([id]);
        return;
      }
      if (activeProviders.includes(id)) {
        const next = activeProviders.filter((existing) => existing !== id);
        setActiveProviders(next.length > 0 ? next : allProviderIds);
        return;
      }
      setActiveProviders([...activeProviders, id]);
    },
    [activeProviders, allProviderIds, isAllProvidersSelected, setActiveProviders]
  );

  const selectedProjectKey =
    projectKeys.length === 1 ? projectKeys[0] : ALL_VALUE;
  const selectedModel = models.length === 1 ? models[0] : ALL_VALUE;

  const hasActiveFilters =
    !isAllProvidersSelected ||
    projectKeys.length > 0 ||
    models.length > 0 ||
    Boolean(dateFilter.startDate) ||
    Boolean(dateFilter.endDate);

  const handleClearAll = useCallback(() => {
    setActiveProviders(allProviderIds);
    onProjectKeysChange([]);
    onDateFilterChange({});
    onModelsChange([]);
  }, [setActiveProviders, allProviderIds, onProjectKeysChange, onDateFilterChange, onModelsChange]);

  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-border bg-muted/20 px-4 py-2">
      {/* Provider */}
      <div className="flex items-center gap-1">
        <button
          type="button"
          onClick={() => handleProviderToggle(ALL_VALUE)}
          className={cn(
            "rounded-md px-2 py-1 text-xs transition-colors",
            isAllProvidersSelected
              ? "bg-primary/10 text-primary"
              : "text-muted-foreground hover:bg-muted"
          )}
        >
          {t("history.filter.allProviders")}
        </button>
        {availableProviders.map((provider) => (
          <button
            key={provider.provider_id}
            type="button"
            onClick={() => handleProviderToggle(provider.provider_id)}
            className={cn(
              "rounded-md px-2 py-1 text-xs transition-colors",
              !isAllProvidersSelected &&
                activeProviders.includes(provider.provider_id as ProviderId)
                ? "bg-primary/10 text-primary"
                : "text-muted-foreground hover:bg-muted"
            )}
          >
            {provider.display_name} ({provider.session_count})
          </button>
        ))}
      </div>

      {/* Project */}
      <Select
        value={selectedProjectKey}
        onValueChange={(value) =>
          onProjectKeysChange(value === ALL_VALUE ? [] : [value])
        }
      >
        <SelectTrigger className="h-7 w-40 border-border text-xs">
          <SelectValue placeholder={t("history.filter.allProjects")} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL_VALUE}>{t("history.filter.allProjects")}</SelectItem>
          {availableProjects.map((project) => {
            const key = `${project.provider_id}:${project.project_key}`;
            return (
              <SelectItem key={key} value={key}>
                {project.project_name} ({project.session_count})
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>

      {/* Date */}
      <div className="flex items-center gap-1 text-xs text-muted-foreground">
        <input
          type="date"
          aria-label={t("history.filter.startDate")}
          className="h-7 rounded-md border border-border bg-background px-2 text-xs"
          value={dateFilter.startDate ?? ""}
          onChange={(e) =>
            onDateFilterChange({
              ...dateFilter,
              startDate: e.target.value || undefined,
            })
          }
        />
        <span>–</span>
        <input
          type="date"
          aria-label={t("history.filter.endDate")}
          className="h-7 rounded-md border border-border bg-background px-2 text-xs"
          value={dateFilter.endDate ?? ""}
          onChange={(e) =>
            onDateFilterChange({
              ...dateFilter,
              endDate: e.target.value || undefined,
            })
          }
        />
      </div>

      {/* Model */}
      <Select
        value={selectedModel}
        onValueChange={(value) => onModelsChange(value === ALL_VALUE ? [] : [value])}
      >
        <SelectTrigger className="h-7 w-36 border-border text-xs">
          <SelectValue placeholder={t("history.filter.allModels")} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL_VALUE}>{t("history.filter.allModels")}</SelectItem>
          {availableModels.map((model) => (
            <SelectItem key={model} value={model}>
              {model}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      {/* Clear all -- only shown once something is actually filtered, so
          the bar doesn't carry a dead button in its default state. */}
      {hasActiveFilters && (
        <button
          type="button"
          onClick={handleClearAll}
          className="ml-auto flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <X className="h-3 w-3" />
          {t("history.filter.clearAll")}
        </button>
      )}
    </div>
  );
};
