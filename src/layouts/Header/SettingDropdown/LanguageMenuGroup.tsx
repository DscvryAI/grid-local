import {
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
} from "@/components/ui/dropdown-menu";
import { supportedLanguages, type SupportedLanguage } from "@/i18n";
import { useLanguageStore } from "@/store/useLanguageStore";
import { useTranslation } from "react-i18next";

const radioItemClass =
  "pl-2 [&>span:first-child]:hidden data-[state=checked]:bg-accent data-[state=checked]:text-accent-foreground";

/** Flat -- see ThemeMenuGroup's own doc comment for why. */
export const LanguageMenuGroup = () => {
  const { language, setLanguage } = useLanguageStore();
  const { t } = useTranslation();

  return (
    <>
      <DropdownMenuLabel>{t("common.settings.language.title")}</DropdownMenuLabel>
      <DropdownMenuRadioGroup
        value={language}
        onValueChange={(value) => {
          if (value in supportedLanguages) {
            void setLanguage(value as SupportedLanguage);
          }
        }}
      >
        {Object.entries(supportedLanguages).map(([code, name]) => (
          <DropdownMenuRadioItem key={code} value={code} className={radioItemClass}>
            <span>{name}</span>
          </DropdownMenuRadioItem>
        ))}
      </DropdownMenuRadioGroup>
    </>
  );
};
