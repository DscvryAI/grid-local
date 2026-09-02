//! DTOs for `archive_db::search` (search-on-FTS).

use serde::{Deserialize, Serialize};

/// One search-result facet kind -- mirrors `search_fts.kind`'s literal
/// string values exactly (see `archive_db::schema`'s migration 6 doc for
/// which normalized column backs each facet).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchResultKind {
    Message,
    Command,
    ToolResult,
    File,
    Error,
    AgentInstruction,
}

impl SearchResultKind {
    /// The exact string `search_fts.kind` stores for this variant --
    /// used to build the SQL `IN (...)` facet filter without a second,
    /// hand-maintained string table.
    pub fn as_sql_str(self) -> &'static str {
        match self {
            SearchResultKind::Message => "message",
            SearchResultKind::Command => "command",
            SearchResultKind::ToolResult => "tool_result",
            SearchResultKind::File => "file",
            SearchResultKind::Error => "error",
            SearchResultKind::AgentInstruction => "agent_instruction",
        }
    }
}

/// One matched row, joined back to its owning session/project/provider
/// for display -- `session_id` is the session's `file_path`, matching
/// every other DTO's "unique ID based on file path" convention (see
/// `models::insights`'s own doc) so a result's drill-down can reuse the
/// existing open-session flow unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub kind: SearchResultKind,
    /// A bounded, `snippet()`-generated excerpt around the match --
    /// never the full body text.
    pub snippet: String,
    pub session_id: String,
    pub project_name: String,
    pub provider_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    /// The exact message's own `uuid`, when `kind` is [`SearchResultKind::
    /// Message`] -- lets a click deep-link to that specific message the
    /// same way the raw-scan search path already does, instead of just
    /// opening the session. `None` for every other facet (a command/
    /// error/file/tool-output/agent-instruction match traces back to a
    /// row that isn't itself a message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_uuid: Option<String>,
    /// The message's own `role` (`"user"`/`"assistant"`/...), alongside
    /// `message_uuid` -- same "only for `Message`" scoping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    pub total_count: usize,
}
