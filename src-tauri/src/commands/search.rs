//! Tauri command exposing `archive_db::search` (search-on-FTS) to the
//! frontend. Mirrors `commands::insights`'s open-connection-then-delegate
//! pattern exactly: no persistent connection held in app state.

use crate::archive_db;
use crate::models::{SearchResult, SearchResultKind};

/// This command previously accepted a caller-supplied `limit` with no
/// upper bound -- a malformed or compromised local UI call could request
/// an arbitrarily large result set. The real frontend caller
/// (`useGlobalSearch.tsx`) only ever asks for 100, so 500 is generous
/// headroom, not a functional restriction.
///
/// `pub(crate)` so `commands::session::search::search_messages` (the raw
/// scan fallback) can share this exact maximum -- that fallback was
/// found to not be sharing it and was still accepting an unbounded
/// caller-supplied `limit`.
pub(crate) const MAX_SEARCH_RESULT_LIMIT: usize = 500;

#[tauri::command]
pub async fn search_archive_fts(
    query: String,
    kinds: Option<Vec<SearchResultKind>>,
    provider_key: Option<String>,
    project_key: Option<String>,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let conn = archive_db::open_connection()?;
    archive_db::search::search_archive(
        &conn,
        &query,
        kinds.as_deref(),
        provider_key.as_deref(),
        project_key.as_deref(),
        limit.clamp(1, MAX_SEARCH_RESULT_LIMIT),
    )
}
