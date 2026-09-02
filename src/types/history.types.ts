/**
 * History Types
 *
 * DTOs for the History surface (spec §13). Mirror
 * `src-tauri/src/models/history.rs` field-for-field.
 */

export interface HistorySessionItem {
  session_id: string;
  actual_session_id: string;
  provider_id: string;
  project_key: string;
  project_name: string;
  file_path: string;
  recency_time: string;
  first_message_time?: string;
  last_message_time?: string;
  message_count: number;
  has_tool_use: boolean;
  has_errors: boolean;
  summary?: string;
  /** `undefined` = unavailable for this provider ("Unknown"), never "no model used". */
  model?: string;
}

export interface HistoryProjectFacet {
  provider_id: string;
  project_key: string;
  project_name: string;
  session_count: number;
}

export interface HistoryProviderFacet {
  provider_id: string;
  display_name: string;
  session_count: number;
}

export interface HistorySessionsPage {
  items: HistorySessionItem[];
  total_count: number;
  has_more: boolean;
  available_projects: HistoryProjectFacet[];
  available_providers: HistoryProviderFacet[];
  /** "Unknown" included only when at least one matched session has no model data. */
  available_models: string[];
  custom_claude_dirs_omitted: boolean;
}

export interface HistoryFilterParams {
  /** Combined `provider_id:project_key` strings — see `combined_project_key` on the backend. */
  project_keys?: string[];
  provider_ids?: string[];
  start_date?: string;
  end_date?: string;
  /** "Unknown" is a valid selectable value here, matching `available_models`. */
  models?: string[];
}
