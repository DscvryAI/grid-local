import {
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { Info, ScrollText, MessageSquare } from "lucide-react";
import { useModal } from "@/contexts/modal";
import type { UseUpdaterReturn } from "@/hooks/useUpdater";

interface AboutMenuGroupProps {
  updater: UseUpdaterReturn;
  onOpenLicenses: () => void;
}

/**
 * "About" section (spec §31): version, licences, report issue. "Report
 * issue" reuses the existing `FeedbackModal` (`openModal("feedback")`) --
 * it already defaults to `feedbackType: "bug"` (see `FeedbackModal.tsx`'s
 * own prefill effect), so no explicit prefill is needed to point it at
 * the right default. Version was previously only shown in the footer
 * status bar; this is a second, more discoverable place to find it, not a
 * replacement for the footer.
 *
 * **No "Check for updates" entry**: the updater plugin is `"active":
 * false` with empty endpoints/pubkey (no Dscvry signing keys or release
 * infra yet, so auto-update is disabled) -- clicking it would always
 * surface an error, a control that can never succeed. Hidden, not
 * removed: `useUpdater`/`SimpleUpdateManager` and its supporting
 * components are left fully intact, ready to be reconnected by
 * re-adding this one menu item once real signing infra exists --
 * deleting and later rewriting working infrastructure would be pure
 * churn. `updater` stays a required prop here only for
 * `state.currentVersion`.
 */
export const AboutMenuGroup: React.FC<AboutMenuGroupProps> = ({ updater, onOpenLicenses }) => {
  const { t } = useTranslation();
  const { openModal } = useModal();

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Info className="mr-2 h-4 w-4 text-foreground" />
        <span>{t("common.settings.about.title")}</span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <DropdownMenuLabel className="font-normal text-2xs text-muted-foreground">
          {t("common.settings.about.version", { version: updater.state.currentVersion })}
        </DropdownMenuLabel>
        <DropdownMenuItem onClick={onOpenLicenses}>
          <ScrollText className="mr-2 h-4 w-4 text-foreground" />
          <span>{t("common.settings.about.licenses")}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => openModal("feedback")}>
          <MessageSquare className="mr-2 h-4 w-4 text-foreground" />
          <span>{t("common.settings.about.reportIssue")}</span>
        </DropdownMenuItem>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
};
