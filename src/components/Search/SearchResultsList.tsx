import React from "react";
import { useTranslation } from "react-i18next";
import {
  Lightbulb,
  MessageSquare,
  Search,
  User,
  Bot,
  Terminal,
  FileEdit,
  AlertTriangle,
  Wrench,
  Workflow,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { getProviderLabel, getProviderBadgeStyle } from "@/utils/providers";
import { cn } from "@/lib/utils";
import type { UseGlobalSearchResult, GlobalSearchResult } from "@/hooks/useGlobalSearch";

/**
 * The evidence-type badge giving results a recognizable taxonomy. Only
 * rendered when `result.searchKind` is set (the archive-FTS path) -- the
 * raw-scan fallback has no kind data, so it keeps the plain role-based
 * badge below unchanged. "Decision"/"Explanation" style categories are
 * deliberately NOT included: no deterministic signal exists anywhere in
 * the schema for either -- never fabricate a category with no real signal
 * behind it. `tool_result` (a non-error tool output) isn't labeled as
 * anything fancier either; it's labeled "Tool output" instead of
 * mislabeling it as something it isn't.
 */
function kindBadge(
  result: GlobalSearchResult,
  t: (key: string, fallback?: string) => string
): { icon: React.ComponentType<{ className?: string }>; label: string; className: string } | null {
  switch (result.searchKind) {
    case "command":
      return { icon: Terminal, label: t("globalSearch.kind.command", "Command"), className: "text-muted-foreground" };
    case "file":
      return { icon: FileEdit, label: t("globalSearch.kind.file", "File change"), className: "text-muted-foreground" };
    case "error":
      return { icon: AlertTriangle, label: t("globalSearch.kind.error", "Error"), className: "text-destructive" };
    case "tool_result":
      return { icon: Wrench, label: t("globalSearch.kind.toolResult", "Tool output"), className: "text-muted-foreground" };
    case "agent_instruction":
      return { icon: Workflow, label: t("globalSearch.kind.agentTask", "Agent task"), className: "text-accent" };
    case "message":
    case undefined:
      return null;
  }
}

interface SearchResultsListProps {
  search: UseGlobalSearchResult;
  /** "dialog" caps the list height for the Cmd/Ctrl+K palette; "page" fills
   * whatever scroll container the full-page Search surface gives it. */
  variant: "dialog" | "page";
}

/**
 * The grouped, keyboard-navigable results list shared by the Cmd/Ctrl+K
 * palette (`GlobalSearchModal`) and the full-page Search surface
 * (`SearchSurface`), extracted so both render from one implementation
 * instead of forking into two search UIs.
 */
export const SearchResultsList: React.FC<SearchResultsListProps> = ({ search, variant }) => {
  const { t } = useTranslation();
  const {
    query,
    results,
    isSearching,
    selectedIndex,
    setSelectedIndex,
    groupedResults,
    handleSelectResult,
    getSessionName,
    getPreviewText,
    getMatchLocation,
    formatTimestamp,
    highlightText,
  } = search;

  const resultsContainerRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (resultsContainerRef.current) {
      const selectedElement = resultsContainerRef.current.querySelector(
        `[data-index="${selectedIndex}"]`
      );
      selectedElement?.scrollIntoView({ block: "nearest" });
    }
  }, [selectedIndex]);

  let currentResultIndex = 0;

  return (
    <div
      ref={resultsContainerRef}
      className={variant === "dialog" ? "max-h-100 overflow-y-auto" : "flex-1 overflow-y-auto"}
    >
      {isSearching && results.length === 0 && (
        <div className="py-4 space-y-3 px-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <div key={i} className="animate-pulse">
              <div className="flex items-center gap-2 mb-1.5">
                <div className="h-4 w-12 bg-muted rounded" />
                <div className="h-3 w-20 bg-muted rounded" />
              </div>
              <div className="h-4 w-full bg-muted rounded mb-1" />
              <div className="h-4 w-3/4 bg-muted rounded" />
            </div>
          ))}
        </div>
      )}

      {!isSearching && query.trim().length >= 2 && results.length === 0 && (
        <div className="px-4 py-8 text-center text-sm text-muted-foreground">
          {t("globalSearch.noResults")}
        </div>
      )}

      {!query && (
        <div className="px-6 py-8 space-y-4">
          <div className="text-center">
            <Search className="w-8 h-8 text-muted-foreground/40 mx-auto mb-3" />
            <p className="text-sm text-muted-foreground">{t("globalSearch.hint")}</p>
          </div>
          <div className="space-y-2">
            <div className="flex items-start gap-2 text-xs text-muted-foreground/70">
              <Lightbulb className="w-3.5 h-3.5 mt-0.5 shrink-0" />
              <span>{t("globalSearch.tips.minChars")}</span>
            </div>
            <div className="flex items-start gap-2 text-xs text-muted-foreground/70">
              <Lightbulb className="w-3.5 h-3.5 mt-0.5 shrink-0" />
              <span>{t("globalSearch.tips.filters")}</span>
            </div>
            <div className="flex items-start gap-2 text-xs text-muted-foreground/70">
              <Lightbulb className="w-3.5 h-3.5 mt-0.5 shrink-0" />
              <span>{t("globalSearch.tips.navigate")}</span>
            </div>
          </div>
        </div>
      )}

      {query && query.trim().length < 2 && !isSearching && (
        <div className="px-4 py-8 text-center text-sm text-muted-foreground">
          {t("globalSearch.tips.minChars")}
        </div>
      )}

      {results.length > 0 && (
        <div className="py-2">
          {Array.from(groupedResults.entries()).map(([groupKey, group]) => (
            <div key={groupKey}>
              <div className="px-4 py-1.5 text-xs font-medium text-muted-foreground bg-muted sticky top-0 truncate flex items-center gap-2">
                {group.provider && group.provider !== "claude" && (
                  <Badge
                    size="sm"
                    className={cn("rounded px-1 py-0 text-2xs", getProviderBadgeStyle(group.provider))}
                  >
                    {getProviderLabel((key, fallback) => t(key, fallback), group.provider)}
                  </Badge>
                )}
                {group.pathUnavailable && (
                  <Badge
                    size="sm"
                    className="rounded px-1 py-0 text-2xs bg-amber-500/15 text-amber-700 dark:text-amber-300"
                    title={t("project.pathUnavailableDescription", {
                      defaultValue: "Last-known location is unavailable",
                    })}
                  >
                    {t("project.pathUnavailable", "Location unavailable")}
                  </Badge>
                )}
                <span className="truncate">{group.label}</span>
              </div>

              {group.items.map((result) => {
                const index = currentResultIndex++;
                const isSelected = index === selectedIndex;
                const sessionName = getSessionName(result);
                const matchLocation = getMatchLocation(result);
                const badge = kindBadge(result, (key, fallback) => t(key, fallback ?? key));

                return (
                  <button
                    key={result.uuid}
                    data-index={index}
                    onMouseEnter={() => setSelectedIndex(index)}
                    onClick={() => handleSelectResult(result)}
                    className={cn(
                      "w-full text-left px-4 py-2.5 hover:bg-muted/50 transition-colors",
                      isSelected && "bg-muted"
                    )}
                  >
                    <div className="flex items-start gap-3">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 mb-1">
                          {badge ? (
                            <span
                              className={cn(
                                "inline-flex items-center gap-1 text-xs px-1.5 py-0.5 rounded font-medium bg-muted",
                                badge.className
                              )}
                            >
                              <badge.icon className="w-3 h-3" />
                              {badge.label}
                            </span>
                          ) : (
                            <span
                              className={cn(
                                "inline-flex items-center gap-1 text-xs px-1.5 py-0.5 rounded font-medium",
                                result.type === "user"
                                  ? "bg-blue-500/10 text-blue-500"
                                  : result.type === "assistant"
                                    ? "bg-amber-500/10 text-amber-500"
                                    : "bg-gray-500/10 text-gray-500"
                              )}
                            >
                              {result.type === "user" && <User className="w-3 h-3" />}
                              {result.type === "assistant" && <Bot className="w-3 h-3" />}
                              {result.searchKind === "message"
                                ? t(
                                    result.type === "user"
                                      ? "globalSearch.kind.userRequest"
                                      : "globalSearch.kind.generatedResponse",
                                    result.type === "user" ? "User request" : "Generated response"
                                  )
                                : result.type}
                            </span>
                          )}
                          <span className="text-xs text-muted-foreground">
                            {formatTimestamp(result.timestamp)}
                          </span>
                          {matchLocation && (
                            <span className="text-2xs text-muted-foreground/70">
                              {t("globalSearch.matchedIn", { location: matchLocation })}
                            </span>
                          )}
                        </div>
                        {sessionName && (
                          <p className="flex items-center gap-1 text-xs text-muted-foreground/70 mb-0.5">
                            <MessageSquare className="w-3 h-3 shrink-0" />
                            <span className="truncate">{sessionName}</span>
                          </p>
                        )}
                        <p className="text-sm text-foreground line-clamp-2">
                          {highlightText(getPreviewText(result))}
                        </p>
                      </div>
                    </div>
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      )}
    </div>
  );
};
