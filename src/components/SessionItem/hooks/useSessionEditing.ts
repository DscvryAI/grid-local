import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useAppStore } from "@/store/useAppStore";
import { isAbsolutePath } from "@/utils/pathUtils";
import {
  getResumeCommand,
  supportsNativeRename as providerSupportsNativeRename,
  supportsResumeCommandForSession,
} from "@/utils/providers";
import type { ClaudeSession } from "@/types";
import { copyTextToClipboard } from "@/utils/clipboard";

/**
 * Session name display + read-only utility actions (copy id/resume/path,
 * reveal in finder). No rename affordance of any kind, including a
 * Grid-local-only one — Grid Local is a read-only viewer (spec §22/§23);
 * the "CLI" badge below only ever reflects a provider's own existing title,
 * it does not let Grid Local set one.
 */
export function useSessionEditing(session: ClaudeSession) {
  const { t } = useTranslation();
  const [isContextMenuOpen, setIsContextMenuOpen] = useState(false);

  const providerId = session.provider ?? "claude";
  const isServerReadOnly = useAppStore((state) => state.isServerReadOnly);
  const selectedProject = useAppStore((state) => state.selectedProject);
  const loadedSessions = useAppStore((state) => state.sessions);
  const projects = useAppStore((state) => state.projects);
  // Read-only: whether this session already carries a provider-native title
  // (displayed as a "CLI" badge). Grid Local cannot set this itself — see
  // read-only guarantee, spec §22/§46 — it only reflects existing data.
  const hasNativeTitleSupport = providerSupportsNativeRename(providerId);
  const supportsRevealInFinder = isAbsolutePath(session.file_path);
  const isArchivedCodexSession =
    providerId === "codex" &&
    /(?:^|[\\/])archived_sessions(?:[\\/]|$)/.test(session.file_path);

  const displayName = session.summary;
  const hasClaudeCodeNamePattern = /^\[.+?\]\s/.test(session.summary ?? "");
  const hasClaudeCodeName =
    providerId === "claude"
      ? hasClaudeCodeNamePattern
      : hasNativeTitleSupport && !!session.is_renamed;
  const isNamed = hasClaudeCodeName || !!session.is_renamed;

  const handleCopyToClipboard = useCallback(
    async (e: React.MouseEvent, text: string, successMsg: string) => {
      e.stopPropagation();
      setIsContextMenuOpen(false);
      try {
        await copyTextToClipboard(text);
        toast.success(successMsg);
      } catch {
        toast.error(t("copyButton.error", "Copy failed"));
      }
    },
    [t]
  );

  const handleCopySessionId = useCallback(
    (e: React.MouseEvent) =>
      handleCopyToClipboard(
        e,
        session.actual_session_id,
        t("session.copiedSessionId", "Session ID copied")
      ),
    [handleCopyToClipboard, session.actual_session_id, t]
  );

  const projectForSession = useMemo(() => {
    const isLoadedInSelectedProject =
      !!selectedProject &&
      loadedSessions.some(
        (loadedSession) =>
          loadedSession.session_id === session.session_id ||
          loadedSession.file_path === session.file_path
      );

    if (isLoadedInSelectedProject) {
      return selectedProject;
    }

    return (
      projects.find(
        (project) =>
          (project.provider ?? "claude") === providerId &&
          project.name === session.project_name
      ) ?? projects.find((project) => project.name === session.project_name)
    );
  }, [loadedSessions, projects, providerId, selectedProject, session.file_path, session.project_name, session.session_id]);

  const projectPathUnavailable = projectForSession?.path_status === "unavailable";
  const projectCwd = projectPathUnavailable ? undefined : projectForSession?.actual_path;
  const supportsResumeCommand =
    supportsResumeCommandForSession(providerId, session.entrypoint) &&
    !projectPathUnavailable;

  const handleCopyResumeCommand = useCallback(
    (e: React.MouseEvent) => {
      const resumeCommand = projectPathUnavailable
        ? null
        : getResumeCommand(
            providerId,
            session.actual_session_id,
            projectCwd,
            session.entrypoint
          );
      if (!resumeCommand) {
        e.stopPropagation();
        setIsContextMenuOpen(false);
        toast.error(
          projectPathUnavailable
            ? t(
                "session.resumeUnavailableLocation",
                "Resume is unavailable because the last-known working directory is missing"
              )
            : t("session.copyResumeCommandError", "Resume command unavailable")
        );
        return;
      }

      return handleCopyToClipboard(
        e,
        resumeCommand,
        projectCwd
          ? t("session.copiedResumeCommand", "Resume command copied")
          : t(
              "session.copiedResumeCommandNoCwd",
              "Resume command copied (working directory unknown)"
            )
      );
    },
    [
      handleCopyToClipboard,
      projectCwd,
      projectPathUnavailable,
      providerId,
      session.actual_session_id,
      session.entrypoint,
      t,
    ]
  );

  const handleCopyFilePath = useCallback(
    (e: React.MouseEvent) =>
      handleCopyToClipboard(
        e,
        session.file_path,
        t("session.copiedFilePath", "File path copied")
      ),
    [handleCopyToClipboard, session.file_path, t]
  );

  const handleRevealInFinder = useCallback(
    async (e: React.MouseEvent) => {
      e.stopPropagation();
      setIsContextMenuOpen(false);
      if (!session.file_path || !supportsRevealInFinder) {
        toast.error(t("session.revealError", "Could not reveal file"));
        return;
      }
      try {
        const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
        await revealItemInDir(session.file_path);
      } catch {
        toast.error(t("session.revealError", "Could not reveal file"));
      }
    },
    [session.file_path, supportsRevealInFinder, t]
  );

  return {
    // State
    isContextMenuOpen,
    displayName,
    hasClaudeCodeName,
    isNamed,
    providerId,
    supportsResumeCommand,
    supportsRevealInFinder,
    isArchivedCodexSession,
    isServerReadOnly,

    // Actions
    setIsContextMenuOpen,
    handleCopySessionId,
    handleCopyResumeCommand,
    handleCopyFilePath,
    handleRevealInFinder,
  };
}
