import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Search, ArrowUp, ArrowDown, CornerDownLeft, X, Loader2 } from "lucide-react";
import { Dialog, DialogContent, Input } from "@/components/ui";
import { useGlobalSearch } from "@/hooks/useGlobalSearch";
import { SearchFilterBar } from "@/components/Search/SearchFilterBar";
import { SearchResultsList } from "@/components/Search/SearchResultsList";

interface GlobalSearchModalProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * Cmd/Ctrl+K quick palette. Consumes the same `useGlobalSearch` hook and
 * `SearchFilterBar`/`SearchResultsList` components as the full-page Search
 * surface (`SearchSurface.tsx`) -- one search implementation, rendered two
 * ways, not two forked search UIs.
 */
export const GlobalSearchModal = ({ isOpen, onClose }: GlobalSearchModalProps) => {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);

  const search = useGlobalSearch({
    onAfterSelect: onClose,
    onEscape: onClose,
  });
  const { query, isSearching, handleInputChange, clearQuery, handleKeyDown, reset, results } = search;

  useEffect(() => {
    if (isOpen) {
      setTimeout(() => inputRef.current?.focus(), 0);
    } else {
      reset();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="sm:max-w-2xl p-0 gap-0 overflow-hidden"
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === "Escape") {
            e.preventDefault();
            handleKeyDown(e.key);
          }
        }}
        showCloseButton={false}
        aria-label={t("globalSearch.title")}
      >
        {/* Search Header */}
        <div className="flex items-center gap-3 px-4 py-3 border-b border-border">
          <Search className="w-4 h-4 text-muted-foreground shrink-0" />
          <Input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => handleInputChange(e.target.value)}
            placeholder={t("globalSearch.placeholder")}
            className="border-0 shadow-none focus-visible:ring-0 px-0 h-auto text-sm"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
          />
          {isSearching && <Loader2 className="w-4 h-4 text-muted-foreground animate-spin shrink-0" />}
          {query && !isSearching && (
            <button
              onClick={() => {
                clearQuery();
                inputRef.current?.focus();
              }}
              className="p-1 hover:bg-muted rounded"
              aria-label={t("globalSearch.clearSearch")}
            >
              <X className="w-3 h-3 text-muted-foreground" />
            </button>
          )}
        </div>

        <SearchFilterBar search={search} />
        <SearchResultsList search={search} variant="dialog" />

        {/* Footer with keyboard hints */}
        <div className="flex items-center justify-between px-4 py-2 border-t border-border bg-muted/30 text-xs text-muted-foreground">
          <div className="flex items-center gap-4">
            <div className="flex items-center gap-1">
              <kbd className="px-1.5 py-0.5 bg-muted rounded border border-border font-mono">
                <ArrowUp className="w-3 h-3 inline" />
              </kbd>
              <kbd className="px-1.5 py-0.5 bg-muted rounded border border-border font-mono">
                <ArrowDown className="w-3 h-3 inline" />
              </kbd>
              <span className="ml-1">{t("globalSearch.navigate")}</span>
            </div>
            <div className="flex items-center gap-1">
              <kbd className="px-1.5 py-0.5 bg-muted rounded border border-border font-mono">
                <CornerDownLeft className="w-3 h-3 inline" />
              </kbd>
              <span className="ml-1">{t("globalSearch.select")}</span>
            </div>
            <div className="flex items-center gap-1">
              <kbd className="px-1.5 py-0.5 bg-muted rounded border border-border font-mono text-px10">esc</kbd>
              <span className="ml-1">{t("globalSearch.close")}</span>
            </div>
          </div>
          {results.length > 0 && <span>{t("globalSearch.results", { count: results.length })}</span>}
        </div>
      </DialogContent>
    </Dialog>
  );
};
