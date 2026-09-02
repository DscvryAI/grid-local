import { useEffect, useState } from "react";
import {
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { Database, Download, FolderOpen, RefreshCw, Loader2, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useAppStore } from "@/store/useAppStore";
import { rebuildGridIndex } from "@/services/archiveSyncApi";
import { getMetadataFolderPath } from "@/services/settingsApi";

const radioItemClass =
  "gap-2 pl-2 [&>span:first-child]:hidden data-[state=checked]:bg-accent data-[state=checked]:text-accent-foreground";

interface DataMenuGroupProps {
  onRequestDelete: () => void;
  onRequestExportDiagnostics: () => void;
}

/**
 * "Data" section (spec §31): Grid archive location, "Open Grid data
 * folder," "Rebuild index," "Delete Grid's local data." Absorbs the
 * archive-sync preference's own radio+"Sync now" UI (formerly a standalone
 * `ArchiveSyncMenuGroup` entry) -- the underlying store actions don't
 * change, only where the controls render.
 */
export const DataMenuGroup: React.FC<DataMenuGroupProps> = ({
  onRequestDelete,
  onRequestExportDiagnostics,
}) => {
  const { t } = useTranslation();
  const userMetadata = useAppStore((s) => s.userMetadata);
  const updateUserSettings = useAppStore((s) => s.updateUserSettings);
  const syncArchiveIndex = useAppStore((s) => s.syncArchiveIndex);
  const [isSyncing, setIsSyncing] = useState(false);
  const [isRebuilding, setIsRebuilding] = useState(false);
  const [dataFolderPath, setDataFolderPath] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getMetadataFolderPath()
      .then((path) => {
        if (!cancelled) setDataFolderPath(path);
      })
      .catch((error) => {
        console.error("Failed to resolve Grid's data folder path:", error);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const mode = userMetadata?.settings?.archiveSyncMode ?? "manual";

  const handleSyncNow = async () => {
    setIsSyncing(true);
    const summary = await syncArchiveIndex();
    setIsSyncing(false);
    if (summary) {
      toast.success(
        t("common.settings.archiveSync.syncSuccess", {
          count: summary.sessionsIngested,
        })
      );
    } else {
      toast.error(t("common.settings.archiveSync.syncError"));
    }
  };

  const handleRebuildIndex = async () => {
    setIsRebuilding(true);
    try {
      const summary = await rebuildGridIndex();
      toast.success(
        t("common.settings.data.rebuildSuccess", {
          count: summary.sessionsIngested,
        })
      );
    } catch (error) {
      console.error("Failed to rebuild Grid's index:", error);
      toast.error(t("common.settings.data.rebuildError"));
    } finally {
      setIsRebuilding(false);
    }
  };

  const handleOpenDataFolder = async () => {
    if (!dataFolderPath) return;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(dataFolderPath);
    } catch (error) {
      console.error("Failed to open Grid's data folder:", error);
      toast.error(t("common.settings.data.openFolderError"));
    }
  };

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Database className="mr-2 h-4 w-4 text-foreground" />
        <span>{t("common.settings.data.title")}</span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="max-h-[70vh] w-64 overflow-y-auto">
        <DropdownMenuLabel className="font-normal text-2xs text-muted-foreground">
          {dataFolderPath ?? t("common.settings.data.resolvingPath")}
        </DropdownMenuLabel>
        <DropdownMenuItem onClick={() => void handleOpenDataFolder()} disabled={!dataFolderPath}>
          <FolderOpen className="mr-2 h-4 w-4 text-foreground" />
          <span>{t("common.settings.data.openFolder")}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />

        <DropdownMenuLabel>{t("common.settings.archiveSync.title")}</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={mode}
          onValueChange={(value) => {
            if (value !== "auto" && value !== "manual") return;
            void updateUserSettings({
              archiveSyncMode: value,
              hasSeenArchiveSyncPrompt: true,
            });
            if (value === "auto") {
              void syncArchiveIndex();
            }
          }}
        >
          <DropdownMenuRadioItem value="auto" className={radioItemClass}>
            <span>{t("common.settings.archiveSync.auto")}</span>
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="manual" className={radioItemClass}>
            <span>{t("common.settings.archiveSync.manual")}</span>
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault();
            void handleSyncNow();
          }}
          disabled={isSyncing}
        >
          {isSyncing ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin text-foreground" />
          ) : (
            <RefreshCw className="mr-2 h-4 w-4 text-foreground" />
          )}
          <span>
            {isSyncing
              ? t("common.settings.archiveSync.syncing")
              : t("common.settings.archiveSync.syncNow")}
          </span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />

        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault();
            void handleRebuildIndex();
          }}
          disabled={isRebuilding}
        >
          {isRebuilding ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin text-foreground" />
          ) : (
            <RefreshCw className="mr-2 h-4 w-4 text-foreground" />
          )}
          <span>
            {isRebuilding
              ? t("common.settings.data.rebuilding")
              : t("common.settings.data.rebuildIndex")}
          </span>
        </DropdownMenuItem>
        <DropdownMenuItem onClick={onRequestExportDiagnostics}>
          <Download className="mr-2 h-4 w-4 text-foreground" />
          <span>{t("common.settings.data.diagnostics.menuItem")}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />

        <DropdownMenuItem
          onClick={onRequestDelete}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 className="mr-2 h-4 w-4" />
          <span>{t("common.settings.data.deleteData")}</span>
        </DropdownMenuItem>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
};
