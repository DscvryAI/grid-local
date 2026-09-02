import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
  Button,
} from "@/components/ui";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";
import { getDiagnosticsSnapshot, recordDiagnosticsEvent } from "@/services/diagnosticsApi";
import type { DiagnosticsLog } from "@/types/diagnostics.types";

interface ExportDiagnosticsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Data > "Export diagnostics" -- satisfies an inspectable-before-export
 * requirement: the user must be able to see exactly what will be exported.
 * Same persistent-sibling-of-DropdownMenu pattern as
 * `DeleteGridDataConfirmDialog` (see that file's own doc comment for why).
 * Unlike that dialog, this one fetches and DISPLAYS the exact payload
 * before any export action is available -- the user sees the real JSON,
 * not a description of it, before deciding to save it anywhere.
 */
export const ExportDiagnosticsDialog: React.FC<ExportDiagnosticsDialogProps> = ({
  open,
  onOpenChange,
}) => {
  const { t } = useTranslation();
  const [log, setLog] = useState<DiagnosticsLog | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isExporting, setIsExporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLog(null);
    setLoadError(null);
    getDiagnosticsSnapshot()
      .then((result) => {
        if (!cancelled) setLog(result);
      })
      .catch((error) => {
        console.error("Failed to load diagnostics snapshot:", error);
        if (!cancelled) {
          setLoadError(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const handleExport = async () => {
    if (!log) return;
    setIsExporting(true);
    try {
      const content = JSON.stringify(log, null, 2);
      // Dynamic import so `@/utils/fileDialog` is genuinely code-splittable
      // -- this was previously the ONLY static importer, which forced the
      // whole module (and `useExport.ts`'s own already-dynamic import of
      // it) into the main bundle regardless.
      const { saveFileDialog } = await import("@/utils/fileDialog");
      const saved = await saveFileDialog(content, {
        defaultPath: "grid-local-diagnostics.json",
        mimeType: "application/json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (saved) {
        void recordDiagnosticsEvent({ kind: "exported", artifactType: "diagnostics" });
        toast.success(t("common.settings.data.diagnostics.exportSuccess", "Diagnostics exported"));
        onOpenChange(false);
      }
    } catch (error) {
      console.error("Failed to export diagnostics:", error);
      toast.error(t("common.settings.data.diagnostics.exportError", "Failed to export diagnostics"));
    } finally {
      setIsExporting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !isExporting && onOpenChange(next)}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {t("common.settings.data.diagnostics.title", "Export diagnostics")}
          </DialogTitle>
          <DialogDescription>
            {t(
              "common.settings.data.diagnostics.description",
              "This is exactly what would be exported -- local usage counts only. No conversation content, prompts, commands, code, filenames, or raw paths are ever included."
            )}
          </DialogDescription>
        </DialogHeader>

        <div className="max-h-80 overflow-y-auto rounded-md border border-border bg-muted/30">
          {loadError ? (
            <p className="p-3 text-sm text-destructive">{loadError}</p>
          ) : log ? (
            <pre className="whitespace-pre-wrap break-all p-3 font-mono text-2xs text-foreground">
              {JSON.stringify(log, null, 2)}
            </pre>
          ) : (
            <div className="flex items-center justify-center gap-2 p-6 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("common.settings.data.diagnostics.loading", "Loading diagnostics…")}
            </div>
          )}
        </div>

        <DialogFooter className="gap-2 sm:gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isExporting}
          >
            {t("common.cancel")}
          </Button>
          <Button
            type="button"
            onClick={() => void handleExport()}
            disabled={isExporting || !log}
          >
            {isExporting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("common.settings.data.diagnostics.exportAction", "Export")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
