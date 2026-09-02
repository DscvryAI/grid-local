/**
 * DTOs for `archive_db::search` (search-on-FTS). Mirror
 * `src-tauri/src/models/search.rs` field-for-field, matching
 * `insights.types.ts`'s own established plain-snake_case convention.
 */

export type SearchResultKind =
  | "message"
  | "command"
  | "tool_result"
  | "file"
  | "error"
  | "agent_instruction";

export interface SearchResult {
  kind: SearchResultKind;
  snippet: string;
  session_id: string;
  project_name: string;
  provider_key: string;
  occurred_at?: string;
  /** The real message that owns this hit, resolved for EVERY kind --
   * previously only set for `kind === "message"`; every other kind
   * silently had no message to navigate to, breaking "open in context".
   * See `archive_db::search::search_archive`'s own doc comment for
   * exactly how each kind's `ref_id` resolves back to a message. */
  message_uuid?: string;
  /** The resolved message's own `role` ("user"/"assistant"/...), set
   * alongside `message_uuid`. */
  message_role?: string;
}
