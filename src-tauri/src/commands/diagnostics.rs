//! Tauri commands for Grid Local's local-only pilot diagnostics log,
//! stored at `~/.grid-local/diagnostics.json` -- see
//! `models::DiagnosticsLog`'s own doc comment for the full schema and
//! privacy scope. Deliberately a separate file/state from
//! `commands::metadata`'s `user-data.json`: this is an append-only-counters
//! concern with its own inspect-before-export UI, not a settings concern.

use crate::models::{DiagnosticsEvent, DiagnosticsLog, IndexRunRecord};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

/// Most index/sync runs to retain -- keeps a long-lived install's
/// diagnostics file from growing unbounded.
const MAX_INDEX_RUNS_RETAINED: usize = 50;

/// Application state for the diagnostics log.
pub struct DiagnosticsState {
    pub log: Mutex<Option<DiagnosticsLog>>,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        Self {
            log: Mutex::new(None),
        }
    }
}

fn get_diagnostics_folder() -> Result<PathBuf, String> {
    let home = crate::utils::resolve_home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".grid-local"))
}

pub(crate) fn get_diagnostics_path() -> Result<PathBuf, String> {
    Ok(get_diagnostics_folder()?.join("diagnostics.json"))
}

fn ensure_diagnostics_folder() -> Result<PathBuf, String> {
    let folder = get_diagnostics_folder()?;
    if !folder.exists() {
        fs::create_dir_all(&folder)
            .map_err(|e| format!("Failed to create diagnostics folder: {e}"))?;
    }
    Ok(folder)
}

fn load_diagnostics_from_disk() -> Result<DiagnosticsLog, String> {
    let path = get_diagnostics_path()?;
    if path.exists() {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read diagnostics file: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse diagnostics: {e}"))
    } else {
        Ok(DiagnosticsLog::new())
    }
}

