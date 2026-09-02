//! Data model for Grid Local's local-only pilot diagnostics log.
//! Deliberately excludes all conversation content, prompts, commands,
//! code, filenames, and raw paths, keeping only a minimal set of
//! counters needed to diagnose issues without exposing sensitive
//! content. Stored separately from `UserMetadata` (`user-data.json`) at
//! `~/.grid-local/diagnostics.json` -- a distinct, purely-additive-counters
//! concern with its own inspect-before-export UI, not a
//! settings/preference concern.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

fn default_version() -> u32 {
    DIAGNOSTICS_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsLog {
    #[serde(default = "default_version")]
    pub version: u32,
    pub installed_at: Option<String>,
    pub first_index_completed_at: Option<String>,
    pub first_populated_home_at: Option<String>,
    pub first_evidence_drilldown_at: Option<String>,
    #[serde(default)]
    pub launch_count: u64,
    /// Distinct `YYYY-MM-DD` (local time) days the app was launched on.
    #[serde(default)]
    pub active_days: Vec<String>,
    #[serde(default)]
    pub search_count: u64,
    #[serde(default)]
    pub search_zero_result_count: u64,
    #[serde(default)]
    pub search_result_open_count: u64,
    /// Keyed by primary surface: "home" / "history" / "search" / "insights".
    #[serde(default)]
    pub surface_visits: HashMap<String, u64>,
    #[serde(default)]
    pub problem_opens: u64,
    #[serde(default)]
    pub agent_run_opens: u64,
    #[serde(default)]
    pub index_runs: Vec<IndexRunRecord>,
    #[serde(default)]
    pub crash_count: u64,
    /// Keyed by artifact type: "markdown" / "json" / "html" / "diagnostics".
    #[serde(default)]
    pub export_counts: HashMap<String, u64>,
}

impl DiagnosticsLog {
    pub fn new() -> Self {
        Self {
            version: DIAGNOSTICS_SCHEMA_VERSION,
            installed_at: None,
            first_index_completed_at: None,
            first_populated_home_at: None,
            first_evidence_drilldown_at: None,
            launch_count: 0,
            active_days: Vec::new(),
            search_count: 0,
            search_zero_result_count: 0,
            search_result_open_count: 0,
            surface_visits: HashMap::new(),
            problem_opens: 0,
            agent_run_opens: 0,
            index_runs: Vec::new(),
            crash_count: 0,
            export_counts: HashMap::new(),
        }
    }
}

impl Default for DiagnosticsLog {
    fn default() -> Self {
        Self::new()
    }
}

/// One completed index/sync run. Capped to the most recent 50 entries
/// (`apply_event`) so a long-lived install's diagnostics file can't grow
/// unbounded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRunRecord {
    pub completed_at: String,
    pub duration_ms: u64,
    pub provider_count: u64,
    pub session_count: u64,
    pub parser_failures: u64,
}

/// Every distinct diagnostics event a frontend action can record. A single
/// tagged enum (not N near-identical commands) so the "how to mutate the
/// log" logic lives in exactly one place (`apply_event`,
/// `commands::diagnostics`) rather than scattered per-command bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DiagnosticsEvent {
    AppLaunched,
    SurfaceVisited { surface: String },
    HomePopulated,
    EvidenceDrilldownOpened,
    SearchExecuted { result_count: u64 },
    SearchResultOpened,
    ProblemOpened,
    AgentRunOpened,
    IndexCompleted {
        duration_ms: u64,
        provider_count: u64,
        session_count: u64,
        parser_failures: u64,
    },
    Crashed,
    Exported { artifact_type: String },
}

#[cfg(test)]
mod serde_shape_tests {
    // Real bug caught by live end-to-end testing:
    // `#[serde(tag = "kind", rename_all = "camelCase")]` on an
    // enum ONLY renames variant tag values, not the fields WITHIN each
    // struct-like variant -- a genuinely easy trap, since it's exactly what
    // the equivalent attribute on a plain struct does do. The frontend
    // (camelCase field names, matching every other DTO in this codebase)
    // silently failed to deserialize on the Rust side with "missing field
    // `result_count`" until `rename_all_fields = "camelCase"` was added
    // alongside it. These tests exercise every multi-field variant's real
    // wire shape so this can't silently regress if a future edit forgets
    // the same attribute pairing.
    use super::*;

    #[test]
    fn search_executed_deserializes_from_camel_case_json() {
        let json = r#"{"kind":"searchExecuted","resultCount":5}"#;
        let event: DiagnosticsEvent = serde_json::from_str(json).expect("should deserialize");
        match event {
            DiagnosticsEvent::SearchExecuted { result_count } => assert_eq!(result_count, 5),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn surface_visited_deserializes_from_camel_case_json() {
        let json = r#"{"kind":"surfaceVisited","surface":"home"}"#;
        let event: DiagnosticsEvent = serde_json::from_str(json).expect("should deserialize");
        match event {
            DiagnosticsEvent::SurfaceVisited { surface } => assert_eq!(surface, "home"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn exported_deserializes_from_camel_case_json() {
        let json = r#"{"kind":"exported","artifactType":"json"}"#;
        let event: DiagnosticsEvent = serde_json::from_str(json).expect("should deserialize");
        match event {
            DiagnosticsEvent::Exported { artifact_type } => assert_eq!(artifact_type, "json"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn index_completed_deserializes_every_camel_case_field() {
        let json = r#"{"kind":"indexCompleted","durationMs":1500,"providerCount":3,"sessionCount":42,"parserFailures":1}"#;
        let event: DiagnosticsEvent = serde_json::from_str(json).expect("should deserialize");
        match event {
            DiagnosticsEvent::IndexCompleted {
                duration_ms,
                provider_count,
                session_count,
                parser_failures,
            } => {
                assert_eq!(duration_ms, 1500);
                assert_eq!(provider_count, 3);
                assert_eq!(session_count, 42);
                assert_eq!(parser_failures, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn unit_variants_deserialize_from_the_bare_kind_tag() {
        let app_launched: DiagnosticsEvent =
            serde_json::from_str(r#"{"kind":"appLaunched"}"#).expect("should deserialize");
        assert!(matches!(app_launched, DiagnosticsEvent::AppLaunched));

        let crashed: DiagnosticsEvent =
            serde_json::from_str(r#"{"kind":"crashed"}"#).expect("should deserialize");
        assert!(matches!(crashed, DiagnosticsEvent::Crashed));
    }

    #[test]
    fn diagnostics_log_serializes_every_field_as_camel_case() {
        // Guards the OTHER half of the same wire contract: DiagnosticsLog
        // (a plain struct, where `rename_all` alone IS sufficient) must
        // still actually produce camelCase keys the frontend's
        // DiagnosticsLog TS interface expects.
        let json = serde_json::to_value(DiagnosticsLog::new()).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("installedAt"));
        assert!(obj.contains_key("firstIndexCompletedAt"));
        assert!(obj.contains_key("launchCount"));
        assert!(obj.contains_key("activeDays"));
        assert!(obj.contains_key("searchZeroResultCount"));
        assert!(obj.contains_key("problemOpens"));
        assert!(!obj.contains_key("installed_at"), "must not leak snake_case keys");
    }
}
