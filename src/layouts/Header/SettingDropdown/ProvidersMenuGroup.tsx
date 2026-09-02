import { useEffect, useMemo, useState } from "react";
import {
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { Puzzle, Folder, RefreshCw, Loader2, CheckCircle2 } from "lucide-react";
import { useAppStore } from "@/store/useAppStore";
import { useModal } from "@/contexts/modal";
import { api } from "@/services/api";
import { getProviderId, getProviderLabel } from "@/utils/providers";
import type { ProviderId, ProviderTier } from "@/types";

interface DetectedProvider {
  id: ProviderId;
  displayName: string;
}

/** Raw `tier` letters (`archive_db.provider.tier`) -> user-facing wording
 * -- never the letter itself (spec's own "no Tier A/B/C jargon exposed"
 * ask). `undefined` (a provider never ingested into the archive yet)
 * renders no badge at all -- "coverage unknown" is not the same claim as
 * "basic support". */
function coverageLabelKey(tier: string | undefined): { label: string; tooltip: string } | null {
  if (tier === "A") {
    return {
      label: "common.settings.providers.coverageFull",
      tooltip: "common.settings.providers.coverageFullTooltip",
    };
  }
  if (tier === "B") {
    return {
      label: "common.settings.providers.coverageBasic",
      tooltip: "common.settings.providers.coverageBasicTooltip",
    };
  }
  return null;
}

/**
 * "Detected providers. Optional rescan." (spec §31). Also houses "Change
 * Folder" (moved from the top-level menu) -- both are about configuring
 * which directories Grid scans, so they share a home here rather than
 * "Data" (which spec scopes to Grid's OWN archive, not provider source
 * configuration).
 *
 * The list is derived from `projects` (always populated with real data),
 * NOT the separate `providers` auto-detection array -- `providers` can
 * sit empty/never-resolving on a real install even with 100+ real
 * multi-provider projects already loaded, so deriving from `projects`
 * avoids shipping a "0 detected" Providers section on a real install.
 *
 * Rescanning still reuses `discoverProviders()` (the SAME explicit-opt-in
 * action `ProviderDiscoveryOnboarding`/the folder-not-found flow already
 * use) -- deliberately NOT a silent auto-scan, and still the right way to
 * find a genuinely NEW provider not yet reflected in existing project
 * data.
 */
export const ProvidersMenuGroup = () => {
  const { t } = useTranslation();
  const { openModal } = useModal();
  const projects = useAppStore((s) => s.projects);
  const isDetectingProviders = useAppStore((s) => s.isDetectingProviders);
  const discoverProviders = useAppStore((s) => s.discoverProviders);

  const detected = useMemo<DetectedProvider[]>(() => {
    const seen = new Map<ProviderId, DetectedProvider>();
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

  // Fetched once per mount, not re-fetched on rescan -- a provider's tier is fixed by which
  // ingest code path handles it (Claude's native parser vs. the generic
  // file-based path), never something a rescan changes.
  const [tierByProvider, setTierByProvider] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    let cancelled = false;
    api<ProviderTier[]>("get_provider_tiers")
      .then((tiers) => {
        if (cancelled) return;
        setTierByProvider(new Map(tiers.map((entry) => [entry.provider_key, entry.tier])));
      })
      .catch((error) => {
        console.error("Failed to load provider coverage tiers:", error);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Puzzle className="mr-2 h-4 w-4 text-foreground" />
        <span>
          {t("common.settings.providers.title")} · {detected.length}
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="max-h-[70vh] overflow-y-auto">
        <DropdownMenuItem
          onClick={() => openModal("folderSelector", { mode: "change" })}
        >
          <Folder className="mr-2 h-4 w-4 text-foreground" />
          <span>{t("common.settings.changeFolder")}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>
          {t("common.settings.providers.detected", { count: detected.length })}
        </DropdownMenuLabel>
        {detected.length === 0 ? (
          <div className="px-2 py-1.5 text-xs text-muted-foreground">
            {t("common.settings.providers.empty")}
          </div>
        ) : (
          detected.map((provider) => {
            const coverage = coverageLabelKey(tierByProvider.get(provider.id));
            return (
              <div
                key={provider.id}
                className="flex items-center gap-2 px-2 py-1.5 text-sm text-foreground"
              >
                <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-accent" />
                <span className="truncate flex-1">{provider.displayName}</span>
                {coverage && (
                  <span
                    className="shrink-0 rounded px-1.5 py-0.5 text-2xs text-muted-foreground bg-muted"
                    title={t(coverage.tooltip)}
                  >
                    {t(coverage.label)}
                  </span>
                )}
              </div>
            );
          })
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={(e) => e.preventDefault()}
          onClick={() => void discoverProviders()}
          disabled={isDetectingProviders}
        >
          {isDetectingProviders ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin text-foreground" />
          ) : (
            <RefreshCw className="mr-2 h-4 w-4 text-foreground" />
          )}
          <span>
            {isDetectingProviders
              ? t("common.settings.providers.scanning")
              : t("common.settings.providers.rescan")}
          </span>
        </DropdownMenuItem>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
};
