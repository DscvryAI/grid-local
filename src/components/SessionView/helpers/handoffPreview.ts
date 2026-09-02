import { describeVerificationStatus } from "./verificationStatusDisplay";
import { deriveUnresolvedItems } from "./sessionIntelligence";
import type { SessionDecisionBrief, FileEvent } from "./sessionIntelligence";
import type { ClaudeSession } from "@/types";

type Translate = (key: string, defaultValueOrOptions?: string | Record<string, unknown>) => string;

/**
 * One-action handoff preview: a single compiled, read-only summary --
 * goal, changes, verification, unresolved items, source -- for a user
 * handing this session off to someone else (or their future self). Every
 * field is a real, already-derived fact reused as-is (goal/verification
 * from the session decision brief, changed files from
 * `extractFileEvents`, unresolved items from `deriveUnresolvedItems`) --
 * nothing here is generated text, per the standing no-AI-generation
 * decision. This function only formats; `Copy as Markdown` in the UI
 * keeps export a separate, explicit action -- clipboard-only, matching
 * this app's existing "copy resume command"/"copy session IDs" precedent
 * rather than adding a new file-write surface for what's fundamentally
 * the same kind of action.
 */
export function buildHandoffPreviewMarkdown(
  session: ClaudeSession,
  brief: SessionDecisionBrief,
  changedFiles: FileEvent[],
  t: Translate
): string {
  const lines: string[] = [`# ${t("session.handoff.title", "Handoff preview")}`];

  if (brief.goal) {
    lines.push("", `**${t("session.brief.goal", "Goal")}:** ${brief.goal}`);
  }

  const verificationDisplay = describeVerificationStatus(brief.verification, t);
  lines.push(
    "",
    `**${t("session.handoff.verification", "Verification")}:** ${
      verificationDisplay ? verificationDisplay.text : t("session.handoff.noChanges", "No changes were made")
    }`
  );

  lines.push("", `**${t("session.handoff.changes", "Changes")} (${changedFiles.length}):**`);
  if (changedFiles.length === 0) {
    lines.push(`- ${t("session.tabs.filesEmpty", "No files changed in this session")}`);
  } else {
    for (const file of changedFiles) {
      lines.push(`- ${file.filePath}`);
    }
  }

  const unresolved = deriveUnresolvedItems(brief);
  lines.push("", `**${t("session.handoff.unresolved", "Unresolved items")}:**`);
  if (unresolved.length === 0) {
    lines.push(`- ${t("session.handoff.noneUnresolved", "None")}`);
  } else {
    for (const kind of unresolved) {
      const text =
        kind === "endedOnError"
          ? t("session.brief.endedOnError", "Session ended on a tool error")
          : (verificationDisplay?.text ?? kind);
      lines.push(`- ${text}`);
    }
  }

  lines.push("", `**${t("session.handoff.source", "Source")}:** ${session.file_path}`);

  return lines.join("\n");
}
