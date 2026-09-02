import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { getThirdPartyNotices } from "@/services/settingsApi";

interface LicensesModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * About > Licences (spec §31). Same "render outside the dropdown's own
 * content" reasoning as `DeleteGridDataConfirmDialog` -- lives at the
 * persistent `SettingDropdown` level so it survives the triggering
 * dropdown item's own unmount. Plain preformatted text, not a full
 * markdown render -- THIRD_PARTY_NOTICES.md is a short, simple
 * attribution list, not worth a markdown-rendering dependency for.
 */
export const LicensesModal: React.FC<LicensesModalProps> = ({ open, onOpenChange }) => {
  const { t } = useTranslation();
  const [notices, setNotices] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setNotices(null);
    setLoadError(null);
    getThirdPartyNotices()
      .then((text) => {
        if (!cancelled) setNotices(text);
      })
      .catch((error) => {
        if (!cancelled) {
          setLoadError(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("common.settings.about.licenses")}</DialogTitle>
        </DialogHeader>
        {notices === null && !loadError && (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        )}
        {loadError && (
          <div className="py-4 text-center text-sm text-destructive">{loadError}</div>
        )}
        {notices !== null && (
          <pre className="max-h-[60vh] overflow-y-auto whitespace-pre-wrap rounded-md border bg-muted/30 p-3 font-mono text-xs text-foreground">
            {notices}
          </pre>
        )}
      </DialogContent>
    </Dialog>
  );
};
