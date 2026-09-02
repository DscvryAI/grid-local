//! Backend for the History surface (spec §13).
//!
//! Merges sessions from every active provider into one date-sortable
//! list: Claude via `archive_db` (fast, indexed), every other provider via
//! a single-scan-per-provider dispatch mirroring
//! `commands::stats::collect_provider_global_file_stats` (bespoke
//! single-scan branches for Codex/Cursor/Antigravity, the generic
//! `scan_stats_projects`+`load_stats_sessions` per-project loop for the
//! rest) -- deliberately reusing that exact provider registry
//! (`commands::stats::StatsProvider` et al, `pub(crate)`) rather than
//! maintaining a second one that could silently drift out of sync.
//!
//! Filtering/sorting/pagination: the full filtered, merged list is built
//! server-side on every call (non-Claude sessions cached per-provider via
//! [`cache::provider_history_sessions_cache`]), then sliced in memory for
//! the requested page -- not true cross-source keyset pagination (unsound
//! here: Claude's rows are one SQL-sorted stream, non-Claude rows are N
//! independently-sorted Vecs, and the split point shifts with every
//! filter), and not client-side filtering over one fetched page (unsound
//! given the pagination contract: `total_count`/`has_more` only mean
//! anything if filters apply before slicing). Sound at "thousands of
//! sessions" for one local user's full history; documented not to be at
//! "tens of thousands."

mod cache;

use crate::commands::multi_provider::CustomClaudePathParam;
use crate::commands::stats::{
    load_stats_sessions, parse_active_stats_providers, scan_stats_projects, stats_provider_id,
    StatsProvider,
};
use crate::models::{
    HistoryFilterParams, HistoryProjectFacet, HistoryProviderFacet, HistorySessionItem,
    HistorySessionsPage,
};
use rayon::prelude::*;
use std::collections::HashSet;

#[tauri::command]
pub async fn list_history_sessions(
    active_providers: Option<Vec<String>>,
    custom_claude_paths: Option<Vec<CustomClaudePathParam>>,
    filters: Option<HistoryFilterParams>,
    offset: usize,
    limit: usize,
) -> Result<HistorySessionsPage, String> {
    let providers_to_include = parse_active_stats_providers(active_providers);
    let filters = filters.unwrap_or_default();
    let limit = limit.clamp(1, 200);

    let mut items: Vec<HistorySessionItem> = Vec::new();
    let mut custom_claude_dirs_omitted = false;

    if providers_to_include.contains(&StatsProvider::Claude) {
        custom_claude_dirs_omitted = custom_claude_paths.as_ref().is_some_and(|p| !p.is_empty());
        items.extend(claude_history_items(
            filters.start_date.as_deref(),
            filters.end_date.as_deref(),
        )?);
    }

    let non_claude_providers: Vec<StatsProvider> = providers_to_include
        .into_iter()
        .filter(|p| *p != StatsProvider::Claude)
        .collect();

    let provider_results: Vec<Vec<HistorySessionItem>> = non_claude_providers
        .par_iter()
        .map(|&provider| collect_provider_history_items(provider))
        .collect();
    for provider_items in provider_results {
        items.extend(provider_items);
    }

    // Non-Claude providers weren't date-filtered at the source -- apply it
    // here so every provider is filtered identically regardless of where
    // its data came from. (Claude's own query already pushed this into
    // SQL; re-checking it here for Claude rows too is a cheap no-op, not
    // worth special-casing around.)
    if filters.start_date.is_some() || filters.end_date.is_some() {
        items.retain(|item| {
            within_date_range(
                &item.recency_time,
                filters.start_date.as_deref(),
                filters.end_date.as_deref(),
            )
        });
    }

    if let Some(project_keys) = &filters.project_keys {
        let allowed: HashSet<&str> = project_keys.iter().map(String::as_str).collect();
        items.retain(|item| allowed.contains(combined_project_key(item).as_str()));
    }
    if let Some(provider_ids) = &filters.provider_ids {
        let allowed: HashSet<&str> = provider_ids.iter().map(String::as_str).collect();
        items.retain(|item| allowed.contains(item.provider_id.as_str()));
    }
    if let Some(models) = &filters.models {
        let allowed: HashSet<&str> = models.iter().map(String::as_str).collect();
        items.retain(|item| allowed.contains(model_label(item)));
    }

    // Recency descending only -- spec §13 asks for date-grouped browsing,
    // not arbitrary sorting.
    items.sort_by(|a, b| b.recency_time.cmp(&a.recency_time));

    let (available_projects, available_providers, available_models) = compute_facets(&items);

    let total_count = items.len();
    let has_more = offset + limit < total_count;
    let page_items = items.into_iter().skip(offset).take(limit).collect();

    Ok(HistorySessionsPage {
        items: page_items,
        total_count,
        has_more,
        available_projects,
        available_providers,
        available_models,
        custom_claude_dirs_omitted,
    })
}

