//! DTOs for the History surface (spec §13).
//!
//! Deliberately not a widened `ClaudeSession`: that type is used
//! pervasively elsewhere with its own per-provider serialization quirks
//! and has no `provider`/`model` field. Widening it for History's sake
//! would have a blast radius across every existing consumer -- a
//! purpose-built DTO matches this module's existing per-feature split
//! (see `stats.rs` alongside `session.rs`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HistorySessionItem {
    /// == `file_path`, matching `ClaudeSession`'s existing "unique ID based
    /// on file path" convention so the existing open-session flow keeps
    /// working unchanged against this DTO.
    pub session_id: String,
    /// The actual session ID from the messages themselves -- distinct from
    /// `session_id` for some providers. Needed alongside `session_id` by
    /// the existing session-open flow's matching logic (`App.tsx`'s
    /// `handleSessionSelect`, `GlobalSearchModal`'s `handleSelectResult`),
    /// which both match on either field.
    pub actual_session_id: String,
    pub provider_id: String,
    /// Provider-scoped, stable -- what the Project filter selects by.
    pub project_key: String,
    pub project_name: String,
    pub file_path: String,
    /// The single timestamp History sorts and buckets on (Today/Yesterday/
    /// Earlier this week) -- `last_message_time`, falling back to
    /// `last_modified` when absent. See `archive_db::history`'s doc for why
    /// this is the right choice over `first_message_time`.
    pub recency_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_message_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_time: Option<String>,
    pub message_count: usize,
    pub has_tool_use: bool,
    pub has_errors: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `None` = unavailable for this provider (Tier B/C degradation), not
    /// "no model was used" -- the frontend renders this as "Unknown", never
    /// as an empty/zero value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HistoryProjectFacet {
    pub provider_id: String,
    pub project_key: String,
    pub project_name: String,
    pub session_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HistoryProviderFacet {
    pub provider_id: String,
    pub display_name: String,
    pub session_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct HistorySessionsPage {
    pub items: Vec<HistorySessionItem>,
    /// Post-filter, pre-pagination -- what "has_more"/page-count UI needs.
    pub total_count: usize,
    pub has_more: bool,
    /// Computed from the full filtered-but-unpaginated set, reflecting the
    /// current selection as a whole (not per-dimension independent
    /// narrowing -- a v1 simplification).
    pub available_projects: Vec<HistoryProjectFacet>,
    pub available_providers: Vec<HistoryProviderFacet>,
    /// "Unknown" included only when at least one matched session has no
    /// model data.
    pub available_models: Vec<String>,
    /// True when the caller has custom Claude directories configured but
    /// this response only reflects the default `~/.claude` directory --
    /// custom directories aren't ingested into `archive_db` yet (matches
    /// `get_global_stats_summary`'s existing precedent). Spec §18's "never
    /// pretend all providers offer equivalent telemetry" principle, applied
    /// honestly instead of silently omitting those sessions with no signal.
    pub custom_claude_dirs_omitted: bool,
}

/// Filter parameters accepted by `commands::history::list_history_sessions`.
/// All fields `None`/empty = no filter on that dimension.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryFilterParams {
    #[serde(default)]
    pub project_keys: Option<Vec<String>>,
    #[serde(default)]
    pub provider_ids: Option<Vec<String>>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    /// "Unknown" is a valid sentinel value here, matching
    /// `available_models`.
    #[serde(default)]
    pub models: Option<Vec<String>>,
}
