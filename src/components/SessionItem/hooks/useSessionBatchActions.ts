import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ClaudeSession } from "@/types";

/**
 * Mass operations for the multi-select session list.
 */
export function useSessionBatchActions() {
  const { t } = useTranslation();

  const copyIds = useCallback(
    async (sessions: ClaudeSession[]) => {
      if (sessions.length === 0) return;
      const text = sessions.map((s) => s.actual_session_id).join("\n");
      try {
        await navigator.clipboard.writeText(text);
        toast.success(
          t("session.selection.copiedIds", {
            count: sessions.length,
            defaultValue: "Copied {{count}} session ID(s)",
          })
        );
      } catch (error) {
        console.error("[session selection] copy ids failed", error);
        toast.error(t("copyButton.error", "Copy failed"));
      }
    },
    [t]
  );

  return { copyIds };
}