pub(crate) fn save_diagnostics_to_disk(log: &DiagnosticsLog) -> Result<(), String> {
    ensure_diagnostics_folder()?;
    let path = get_diagnostics_path()?;

    let temp_path = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(log)
        .map_err(|e| format!("Failed to serialize diagnostics: {e}"))?;

    let mut file =
        fs::File::create(&temp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("Failed to sync temp file: {e}"))?;

    super::fs_utils::atomic_rename(&temp_path, &path)?;

    Ok(())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Applies one event's mutation to the in-memory log. Pure w.r.t. I/O, so
/// it's unit-tested directly without touching disk.
pub(crate) fn apply_event(log: &mut DiagnosticsLog, event: DiagnosticsEvent) {
    match event {
        DiagnosticsEvent::AppLaunched => {
            if log.installed_at.is_none() {
                log.installed_at = Some(now_iso());
            }
            log.launch_count += 1;
            let today = today_local();
            if !log.active_days.contains(&today) {
                log.active_days.push(today);
            }
        }
        DiagnosticsEvent::SurfaceVisited { surface } => {
            *log.surface_visits.entry(surface).or_insert(0) += 1;
        }
        DiagnosticsEvent::HomePopulated => {
            if log.first_populated_home_at.is_none() {
                log.first_populated_home_at = Some(now_iso());
            }
        }
        DiagnosticsEvent::EvidenceDrilldownOpened => {
            if log.first_evidence_drilldown_at.is_none() {
                log.first_evidence_drilldown_at = Some(now_iso());
            }
        }
        DiagnosticsEvent::SearchExecuted { result_count } => {
            log.search_count += 1;
            if result_count == 0 {
                log.search_zero_result_count += 1;
            }
        }
        DiagnosticsEvent::SearchResultOpened => {
            log.search_result_open_count += 1;
        }
        DiagnosticsEvent::ProblemOpened => {
            log.problem_opens += 1;
        }
        DiagnosticsEvent::AgentRunOpened => {
            log.agent_run_opens += 1;
        }
        DiagnosticsEvent::IndexCompleted {
            duration_ms,
            provider_count,
            session_count,
            parser_failures,
        } => {
            if log.first_index_completed_at.is_none() {
                log.first_index_completed_at = Some(now_iso());
            }
            log.index_runs.push(IndexRunRecord {
                completed_at: now_iso(),
                duration_ms,
                provider_count,
                session_count,
                parser_failures,
            });
            let len = log.index_runs.len();
            if len > MAX_INDEX_RUNS_RETAINED {
                log.index_runs.drain(0..len - MAX_INDEX_RUNS_RETAINED);
            }
        }
        DiagnosticsEvent::Crashed => {
            log.crash_count += 1;
        }
        DiagnosticsEvent::Exported { artifact_type } => {
            *log.export_counts.entry(artifact_type).or_insert(0) += 1;
        }
    }
}

/// Loads the cached log if present, otherwise loads it from disk into the
/// cache first. Split out of `record_diagnostics_event`/
/// `get_diagnostics_snapshot` so neither ever runs blocking file I/O
/// directly on the async runtime thread.
async fn ensure_loaded(state: &State<'_, DiagnosticsState>) -> Result<(), String> {
    let needs_load = {
        let cached = state
            .log
            .lock()
            .map_err(|e| format!("Failed to lock diagnostics: {e}"))?;
        cached.is_none()
    };
    if needs_load {
        let loaded = tauri::async_runtime::spawn_blocking(load_diagnostics_from_disk)
            .await
            .map_err(|e| format!("Task join error: {e}"))??;
        let mut cached = state
            .log
            .lock()
            .map_err(|e| format!("Failed to lock diagnostics: {e}"))?;
        if cached.is_none() {
            *cached = Some(loaded);
        }
    }
    Ok(())
}

/// Records one diagnostics event, mutating and persisting the log.
/// Never surfaces as user-visible failure -- callers should log and
/// continue on error rather than block on a diagnostics write, but this
/// still returns `Result` so a caller CAN observe/log a failure if it
/// wants to.
#[tauri::command]
pub async fn record_diagnostics_event(
    event: DiagnosticsEvent,
    state: State<'_, DiagnosticsState>,
) -> Result<(), String> {
    ensure_loaded(&state).await?;

    let log_to_save = {
        let mut cached = state
            .log
            .lock()
            .map_err(|e| format!("Failed to lock diagnostics: {e}"))?;
        let log = cached.get_or_insert_with(DiagnosticsLog::new);
        apply_event(log, event);
        log.clone()
    };

    tauri::async_runtime::spawn_blocking(move || save_diagnostics_to_disk(&log_to_save))
        .await
        .map_err(|e| format!("Task join error: {e}"))??;

    Ok(())
}

/// Returns the current diagnostics log, for the "inspect before export"
/// dialog -- never writes anything.
#[tauri::command]
pub async fn get_diagnostics_snapshot(
    state: State<'_, DiagnosticsState>,
) -> Result<DiagnosticsLog, String> {
    ensure_loaded(&state).await?;
    let cached = state
        .log
        .lock()
        .map_err(|e| format!("Failed to lock diagnostics: {e}"))?;
    Ok(cached.clone().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_launched_sets_installed_at_once_and_dedupes_active_days() {
        let mut log = DiagnosticsLog::new();
        apply_event(&mut log, DiagnosticsEvent::AppLaunched);
        let first_installed_at = log.installed_at.clone();
        assert!(first_installed_at.is_some());
        assert_eq!(log.launch_count, 1);
        assert_eq!(log.active_days.len(), 1);

        apply_event(&mut log, DiagnosticsEvent::AppLaunched);
        assert_eq!(log.installed_at, first_installed_at);
        assert_eq!(log.launch_count, 2);
        // Same real day -- still exactly one active-day entry.
        assert_eq!(log.active_days.len(), 1);
    }

    #[test]
    fn surface_visited_increments_the_right_key_only() {
        let mut log = DiagnosticsLog::new();
        apply_event(
            &mut log,
            DiagnosticsEvent::SurfaceVisited {
                surface: "home".to_string(),
            },
        );
        apply_event(
            &mut log,
            DiagnosticsEvent::SurfaceVisited {
                surface: "home".to_string(),
            },
        );
        apply_event(
            &mut log,
            DiagnosticsEvent::SurfaceVisited {
                surface: "insights".to_string(),
            },
        );
        assert_eq!(log.surface_visits.get("home"), Some(&2));
        assert_eq!(log.surface_visits.get("insights"), Some(&1));
        assert_eq!(log.surface_visits.get("history"), None);
    }

    #[test]
    fn search_executed_counts_zero_result_searches_separately() {
        let mut log = DiagnosticsLog::new();
        apply_event(&mut log, DiagnosticsEvent::SearchExecuted { result_count: 3 });
        apply_event(&mut log, DiagnosticsEvent::SearchExecuted { result_count: 0 });
        assert_eq!(log.search_count, 2);
        assert_eq!(log.search_zero_result_count, 1);
    }

    #[test]
    fn home_populated_and_evidence_drilldown_are_each_recorded_once() {
        let mut log = DiagnosticsLog::new();
        apply_event(&mut log, DiagnosticsEvent::HomePopulated);
        let first = log.first_populated_home_at.clone();
        apply_event(&mut log, DiagnosticsEvent::HomePopulated);
        assert_eq!(log.first_populated_home_at, first);

        apply_event(&mut log, DiagnosticsEvent::EvidenceDrilldownOpened);
        let first_drilldown = log.first_evidence_drilldown_at.clone();
        apply_event(&mut log, DiagnosticsEvent::EvidenceDrilldownOpened);
        assert_eq!(log.first_evidence_drilldown_at, first_drilldown);
    }

    #[test]
    fn problem_and_agent_run_opens_are_independent_counters() {
        let mut log = DiagnosticsLog::new();
        apply_event(&mut log, DiagnosticsEvent::ProblemOpened);
        apply_event(&mut log, DiagnosticsEvent::ProblemOpened);
        apply_event(&mut log, DiagnosticsEvent::AgentRunOpened);
        assert_eq!(log.problem_opens, 2);
        assert_eq!(log.agent_run_opens, 1);
    }

    #[test]
    fn index_completed_sets_first_completed_at_once_and_appends_a_run_record() {
        let mut log = DiagnosticsLog::new();
        apply_event(
            &mut log,
            DiagnosticsEvent::IndexCompleted {
                duration_ms: 1500,
                provider_count: 3,
                session_count: 42,
                parser_failures: 1,
            },
        );
        let first_completed_at = log.first_index_completed_at.clone();
        assert!(first_completed_at.is_some());
        assert_eq!(log.index_runs.len(), 1);
        assert_eq!(log.index_runs[0].duration_ms, 1500);
        assert_eq!(log.index_runs[0].parser_failures, 1);

        apply_event(
            &mut log,
            DiagnosticsEvent::IndexCompleted {
                duration_ms: 200,
                provider_count: 3,
                session_count: 43,
                parser_failures: 0,
            },
        );
        assert_eq!(log.first_index_completed_at, first_completed_at);
        assert_eq!(log.index_runs.len(), 2);
    }

    #[test]
    fn index_runs_are_capped_at_the_retention_limit() {
        let mut log = DiagnosticsLog::new();
        for i in 0..(MAX_INDEX_RUNS_RETAINED + 5) {
            apply_event(
                &mut log,
                DiagnosticsEvent::IndexCompleted {
                    duration_ms: i as u64,
                    provider_count: 1,
                    session_count: 1,
                    parser_failures: 0,
                },
            );
        }
        assert_eq!(log.index_runs.len(), MAX_INDEX_RUNS_RETAINED);
        // The oldest entries were dropped -- the retained window ends with
        // the most recently recorded run.
        assert_eq!(
            log.index_runs.last().unwrap().duration_ms,
            (MAX_INDEX_RUNS_RETAINED + 4) as u64
        );
    }

    #[test]
    fn crashed_increments_the_crash_count() {
        let mut log = DiagnosticsLog::new();
        apply_event(&mut log, DiagnosticsEvent::Crashed);
        apply_event(&mut log, DiagnosticsEvent::Crashed);
        assert_eq!(log.crash_count, 2);
    }

    #[test]
    fn exported_increments_the_right_artifact_type_only() {
        let mut log = DiagnosticsLog::new();
        apply_event(
            &mut log,
            DiagnosticsEvent::Exported {
                artifact_type: "json".to_string(),
            },
        );
        apply_event(
            &mut log,
            DiagnosticsEvent::Exported {
                artifact_type: "json".to_string(),
            },
        );
        apply_event(
            &mut log,
            DiagnosticsEvent::Exported {
                artifact_type: "diagnostics".to_string(),
            },
        );
        assert_eq!(log.export_counts.get("json"), Some(&2));
        assert_eq!(log.export_counts.get("diagnostics"), Some(&1));
        assert_eq!(log.export_counts.get("markdown"), None);
    }

    #[test]
    fn diagnostics_log_never_contains_content_shaped_fields() {
        // Structural guard against scope creep: assert the type's own
        // field names contain none of the excluded categories the spec
        // names (conversation content/prompts/commands/code/filenames/raw
        // paths). This won't catch a future field that's misused, but it
        // does catch an obviously-named violation being added.
        let json = serde_json::to_value(DiagnosticsLog::new()).unwrap();
        let keys: Vec<String> = json.as_object().unwrap().keys().cloned().collect();
        for forbidden in ["prompt", "command", "code", "filename", "path", "content"] {
            assert!(
                !keys.iter().any(|k| k.to_lowercase().contains(forbidden)),
                "DiagnosticsLog must never gain a field shaped like {forbidden:?}"
            );
        }
    }
}
