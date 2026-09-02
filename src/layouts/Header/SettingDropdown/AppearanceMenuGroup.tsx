import {
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { Palette } from "lucide-react";
import { ThemeMenuGroup } from "./ThemeMenuGroup";
import { FontMenuGroup } from "./FontMenuGroup";
import { AccessibilityMenuGroup } from "./AccessibilityMenuGroup";
import { LanguageMenuGroup } from "./LanguageMenuGroup";
import { FilterMenuGroup } from "./FilterMenuGroup";

/**
 * "Appearance" is spec's own section (spec §31) for System/Light/Dark
 * theme, but today's font-size/accessibility/language controls fold in
 * here too rather than being dropped -- and the message-display filter
 * toggles (Show System/Sub-agent Messages) follow the same "fold in,
 * don't drop a working control with no other home" precedent. Each
 * nested group is reused UNCHANGED (Radix supports DropdownMenuSub nested
 * inside another Sub's content natively) rather than flattening their
 * internals into one giant panel.
 */
export const AppearanceMenuGroup = () => {
  const { t } = useTranslation();

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Palette className="mr-2 h-4 w-4 text-foreground" />
        <span>{t("common.settings.appearance.title")}</span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="max-h-[70vh] overflow-y-auto">
        <ThemeMenuGroup />
        <DropdownMenuSeparator />
        <FontMenuGroup />
        <DropdownMenuSeparator />
        <AccessibilityMenuGroup />
        <DropdownMenuSeparator />
        <LanguageMenuGroup />
        <DropdownMenuSeparator />
        <FilterMenuGroup />
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
};
