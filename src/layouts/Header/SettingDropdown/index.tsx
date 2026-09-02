import { useState, Suspense, lazy } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { Settings, Loader2 } from "lucide-react";

import type { UseUpdaterReturn } from "@/hooks/useUpdater";
import { useTranslation } from "react-i18next";
import { DataMenuGroup } from "./DataMenuGroup";
import { ProvidersMenuGroup } from "./ProvidersMenuGroup";
import { AppearanceMenuGroup } from "./AppearanceMenuGroup";
import { AboutMenuGroup } from "./AboutMenuGroup";
import { DeleteGridDataConfirmDialog } from "./DeleteGridDataConfirmDialog";
import { ExportDiagnosticsDialog } from "./ExportDiagnosticsDialog";

// A rarely-opened About/Licenses modal -- code-split so its chunk only
// loads the first time a user actually opens it.
const LicensesModal = lazy(() =>
  import("./LicensesModal").then((m) => ({ default: m.LicensesModal }))
);

interface SettingDropdownProps {
  updater: UseUpdaterReturn;
}

/**
 * "Settings should be tiny" (spec §31). Shrunk to
 * exactly the 4 sections spec lists -- Data / Providers / Appearance /
 * About -- each its own flyout submenu, so the top-level menu itself
 * never shows more than 4 rows regardless of how much lives inside each
 * section. The two confirmation dialogs (delete data, licences) are
 * rendered here at the persistent wrapper level rather than inside their
 * triggering DataMenuGroup/AboutMenuGroup -- a DropdownMenuItem click
 * closes (and Radix unmounts) the dropdown's own content, which would
 * destroy any state local to a component living inside it.
 */
export const SettingDropdown = ({ updater }: SettingDropdownProps) => {
  const { t } = useTranslation();
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showExportDiagnostics, setShowExportDiagnostics] = useState(false);
  const [showLicenses, setShowLicenses] = useState(false);

  const isCheckingForUpdates = updater.state.isChecking;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            id="app-settings-button"
            className="p-2 rounded-lg transition-colors cursor-pointer relative text-muted-foreground/50 hover:text-foreground/80 hover:bg-muted"
            aria-label={t("common.settings.title")}
          >
            <Settings className="w-5 h-5 text-foreground" />
            {isCheckingForUpdates && (
              <Loader2 className="absolute -top-1 -right-1 w-3 h-3 animate-spin text-blue-500" />
            )}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuLabel>{t('common.settings.title')}</DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DataMenuGroup
            onRequestDelete={() => setShowDeleteConfirm(true)}
            onRequestExportDiagnostics={() => setShowExportDiagnostics(true)}
          />
          <ProvidersMenuGroup />
          <AppearanceMenuGroup />
          <AboutMenuGroup updater={updater} onOpenLicenses={() => setShowLicenses(true)} />
        </DropdownMenuContent>
      </DropdownMenu>
      <DeleteGridDataConfirmDialog
        open={showDeleteConfirm}
        onOpenChange={setShowDeleteConfirm}
      />
      <ExportDiagnosticsDialog
        open={showExportDiagnostics}
        onOpenChange={setShowExportDiagnostics}
      />
      <Suspense fallback={null}>
        <LicensesModal open={showLicenses} onOpenChange={setShowLicenses} />
      </Suspense>
    </>
  );
};
