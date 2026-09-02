import type { ClaudeSession, HistorySessionItem, ProviderId } from "../types";

/**
 * A `HistorySessionItem` carries only session-level fields (no `model`
 * distribution, no `is_renamed`/`storage_type`/`entrypoint` -- those are
 * either provider-internal or not yet known at History's list-granularity).
 * `SessionItem` only reads fields this shape actually has, so the gaps are
 * safe, sensible defaults, not lossy guesses. Shared between History's own
 * `SessionDateList` and Home's "Recent work" section (spec §11), both of
 * which render a `HistorySessionsPage`'s items via the same `SessionItem`
 * row component.
 */
export function toClaudeSessionFromHistoryItem(item: HistorySessionItem): ClaudeSession {
  return {
    session_id: item.session_id,
    actual_session_id: item.actual_session_id,
    file_path: item.file_path,
    project_name: item.project_name,
    message_count: item.message_count,
    first_message_time: item.first_message_time ?? "",
    last_message_time: item.last_message_time ?? "",
    last_modified: item.recency_time,
    has_tool_use: item.has_tool_use,
    has_errors: item.has_errors,
    summary: item.summary,
    is_renamed: false,
    // HistorySessionItem.provider_id is a plain string on the backend DTO,
    // but it's always one of the real provider ids ClaudeSession.provider's
    // ProviderId union already covers.
    provider: item.provider_id as ProviderId,
  };
}
