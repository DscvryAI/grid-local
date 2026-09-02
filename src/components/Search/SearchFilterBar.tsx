import React, { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Filter, User, Bot, MessageSquare, X } from "lucide-react";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useAppStore } from "@/store/useAppStore";
import { cn } from "@/lib/utils";
import { getProviderId, getProviderLabel } from "@/utils/providers";
import type { UseGlobalSearchResult } from "@/hooks/useGlobalSearch";
import type { ProviderId } from "@/types";

interface SearchFilterBarProps {
  search: UseGlobalSearchResult;
}

interface AvailableProvider {
  id: ProviderId;
  displayName: string;
}

/**
 * Search's Message Type / Project / Provider / Date filters (spec §15),
 * shared by the Cmd/Ctrl+K palette and the full-page Search surface.
 * Provider reuses the store's `activeProviders` field the same way
 * History's own filter bar does (`HistoryFilterBar.tsx`) rather than a
 * second, competing filter state.
 */
export const SearchFilterBar: React.FC<SearchFilterBarProps> = ({ search }) => {
  const { t } = useTranslation();
  const { projects, activeProviders, setActiveProviders } = useAppStore();
  const {
    messageTypeFilter,
    setMessageTypeFilter,
    selectedProjectPath,
    setSelectedProjectPath,
    dateFilter,
    setDateFilter,
    dateRangeError,
  } = search;

  // Derived from `projects` (the already-loaded, always-populated project
  // list), NOT the separate `providers` detection array -- that array can
  // legitimately be empty (provider auto-detection never ran/completed
  // for this install) even while `projects` is full of real, multi-
  // provider data. Found via live UI testing: an empty `providers` array
  // silently hid this whole filter section and, worse, broke "Clear
  // filters"/"All Providers" (see the `allProviderIds` note below).
  // Mirrors `ProjectTree`'s own `discoveredFromProjects` fallback.
  const availableProviders = useMemo(() => {
    const seen = new Map<ProviderId, AvailableProvider>();
    for (const project of projects) {
      const id = getProviderId(project.provider);
      if (!seen.has(id)) {
        seen.set(id, {
          id,
          displayName: getProviderLabel((key, fallback) => t(key, fallback), id),
        });
      }
    }
    return Array.from(seen.values());
  }, [projects, t]);
  const allProviderIds = useMemo(
    () => availableProviders.map((p) => p.id),
    [availableProviders]
  );
  const isAllProvidersSelected =
    activeProviders.length === 0 ||
    availableProviders.every((p) => activeProviders.includes(p.id));

  // Mirrors HistoryFilterBar's own hasActiveFilters/handleClearAll --
  // "Clear filters" only shown once something is actually narrowed, so
  // the bar doesn't carry a dead button in its default state.
  const hasActiveFilters =
    !isAllProvidersSelected ||
    selectedProjectPath !== "all" ||
    messageTypeFilter !== "all" ||
    Boolean(dateFilter.startDate) ||
    Boolean(dateFilter.endDate);

  const handleClearAll = useCallback(() => {
    // NOT `[]` -- ProjectTree's own isAllProvidersSelected/
    // matchesProviderFilter (the sidebar's authoritative filter) treats
    // an empty activeProviders array as "match nothing," not "match
    // everything" -- it requires every selectable provider explicitly
    // listed. Found via live UI testing: clicking this button emptied
    // the entire sidebar. applyProviderSelection's own `normalized.
    // length === 0` guard confirms this is the established invariant.
    setActiveProviders(allProviderIds);
    setSelectedProjectPath("all");
    setMessageTypeFilter("all");
    setDateFilter({});
  }, [setActiveProviders, allProviderIds, setSelectedProjectPath, setMessageTypeFilter, setDateFilter]);

  return (
    <div className="flex flex-wrap items-center gap-2 px-4 py-2 border-b border-border bg-muted/20">
      {/* Message Type Filter */}
      <div className="flex items-center gap-1">
        {(["all", "user", "assistant"] as const).map((type) => (
          <button
            key={type}
            onClick={() => setMessageTypeFilter(type)}
            className={cn(
              "flex items-center gap-1 px-2 py-1 text-xs rounded-md transition-colors",
              messageTypeFilter === type
                ? "bg-foreground/10 text-foreground font-medium"
                : "text-muted-foreground hover:text-foreground hover:bg-muted"
            )}
            aria-label={t(`globalSearch.filterType.${type}`)}
          >
            {type === "all" && <MessageSquare className="w-3 h-3" />}
            {type === "user" && <User className="w-3 h-3" />}
            {type === "assistant" && <Bot className="w-3 h-3" />}
            <span>{t(`globalSearch.filterType.${type}`)}</span>
          </button>
        ))}
      </div>

      {(projects.length > 1 || availableProviders.length > 1) && (
        <div className="w-px h-4 bg-border" />
      )}

      {/* Project Filter */}
      {projects.length > 1 && (
        <>
          <Filter className="w-3.5 h-3.5 text-muted-foreground shrink-0" />
          <Select value={selectedProjectPath} onValueChange={setSelectedProjectPath}>
            <SelectTrigger className="h-7 text-xs border-border w-40">
              <SelectValue placeholder={t("globalSearch.allProjects")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t("globalSearch.allProjects")}</SelectItem>
              {projects.map((project) => (
                <SelectItem key={project.path} value={project.path}>
                  {project.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </>
      )}

      {/* Provider Filter -- same activeProviders toggle pattern as
          HistoryFilterBar's Provider buttons. */}
      {availableProviders.length > 1 && (
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setActiveProviders(allProviderIds)}
            className={cn(
              "rounded-md px-2 py-1 text-xs transition-colors",
              isAllProvidersSelected
                ? "bg-primary/10 text-primary"
                : "text-muted-foreground hover:bg-muted"
            )}
          >
            {t("globalSearch.filter.allProviders")}
          </button>
          {availableProviders.map((provider) => (
            <button
              key={provider.id}
              type="button"
              onClick={() => {
                if (isAllProvidersSelected) {
                  setActiveProviders([provider.id]);
                  return;
                }
                if (activeProviders.includes(provider.id)) {
                  const next = activeProviders.filter((id) => id !== provider.id);
                  setActiveProviders(next.length > 0 ? next : allProviderIds);
                  return;
                }
                setActiveProviders([...activeProviders, provider.id]);
              }}
              className={cn(
                "rounded-md px-2 py-1 text-xs transition-colors",
                !isAllProvidersSelected && activeProviders.includes(provider.id)
                  ? "bg-primary/10 text-primary"
                  : "text-muted-foreground hover:bg-muted"
              )}
            >
              {provider.displayName}
            </button>
          ))}
        </div>
      )}

      {/* Date Filter -- `max`/`min` keep the calendar picker itself from
          offering an inverted range; `dateRangeError` (shown inline) is
          the fallback for a range typed in manually past those bounds,
          so performSearch never round-trips an invalid range to the
          backend just to fail. */}
      <div className="flex items-center gap-1 text-xs text-muted-foreground">
        <input
          type="date"
          aria-label={t("globalSearch.filter.startDate")}
          aria-invalid={Boolean(dateRangeError)}
          className={cn(
            "h-7 rounded-md border bg-background px-2 text-xs",
            dateRangeError ? "border-destructive" : "border-border"
          )}
          value={dateFilter.startDate ?? ""}
          max={dateFilter.endDate}
          onChange={(e) =>
            setDateFilter({ ...dateFilter, startDate: e.target.value || undefined })
          }
        />
        <span>–</span>
        <input
          type="date"
          aria-label={t("globalSearch.filter.endDate")}
          aria-invalid={Boolean(dateRangeError)}
          className={cn(
            "h-7 rounded-md border bg-background px-2 text-xs",
            dateRangeError ? "border-destructive" : "border-border"
          )}
          value={dateFilter.endDate ?? ""}
          min={dateFilter.startDate}
          onChange={(e) => setDateFilter({ ...dateFilter, endDate: e.target.value || undefined })}
        />
        {dateRangeError && <span className="text-destructive">{dateRangeError}</span>}
      </div>

      {/* Clear all -- only shown once something is actually filtered. */}
      {hasActiveFilters && (
        <button
          type="button"
          onClick={handleClearAll}
          className="ml-auto flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <X className="h-3 w-3" />
          {t("globalSearch.filter.clearAll")}
        </button>
      )}
    </div>
  );
};