/// `provider_id:project_key` -- the collision-safe combined key both the
/// Project filter and `available_projects` facets key by (two different
/// providers may otherwise reuse the same raw path/workspace-id string).
fn combined_project_key(item: &HistorySessionItem) -> String {
    format!("{}:{}", item.provider_id, item.project_key)
}

fn model_label(item: &HistorySessionItem) -> &str {
    item.model.as_deref().unwrap_or("Unknown")
}

fn within_date_range(recency_time: &str, start: Option<&str>, end: Option<&str>) -> bool {
    if let Some(start) = start {
        if recency_time < start {
            return false;
        }
    }
    if let Some(end) = end {
        if recency_time > end {
            return false;
        }
    }
    true
}

fn claude_history_items(
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<Vec<HistorySessionItem>, String> {
    let conn = crate::archive_db::open_connection()?;
    crate::archive_db::history::query_claude_history_sessions(&conn, start_date, end_date)
}

/// Dispatches one provider's session listing without re-walking its store
/// once per project -- the bug this session's Codex investigation found
/// and fixed for stats, applied identically here. Cached per-provider
/// ([`cache::provider_history_sessions_cache`]) since this is called on
/// every `list_history_sessions` request.
fn collect_provider_history_items(provider: StatsProvider) -> Vec<HistorySessionItem> {
    let cached = cache::provider_history_sessions_cache().get_or_build(provider, || {
        Ok(scan_provider_sessions(provider))
    });

    let Ok(sessions) = cached else {
        return Vec::new();
    };

    sessions
        .iter()
        .map(|(project_key, session, model)| {
            to_history_item(provider, project_key, session, model.clone())
        })
        .collect()
}

fn scan_provider_sessions(
    provider: StatsProvider,
) -> Vec<(String, crate::models::ClaudeSession, Option<String>)> {
    match provider {
        // No real per-project directory split -- must scan the whole
        // store exactly once, never once per project (see this module's
        // doc and providers::codex::scan_all_session_info's doc for why).
        StatsProvider::Codex => crate::providers::codex::collect_global_history_sessions(),
        StatsProvider::Cursor => crate::providers::cursor::collect_global_history_sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|(project_key, session)| (project_key, session, None))
            .collect(),
        StatsProvider::Antigravity => {
            let Ok(root) = crate::commands::antigravity::resolve_antigravity_root()
                .ok_or_else(|| "Cannot determine antigravity root directory".to_string())
            else {
                return Vec::new();
            };
            crate::providers::antigravity::load_sessions(&root.to_string_lossy(), false)
                .unwrap_or_default()
                .into_iter()
                .map(|session| ("Antigravity".to_string(), session, None))
                .collect()
        }
        // Every other provider has a real per-project directory boundary
        // -- the existing scan_stats_projects/load_stats_sessions loop is
        // already proven not to have the rescan bug for these.
        _ => {
            let projects = scan_stats_projects(provider).unwrap_or_default();
            let mut sessions = Vec::new();
            for project in projects {
                let project_sessions =
                    load_stats_sessions(provider, &project.path).unwrap_or_default();
                for session in project_sessions {
                    sessions.push((project.path.clone(), session, None));
                }
            }
            sessions
        }
    }
}

fn to_history_item(
    provider: StatsProvider,
    project_key: &str,
    session: &crate::models::ClaudeSession,
    model: Option<String>,
) -> HistorySessionItem {
    let recency_time = if session.last_message_time.is_empty() {
        session.last_modified.clone()
    } else {
        session.last_message_time.clone()
    };
    HistorySessionItem {
        session_id: session.session_id.clone(),
        actual_session_id: session.actual_session_id.clone(),
        provider_id: stats_provider_id(provider).to_string(),
        project_key: project_key.to_string(),
        project_name: session.project_name.clone(),
        file_path: session.file_path.clone(),
        recency_time,
        first_message_time: (!session.first_message_time.is_empty())
            .then(|| session.first_message_time.clone()),
        last_message_time: (!session.last_message_time.is_empty())
            .then(|| session.last_message_time.clone()),
        message_count: session.message_count,
        has_tool_use: session.has_tool_use,
        has_errors: session.has_errors,
        summary: session.summary.clone(),
        model,
    }
}

