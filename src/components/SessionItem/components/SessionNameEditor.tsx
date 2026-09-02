import React from "react";
import {
  Link2,
  Copy,
  FileText,
  FolderOpen,
  Play,
  MoreHorizontal,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { SessionNameEditorProps } from "../types";

export const SessionNameEditor: React.FC<SessionNameEditorProps> = ({
  displayName,
  hasClaudeCodeName,
  isNamed,
  isSelected,
  isContextMenuOpen,
  providerId,
  supportsResumeCommand,
  supportsRevealInFinder,
  onCopySessionId,
  onCopyResumeCommand,
  onCopyFilePath,
  onRevealInFinder,
  onContextMenuOpenChange,
}) => {
  const { t } = useTranslation();
  const cliSyncTitle =
    providerId === "codex"
      ? t("session.cliSync.titleCodex", "Session name synced with Codex CLI")
      : providerId === "opencode"
        ? t("session.cliSync.titleOpenCode", "Session name synced with OpenCode")
        : providerId === "forgecode"
          ? t("session.cliSync.titleForgeCode", "Session name synced with ForgeCode")
          : t("session.cliSync.title", "Session name synced with CLI");
  const cliSyncDescription =
    providerId === "codex"
      ? t("session.cliSync.descriptionCodex", "This session's name is also visible in Codex CLI")
      : providerId === "opencode"
        ? t("session.cliSync.descriptionOpenCode", "This session's name is also visible in OpenCode")
        : providerId === "forgecode"
          ? t("session.cliSync.descriptionForgeCode", "This session's name is also visible in ForgeCode")
          : t("session.cliSync.description", "This session's name is also visible in Claude Code CLI");

  return (
    <>
      <span
        className={cn(
          "text-xs leading-relaxed line-clamp-2 transition-colors duration-300 flex-1 flex items-start gap-1",
          isSelected ? "text-accent font-medium" : "text-sidebar-foreground/70"
        )}
        title={displayName}
      >
        {hasClaudeCodeName && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                className="inline-flex items-center gap-0.5 px-1 py-0.5 rounded bg-blue-500/10 border border-blue-500/20 hover:bg-blue-500/20 transition-colors cursor-help shrink-0"
                aria-label={cliSyncTitle}
              >
                <Link2
                  className="w-2.5 h-2.5 text-blue-400"
                  aria-hidden="true"
                />
                <span className="text-px9 font-medium text-blue-400 uppercase tracking-wide">
                  {t("session.cliSync.badge", "CLI")}
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-xs">
              <p className="font-medium">{cliSyncTitle}</p>
              <p className="text-xs text-muted-foreground mt-1">
                {cliSyncDescription}
              </p>
            </TooltipContent>
          </Tooltip>
        )}
        <span className={cn("flex-1", isNamed ? "font-bold" : "italic opacity-70")}>
          {displayName || t("session.summaryNotFound", "No summary")}
        </span>
      </span>

      {/* Session actions (copy/reveal only — Grid Local is read-only) */}
      <DropdownMenu
        open={isContextMenuOpen}
        onOpenChange={onContextMenuOpenChange}
      >
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            onClick={(e) => e.stopPropagation()}
            className={cn(
              "p-1 rounded opacity-40 md:opacity-0 md:group-hover:opacity-100 transition-opacity",
              "hover:bg-accent/20 text-muted-foreground hover:text-accent",
              isContextMenuOpen && "opacity-100"
            )}
            title={t("session.actions", "Session actions")}
            aria-label={t("session.actions", "Session actions")}
          >
            <MoreHorizontal className="w-3 h-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuItem onClick={onCopySessionId}>
            <Copy className="w-3 h-3 mr-2" />
            {t("session.copySessionId", "Copy Session ID")}
          </DropdownMenuItem>
          {supportsResumeCommand && (
            <DropdownMenuItem onClick={onCopyResumeCommand}>
              <Play className="w-3 h-3 mr-2" />
              {t("session.copyResumeCommand", "Copy Resume Command")}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onClick={onCopyFilePath}>
            <FileText className="w-3 h-3 mr-2" />
            {t("session.copyFilePath", "Copy File Path")}
          </DropdownMenuItem>
          {supportsRevealInFinder && (
            <DropdownMenuItem onClick={onRevealInFinder}>
              <FolderOpen className="w-3 h-3 mr-2" />
              {t("session.showJsonlFile", "Show JSONL File")}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
};
