import React from "react";
import { useTranslation } from "react-i18next";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
  Button,
} from "@/components/ui";
import { useAppStore } from "@/store/useAppStore";

/**
 * One-time, non-blocking preference prompt: should Grid keep its local
 * archive index (`archive_db`, used by Insights and the future Home
 * surface) up to date automatically, or only when the user asks?
 *
 * Without this prompt, no frontend code path ever calls `sync_grid_index`,
 * so the archive would never get populated for a real install.
 * Deliberately NOT a full-screen blocking step like
 * `ProviderDiscoveryOnboarding` -- History/Search work fine without
 * archive_db, so this renders as a dismissible dialog over the already-
 * usable app instead of gating it. Gated on
 * `userMetadata.settings.hasSeenArchiveSyncPrompt` in
 * `projectSlice.initializeApp`; dismissing (Escape/backdrop/close) is
 * treated as picking "manual" (the agreed safe default), matching this
 * app's existing "never scan without an explicit choice" precedent.
 */
export const ArchiveSyncPreferencePrompt: React.FC = () => {
  const { t } = useTranslation();
  const showArchiveSyncPrompt = useAppStore((s) => s.showArchiveSyncPrompt);
  const completeArchiveSyncPrompt = useAppStore(
    (s) => s.completeArchiveSyncPrompt
  );

  return (
    <Dialog
      open={showArchiveSyncPrompt}
      onOpenChange={(open) => {
        if (!open) void completeArchiveSyncPrompt("manual");
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {t("common.archiveSyncPrompt.title")}
          </DialogTitle>
          <DialogDescription>
            {t("common.archiveSyncPrompt.description")}
          </DialogDescription>
        </DialogHeader>
        <p className="text-xs text-muted-foreground">
          {t("common.archiveSyncPrompt.changeLater")}
        </p>
        <DialogFooter className="flex-col gap-2 sm:flex-col">
          <Button
            type="button"
            className="w-full"
            onClick={() => void completeArchiveSyncPrompt("auto")}
          >
            {t("common.archiveSyncPrompt.auto")}
          </Button>
          <Button
            type="button"
            variant="outline"
            className="w-full"
            onClick={() => void completeArchiveSyncPrompt("manual")}
          >
            {t("common.archiveSyncPrompt.manual")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