fn compute_facets(
    items: &[HistorySessionItem],
) -> (
    Vec<HistoryProjectFacet>,
    Vec<HistoryProviderFacet>,
    Vec<String>,
) {
    use std::collections::HashMap;

    let mut projects: HashMap<String, HistoryProjectFacet> = HashMap::new();
    let mut providers: HashMap<String, HistoryProviderFacet> = HashMap::new();
    let mut models: HashSet<String> = HashSet::new();

    for item in items {
        let key = combined_project_key(item);
        projects
            .entry(key)
            .or_insert_with(|| HistoryProjectFacet {
                provider_id: item.provider_id.clone(),
                project_key: item.project_key.clone(),
                project_name: item.project_name.clone(),
                session_count: 0,
            })
            .session_count += 1;

        providers
            .entry(item.provider_id.clone())
            .or_insert_with(|| HistoryProviderFacet {
                provider_id: item.provider_id.clone(),
                display_name: item.provider_id.clone(),
                session_count: 0,
            })
            .session_count += 1;

        models.insert(model_label(item).to_string());
    }

    let mut available_projects: Vec<_> = projects.into_values().collect();
    available_projects.sort_by(|a, b| b.session_count.cmp(&a.session_count));

    let mut available_providers: Vec<_> = providers.into_values().collect();
    available_providers.sort_by(|a, b| b.session_count.cmp(&a.session_count));

    let mut available_models: Vec<_> = models.into_iter().collect();
    available_models.sort();

    (available_projects, available_providers, available_models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ClaudeSession;

    fn fixture_session(session_id: &str, last_message_time: &str) -> ClaudeSession {
        ClaudeSession {
            session_id: session_id.to_string(),
            actual_session_id: session_id.to_string(),
            file_path: format!("/fixtures/{session_id}.jsonl"),
            project_name: "demo-project".to_string(),
            message_count: 3,
            first_message_time: "2026-01-01T00:00:00Z".to_string(),
            last_message_time: last_message_time.to_string(),
            last_modified: last_message_time.to_string(),
            has_tool_use: false,
            has_errors: false,
            summary: Some("Fixture session".to_string()),
            is_renamed: false,
            provider: Some("codex".to_string()),
            storage_type: None,
            entrypoint: None,
        }
    }

    #[test]
    fn to_history_item_falls_back_to_last_modified_when_last_message_time_is_empty() {
        let mut session = fixture_session("s1", "");
        session.last_modified = "2026-02-02T00:00:00Z".to_string();

        let item = to_history_item(StatsProvider::Codex, "codex://cwd", &session, None);

        assert_eq!(item.recency_time, "2026-02-02T00:00:00Z");
        assert_eq!(item.last_message_time, None, "empty string becomes None");
        assert_eq!(item.provider_id, "codex");
        assert_eq!(item.project_key, "codex://cwd");
    }

    #[test]
    fn to_history_item_prefers_last_message_time_when_present() {
        let session = fixture_session("s1", "2026-03-03T00:00:00Z");

        let item = to_history_item(StatsProvider::Codex, "codex://cwd", &session, None);

        assert_eq!(item.recency_time, "2026-03-03T00:00:00Z");
        assert_eq!(item.last_message_time.as_deref(), Some("2026-03-03T00:00:00Z"));
    }

    #[test]
    fn combined_project_key_is_collision_safe_across_providers() {
        let codex_session = fixture_session("s1", "2026-01-01T00:00:00Z");
        let cursor_session = fixture_session("s2", "2026-01-01T00:00:00Z");

        let codex_item = to_history_item(StatsProvider::Codex, "/same/path", &codex_session, None);
        let cursor_item =
            to_history_item(StatsProvider::Cursor, "/same/path", &cursor_session, None);

        assert_ne!(
            combined_project_key(&codex_item),
            combined_project_key(&cursor_item),
            "two providers reusing the same raw project_key must not collide"
        );
    }

    #[test]
    fn model_label_defaults_to_unknown() {
        let session = fixture_session("s1", "2026-01-01T00:00:00Z");
        let with_model = to_history_item(
            StatsProvider::Codex,
            "codex://cwd",
            &session,
            Some("gpt-5".to_string()),
        );
        let without_model = to_history_item(StatsProvider::Codex, "codex://cwd", &session, None);

        assert_eq!(model_label(&with_model), "gpt-5");
        assert_eq!(model_label(&without_model), "Unknown");
    }

    #[test]
    fn within_date_range_is_inclusive_on_both_bounds() {
        assert!(within_date_range(
            "2026-06-15T00:00:00Z",
            Some("2026-06-01T00:00:00Z"),
            Some("2026-06-30T00:00:00Z")
        ));
        assert!(within_date_range(
            "2026-06-01T00:00:00Z",
            Some("2026-06-01T00:00:00Z"),
            None
        ));
        assert!(!within_date_range(
            "2026-05-31T23:59:59Z",
            Some("2026-06-01T00:00:00Z"),
            None
        ));
        assert!(!within_date_range(
            "2026-07-01T00:00:00Z",
            None,
            Some("2026-06-30T23:59:59Z")
        ));
    }

    #[test]
    fn compute_facets_groups_by_combined_key_and_collects_sorted_models() {
        let session_a = fixture_session("a", "2026-01-01T00:00:00Z");
        let session_b = fixture_session("b", "2026-01-02T00:00:00Z");
        let items = vec![
            to_history_item(StatsProvider::Codex, "cwd-1", &session_a, Some("gpt-5".to_string())),
            to_history_item(StatsProvider::Codex, "cwd-1", &session_b, None),
        ];

        let (projects, providers, models) = compute_facets(&items);

        assert_eq!(projects.len(), 1, "same provider+project_key merges into one facet");
        assert_eq!(projects[0].session_count, 2);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].session_count, 2);
        assert_eq!(models, vec!["Unknown".to_string(), "gpt-5".to_string()]);
    }
}
