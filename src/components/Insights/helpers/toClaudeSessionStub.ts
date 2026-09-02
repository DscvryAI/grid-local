import type { ClaudeSession } from "@/types";

/**
 * Builds a minimal `ClaudeSession` from an Insights card's own fields, so
 * a card can drill straight into `onSessionSelect` the same way History's
 * `SessionDateList` does (`toClaudeSession` there). Insights cards carry
 * only `session_id`/`project_name`/an optional summary -- the gaps below
 * (message_count, timestamps, has_tool_use/has_errors) are safe, sensible
 * defaults, not lossy guesses; `session_id` doubles as `file_path`, and
 * `archive_db` is Claude-only today so `provider: "claude"` is always
 * correct here.
 */
export function toClaudeSessionStub(
  sessionId: string,
  projectName: string,
  summary?: string
): ClaudeSession {
  return {
    session_id: sessionId,
    actual_session_id: sessionId,
    file_path: sessionId,
    project_name: projectName,
    message_count: 0,
    first_message_time: "",
    last_message_time: "",
    last_modified: "",
    has_tool_use: false,
    has_errors: false,
    summary,
    is_renamed: false,
    provider: "claude",
  };
}
