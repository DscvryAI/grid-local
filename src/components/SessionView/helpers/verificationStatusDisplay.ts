import type React from "react";
import { CheckCircle2, XCircle, AlertTriangle, HelpCircle } from "lucide-react";
import { formatRelativeTime } from "@/utils/time";
import type { VerificationStatus } from "./sessionIntelligence";

export interface VerificationStatusDisplay {
  icon: React.ComponentType<{ className?: string }>;
  className: string;
  text: string;
}

/**
 * Shared verification-status -> icon/color/text mapping, used by both
 * `OverviewTab`'s decision brief and `FilesTab`'s per-file detail -- kept
 * in one place so the two surfaces never drift into describing the same
 * `VerificationStatus` value differently. `t` is passed in (rather than
 * calling `useTranslation` here) so this stays a plain function, callable
 * from anywhere, not a hook.
 */
export function describeVerificationStatus(
  verification: VerificationStatus,
  t: (key: string, options?: Record<string, unknown>) => string
): VerificationStatusDisplay | null {
  switch (verification.kind) {
    case "no-changes":
      return null;
    case "unverified":
      return {
        icon: HelpCircle,
        className: "text-muted-foreground",
        text: t("session.brief.unverified", {
          count: verification.fileCount,
          defaultValue: "{{count}} files changed; no test/build command found to verify them",
        }),
      };
    case "verified":
      return {
        icon: CheckCircle2,
        className: "text-success",
        text: t("session.brief.verified", {
          command: verification.command,
          time: formatRelativeTime(verification.timestamp),
          defaultValue: 'Verified: "{{command}}" passed {{time}}',
        }),
      };
    case "failed":
      return {
        icon: XCircle,
        className: "text-destructive",
        text: t("session.brief.failed", {
          command: verification.command,
          time: formatRelativeTime(verification.timestamp),
          defaultValue: 'Verification failed: "{{command}}" {{time}}',
        }),
      };
    case "stale":
      return {
        icon: AlertTriangle,
        className: "text-warning",
        text: t("session.brief.stale", {
          count: verification.filesChangedSince,
          time: formatRelativeTime(verification.timestamp),
          defaultValue: "{{count}} files changed after the last passing verification ({{time}})",
        }),
      };
  }
}
