import type { ClaudeSession } from "@/types";

export interface SessionItemProps {
  session: ClaudeSession;
  isSelected: boolean;
  onSelect: () => void;
  onHover?: () => void;
  formatTimeAgo: (date: string) => string;
  /** Whether the list is in multi-select mode (renders a checkbox) */
  isSelectionMode?: boolean;
  /** Whether this row is checked in multi-select mode */
  isChecked?: boolean;
  /**
   * Toggle this row's checkbox. Receives the mouse event so the caller can
   * read modifier keys (Shift = range, Cmd/Ctrl = individual toggle).
   */
  onToggleSelect?: (e: React.MouseEvent | React.KeyboardEvent) => void;
  /**
   * Start/extend a selection from normal mode via a modifier click
   * (Cmd/Ctrl+click or Shift+click). Enters selection mode.
   */
  onModifierSelect?: (e: React.MouseEvent) => void;
}

export interface SessionHeaderProps {
  isArchivedCodexSession: boolean;
  isSelected: boolean;
}

export interface SessionNameEditorProps {
  displayName: string | undefined;
  hasClaudeCodeName: boolean;
  isNamed: boolean;
  isSelected: boolean;
  isContextMenuOpen: boolean;
  providerId: string;
  supportsResumeCommand: boolean;
  supportsRevealInFinder: boolean;
  onCopySessionId: (e: React.MouseEvent) => void;
  onCopyResumeCommand: (e: React.MouseEvent) => void;
  onCopyFilePath: (e: React.MouseEvent) => void;
  onRevealInFinder: (e: React.MouseEvent) => void;
  onContextMenuOpenChange: (open: boolean) => void;
}

export interface SessionMetaProps {
  session: ClaudeSession;
  isSelected: boolean;
  formatTimeAgo: (date: string) => string;
}
