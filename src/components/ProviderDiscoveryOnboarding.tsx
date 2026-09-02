import React, { useState } from "react";
import { Search, Loader2, CheckCircle2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useAppStore } from "@/store/useAppStore";

/**
 * Screen 1 of the first-run experience (spec §5/§6): a single branded
 * screen with one primary action. Clicking it triggers automatic discovery
 * across every supported provider — no separate "just use Claude Code"
 * choice (spec §6: "No user configuration unless discovery fails"; the
 * existing Claude-folder-picker fallback still applies if discovery finds
 * nothing). Shown once — gated on
 * `userMetadata.settings.hasSeenProviderDiscoveryPrompt` in
 * `projectSlice.initializeApp`. See read-only guarantee (spec §22/§46): no
 * provider directory is scanned before this explicit, single tap.
 *
 * Two-step flow: the first tap only runs the lightweight `detectProviders()`
 * (no project scan yet) and, when it finds anything, shows a "Ready to
 * scan" manifest -- naming each detected provider and its real path before
 * the actual scan runs. A second, explicit tap on that screen triggers the
 * real scan (the original `completeProviderDiscoveryOnboarding` flow,
 * unchanged). When detection fails or finds nothing, this falls straight
 * through to that same flow exactly as before -- preserving the existing
 * manual-fallback path (an empty manifest would be a worse experience than
 * no manifest at all).
 */
export const ProviderDiscoveryOnboarding: React.FC = () => {
  const { t } = useTranslation();
  const [showPrivacyInfo, setShowPrivacyInfo] = useState(false);
  const [hasPreviewedProviders, setHasPreviewedProviders] = useState(false);
  const completeProviderDiscoveryOnboarding = useAppStore(
    (s) => s.completeProviderDiscoveryOnboarding
  );
  const detectProviders = useAppStore((s) => s.detectProviders);
  const providers = useAppStore((s) => s.providers);
  const isDetectingProviders = useAppStore((s) => s.isDetectingProviders);
  const isLoadingProjects = useAppStore((s) => s.isLoadingProjects);
  const isBusy = isDetectingProviders || isLoadingProjects;
  const availableProviders = providers.filter((p) => p.is_available);
  const showManifest = hasPreviewedProviders && availableProviders.length > 0;

  const handlePrimaryClick = async () => {
    if (hasPreviewedProviders) {
      void completeProviderDiscoveryOnboarding();
      return;
    }
    const detected = await detectProviders();
    const fresh = useAppStore.getState().providers.filter((p) => p.is_available);
    if (!detected || fresh.length === 0) {
      // Nothing real to preview -- proceed exactly as the original
      // single-tap flow did, so the existing manual-fallback path (no
      // supported provider found) is untouched.
      void completeProviderDiscoveryOnboarding();
      return;
    }
    setHasPreviewedProviders(true);
  };

  return (
    <div className="h-screen flex items-center justify-center bg-background">
      <div className="text-center max-w-md mx-auto p-8">
        <div className="w-16 h-16 rounded-2xl bg-accent/10 flex items-center justify-center mx-auto mb-6">
          {isBusy ? (
            <Loader2 className="w-8 h-8 text-accent animate-spin" />
          ) : (
            <Search className="w-8 h-8 text-accent" />
          )}
        </div>
        <h1 className="text-2xl font-semibold text-foreground mb-1">
          {t("onboarding.firstRun.appName", "Grid Local")}
        </h1>
        <p className="text-sm font-medium text-foreground mb-4">
          {t(
            "onboarding.firstRun.tagline",
            "Your AI coding history, finally useful."
          )}
        </p>

        {showManifest ? (
          <>
            <h2 className="text-sm font-semibold text-foreground mb-3">
              {t("onboarding.firstRun.readyToScan", "Ready to scan")}
            </h2>
            <ul className="text-left mb-4 rounded-lg border border-border/50 divide-y divide-border/50 overflow-hidden">
              {availableProviders.map((provider) => (
                <li
                  key={provider.id}
                  className="flex items-center gap-2 px-3 py-2 text-sm"
                >
                  <CheckCircle2 className="w-3.5 h-3.5 shrink-0 text-success" />
                  <span className="font-medium text-foreground">
                    {provider.display_name}
                  </span>
                  <span className="text-xs text-muted-foreground shrink-0">
                    {t("onboarding.firstRun.detected", "detected")}
                  </span>
                  <span
                    className="ml-auto min-w-0 truncate text-2xs text-muted-foreground"
                    title={provider.base_path}
                  >
                    {provider.base_path}
                  </span>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <p className="text-sm text-muted-foreground mb-2">
            {t(
              "onboarding.firstRun.description",
              "Grid reads the AI coding sessions already stored on this computer and turns them into searchable history and useful insights."
            )}
          </p>
        )}

        <p className="text-xs text-muted-foreground mb-6">
          {t(
            "onboarding.firstRun.privacyNote",
            "Grid reads your coding history without modifying it. Its index and preferences remain in Grid's local application storage."
          )}
        </p>
        <div className="flex flex-col items-center gap-3">
          <button
            type="button"
            onClick={() => void handlePrimaryClick()}
            disabled={isBusy}
            className="action-btn primary w-full"
          >
            {isBusy
              ? t(
                  "project.discoveringProviders",
                  "Searching for providers..."
                )
              : showManifest
                ? t("onboarding.firstRun.confirmScan", "Scan coding history")
                : t(
                    "onboarding.firstRun.scan",
                    "Scan my coding history"
                  )}
          </button>
          <button
            type="button"
            onClick={() => setShowPrivacyInfo((prev) => !prev)}
            className="text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
          >
            {t(
              "onboarding.firstRun.privacyLink",
              "How Grid handles my data"
            )}
          </button>
        </div>
        {showPrivacyInfo && (
          <div className="mt-4 text-left text-xs text-muted-foreground bg-muted/30 border border-border/50 rounded-lg p-4 space-y-1">
            <p>
              {t(
                "onboarding.firstRun.privacyDetail.readOnly",
                "Grid only reads session files already on this computer — it never writes into Claude Code, Codex, Cursor, or any other tool's directories."
              )}
            </p>
            <p>
              {t(
                "onboarding.firstRun.privacyDetail.noUpload",
                "Conversation content never leaves this device. No account, no cloud sync, no telemetry."
              )}
            </p>
            <p>
              {t(
                "onboarding.firstRun.privacyDetail.export",
                "Nothing is sent anywhere unless you explicitly export or share it yourself."
              )}
            </p>
          </div>
        )}
      </div>
    </div>
  );
};
