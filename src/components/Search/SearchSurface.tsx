import React, { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Search, Loader2, X } from "lucide-react";
import { Input } from "@/components/ui";
import { useGlobalSearch } from "@/hooks/useGlobalSearch";
import { SearchFilterBar } from "@/components/Search/SearchFilterBar";
import { SearchResultsList } from "@/components/Search/SearchResultsList";

/**
 * Full-page Search surface (spec §15). Renders the SAME `useGlobalSearch`
 * hook and `SearchFilterBar`/`SearchResultsList` components as the
 * Cmd/Ctrl+K palette (`GlobalSearchModal.tsx`) -- one search implementation,
 * not a forked second one. Replaces `AppLayout`'s previous inline
 * `<GlobalSearchModal isOpen onClose={...}/>` wiring with a genuine
 * non-Dialog surface.
 */
export const SearchSurface: React.FC = () => {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const search = useGlobalSearch();
  const { query, isSearching, handleInputChange, clearQuery, handleKeyDown, results } = search;

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="flex items-center gap-3 border-b border-border px-4 py-3">
        <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
        <Input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => handleInputChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter") {
              e.preventDefault();
              handleKeyDown(e.key);
            }
          }}
          placeholder={t("globalSearch.placeholder")}
          className="h-auto border-0 px-0 text-base shadow-none focus-visible:ring-0"
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
        />
        {isSearching && <Loader2 className="h-4 w-4 shrink-0 animate-spin text-muted-foreground" />}
        {query && !isSearching && (
          <button
            onClick={() => {
              clearQuery();
              inputRef.current?.focus();
            }}
            className="rounded p-1 hover:bg-muted"
            aria-label={t("globalSearch.clearSearch")}
          >
            <X className="h-3.5 w-3.5 text-muted-foreground" />
          </button>
        )}
        {results.length > 0 && (
          <span className="shrink-0 text-xs text-muted-foreground">
            {t("globalSearch.results", { count: results.length })}
          </span>
        )}
      </div>

      <SearchFilterBar search={search} />
      <SearchResultsList search={search} variant="page" />
    </div>
  );
};
