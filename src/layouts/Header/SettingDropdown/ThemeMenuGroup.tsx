import {
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
} from "@/components/ui/dropdown-menu";
import { useTranslation } from "react-i18next";
import { Sun, Moon, Laptop } from "lucide-react";
import { useTheme } from "@/contexts/theme";

const THEME_ITEMS = [
  { icon: Sun, labelKey: "common.settings.theme.light", value: "light" },
  { icon: Moon, labelKey: "common.settings.theme.dark", value: "dark" },
  { icon: Laptop, labelKey: "common.settings.theme.system", value: "system" },
] as const;

const radioItemClass =
  "gap-2 pl-2 [&>span:first-child]:hidden data-[state=checked]:bg-accent data-[state=checked]:text-accent-foreground";

/**
 * Flat (no own `DropdownMenuSub` flyout) since this is only ever embedded
 * inside `AppearanceMenuGroup`'s own single flyout -- Radix's
 * `DropdownMenuSub` nested two levels deep (a Sub inside another Sub's
 * content) was found live to close the ENTIRE menu on click instead of
 * opening its own flyout, not just a cosmetic quirk. Flattening avoids
 * the whole class of bug rather than chasing it further.
 */
export const ThemeMenuGroup = () => {
  const { theme, setTheme } = useTheme();
  const { t } = useTranslation();

  return (
    <>
      <DropdownMenuLabel>{t("common.settings.theme.title")}</DropdownMenuLabel>
      <DropdownMenuRadioGroup
        value={theme}
        onValueChange={(value) => {
          if (value === "light" || value === "dark" || value === "system") {
            void setTheme(value);
          }
        }}
      >
        {THEME_ITEMS.map(({ icon: Icon, labelKey, value }) => (
          <DropdownMenuRadioItem key={value} value={value} className={radioItemClass}>
            <Icon className="h-4 w-4" />
            <span>{t(labelKey)}</span>
          </DropdownMenuRadioItem>
        ))}
      </DropdownMenuRadioGroup>
    </>
  );
};
