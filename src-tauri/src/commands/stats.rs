#[cfg(test)]
use crate::models::MessageContent;
use crate::models::{
    ActivityHeatmap, ClaudeMessage, DailyStats, GlobalStatsSummary, ModelContextStats, ModelStats,
    ProjectRanking, ProjectStatsSummary, ProviderUsageStats, RawLogEntry, SessionComparison,
    SessionTokenStats, TokenDistribution, TokenUsage, ToolUsageStats,
};
use crate::providers;
use crate::utils::find_line_ranges;
use chrono::{DateTime, Datelike, Timelike, Utc};
use memmap2::Mmap;
use rayon::prelude::*;
use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

mod cache;
mod provider_registry;
pub(crate) use provider_registry::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatsMode {
    BillingTotal,
    ConversationOnly,
}

impl StatsMode {
    /// Return whether the current stats mode includes sidechain messages.
    fn include_sidechain(self) -> bool {
        matches!(self, Self::BillingTotal)
    }
}

/// Parse the requested stats mode, defaulting to billing totals.
fn parse_stats_mode(stats_mode: Option<String>) -> StatsMode {
    match stats_mode.as_deref() {
        Some("conversation_only") => StatsMode::ConversationOnly,
        Some("billing_total") | None => StatsMode::BillingTotal,
        Some(raw) => {
            log::warn!("Unknown stats_mode '{raw}', defaulting to 'billing_total'");
            StatsMode::BillingTotal
        }
    }
}

/// Return whether a message type is always counted in stats.
fn is_core_message_type(message_type: &str) -> bool {
    matches!(message_type, "user" | "assistant" | "system")
}

/// Return whether a message type represents a conversation turn.
fn is_conversation_message_type(message_type: &str) -> bool {
    matches!(message_type, "user" | "assistant")
}

/// Return whether a message type is non-conversational noise.
fn is_non_message_noise_type(message_type: &str) -> bool {
    matches!(
        message_type,
        "progress" | "queue-operation" | "file-history-snapshot"
    )
}

/// Return whether token usage contains any populated token counters.
fn token_usage_has_token_fields(usage: &TokenUsage) -> bool {
    usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.cache_creation_input_tokens.is_some()
        || usage.cache_creation_input_tokens_5m.is_some()
        || usage.cache_creation_input_tokens_1h.is_some()
        || usage.cache_creation.is_some()
        || usage.cache_read_input_tokens.is_some()
        || usage.reasoning_tokens.is_some()
}

/// Normalize nested cache-write usage into the flat fields used by the
/// dashboard while retaining the provider's 5-minute/1-hour split.
pub(crate) fn normalize_token_usage(mut usage: TokenUsage) -> TokenUsage {
    if let Some(cache_creation) = usage.cache_creation.take() {
        let five_minute = cache_creation.ephemeral_5m_input_tokens;
        let one_hour = cache_creation.ephemeral_1h_input_tokens;
        if usage.cache_creation_input_tokens_5m.is_none() {
            usage.cache_creation_input_tokens_5m = five_minute;
        }
        if usage.cache_creation_input_tokens_1h.is_none() {
            usage.cache_creation_input_tokens_1h = one_hour;
        }
        if usage.cache_creation_input_tokens.is_none() {
            usage.cache_creation_input_tokens = Some(
                five_minute
                    .unwrap_or(0)
                    .saturating_add(one_hour.unwrap_or(0)),
            );
        }
    }
    usage
}

/// Summarize token usage into input, output, cache, and total counts.
fn token_usage_totals(usage: &TokenUsage) -> (u64, u64, u64, u64, u64, u64) {
    let input_tokens = u64::from(usage.input_tokens.unwrap_or(0));
    let output_tokens = u64::from(usage.output_tokens.unwrap_or(0));
    let cache_creation_tokens = u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
    let cache_read_tokens = u64::from(usage.cache_read_input_tokens.unwrap_or(0));
    let reasoning_tokens = u64::from(usage.reasoning_tokens.unwrap_or(0));
    let total_tokens =
        input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens + reasoning_tokens;
    (
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        reasoning_tokens,
        total_tokens,
    )
}

#[derive(Debug, Clone)]
struct AntigravityUsageRecord {
    timestamp: DateTime<Utc>,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    conversation_input_tokens: u64,
    conversation_cache_creation_tokens: u64,
    conversation_cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
}

fn scale_token_count(value: u64, numerator: u64, denominator: u64) -> u64 {
    if value == 0 || numerator == 0 || denominator == 0 {
        return 0;
    }

    let scaled = (u128::from(value) * u128::from(numerator)) + (u128::from(denominator) / 2);
    (scaled / u128::from(denominator)) as u64
}

fn antigravity_chat_token_breakdown(value: &serde_json::Value) -> Option<(u64, u64)> {
    let token_breakdown =
        &value["raw"]["chatModel"]["chatStartMetadata"]["contextWindowMetadata"]["tokenBreakdown"];
    let total_tokens = token_breakdown["totalTokens"].as_u64().or_else(|| {
        value["raw"]["chatModel"]["chatStartMetadata"]["contextWindowMetadata"]
            ["estimatedTokensUsed"]
            .as_u64()
    })?;

    if total_tokens == 0 {
        return None;
    }

    // When the `groups` array is missing entirely (e.g. estimatedTokensUsed
    // was provided without a breakdown), return None so the caller falls
    // back to the full input/cache totals rather than scaling everything
    // to zero. An explicit empty `groups` array, or one without any
    // TOKEN_TYPE_CHAT_MESSAGES entries, is still a legitimate "0 chat
    // tokens" result and keeps the existing behavior.
    let groups = token_breakdown["groups"].as_array()?;
    let chat_tokens = groups
        .iter()
        .filter(|group| group["type"].as_str() == Some("TOKEN_TYPE_CHAT_MESSAGES"))
        .map(|group| group["numTokens"].as_u64().unwrap_or(0))
        .sum::<u64>()
        .min(total_tokens);

    Some((chat_tokens, total_tokens))
}

/// Return whether a message should be counted for the active stats mode.
fn should_include_stats_entry(
    message_type: &str,
    is_sidechain: Option<bool>,
    has_usage: bool,
    mode: StatsMode,
) -> bool {
    if message_type == "summary" {
        return false;
    }

    if !mode.include_sidechain() && is_sidechain.unwrap_or(false) {
        return false;
    }

    if matches!(mode, StatsMode::ConversationOnly) {
        return is_conversation_message_type(message_type);
    }

    if is_core_message_type(message_type) {
        return true;
    }

    if is_non_message_noise_type(message_type) {
        return has_usage;
    }

    has_usage
}

fn is_synthetic_antigravity_prompt(message: &ClaudeMessage) -> bool {
    message.provider.as_deref() == Some("antigravity")
        && message.message_type == "user"
        && message.usage.is_none()
}

fn should_include_stats_message(message: &ClaudeMessage, mode: StatsMode) -> bool {
    if is_synthetic_antigravity_prompt(message) {
        return false;
    }

    let usage = extract_token_usage(message);
    let has_usage = token_usage_has_token_fields(&usage);
    should_include_stats_entry(&message.message_type, message.is_sidechain, has_usage, mode)
}


/// Parse a line using simd-json (requires mutable slice)
/// Returns None if parsing fails
#[inline]
/// Parse a raw log entry with simd-json.
fn parse_raw_log_entry_simd(line: &mut [u8]) -> Option<RawLogEntry> {
    simd_json::serde::from_slice(line).ok()
}

// ---------------------------------------------------------------------------
// Lightweight struct for global stats: only the fields we actually need.
// Skips expensive fields like snapshot, data, hook_infos, etc.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GlobalStatsLogEntry {
    #[serde(rename = "type")]
    message_type: String,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    /// Row identifier — fallback dedup key when `message.id` is absent (#283).
    uuid: Option<String>,
    message: Option<GlobalStatsMessageContent>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    #[serde(rename = "toolUse")]
    tool_use: Option<GlobalStatsToolUse>,
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<GlobalStatsToolUseResult>,
}

#[derive(Debug, Deserialize)]
struct GlobalStatsMessageContent {
    #[allow(dead_code)]
    role: String,
    /// Assistant turn identifier — primary dedup key (#283).
    /// Multiple JSONL rows belonging to one turn share this id.
    id: Option<String>,
    content: Option<serde_json::Value>,
    model: Option<String>,
    usage: Option<TokenUsage>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct GlobalStatsToolUse {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GlobalStatsToolUseResult {
    is_error: Option<bool>,
    usage: Option<serde_json::Value>,
    #[serde(rename = "totalTokens")]
    total_tokens: Option<u64>,
}

#[inline]
/// Parse a lightweight global-stats entry with simd-json.
fn parse_global_stats_entry_simd(line: &mut [u8]) -> Option<GlobalStatsLogEntry> {
    simd_json::serde::from_slice(line).ok()
}

/// Read the first numeric token field that a provider exposes under one of
/// its known `snake_case`/`camelCase` aliases.
fn usage_u32(value: &serde_json::Value, keys: &[&str]) -> Option<u32> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_u64))
        .and_then(|value| u32::try_from(value).ok())
}

/// Apply token usage fields from a JSON value into a token-usage struct.
fn apply_usage_fields_from_value(usage_obj: &serde_json::Value, usage: &mut TokenUsage) {
    usage.input_tokens =
        usage_u32(usage_obj, &["input_tokens", "inputTokens"]).or(usage.input_tokens);
    usage.output_tokens =
        usage_u32(usage_obj, &["output_tokens", "outputTokens"]).or(usage.output_tokens);
    usage.cache_creation_input_tokens = usage_u32(
        usage_obj,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cacheWrite",
        ],
    )
    .or(usage.cache_creation_input_tokens);
    if let Some(cache_creation) = usage_obj.get("cache_creation") {
        usage.cache_creation_input_tokens_5m = usage_u32(
            cache_creation,
            &["ephemeral_5m_input_tokens", "ephemeral5mInputTokens"],
        )
        .or(usage.cache_creation_input_tokens_5m);
        usage.cache_creation_input_tokens_1h = usage_u32(
            cache_creation,
            &["ephemeral_1h_input_tokens", "ephemeral1hInputTokens"],
        )
        .or(usage.cache_creation_input_tokens_1h);
        if usage.cache_creation_input_tokens.is_none() {
            usage.cache_creation_input_tokens = Some(
                usage
                    .cache_creation_input_tokens_5m
                    .unwrap_or(0)
                    .saturating_add(usage.cache_creation_input_tokens_1h.unwrap_or(0)),
            );
        }
    }
    usage.cache_read_input_tokens = usage_u32(
        usage_obj,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cacheRead",
        ],
    )
    .or(usage.cache_read_input_tokens);
    usage.reasoning_tokens = usage_u32(
        usage_obj,
        &[
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning",
            "thoughtsTokenCount",
        ],
    )
    .or(usage.reasoning_tokens);
    if let Some(tier) = usage_obj
        .get("service_tier")
        .or_else(|| usage_obj.get("serviceTier"))
        .and_then(serde_json::Value::as_str)
    {
        usage.service_tier = Some(tier.to_string());
    }
}

/// Extract token usage from the lightweight global stats entry
fn extract_token_usage_from_global_entry(entry: &GlobalStatsLogEntry) -> TokenUsage {
    // 1. From message.usage (most common for assistant messages)
    if let Some(msg) = &entry.message {
        if let Some(usage) = &msg.usage {
            return normalize_token_usage(usage.clone());
        }

        if let Some(content) = &msg.content {
            if content.is_object() && content.get("usage").is_some() {
                let mut usage = TokenUsage {
                    input_tokens: None,
                    output_tokens: None,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_tokens: None,
                    service_tier: None,
                    ..Default::default()
                };
                if let Some(usage_obj) = content.get("usage") {
                    apply_usage_fields_from_value(usage_obj, &mut usage);
                    if token_usage_has_token_fields(&usage) {
                        return usage;
                    }
                }
            }
        }
    }

    let mut usage = TokenUsage {
        input_tokens: None,
        output_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        reasoning_tokens: None,
        service_tier: None,
        ..Default::default()
    };

    // 2. From tool_use_result.usage
    if let Some(tur) = &entry.tool_use_result {
        if let Some(usage_obj) = &tur.usage {
            apply_usage_fields_from_value(usage_obj, &mut usage);
        }

        // 3. From tool_use_result.totalTokens fallback
        if usage.input_tokens.is_none() && usage.output_tokens.is_none() {
            if let Some(total) = tur.total_tokens {
                if entry.message_type == "assistant" {
                    usage.output_tokens = Some(total as u32);
                } else {
                    usage.input_tokens = Some(total as u32);
                }
            }
        }
    }

    normalize_token_usage(usage)
}

/// Track tool usage from the lightweight global stats entry
fn track_tool_usage_from_global_entry(
    entry: &GlobalStatsLogEntry,
    tool_usage: &mut HashMap<String, (u32, u32)>,
) {
    // From assistant content array
    if entry.message_type == "assistant" {
        if let Some(msg) = &entry.message {
            if let Some(content) = &msg.content {
                if let Some(arr) = content.as_array() {
                    for item in arr {
                        if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                let e = tool_usage.entry(name.to_string()).or_insert((0, 0));
                                e.0 += 1;
                                let is_error = item
                                    .get("is_error")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                if !is_error {
                                    e.1 += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // From explicit tool_use field
    if let Some(tu) = &entry.tool_use {
        if let Some(name) = &tu.name {
            let e = tool_usage.entry(name.clone()).or_insert((0, 0));
            e.0 += 1;
            if let Some(tur) = &entry.tool_use_result {
                let is_error = tur.is_error.unwrap_or(false);
                if !is_error {
                    e.1 += 1;
                }
            }
        }
    }
}

/// Intermediate stats collected from a single session file (for parallel processing)
type ModelUsageAggregate = (u32, u64, u64, u64, u64, u64, u64);
type ModelContextUsageMap = HashMap<String, HashMap<u64, ModelContextStats>>;
const UNKNOWN_MODEL_NAME: &str = "unknown";
const MODEL_USAGE_KEY_SEPARATOR: char = '\u{1f}';

fn normalize_service_tier(service_tier: Option<&str>) -> Option<String> {
    service_tier
        .map(str::trim)
        .filter(|tier| !tier.is_empty())
        .map(|tier| match tier.to_ascii_lowercase().as_str() {
            // OpenAI renamed Priority to Fast while keeping the old API value
            // accepted for compatibility. Normalize both to one display row.
            "priority" => "fast".to_string(),
            normalized => normalized.to_string(),
        })
}

fn model_usage_key(model_name: &str, service_tier: Option<&str>) -> String {
    let model_name = model_name.to_string();
    match normalize_service_tier(service_tier) {
        Some(service_tier) => format!("{model_name}{MODEL_USAGE_KEY_SEPARATOR}{service_tier}"),
        None => model_name,
    }
}

fn split_model_usage_key(key: &str) -> (&str, Option<&str>) {
    key.split_once(MODEL_USAGE_KEY_SEPARATOR)
        .map_or((key, None), |(model_name, service_tier)| {
            (model_name, Some(service_tier))
        })
}

fn context_tier_min_tokens(model_name: &str, context_tokens: u64) -> u64 {
    let normalized = model_name.trim().to_ascii_lowercase();
    let model = normalized
        .strip_prefix("models/")
        .unwrap_or(&normalized)
        .rsplit('/')
        .next()
        .unwrap_or(&normalized);
    let matches_model = |key: &str| {
        model == key
            || model.starts_with(&format!("{key}-"))
            || model.starts_with(&format!("{key}@"))
            || model.starts_with(&format!("{key}:"))
    };
    let threshold = if [
        "gpt-5.6",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "gpt-5.4",
    ]
    .into_iter()
    .any(matches_model)
    {
        Some(272_001)
    } else if matches_model("gemini-3.1-pro-preview") || matches_model("gemini-2.5-pro") {
        Some(200_001)
    } else if matches_model("minimax-m3") {
        Some(512_001)
    } else if [
        "grok-4.6",
        "grok-4.5",
        "grok-4.5-build",
        "grok-build-latest",
        "grok-build-0.1",
        "grok-code-fast-1-0825",
        "grok-code-fast-1",
        "grok-code-fast",
        "grok-4.3",
        "grok-4.20-multi-agent-0309",
        "grok-4.20-0309-reasoning",
        "grok-4.20-0309-non-reasoning",
        "grok-build",
    ]
    .into_iter()
    .any(matches_model)
    {
        Some(200_001)
    } else {
        None
    };
    threshold
        .filter(|threshold| context_tokens >= *threshold)
        .unwrap_or(0)
}

#[derive(Debug, Default, Clone, PartialEq)]
struct SessionFileStats {
    total_messages: u32,
    total_tokens: u64,
    token_distribution: TokenDistribution,
    tool_usage: HashMap<String, (u32, u32)>, // (usage_count, success_count)
    skill_usage: HashMap<String, (u32, u32)>, // Skill tool, keyed by input.skill (#321)
    subagent_usage: HashMap<String, (u32, u32)>, // Agent tool, keyed by input.subagent_type (#321)
    daily_stats: HashMap<String, DailyStats>,
    activity_data: HashMap<(u8, u8), (u32, u64)>, // (hour, day) -> (count, tokens)
    model_usage: HashMap<String, ModelUsageAggregate>, // model -> (msg_count, total, input, output, cache_create, cache_read, reasoning)
    model_context_usage: ModelContextUsageMap,
    model_costs: HashMap<String, f64>, // model -> authoritative source cost when present
    session_duration_minutes: u64,
    first_message: Option<DateTime<Utc>>,
    last_message: Option<DateTime<Utc>>,
    project_name: String,
    provider: StatsProvider,
}

/// Project display name for a Claude session file (parent directory name).
fn claude_session_project_name(session_path: &Path) -> String {
    session_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string()
}

/// Process a session file into the lightweight global stats representation.
/// Served from the per-file daily-aggregate cache when the file is unchanged
/// and the date filter composes from daily buckets; otherwise falls back to
/// the full scan (see the `cache` module design note).
fn process_session_file_for_global_stats(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionFileStats> {
    if let Some(aggregate) = cache::global_stats_cache().get_or_build(session_path, mode, || {
        cache::build_global_file_aggregate(session_path, mode)
    }) {
        if let cache::Composed::Ready(stats) = cache::compose_global(
            &aggregate,
            claude_session_project_name(session_path),
            s_limit,
            e_limit,
        ) {
            return Some(stats);
        }
    }
    scan_session_file_for_global_stats(session_path, mode, s_limit, e_limit)
}

/// Process a single session file using lightweight deserialization for global stats.
/// Only parses fields needed for stats (timestamp, usage, model, tool names).
#[allow(unsafe_code)] // Required for mmap performance optimization
/// Full-scan path for global stats (cache miss / non-composable date filter).
fn scan_session_file_for_global_stats(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionFileStats> {
    let file = fs::File::open(session_path).ok()?;

    // SAFETY: We're only reading the file, and the file handle is kept open
    // for the duration of the mmap's lifetime. Session files are append-only.
    let mmap = unsafe { Mmap::map(&file) }.ok()?;

    let mut stats = SessionFileStats {
        project_name: claude_session_project_name(session_path),
        provider: StatsProvider::Claude,
        ..Default::default()
    };

    let mut session_timestamps: Vec<DateTime<Utc>> = Vec::new();
    // #283: stream entries one at a time with owned-key dedup so we never
    // buffer parsed log entries (which can carry MB-sized `content` payloads).
    let mut seen_usage_keys: HashSet<String> = HashSet::new();
    let mut seen_cost_keys: HashSet<String> = HashSet::new();

    // Use SIMD-accelerated line detection
    let line_ranges = find_line_ranges(&mmap);

    for (start, end) in line_ranges {
        let mut line_bytes = mmap[start..end].to_vec();
        let Some(entry) = parse_global_stats_entry_simd(&mut line_bytes) else {
            continue;
        };

        let usage = extract_token_usage_from_global_entry(&entry);
        let has_usage = token_usage_has_token_fields(&usage);

        if !should_include_stats_entry(&entry.message_type, entry.is_sidechain, has_usage, mode) {
            continue;
        }

        // Date-range filtering: parse timestamp early and skip messages outside the window.
        // When no date limits are set, all messages pass through (preserving original behaviour).
        let has_date_filter = s_limit.is_some() || e_limit.is_some();
        let parsed_timestamp = entry.timestamp.as_ref().and_then(|ts_str| {
            DateTime::parse_from_rfc3339(ts_str)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        if has_date_filter && !is_within_date_limits(parsed_timestamp, s_limit, e_limit) {
            continue;
        }

        stats.total_messages = stats.total_messages.saturating_add(1);
        let message_id = entry.message.as_ref().and_then(|m| m.id.as_deref());
        let uuid = entry.uuid.as_deref().unwrap_or("");
        let (
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens,
            tokens,
        ) = dedup_token_totals(&mut seen_usage_keys, "", message_id, uuid, &usage);
        let source_cost = entry
            .cost_usd
            .or_else(|| entry.message.as_ref().and_then(|message| message.cost_usd));
        let deduped_source_cost =
            dedup_source_cost(&mut seen_cost_keys, "", message_id, uuid, source_cost);

        stats.total_tokens += tokens;
        stats.token_distribution.input += input_tokens;
        stats.token_distribution.output += output_tokens;
        stats.token_distribution.cache_creation += cache_creation_tokens;
        stats.token_distribution.cache_read += cache_read_tokens;
        stats.token_distribution.reasoning += reasoning_tokens;
        let model_name = entry
            .message
            .as_ref()
            .and_then(|message| message.model.as_deref())
            .unwrap_or(UNKNOWN_MODEL_NAME);
        let has_model = entry
            .message
            .as_ref()
            .is_some_and(|message| message.model.is_some());
        if has_model || tokens > 0 || deduped_source_cost.is_some() {
            let model_entry = stats
                .model_usage
                .entry(model_name.to_string())
                .or_insert((0, 0, 0, 0, 0, 0, 0));
            model_entry.0 += 1;
            model_entry.1 += tokens;
            model_entry.2 += input_tokens;
            model_entry.3 += output_tokens;
            model_entry.4 += cache_creation_tokens;
            model_entry.5 += cache_read_tokens;
            model_entry.6 += reasoning_tokens;
            if let Some(cost_usd) = deduped_source_cost {
                *stats
                    .model_costs
                    .entry(model_name.to_string())
                    .or_insert(0.0) += cost_usd;
            }
        }

        let Some(timestamp) = parsed_timestamp else {
            track_tool_usage_from_global_entry(&entry, &mut stats.tool_usage);
            track_skill_and_subagent_usage_from_global_entry(
                &entry,
                &mut stats.skill_usage,
                &mut stats.subagent_usage,
            );
            continue;
        };

        session_timestamps.push(timestamp);

        // Track first/last message
        if stats
            .first_message
            .map_or(true, |current| timestamp < current)
        {
            stats.first_message = Some(timestamp);
        }
        if stats
            .last_message
            .map_or(true, |current| timestamp > current)
        {
            stats.last_message = Some(timestamp);
        }

        let hour = timestamp.hour() as u8;
        let day = timestamp.weekday().num_days_from_sunday() as u8;

        // Activity data
        let activity_entry = stats.activity_data.entry((hour, day)).or_insert((0, 0));
        activity_entry.0 += 1;
        activity_entry.1 += tokens;

        // Daily stats
        let date = timestamp.format("%Y-%m-%d").to_string();
        let daily_entry = stats
            .daily_stats
            .entry(date.clone())
            .or_insert_with(|| DailyStats {
                date,
                ..Default::default()
            });
        daily_entry.total_tokens += tokens;
        daily_entry.input_tokens += input_tokens;
        daily_entry.output_tokens += output_tokens;
        daily_entry.message_count += 1;

        // Track tool usage
        track_tool_usage_from_global_entry(&entry, &mut stats.tool_usage);
        track_skill_and_subagent_usage_from_global_entry(
            &entry,
            &mut stats.skill_usage,
            &mut stats.subagent_usage,
        );
    }

    // Calculate session duration
    calculate_session_duration(&mut session_timestamps, &mut stats);

    Some(stats)
}

/// Calculate active session duration from sorted timestamps
fn calculate_session_duration(
    session_timestamps: &mut Vec<DateTime<Utc>>,
    stats: &mut SessionFileStats,
) {
    const SESSION_BREAK_THRESHOLD_MINUTES: i64 = 120;

    if session_timestamps.len() >= 2 {
        session_timestamps.sort_unstable();
        let mut current_period_start = session_timestamps[0];
        let mut total_active_minutes = 0u64;

        for i in 0..session_timestamps.len() - 1 {
            let current = session_timestamps[i];
            let next = session_timestamps[i + 1];
            let gap_minutes = (next - current).num_minutes();

            if gap_minutes > SESSION_BREAK_THRESHOLD_MINUTES {
                let period_duration = (current - current_period_start).num_minutes();
                total_active_minutes += period_duration.max(1) as u64;
                current_period_start = next;
            }
        }

        let last_timestamp = session_timestamps[session_timestamps.len() - 1];
        let final_period = (last_timestamp - current_period_start).num_minutes();
        total_active_minutes += final_period.max(1) as u64;

        stats.session_duration_minutes = total_active_minutes;
    } else if session_timestamps.len() == 1 {
        stats.session_duration_minutes = 1;
    }
}

/// Build global stats from already-loaded provider messages.
fn build_global_session_file_stats_from_messages(
    provider: StatsProvider,
    project_name: String,
    messages: &[ClaudeMessage],
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionFileStats> {
    if messages.is_empty() {
        return None;
    }

    let mut stats = SessionFileStats {
        project_name,
        provider,
        ..Default::default()
    };

    let mut session_timestamps: Vec<DateTime<Utc>> = Vec::new();
    // #283: counts rows but only adds usage once per (session_id, message.id).
    let mut seen_usage_keys: HashSet<String> = HashSet::with_capacity(messages.len());
    let mut seen_cost_keys: HashSet<String> = HashSet::with_capacity(messages.len());

    let has_date_filter = s_limit.is_some() || e_limit.is_some();

    for message in messages {
        if !should_include_stats_message(message, mode) {
            continue;
        }

        let usage = extract_token_usage(message);

        // Date-range filtering: parse timestamp early and skip messages outside the window.
        let parsed_timestamp = parse_timestamp_utc(&message.timestamp);
        if has_date_filter && !is_within_date_limits(parsed_timestamp, s_limit, e_limit) {
            continue;
        }

        stats.total_messages = stats.total_messages.saturating_add(1);
        let (
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens,
            tokens,
        ) = dedup_token_totals_msg(&mut seen_usage_keys, message, &usage);
        let deduped_source_cost = dedup_source_cost(
            &mut seen_cost_keys,
            &message.session_id,
            message.message_id.as_deref(),
            &message.uuid,
            message.cost_usd,
        );

        stats.total_tokens += tokens;
        stats.token_distribution.input += input_tokens;
        stats.token_distribution.output += output_tokens;
        stats.token_distribution.cache_creation += cache_creation_tokens;
        stats.token_distribution.cache_read += cache_read_tokens;
        stats.token_distribution.reasoning += reasoning_tokens;
        if message.model.is_some() || tokens > 0 || deduped_source_cost.is_some() {
            let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
            let model_entry = stats
                .model_usage
                .entry(model_name.to_string())
                .or_insert((0, 0, 0, 0, 0, 0, 0));
            model_entry.0 += 1;
            model_entry.1 += tokens;
            model_entry.2 += input_tokens;
            model_entry.3 += output_tokens;
            model_entry.4 += cache_creation_tokens;
            model_entry.5 += cache_read_tokens;
            model_entry.6 += reasoning_tokens;
            if let Some(cost_usd) = deduped_source_cost {
                *stats
                    .model_costs
                    .entry(model_name.to_string())
                    .or_insert(0.0) += cost_usd;
            }
        }

        if let Some(timestamp) = parsed_timestamp {
            session_timestamps.push(timestamp);

            // Track first/last message
            if stats.first_message.is_none() || timestamp < stats.first_message.unwrap() {
                stats.first_message = Some(timestamp);
            }
            if stats.last_message.is_none() || timestamp > stats.last_message.unwrap() {
                stats.last_message = Some(timestamp);
            }

            let hour = timestamp.hour() as u8;
            let day = timestamp.weekday().num_days_from_sunday() as u8;

            // Activity data
            let activity_entry = stats.activity_data.entry((hour, day)).or_insert((0, 0));
            activity_entry.0 += 1;
            activity_entry.1 += tokens;

            // Daily stats
            let date = timestamp.format("%Y-%m-%d").to_string();
            let daily_entry = stats
                .daily_stats
                .entry(date.clone())
                .or_insert_with(|| DailyStats {
                    date,
                    ..Default::default()
                });
            daily_entry.total_tokens += tokens;
            daily_entry.input_tokens += input_tokens;
            daily_entry.output_tokens += output_tokens;
            daily_entry.message_count += 1;
        }

        // Track tool usage
        track_tool_usage(message, &mut stats.tool_usage);
        track_skill_and_subagent_usage(message, &mut stats.skill_usage, &mut stats.subagent_usage);
    }

    // Calculate session duration
    const SESSION_BREAK_THRESHOLD_MINUTES: i64 = 120;

    if session_timestamps.len() >= 2 {
        session_timestamps.sort();
        let mut current_period_start = session_timestamps[0];
        let mut total_active_minutes = 0u64;

        for i in 0..session_timestamps.len() - 1 {
            let current = session_timestamps[i];
            let next = session_timestamps[i + 1];
            let gap_minutes = (next - current).num_minutes();

            if gap_minutes > SESSION_BREAK_THRESHOLD_MINUTES {
                let period_duration = (current - current_period_start).num_minutes();
                total_active_minutes += period_duration.max(1) as u64;
                current_period_start = next;
            }
        }

        let last_timestamp = session_timestamps[session_timestamps.len() - 1];
        let final_period = (last_timestamp - current_period_start).num_minutes();
        total_active_minutes += final_period.max(1) as u64;

        stats.session_duration_minutes = total_active_minutes;
    } else if session_timestamps.len() == 1 {
        stats.session_duration_minutes = 1;
    }

    Some(stats)
}

/// Collect global stats rows for a non-Claude provider.
fn collect_provider_global_file_stats(
    provider: StatsProvider,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> (Vec<SessionFileStats>, HashSet<String>) {
    let mut project_keys = HashSet::new();

    if provider == StatsProvider::Cursor {
        let Ok(sessions) = providers::cursor::collect_global_stats_sessions() else {
            return (Vec::new(), project_keys);
        };
        let mut all_stats = Vec::with_capacity(sessions.len());
        for (project_display_name, messages) in sessions {
            project_keys.insert(format!(
                "cursor:{}",
                project_display_name
                    .strip_suffix(" [cursor]")
                    .unwrap_or(&project_display_name)
            ));
            if let Some(stats) = build_global_session_file_stats_from_messages(
                StatsProvider::Cursor,
                project_display_name,
                &messages,
                mode,
                s_limit,
                e_limit,
            ) {
                all_stats.push(stats);
            }
        }
        return (all_stats, project_keys);
    }

    if provider == StatsProvider::Antigravity {
        // Use the resolver that honors the external-state override so an
        // external Antigravity root contributes to the global summary
        // (the bare get_antigravity_root only returns the default path).
        let Ok(root) = crate::commands::antigravity::resolve_antigravity_root()
            .ok_or_else(|| "Cannot determine antigravity root directory".to_string())
        else {
            return (Vec::new(), project_keys);
        };
        let Ok(sessions) = providers::antigravity::load_sessions(&root.to_string_lossy(), false)
        else {
            return (Vec::new(), project_keys);
        };
        project_keys.insert(format!(
            "{}:{}",
            stats_provider_id(provider),
            "Antigravity [antigravity]"
        ));

        let mut all_stats = Vec::new();
        for session in &sessions {
            let records = match load_antigravity_usage_records(&session.file_path) {
                Ok(records) => records
                    .into_iter()
                    .filter(|record| {
                        is_within_date_limits(Some(record.timestamp), s_limit, e_limit)
                    })
                    .collect::<Vec<_>>(),
                Err(_) => continue,
            };
            if records.is_empty() {
                continue;
            }

            let mut stats = SessionFileStats {
                project_name: "Antigravity [antigravity]".to_string(),
                provider,
                ..Default::default()
            };
            if let Ok(messages) = providers::antigravity::load_messages(&session.file_path) {
                for message in &messages {
                    track_tool_usage(message, &mut stats.tool_usage);
                }
            }
            let mut timestamps = Vec::new();
            for record in records {
                let input_tokens = match mode {
                    StatsMode::BillingTotal => record.input_tokens,
                    StatsMode::ConversationOnly => record.conversation_input_tokens,
                };
                let cache_creation_tokens = match mode {
                    StatsMode::BillingTotal => record.cache_creation_tokens,
                    StatsMode::ConversationOnly => record.conversation_cache_creation_tokens,
                };
                let cache_read_tokens = match mode {
                    StatsMode::BillingTotal => record.cache_read_tokens,
                    StatsMode::ConversationOnly => record.conversation_cache_read_tokens,
                };
                let total_tokens = match mode {
                    StatsMode::BillingTotal => record.total_tokens,
                    StatsMode::ConversationOnly => {
                        input_tokens
                            + record.output_tokens
                            + cache_creation_tokens
                            + cache_read_tokens
                            + record.reasoning_tokens
                    }
                };

                stats.total_messages += 1;
                stats.total_tokens += total_tokens;
                stats.token_distribution.input += input_tokens;
                stats.token_distribution.output += record.output_tokens;
                stats.token_distribution.cache_creation += cache_creation_tokens;
                stats.token_distribution.cache_read += cache_read_tokens;
                stats.token_distribution.reasoning += record.reasoning_tokens;

                let model_entry = stats
                    .model_usage
                    .entry(record.model.clone())
                    .or_insert((0, 0, 0, 0, 0, 0, 0));
                model_entry.0 += 1;
                model_entry.1 += total_tokens;
                model_entry.2 += input_tokens;
                model_entry.3 += record.output_tokens;
                model_entry.4 += cache_creation_tokens;
                model_entry.5 += cache_read_tokens;
                model_entry.6 += record.reasoning_tokens;

                let date = record.timestamp.format("%Y-%m-%d").to_string();
                let daily_entry =
                    stats
                        .daily_stats
                        .entry(date.clone())
                        .or_insert_with(|| DailyStats {
                            date,
                            ..Default::default()
                        });
                daily_entry.total_tokens += total_tokens;
                daily_entry.input_tokens += input_tokens;
                daily_entry.output_tokens += record.output_tokens;
                daily_entry.message_count += 1;

                let hour = record.timestamp.hour() as u8;
                let day = record.timestamp.weekday().num_days_from_sunday() as u8;
                let activity_entry = stats.activity_data.entry((hour, day)).or_insert((0, 0));
                activity_entry.0 += 1;
                activity_entry.1 += total_tokens;

                timestamps.push(record.timestamp);
                if stats
                    .first_message
                    .map_or(true, |current| record.timestamp < current)
                {
                    stats.first_message = Some(record.timestamp);
                }
                if stats
                    .last_message
                    .map_or(true, |current| record.timestamp > current)
                {
                    stats.last_message = Some(record.timestamp);
                }
            }

            stats.session_duration_minutes =
                u64::from(calculate_session_active_minutes(&mut timestamps));
            all_stats.push(stats);
        }

        return (all_stats, project_keys);
    }

    if provider == StatsProvider::Codex {
        // The generic path below calls load_stats_sessions once PER
        // PROJECT, which for Codex means re-walking and re-stat-checking
        // the entire rollout store once per distinct working directory
        // Codex was ever run from -- confirmed on a real machine with
        // hundreds of such "projects": each individually-fast per-project
        // call (thanks to extract_session_info_cached) still multiplied
        // out to 10+ seconds of total wall-clock. collect_global_stats_sessions
        // walks the store exactly once regardless of project count.
        let sessions = providers::codex::collect_global_stats_sessions();
        let provider_tag = stats_provider_id(provider);

        for (cwd, _session) in &sessions {
            project_keys.insert(format!("{provider_tag}:codex://{cwd}"));
        }

        let all_stats: Vec<SessionFileStats> = sessions
            .par_iter()
            .filter_map(|(_cwd, session)| {
                let project_name = format!("{} [{provider_tag}]", session.project_name);
                let build = || {
                    let messages =
                        load_stats_messages(provider, &session.file_path).unwrap_or_default();
                    build_global_session_file_stats_from_messages(
                        provider,
                        project_name.clone(),
                        &messages,
                        mode,
                        s_limit,
                        e_limit,
                    )
                };

                if s_limit.is_some() || e_limit.is_some() {
                    return build();
                }

                cache::provider_session_stats_cache().get_or_build(
                    Path::new(&session.file_path),
                    mode,
                    build,
                )
            })
            .collect();

        return (all_stats, project_keys);
    }

    let projects = scan_stats_projects(provider).unwrap_or_default();

    let provider_tag = stats_provider_id(provider);

    // Collect all (project_display_name, session_file_path) pairs first
    let mut session_tasks: Vec<(String, String)> = Vec::new();

    for project in projects {
        let project_display_name = format!("{} [{}]", project.name, provider_tag);
        project_keys.insert(format!("{provider_tag}:{}", project.path));

        let sessions = load_stats_sessions(provider, &project.path).unwrap_or_default();

        for session in sessions {
            session_tasks.push((project_display_name.clone(), session.file_path));
        }
    }

    // Process all sessions in parallel. Cached per (file_path, mode) when no
    // date filter is active -- a filtered call always rebuilds, since this
    // cache stores the whole computed result, not day buckets to compose
    // from (see cache.rs's "Generic-provider global stats cache" doc). This
    // is what makes a REPEAT global stats load fast; the first load for a
    // given file still pays the full parse cost once.
    let all_stats: Vec<SessionFileStats> = session_tasks
        .par_iter()
        .filter_map(|(project_name, file_path)| {
            let build = || {
                let messages = load_stats_messages(provider, file_path).unwrap_or_default();
                build_global_session_file_stats_from_messages(
                    provider,
                    project_name.clone(),
                    &messages,
                    mode,
                    s_limit,
                    e_limit,
                )
            };

            if s_limit.is_some() || e_limit.is_some() {
                return build();
            }

            cache::provider_session_stats_cache().get_or_build(Path::new(file_path), mode, build)
        })
        .collect();

    (all_stats, project_keys)
}

/// Intermediate stats collected from a single session file (for project stats)
#[derive(Default)]
struct ProjectSessionFileStats {
    total_messages: u32,
    token_distribution: TokenDistribution,
    model_usage: HashMap<String, ModelUsageAggregate>,
    model_context_usage: ModelContextUsageMap,
    model_costs: HashMap<String, f64>,
    tool_usage: HashMap<String, (u32, u32)>,
    skill_usage: HashMap<String, (u32, u32)>, // Skill tool, keyed by input.skill (#321)
    subagent_usage: HashMap<String, (u32, u32)>, // Agent tool, keyed by input.subagent_type (#321)
    daily_stats: HashMap<String, DailyStats>,
    activity_data: HashMap<(u8, u8), (u32, u64)>,
    session_duration_minutes: u32,
    session_dates: HashSet<String>,
    timestamps: Vec<DateTime<Utc>>,
}

/// Process a session file into project-level stats.
/// Served from the per-file daily-aggregate cache when possible; falls back
/// to the full scan (see the `cache` module design note).
fn process_session_file_for_project_stats(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<ProjectSessionFileStats> {
    if let Some(aggregate) = cache::message_stats_cache().get_or_build(session_path, mode, || {
        cache::build_message_file_aggregate(session_path, mode)
    }) {
        if let cache::Composed::Ready(stats) = cache::compose_project(&aggregate, s_limit, e_limit)
        {
            return stats;
        }
    }
    scan_session_file_for_project_stats(session_path, mode, s_limit, e_limit)
}

/// Process a single session file for project stats
#[allow(unsafe_code)] // Required for mmap performance optimization
/// Full-scan path for project stats (cache miss / non-composable date filter).
fn scan_session_file_for_project_stats(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<ProjectSessionFileStats> {
    let file = fs::File::open(session_path).ok()?;

    // SAFETY: We're only reading the file, and the file handle is kept open
    // for the duration of the mmap's lifetime. Session files are append-only.
    let mmap = unsafe { Mmap::map(&file) }.ok()?;

    let mut stats = ProjectSessionFileStats::default();
    let mut session_timestamps: Vec<DateTime<Utc>> = Vec::new();

    // Use SIMD-accelerated line detection
    let line_ranges = find_line_ranges(&mmap);

    // #283: stream entries with owned-key dedup so we never buffer parsed
    // messages (which can carry MB-sized `content` payloads).
    let mut seen_usage_keys: HashSet<String> = HashSet::new();
    let mut seen_cost_keys: HashSet<String> = HashSet::new();

    for (start, end) in line_ranges {
        let mut line_bytes = mmap[start..end].to_vec();
        let Some(log_entry) = parse_raw_log_entry_simd(&mut line_bytes) else {
            continue;
        };
        let Ok(message) = ClaudeMessage::try_from(log_entry) else {
            continue;
        };

        let usage = extract_token_usage(&message);
        let has_usage = token_usage_has_token_fields(&usage);
        if !should_include_stats_entry(&message.message_type, message.is_sidechain, has_usage, mode)
        {
            continue;
        }

        // Per-message date filtering
        let parsed_ts = parse_timestamp_utc(&message.timestamp);
        if !is_within_date_limits(parsed_ts, s_limit, e_limit) {
            continue;
        }

        stats.total_messages += 1;
        let (
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens,
            tokens,
        ) = dedup_token_totals_msg(&mut seen_usage_keys, &message, &usage);
        let deduped_source_cost = dedup_source_cost(
            &mut seen_cost_keys,
            &message.session_id,
            message.message_id.as_deref(),
            &message.uuid,
            message.cost_usd,
        );

        let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
        if message.model.is_some() || tokens > 0 || deduped_source_cost.is_some() {
            accumulate_model_usage(
                &mut stats.model_usage,
                &mut stats.model_context_usage,
                &mut stats.model_costs,
                ModelUsageUpdate {
                    model_name,
                    service_tier: usage.service_tier.as_deref(),
                    totals: (
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                        reasoning_tokens,
                        tokens,
                    ),
                    cache_creation_tokens_1h: u64::from(
                        usage.cache_creation_input_tokens_1h.unwrap_or(0),
                    ),
                    source_cost: deduped_source_cost,
                },
            );
        }

        stats.token_distribution.input += input_tokens;
        stats.token_distribution.output += output_tokens;
        stats.token_distribution.cache_creation += cache_creation_tokens;
        stats.token_distribution.cache_read += cache_read_tokens;
        stats.token_distribution.reasoning += reasoning_tokens;

        if let Some(timestamp) = parsed_ts {
            session_timestamps.push(timestamp);

            let hour = timestamp.hour() as u8;
            let day = timestamp.weekday().num_days_from_sunday() as u8;

            let activity_entry = stats.activity_data.entry((hour, day)).or_insert((0, 0));
            activity_entry.0 += 1;
            activity_entry.1 += tokens;

            let date = timestamp.format("%Y-%m-%d").to_string();
            stats.session_dates.insert(date.clone());

            let daily_entry = stats
                .daily_stats
                .entry(date.clone())
                .or_insert_with(|| DailyStats {
                    date,
                    ..Default::default()
                });
            daily_entry.total_tokens += tokens;
            daily_entry.input_tokens += input_tokens;
            daily_entry.output_tokens += output_tokens;
            daily_entry.message_count += 1;
        }

        // Track tool usage
        track_tool_usage(&message, &mut stats.tool_usage);
        track_skill_and_subagent_usage(&message, &mut stats.skill_usage, &mut stats.subagent_usage);
    }

    if stats.total_messages == 0 {
        return None;
    }

    // Calculate session duration
    const SESSION_BREAK_THRESHOLD_MINUTES: i64 = 120;

    if session_timestamps.len() >= 2 {
        session_timestamps.sort();
        let mut current_period_start = session_timestamps[0];
        let mut session_total_minutes = 0u32;

        for i in 0..session_timestamps.len() - 1 {
            let current = session_timestamps[i];
            let next = session_timestamps[i + 1];
            let gap_minutes = (next - current).num_minutes();

            if gap_minutes > SESSION_BREAK_THRESHOLD_MINUTES {
                let period_duration = (current - current_period_start).num_minutes();
                session_total_minutes += period_duration.max(1) as u32;
                current_period_start = next;
            }
        }

        let last = session_timestamps[session_timestamps.len() - 1];
        let final_period = (last - current_period_start).num_minutes();
        session_total_minutes += final_period.max(1) as u32;

        stats.session_duration_minutes = session_total_minutes;
    } else if session_timestamps.len() == 1 {
        stats.session_duration_minutes = 1;
    }

    stats.timestamps = session_timestamps;
    Some(stats)
}

/// Track tool usage counters for a normalized message.
fn track_tool_usage(message: &ClaudeMessage, tool_usage: &mut HashMap<String, (u32, u32)>) {
    // Tool usage from assistant content
    if message.message_type == "assistant" {
        if let Some(content) = &message.content {
            if let Some(content_array) = content.as_array() {
                for item in content_array {
                    if let Some(item_type) = item.get("type").and_then(|v| v.as_str()) {
                        if item_type == "tool_use" {
                            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                let tool_entry =
                                    tool_usage.entry(name.to_string()).or_insert((0, 0));
                                tool_entry.0 += 1;
                                // Check for success/error similar to explicit tool_use
                                let is_error = item
                                    .get("is_error")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                if !is_error {
                                    tool_entry.1 += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Tool usage from explicit tool_use field
    if let Some(tool_use) = &message.tool_use {
        if let Some(name) = tool_use.get("name").and_then(|v| v.as_str()) {
            let tool_entry = tool_usage.entry(name.to_string()).or_insert((0, 0));
            tool_entry.0 += 1;
            if let Some(result) = &message.tool_use_result {
                let is_error = result
                    .get("is_error")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if !is_error {
                    tool_entry.1 += 1;
                }
            }
        }
    }
}

/// Record one usage of a tool keyed by a value inside its `input` — e.g. the
/// `Skill` tool keyed by `input.skill`, or the `Agent` tool keyed by
/// `input.subagent_type` (issue #321). `item` is a single `tool_use` value.
fn record_input_value_usage(
    item: &serde_json::Value,
    usage: &mut HashMap<String, (u32, u32)>,
    tool_name: &str,
    input_key: &str,
) {
    if item.get("name").and_then(|v| v.as_str()) != Some(tool_name) {
        return;
    }
    if let Some(key) = item
        .get("input")
        .and_then(|input| input.get(input_key))
        .and_then(|v| v.as_str())
    {
        let entry = usage.entry(key.to_string()).or_insert((0, 0));
        entry.0 += 1;
        let is_error = item
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !is_error {
            entry.1 += 1;
        }
    }
}

/// Aggregate Skill (`input.skill`) and Agent (`input.subagent_type`) invocations
/// from a normalized message, mirroring `track_tool_usage`'s extraction paths.
fn track_skill_and_subagent_usage(
    message: &ClaudeMessage,
    skill_usage: &mut HashMap<String, (u32, u32)>,
    subagent_usage: &mut HashMap<String, (u32, u32)>,
) {
    if message.message_type == "assistant" {
        if let Some(arr) = message.content.as_ref().and_then(|c| c.as_array()) {
            for item in arr {
                if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                    record_input_value_usage(item, skill_usage, "Skill", "skill");
                    record_input_value_usage(item, subagent_usage, "Agent", "subagent_type");
                }
            }
        }
    }
    if let Some(tool_use) = &message.tool_use {
        record_input_value_usage(tool_use, skill_usage, "Skill", "skill");
        record_input_value_usage(tool_use, subagent_usage, "Agent", "subagent_type");
    }
}

/// Skill/subagent variant of `track_tool_usage_from_global_entry`. Skill and
/// Agent calls land in the assistant `content` array, which is the only path
/// the lightweight global entry preserves as raw JSON.
fn track_skill_and_subagent_usage_from_global_entry(
    entry: &GlobalStatsLogEntry,
    skill_usage: &mut HashMap<String, (u32, u32)>,
    subagent_usage: &mut HashMap<String, (u32, u32)>,
) {
    if entry.message_type != "assistant" {
        return;
    }
    if let Some(arr) = entry
        .message
        .as_ref()
        .and_then(|m| m.content.as_ref())
        .and_then(|c| c.as_array())
    {
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                record_input_value_usage(item, skill_usage, "Skill", "skill");
                record_input_value_usage(item, subagent_usage, "Agent", "subagent_type");
            }
        }
    }
}

/// Track tool usage across a slice of Antigravity messages while honoring
/// the active date filter. Per-project token totals filter by record
/// timestamp; this mirrors that behavior at the message level so the tool
/// breakdown does not drift from the token totals.
fn track_antigravity_tool_usage(
    messages: &[ClaudeMessage],
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
    tool_usage_map: &mut HashMap<String, (u32, u32)>,
) {
    let has_date_filter = s_limit.is_some() || e_limit.is_some();
    for message in messages {
        if has_date_filter
            && !is_within_date_limits(parse_timestamp_utc(&message.timestamp), s_limit, e_limit)
        {
            continue;
        }
        track_tool_usage(message, tool_usage_map);
    }
}

/// Extract token usage from a normalized message.
fn extract_token_usage(message: &ClaudeMessage) -> TokenUsage {
    if let Some(usage) = &message.usage {
        return normalize_token_usage(usage.clone());
    }

    let mut usage = TokenUsage {
        input_tokens: None,
        output_tokens: None,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        reasoning_tokens: None,
        service_tier: None,
        ..Default::default()
    };

    if let Some(content) = &message.content {
        let usage_obj = if content.is_object() && content.get("usage").is_some() {
            content.get("usage")
        } else {
            None
        };

        if let Some(usage_obj) = usage_obj {
            apply_usage_fields_from_value(usage_obj, &mut usage);
        }
    }

    if let Some(tool_result) = &message.tool_use_result {
        if let Some(usage_obj) = tool_result.get("usage") {
            apply_usage_fields_from_value(usage_obj, &mut usage);
        }

        if let Some(total_tokens) = tool_result
            .get("totalTokens")
            .and_then(serde_json::Value::as_u64)
        {
            if usage.input_tokens.is_none() && usage.output_tokens.is_none() {
                if message.message_type == "assistant" {
                    usage.output_tokens = Some(total_tokens as u32);
                } else {
                    usage.input_tokens = Some(total_tokens as u32);
                }
            }
        }
    }

    normalize_token_usage(usage)
}

/// Dedup-aware token totals for usage accounting (#283).
///
/// Claude assistant turns split content (`thinking`, `tool_use`, `text`)
/// across multiple JSONL rows that share the same `message.id` and embed
/// an identical `usage` payload. Aggregators call this once per row and
/// add the returned totals unconditionally — duplicates contribute zero
/// while row counts (`total_messages`, `model.msg_count`, etc.) stay
/// per-row.
///
/// Key precedence: `(session_id, message_id)` if `message_id` is non-empty,
/// otherwise `(session_id, uuid)`. If both `message_id` and `uuid` are
/// empty/missing the row has no identity to dedup by, so it always counts
/// (returns full totals) — this avoids silently undercounting rows that
/// genuinely cannot be keyed.
///
/// Owned `String` keys keep the helper streaming-friendly: callers don't
/// need to buffer their parsed entries to satisfy borrow lifetimes.
#[inline]
fn dedup_token_totals(
    seen: &mut HashSet<String>,
    session_id: &str,
    message_id: Option<&str>,
    uuid: &str,
    usage: &TokenUsage,
) -> (u64, u64, u64, u64, u64, u64) {
    let Some(key) = dedup_usage_key(session_id, message_id, uuid) else {
        return token_usage_totals(usage);
    };
    if seen.insert(key) {
        token_usage_totals(usage)
    } else {
        (0, 0, 0, 0, 0, 0)
    }
}

/// Dedup an authoritative source cost using the same identity as token usage.
/// Claude JSONL can repeat a complete `costUSD` value on every row belonging
/// to one assistant turn, so summing every row would overstate billing.
#[inline]
fn dedup_source_cost(
    seen: &mut HashSet<String>,
    session_id: &str,
    message_id: Option<&str>,
    uuid: &str,
    cost_usd: Option<f64>,
) -> Option<f64> {
    let cost_usd = cost_usd?;
    let Some(key) = dedup_usage_key(session_id, message_id, uuid) else {
        return Some(cost_usd);
    };
    if seen.insert(key) {
        Some(cost_usd)
    } else {
        None
    }
}

/// Build the dedup identity key for a row (#283), or `None` when the row has
/// no identity to dedup by and must always count.
#[inline]
fn dedup_usage_key(session_id: &str, message_id: Option<&str>, uuid: &str) -> Option<String> {
    match message_id.filter(|s| !s.is_empty()) {
        Some(mid) => Some(format!("{session_id}|m:{mid}")),
        None if !uuid.is_empty() => Some(format!("{session_id}|u:{uuid}")),
        None => None,
    }
}

/// Convenience wrapper for `ClaudeMessage`-based aggregators.
#[inline]
fn dedup_token_totals_msg(
    seen: &mut HashSet<String>,
    message: &ClaudeMessage,
    usage: &TokenUsage,
) -> (u64, u64, u64, u64, u64, u64) {
    dedup_token_totals(
        seen,
        &message.session_id,
        message.message_id.as_deref(),
        &message.uuid,
        usage,
    )
}

/// Parse an optional inclusive date limit for stats filtering.
fn parse_date_limit(date_str: Option<String>, label: &str) -> Option<DateTime<Utc>> {
    let raw = date_str?;
    match DateTime::parse_from_rfc3339(&raw) {
        Ok(dt) => Some(dt.with_timezone(&Utc)),
        Err(e) => {
            log::warn!("Invalid RFC3339 {label} '{raw}': {e}");
            None
        }
    }
}

/// Parse a timestamp string into UTC.
fn parse_timestamp_utc(timestamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Return whether a timestamp falls within the active date limits.
fn is_within_date_limits(
    timestamp: Option<DateTime<Utc>>,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> bool {
    if s_limit.is_none() && e_limit.is_none() {
        return true;
    }

    let Some(ts) = timestamp else {
        return false;
    };

    let after_start = s_limit.map(|s| ts >= *s).unwrap_or(true);
    let before_end = e_limit.map(|e| ts <= *e).unwrap_or(true);
    after_start && before_end
}

/// Estimate active session duration by collapsing long idle gaps.
fn calculate_session_active_minutes(timestamps: &mut [DateTime<Utc>]) -> u32 {
    const SESSION_BREAK_THRESHOLD_MINUTES: i64 = 120;

    if timestamps.is_empty() {
        return 0;
    }

    if timestamps.len() == 1 {
        return 1;
    }

    timestamps.sort();
    let mut current_period_start = timestamps[0];
    let mut session_total_minutes = 0u32;

    for i in 0..timestamps.len() - 1 {
        let current = timestamps[i];
        let next = timestamps[i + 1];
        let gap_minutes = (next - current).num_minutes();

        if gap_minutes > SESSION_BREAK_THRESHOLD_MINUTES {
            let period_duration = (current - current_period_start).num_minutes();
            session_total_minutes += period_duration.max(1) as u32;
            current_period_start = next;
        }
    }

    let last = timestamps[timestamps.len() - 1];
    let final_period = (last - current_period_start).num_minutes();
    session_total_minutes + final_period.max(1) as u32
}

fn load_antigravity_usage_records(
    session_path: &str,
) -> Result<Vec<AntigravityUsageRecord>, String> {
    let Some(usage_path) = providers::antigravity::resolve_usage_jsonl_path(session_path) else {
        return Ok(vec![]);
    };

    let content = fs::read_to_string(&usage_path)
        .map_err(|e| format!("Failed to read {}: {}", usage_path.display(), e))?;
    let mut records = Vec::new();

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["recordType"].as_str() != Some("usage") {
            continue;
        }

        let Some(created_at) = value["raw"]["chatModel"]["chatStartMetadata"]["createdAt"].as_str()
        else {
            continue;
        };
        let Some(timestamp) = parse_timestamp_utc(created_at) else {
            continue;
        };

        let input_tokens = value["inputTokens"].as_u64().unwrap_or(0);
        let output_tokens = value["outputTokens"].as_u64().unwrap_or(0);
        let cache_read_tokens = value["cacheReadTokens"].as_u64().unwrap_or(0);
        let cache_creation_tokens = value["cacheWriteTokens"].as_u64().unwrap_or(0);
        let reasoning_tokens = value["reasoningTokens"].as_u64().unwrap_or(0);
        let total_tokens = value["totalTokens"].as_u64().unwrap_or(0).max(
            input_tokens
                + output_tokens
                + cache_read_tokens
                + cache_creation_tokens
                + reasoning_tokens,
        );
        let (
            conversation_input_tokens,
            conversation_cache_creation_tokens,
            conversation_cache_read_tokens,
        ) = antigravity_chat_token_breakdown(&value)
            .map(|(chat_tokens, total_context_tokens)| {
                (
                    scale_token_count(input_tokens, chat_tokens, total_context_tokens),
                    scale_token_count(cache_creation_tokens, chat_tokens, total_context_tokens),
                    scale_token_count(cache_read_tokens, chat_tokens, total_context_tokens),
                )
            })
            .unwrap_or((input_tokens, cache_creation_tokens, cache_read_tokens));

        records.push(AntigravityUsageRecord {
            timestamp,
            model: value["model"].as_str().unwrap_or("unknown").to_string(),
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            conversation_input_tokens,
            conversation_cache_creation_tokens,
            conversation_cache_read_tokens,
            reasoning_tokens,
            total_tokens,
        });
    }

    Ok(records)
}

fn build_antigravity_session_token_stats(
    session: &crate::models::ClaudeSession,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Result<Option<(SessionTokenStats, Vec<AntigravityUsageRecord>)>, String> {
    let mut records = load_antigravity_usage_records(&session.file_path)?;
    records.retain(|record| is_within_date_limits(Some(record.timestamp), s_limit, e_limit));
    if records.is_empty() {
        return Ok(None);
    }

    let first_message_time = records
        .iter()
        .map(|record| record.timestamp)
        .min()
        .map(|ts| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let last_message_time = records
        .iter()
        .map(|record| record.timestamp)
        .max()
        .map(|ts| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());

    let mut model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut model_costs: HashMap<String, f64> = HashMap::new();
    for record in &records {
        let (input_tokens, cache_creation_tokens, cache_read_tokens, token_count) = match mode {
            StatsMode::BillingTotal => (
                record.input_tokens,
                record.cache_creation_tokens,
                record.cache_read_tokens,
                record.total_tokens,
            ),
            StatsMode::ConversationOnly => (
                record.conversation_input_tokens,
                record.conversation_cache_creation_tokens,
                record.conversation_cache_read_tokens,
                record.conversation_input_tokens
                    + record.output_tokens
                    + record.conversation_cache_creation_tokens
                    + record.conversation_cache_read_tokens
                    + record.reasoning_tokens,
            ),
        };
        accumulate_model_usage(
            &mut model_usage,
            &mut model_context_usage,
            &mut model_costs,
            ModelUsageUpdate {
                model_name: &record.model,
                service_tier: None,
                totals: (
                    input_tokens,
                    record.output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    record.reasoning_tokens,
                    token_count,
                ),
                cache_creation_tokens_1h: 0,
                source_cost: None,
            },
        );
    }

    let stats = SessionTokenStats {
        session_id: session.actual_session_id.clone(),
        project_name: session.project_name.clone(),
        total_input_tokens: records
            .iter()
            .map(|record| match mode {
                StatsMode::BillingTotal => record.input_tokens,
                StatsMode::ConversationOnly => record.conversation_input_tokens,
            })
            .sum(),
        total_output_tokens: records.iter().map(|record| record.output_tokens).sum(),
        total_cache_creation_tokens: records
            .iter()
            .map(|record| match mode {
                StatsMode::BillingTotal => record.cache_creation_tokens,
                StatsMode::ConversationOnly => record.conversation_cache_creation_tokens,
            })
            .sum(),
        total_cache_read_tokens: records
            .iter()
            .map(|record| match mode {
                StatsMode::BillingTotal => record.cache_read_tokens,
                StatsMode::ConversationOnly => record.conversation_cache_read_tokens,
            })
            .sum(),
        total_reasoning_tokens: records.iter().map(|record| record.reasoning_tokens).sum(),
        total_tokens: records
            .iter()
            .map(|record| match mode {
                StatsMode::BillingTotal => record.total_tokens,
                StatsMode::ConversationOnly => {
                    record.conversation_input_tokens
                        + record.output_tokens
                        + record.conversation_cache_creation_tokens
                        + record.conversation_cache_read_tokens
                        + record.reasoning_tokens
                }
            })
            .sum(),
        message_count: records.len(),
        first_message_time,
        last_message_time,
        summary: session.summary.clone(),
        most_used_tools: Vec::new(),
        model_distribution: build_model_stats(
            StatsProvider::Antigravity,
            model_usage,
            model_context_usage,
            model_costs,
        ),
    };

    Ok(Some((stats, records)))
}

/// Build sorted tool usage stats from aggregate counters.
fn build_tool_usage_stats(tool_usage: HashMap<String, (u32, u32)>) -> Vec<ToolUsageStats> {
    let mut tools = tool_usage
        .into_iter()
        .map(|(name, (usage, success))| ToolUsageStats {
            tool_name: name,
            usage_count: usage,
            success_rate: if usage > 0 {
                (success as f32 / usage as f32) * 100.0
            } else {
                0.0
            },
            avg_execution_time: None,
        })
        .collect::<Vec<_>>();

    tools.sort_by_key(|tool| Reverse(tool.usage_count));
    tools
}

/// Add one row to a model aggregate. Token totals are already deduplicated by
/// the caller, while `msg_count` intentionally remains row-based so the
/// model breakdown agrees with the message count shown by the dashboard.
struct ModelUsageUpdate<'a> {
    model_name: &'a str,
    service_tier: Option<&'a str>,
    totals: (u64, u64, u64, u64, u64, u64),
    cache_creation_tokens_1h: u64,
    source_cost: Option<f64>,
}

fn accumulate_model_usage(
    model_usage: &mut HashMap<String, ModelUsageAggregate>,
    model_context_usage: &mut ModelContextUsageMap,
    model_costs: &mut HashMap<String, f64>,
    update: ModelUsageUpdate<'_>,
) {
    let ModelUsageUpdate {
        model_name,
        service_tier,
        totals,
        cache_creation_tokens_1h,
        source_cost,
    } = update;
    let (
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        reasoning_tokens,
        tokens,
    ) = totals;
    let model_key = model_usage_key(model_name, service_tier);
    let entry = model_usage
        .entry(model_key.clone())
        .or_insert((0, 0, 0, 0, 0, 0, 0));
    entry.0 += 1;
    entry.1 += tokens;
    entry.2 += input_tokens;
    entry.3 += output_tokens;
    entry.4 += cache_creation_tokens;
    entry.5 += cache_read_tokens;
    entry.6 += reasoning_tokens;
    let context_tier = context_tier_min_tokens(
        model_name,
        input_tokens + cache_creation_tokens + cache_read_tokens,
    );
    let context = model_context_usage
        .entry(model_key.clone())
        .or_default()
        .entry(context_tier)
        .or_insert_with(|| ModelContextStats {
            min_context_tokens: context_tier,
            ..Default::default()
        });
    let cache_creation_tokens_1h = cache_creation_tokens_1h.min(cache_creation_tokens);
    context.token_count += tokens;
    context.input_tokens += input_tokens;
    context.output_tokens += output_tokens;
    context.cache_creation_tokens += cache_creation_tokens;
    context.cache_creation_tokens_1h += cache_creation_tokens_1h;
    context.cache_creation_tokens_5m += cache_creation_tokens - cache_creation_tokens_1h;
    context.cache_read_tokens += cache_read_tokens;
    context.reasoning_tokens += reasoning_tokens;
    if let Some(cost_usd) = source_cost {
        *model_costs.entry(model_key).or_insert(0.0) += cost_usd;
    }
}

fn merge_model_context_usage(target: &mut ModelContextUsageMap, source: &ModelContextUsageMap) {
    for (model, buckets) in source {
        for (min_context_tokens, values) in buckets {
            let entry = target
                .entry(model.clone())
                .or_default()
                .entry(*min_context_tokens)
                .or_insert_with(|| ModelContextStats {
                    min_context_tokens: *min_context_tokens,
                    ..Default::default()
                });
            entry.token_count += values.token_count;
            entry.input_tokens += values.input_tokens;
            entry.output_tokens += values.output_tokens;
            entry.cache_creation_tokens += values.cache_creation_tokens;
            entry.cache_creation_tokens_5m += values.cache_creation_tokens_5m;
            entry.cache_creation_tokens_1h += values.cache_creation_tokens_1h;
            entry.cache_read_tokens += values.cache_read_tokens;
            entry.reasoning_tokens += values.reasoning_tokens;
        }
    }
}

/// Convert an internal model aggregate into the serialized model breakdown
/// consumed by all billing views.
fn build_model_stats(
    provider: StatsProvider,
    model_usage: HashMap<String, ModelUsageAggregate>,
    model_context_usage: ModelContextUsageMap,
    model_costs: HashMap<String, f64>,
) -> Vec<ModelStats> {
    let provider_id = stats_provider_id(provider).to_string();
    let mut models = model_usage
        .into_iter()
        .map(
            |(
                model_key,
                (
                    message_count,
                    token_count,
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    reasoning_tokens,
                ),
            )| {
                let (model_name, service_tier) = split_model_usage_key(&model_key);
                let mut context_breakdown = model_context_usage
                    .get(&model_key)
                    .map(|buckets| buckets.values().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                context_breakdown.sort_by_key(|bucket| bucket.min_context_tokens);
                ModelStats {
                    provider_id: Some(provider_id.clone()),
                    model_name: model_name.to_string(),
                    service_tier: service_tier.map(str::to_string),
                    message_count,
                    token_count,
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    reasoning_tokens,
                    cost_usd: model_costs.get(&model_key).copied(),
                    context_breakdown,
                }
            },
        )
        .collect::<Vec<_>>();
    models.sort_by_key(|model| Reverse(model.token_count));
    models
}

fn merge_model_stats(
    model_usage: &mut HashMap<String, ModelUsageAggregate>,
    model_context_usage: &mut ModelContextUsageMap,
    model_costs: &mut HashMap<String, f64>,
    models: Vec<ModelStats>,
) {
    for model in models {
        let model_key = model_usage_key(&model.model_name, model.service_tier.as_deref());
        let entry = model_usage
            .entry(model_key.clone())
            .or_insert((0, 0, 0, 0, 0, 0, 0));
        entry.0 += model.message_count;
        entry.1 += model.token_count;
        entry.2 += model.input_tokens;
        entry.3 += model.output_tokens;
        entry.4 += model.cache_creation_tokens;
        entry.5 += model.cache_read_tokens;
        entry.6 += model.reasoning_tokens;
        if let Some(cost_usd) = model.cost_usd {
            *model_costs.entry(model_key.clone()).or_insert(0.0) += cost_usd;
        }
        if model.context_breakdown.is_empty() {
            let context_tier = context_tier_min_tokens(
                &model.model_name,
                model.input_tokens + model.cache_creation_tokens + model.cache_read_tokens,
            );
            let fallback = ModelContextStats {
                min_context_tokens: context_tier,
                token_count: model.token_count,
                input_tokens: model.input_tokens,
                output_tokens: model.output_tokens,
                cache_creation_tokens: model.cache_creation_tokens,
                cache_creation_tokens_5m: model.cache_creation_tokens,
                cache_creation_tokens_1h: 0,
                cache_read_tokens: model.cache_read_tokens,
                reasoning_tokens: model.reasoning_tokens,
            };
            model_context_usage
                .entry(model_key)
                .or_default()
                .entry(context_tier)
                .and_modify(|entry| {
                    entry.token_count += fallback.token_count;
                    entry.input_tokens += fallback.input_tokens;
                    entry.output_tokens += fallback.output_tokens;
                    entry.cache_creation_tokens += fallback.cache_creation_tokens;
                    entry.cache_creation_tokens_5m += fallback.cache_creation_tokens_5m;
                    entry.cache_read_tokens += fallback.cache_read_tokens;
                    entry.reasoning_tokens += fallback.reasoning_tokens;
                })
                .or_insert(fallback);
        } else {
            for context in model.context_breakdown {
                model_context_usage
                    .entry(model_key.clone())
                    .or_default()
                    .entry(context.min_context_tokens)
                    .and_modify(|entry| {
                        entry.token_count += context.token_count;
                        entry.input_tokens += context.input_tokens;
                        entry.output_tokens += context.output_tokens;
                        entry.cache_creation_tokens += context.cache_creation_tokens;
                        entry.cache_creation_tokens_5m += context.cache_creation_tokens_5m;
                        entry.cache_creation_tokens_1h += context.cache_creation_tokens_1h;
                        entry.cache_read_tokens += context.cache_read_tokens;
                        entry.reasoning_tokens += context.reasoning_tokens;
                    })
                    .or_insert(context);
            }
        }
    }
}

fn fallback_provider_name(provider: StatsProvider, path: &str) -> String {
    let raw = path.split_once("://").map(|(_, rest)| rest).unwrap_or(path);
    Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| stats_provider_id(provider))
        .to_string()
}

/// Resolve the display name for a provider project path.
fn resolve_provider_project_name(provider: StatsProvider, project_path: &str) -> String {
    match provider {
        StatsProvider::Claude => PathBuf::from(project_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string(),
        StatsProvider::Codebuddy => {
            if let Ok(projects) = providers::codebuddy::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            PathBuf::from(project_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string()
        }
        StatsProvider::Codex => {
            let cwd = project_path
                .strip_prefix("codex://")
                .unwrap_or(project_path);
            PathBuf::from(cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(cwd)
                .to_string()
        }
        StatsProvider::ForgeCode => {
            if let Ok(projects) = providers::forgecode::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            project_path
                .strip_prefix("forgecode://workspace/")
                .unwrap_or(project_path)
                .to_string()
        }
        StatsProvider::OpenCode => {
            if let Ok(projects) = providers::opencode::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            project_path
                .strip_prefix("opencode://")
                .unwrap_or(project_path)
                .to_string()
        }
        StatsProvider::Grok => {
            if let Ok(projects) = providers::grok::scan_projects() {
                if let Some(project) = projects
                    .into_iter()
                    .find(|p| grok_virtual_paths_match(&p.path, project_path))
                {
                    return project.name;
                }
            }
            project_path
                .strip_prefix("grok://")
                .and_then(|p| {
                    PathBuf::from(p).file_name().map(|n| {
                        let encoded = n.to_string_lossy().to_string();
                        let decoded = urlencoding::decode(&encoded)
                            .map(std::borrow::Cow::into_owned)
                            .unwrap_or_else(|_| encoded.clone());
                        Path::new(&decoded)
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .filter(|name| !name.is_empty())
                            .unwrap_or(encoded)
                    })
                })
                .unwrap_or_else(|| project_path.to_string())
        }
        StatsProvider::Kimi => {
            if let Ok(projects) = providers::kimi::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            project_path
                .strip_prefix("kimi://")
                .and_then(|p| {
                    PathBuf::from(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| project_path.to_string())
        }
        StatsProvider::Antigravity => {
            if let Ok(projects) = providers::antigravity::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            "Antigravity".to_string()
        }
        StatsProvider::Copilot => providers::copilot::scan_projects()
            .ok()
            .and_then(|projects| {
                projects
                    .into_iter()
                    .find(|project| project.path == project_path)
                    .map(|project| project.name)
            })
            .unwrap_or_else(|| "Copilot".to_string()),
        StatsProvider::Ompi | StatsProvider::Pi => {
            if let Ok(projects) = match provider {
                StatsProvider::Ompi => providers::ompi::scan_projects(),
                _ => providers::pi::scan_projects(),
            } {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            PathBuf::from(project_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string()
        }
        StatsProvider::Gemini => {
            if let Ok(projects) = providers::gemini::scan_projects() {
                if let Some(project) = projects.into_iter().find(|p| p.path == project_path) {
                    return project.name;
                }
            }
            project_path
                .strip_prefix("gemini://")
                .and_then(|p| {
                    PathBuf::from(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "Gemini".to_string())
        }
        StatsProvider::Cursor => {
            if let Ok(projects) = providers::cursor::scan_projects() {
                if let Some(project) = projects
                    .into_iter()
                    .find(|p| cursor_virtual_paths_match(&p.path, project_path))
                {
                    return project.name;
                }
            }
            if let Some(name) = providers::cursor::display_name_for_project_path(project_path) {
                return name;
            }
            project_path
                .strip_prefix("cursor://")
                .and_then(|p| {
                    PathBuf::from(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "Cursor".to_string())
        }
        StatsProvider::Aider
        | StatsProvider::AmazonQ
        | StatsProvider::Cline
        | StatsProvider::Continue
        | StatsProvider::OpenHands
        | StatsProvider::OpenInterpreter
        | StatsProvider::PearAI
        | StatsProvider::Qwen
        | StatsProvider::Trae
        | StatsProvider::Vibe
        | StatsProvider::Zed
        | StatsProvider::Crush
        | StatsProvider::CursorAgent
        | StatsProvider::Goose
        | StatsProvider::Kiro
        | StatsProvider::Llm => fallback_provider_name(provider, project_path),
    }
}

/// Resolve the display name for a provider session path.
fn resolve_provider_project_name_from_session(
    provider: StatsProvider,
    session_path: &str,
) -> String {
    match provider {
        StatsProvider::ForgeCode => {
            let workspace_id = session_path
                .strip_prefix("forgecode-db://workspace/")
                .or_else(|| session_path.strip_prefix("forgecode://workspace/"))
                .and_then(|rest| rest.split("/conversation/").next())
                .unwrap_or("unknown");
            let project_path = format!("forgecode://workspace/{workspace_id}");
            resolve_provider_project_name(provider, &project_path)
        }
        StatsProvider::OpenCode => {
            let project_part = session_path
                .strip_prefix("opencode://")
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("unknown");
            let project_path = format!("opencode://{project_part}");
            resolve_provider_project_name(provider, &project_path)
        }
        StatsProvider::Codebuddy => {
            if let Ok(projects) = providers::codebuddy::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::codebuddy::load_sessions(&project.path, false)
                    {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "codebuddy".to_string()
        }
        StatsProvider::Codex => {
            if let Ok(projects) = providers::codex::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::codex::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "codex".to_string()
        }
        StatsProvider::Grok => {
            if let Some(project_dir) = Path::new(session_path).parent() {
                let project_path = format!("grok://{}", project_dir.to_string_lossy());
                return resolve_provider_project_name(provider, &project_path);
            }
            "grok".to_string()
        }
        StatsProvider::Kimi => {
            if let Some(project_dir) = Path::new(session_path).parent() {
                let project_path = format!("kimi://{}", project_dir.to_string_lossy());
                return resolve_provider_project_name(provider, &project_path);
            }
            "kimi".to_string()
        }
        StatsProvider::Antigravity => "Antigravity".to_string(),
        StatsProvider::Copilot => {
            if let Ok(projects) = providers::copilot::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::copilot::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "Copilot".to_string()
        }
        StatsProvider::Ompi => {
            if let Ok(projects) = providers::ompi::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::ompi::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "oh-my-pi".to_string()
        }
        StatsProvider::Pi => {
            if let Ok(projects) = providers::pi::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::pi::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "Pi".to_string()
        }
        StatsProvider::Gemini => {
            if let Ok(projects) = providers::gemini::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::gemini::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "Gemini".to_string()
        }
        StatsProvider::Cursor => {
            if let Ok(projects) = providers::cursor::scan_projects() {
                for project in projects {
                    if let Ok(sessions) = providers::cursor::load_sessions(&project.path, false) {
                        if sessions.iter().any(|s| s.file_path == session_path) {
                            return project.name;
                        }
                    }
                }
            }
            "Cursor".to_string()
        }
        StatsProvider::Aider
        | StatsProvider::AmazonQ
        | StatsProvider::Cline
        | StatsProvider::Continue
        | StatsProvider::OpenHands
        | StatsProvider::OpenInterpreter
        | StatsProvider::PearAI
        | StatsProvider::Qwen
        | StatsProvider::Trae
        | StatsProvider::Vibe
        | StatsProvider::Zed
        | StatsProvider::Crush
        | StatsProvider::CursorAgent
        | StatsProvider::Goose
        | StatsProvider::Kiro
        | StatsProvider::Llm => fallback_provider_name(provider, session_path),
        StatsProvider::Claude => "unknown".to_string(),
    }
}

/// Load sessions for a provider-specific stats request.
fn load_provider_sessions_for_stats(
    provider: StatsProvider,
    project_path: &str,
) -> Result<Vec<crate::models::ClaudeSession>, String> {
    load_stats_sessions(provider, project_path)
}

/// Load messages for a provider-specific stats request.
fn load_provider_messages_for_stats(
    provider: StatsProvider,
    session: &crate::models::ClaudeSession,
) -> Result<Vec<ClaudeMessage>, String> {
    load_stats_messages(provider, &session.file_path)
}

struct SessionTokenStatsOptions {
    provider: StatsProvider,
    session_id: String,
    project_name: String,
    summary: Option<String>,
    mode: StatsMode,
    start_date: Option<DateTime<Utc>>,
    end_date: Option<DateTime<Utc>>,
}

/// Build session token stats from normalized provider messages.
fn build_session_token_stats_from_messages(
    options: SessionTokenStatsOptions,
    messages: &[ClaudeMessage],
) -> Option<SessionTokenStats> {
    let SessionTokenStatsOptions {
        provider,
        session_id,
        project_name,
        summary,
        mode,
        start_date,
        end_date,
    } = options;
    let s_limit = start_date.as_ref();
    let e_limit = end_date.as_ref();
    if messages.is_empty() {
        return None;
    }

    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_cache_creation_tokens = 0u64;
    let mut total_cache_read_tokens = 0u64;
    let mut total_reasoning_tokens = 0u64;
    let mut tool_usage: HashMap<String, (u32, u32)> = HashMap::new();
    // #283: only add usage once per (session_id, message.id).
    let mut seen_usage_keys: HashSet<String> = HashSet::with_capacity(messages.len());
    let mut seen_cost_keys: HashSet<String> = HashSet::with_capacity(messages.len());
    let mut model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut model_costs: HashMap<String, f64> = HashMap::new();

    let mut first_time: Option<DateTime<Utc>> = None;
    let mut last_time: Option<DateTime<Utc>> = None;
    let mut first_time_raw: Option<String> = None;
    let mut last_time_raw: Option<String> = None;
    let mut included_message_count = 0usize;

    for message in messages {
        let parsed_timestamp = parse_timestamp_utc(&message.timestamp);
        if !is_within_date_limits(parsed_timestamp, s_limit, e_limit) {
            continue;
        }

        if !should_include_stats_message(message, mode) {
            continue;
        }

        let usage = extract_token_usage(message);
        included_message_count += 1;
        let (
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens,
            tokens,
        ) = dedup_token_totals_msg(&mut seen_usage_keys, message, &usage);
        let deduped_source_cost = dedup_source_cost(
            &mut seen_cost_keys,
            &message.session_id,
            message.message_id.as_deref(),
            &message.uuid,
            message.cost_usd,
        );
        let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
        if message.model.is_some() || tokens > 0 || deduped_source_cost.is_some() {
            accumulate_model_usage(
                &mut model_usage,
                &mut model_context_usage,
                &mut model_costs,
                ModelUsageUpdate {
                    model_name,
                    service_tier: usage.service_tier.as_deref(),
                    totals: (
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                        reasoning_tokens,
                        tokens,
                    ),
                    cache_creation_tokens_1h: u64::from(
                        usage.cache_creation_input_tokens_1h.unwrap_or(0),
                    ),
                    source_cost: deduped_source_cost,
                },
            );
        }
        total_input_tokens += input_tokens;
        total_output_tokens += output_tokens;
        total_cache_creation_tokens += cache_creation_tokens;
        total_cache_read_tokens += cache_read_tokens;
        total_reasoning_tokens += reasoning_tokens;

        if let Some(ts) = parsed_timestamp {
            if first_time.map_or(true, |current| ts < current) {
                first_time = Some(ts);
                first_time_raw = Some(message.timestamp.clone());
            }
            if last_time.map_or(true, |current| ts > current) {
                last_time = Some(ts);
                last_time_raw = Some(message.timestamp.clone());
            }
        }

        track_tool_usage(message, &mut tool_usage);
    }

    let total_tokens = total_input_tokens
        + total_output_tokens
        + total_cache_creation_tokens
        + total_cache_read_tokens
        + total_reasoning_tokens;
    if included_message_count == 0 {
        return None;
    }

    Some(SessionTokenStats {
        session_id,
        project_name,
        total_input_tokens,
        total_output_tokens,
        total_cache_creation_tokens,
        total_cache_read_tokens,
        total_reasoning_tokens,
        total_tokens,
        message_count: included_message_count,
        first_message_time: first_time_raw.unwrap_or_else(|| "unknown".to_string()),
        last_message_time: last_time_raw.unwrap_or_else(|| "unknown".to_string()),
        summary,
        most_used_tools: build_tool_usage_stats(tool_usage),
        model_distribution: build_model_stats(
            provider,
            model_usage,
            model_context_usage,
            model_costs,
        ),
    })
}

/// Build paginated project token stats for a non-Claude provider.
fn get_provider_project_token_stats(
    provider: StatsProvider,
    project_path: &str,
    offset: usize,
    limit: usize,
    start_date: Option<String>,
    end_date: Option<String>,
    mode: StatsMode,
) -> Result<PaginatedTokenStats, String> {
    if provider == StatsProvider::Antigravity && !is_antigravity_cli_path(project_path) {
        let sessions = load_provider_sessions_for_stats(provider, project_path)?;
        let s_limit = parse_date_limit(start_date, "start_date");
        let e_limit = parse_date_limit(end_date, "end_date");
        let mut all_stats = Vec::new();

        for session in &sessions {
            if let Some((stats, _records)) = build_antigravity_session_token_stats(
                session,
                mode,
                s_limit.as_ref(),
                e_limit.as_ref(),
            )? {
                all_stats.push(stats);
            }
        }

        let total_count = all_stats.len();
        all_stats.sort_by_key(|s| std::cmp::Reverse(s.total_tokens));
        let items = all_stats
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let has_more = offset + items.len() < total_count;

        return Ok(PaginatedTokenStats {
            items,
            total_count,
            offset,
            limit,
            has_more,
        });
    }

    let project_name = resolve_provider_project_name(provider, project_path);
    let mut all_stats = Vec::new();
    let sessions = load_provider_sessions_for_stats(provider, project_path)?;
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    for session in &sessions {
        let messages = load_provider_messages_for_stats(provider, session)?;
        if let Some(stats) = build_session_token_stats_from_messages(
            SessionTokenStatsOptions {
                provider,
                session_id: session.actual_session_id.clone(),
                project_name: if session.project_name.is_empty() {
                    project_name.clone()
                } else {
                    session.project_name.clone()
                },
                summary: session.summary.clone(),
                mode,
                start_date: s_limit,
                end_date: e_limit,
            },
            &messages,
        ) {
            all_stats.push(stats);
        }
    }

    let total_count = all_stats.len();
    all_stats.sort_by_key(|stats| Reverse(stats.total_tokens));
    let items = all_stats
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let has_more = offset + items.len() < total_count;

    Ok(PaginatedTokenStats {
        items,
        total_count,
        offset,
        limit,
        has_more,
    })
}

/// Build a project stats summary for a non-Claude provider.
fn get_provider_project_stats_summary(
    provider: StatsProvider,
    project_path: &str,
    start_date: Option<String>,
    end_date: Option<String>,
    mode: StatsMode,
) -> Result<ProjectStatsSummary, String> {
    if provider == StatsProvider::Antigravity && !is_antigravity_cli_path(project_path) {
        let sessions = load_provider_sessions_for_stats(provider, project_path)?;
        let s_limit = parse_date_limit(start_date, "start_date");
        let e_limit = parse_date_limit(end_date, "end_date");

        let mut summary = ProjectStatsSummary::default();
        summary.project_name = resolve_provider_project_name(provider, project_path);

        let mut session_durations = Vec::new();
        let mut tool_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
        let mut daily_stats_map: HashMap<String, DailyStats> = HashMap::new();
        let mut activity_map: HashMap<(u8, u8), (u32, u64)> = HashMap::new();
        let mut project_model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
        let mut project_model_context_usage: ModelContextUsageMap = HashMap::new();
        let mut project_model_costs: HashMap<String, f64> = HashMap::new();

        for session in &sessions {
            let Some((session_stats, records)) = build_antigravity_session_token_stats(
                session,
                mode,
                s_limit.as_ref(),
                e_limit.as_ref(),
            )?
            else {
                continue;
            };

            summary.total_sessions += 1;
            summary.total_messages += session_stats.message_count;
            summary.total_tokens += session_stats.total_tokens;
            summary.token_distribution.input += session_stats.total_input_tokens;
            summary.token_distribution.output += session_stats.total_output_tokens;
            summary.token_distribution.cache_creation += session_stats.total_cache_creation_tokens;
            summary.token_distribution.cache_read += session_stats.total_cache_read_tokens;
            summary.token_distribution.reasoning += session_stats.total_reasoning_tokens;
            merge_model_stats(
                &mut project_model_usage,
                &mut project_model_context_usage,
                &mut project_model_costs,
                session_stats.model_distribution,
            );

            if let Ok(messages) = providers::antigravity::load_messages(&session.file_path) {
                track_antigravity_tool_usage(
                    &messages,
                    s_limit.as_ref(),
                    e_limit.as_ref(),
                    &mut tool_usage_map,
                );
            }

            let mut timestamps = records
                .iter()
                .map(|record| record.timestamp)
                .collect::<Vec<_>>();
            let duration = calculate_session_active_minutes(&mut timestamps);
            if duration > 0 {
                session_durations.push(duration);
            }

            let mut session_dates = HashSet::new();
            for record in records {
                let (mode_input_tokens, mode_output_tokens, mode_total_tokens) = match mode {
                    StatsMode::ConversationOnly => {
                        let input_tokens = record.conversation_input_tokens;
                        let output_tokens = record.output_tokens;
                        let total_tokens = input_tokens
                            + output_tokens
                            + record.conversation_cache_creation_tokens
                            + record.conversation_cache_read_tokens
                            + record.reasoning_tokens;
                        (input_tokens, output_tokens, total_tokens)
                    }
                    StatsMode::BillingTotal => (
                        record.input_tokens,
                        record.output_tokens,
                        record.total_tokens,
                    ),
                };
                let hour = record.timestamp.hour() as u8;
                let day = record.timestamp.weekday().num_days_from_sunday() as u8;
                let date = record.timestamp.format("%Y-%m-%d").to_string();
                session_dates.insert(date.clone());

                let activity_entry = activity_map.entry((hour, day)).or_insert((0, 0));
                activity_entry.0 += 1;
                activity_entry.1 += mode_total_tokens;

                let daily_entry =
                    daily_stats_map
                        .entry(date.clone())
                        .or_insert_with(|| DailyStats {
                            date,
                            ..Default::default()
                        });
                daily_entry.total_tokens += mode_total_tokens;
                daily_entry.input_tokens += mode_input_tokens;
                daily_entry.output_tokens += mode_output_tokens;
                daily_entry.message_count += 1;
            }

            for date in session_dates {
                let entry = daily_stats_map
                    .entry(date.clone())
                    .or_insert_with(|| DailyStats {
                        date,
                        ..Default::default()
                    });
                entry.session_count += 1;
            }
        }

        for daily_stat in daily_stats_map.values_mut() {
            daily_stat.active_hours = if daily_stat.message_count > 0 {
                std::cmp::min(24, std::cmp::max(1, daily_stat.message_count / 10))
            } else {
                0
            };
        }

        summary.daily_stats = daily_stats_map.into_values().collect();
        summary.daily_stats.sort_by(|a, b| a.date.cmp(&b.date));
        summary.most_used_tools = build_tool_usage_stats(tool_usage_map);
        summary.model_distribution = build_model_stats(
            StatsProvider::Antigravity,
            project_model_usage,
            project_model_context_usage,
            project_model_costs,
        );
        summary.activity_heatmap = activity_map
            .into_iter()
            .map(|((hour, day), (count, tokens))| ActivityHeatmap {
                hour,
                day,
                activity_count: count,
                tokens_used: tokens,
            })
            .collect();
        summary.avg_tokens_per_session = if summary.total_sessions > 0 {
            summary.total_tokens / summary.total_sessions as u64
        } else {
            0
        };
        summary.total_session_duration = session_durations.iter().sum::<u32>();
        summary.avg_session_duration = if session_durations.is_empty() {
            0
        } else {
            summary.total_session_duration / session_durations.len() as u32
        };
        summary.most_active_hour = summary
            .activity_heatmap
            .iter()
            .max_by_key(|item| item.activity_count)
            .map(|item| item.hour)
            .unwrap_or(0);

        return Ok(summary);
    }

    let project_name = resolve_provider_project_name(provider, project_path);
    let sessions = load_provider_sessions_for_stats(provider, project_path)?;
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    let mut summary = ProjectStatsSummary::default();
    summary.project_name = project_name;

    let mut session_durations: Vec<u32> = Vec::new();
    let mut tool_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut project_model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut project_model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut project_model_costs: HashMap<String, f64> = HashMap::new();
    let mut daily_stats_map: HashMap<String, DailyStats> = HashMap::new();
    let mut activity_map: HashMap<(u8, u8), (u32, u64)> = HashMap::new();

    for session in &sessions {
        let messages = load_provider_messages_for_stats(provider, session)?;
        if messages.is_empty() {
            continue;
        }

        let mut included_messages = 0usize;
        let mut parsed_timestamps = Vec::new();
        let mut session_dates = HashSet::new();
        // #283: per-session dedup
        let mut seen_usage_keys: HashSet<String> = HashSet::with_capacity(messages.len());
        let mut seen_cost_keys: HashSet<String> = HashSet::with_capacity(messages.len());

        for message in &messages {
            if !should_include_stats_message(message, mode) {
                continue;
            }

            let usage = extract_token_usage(message);

            // Per-message date filtering
            let parsed_ts = parse_timestamp_utc(&message.timestamp);
            if !is_within_date_limits(parsed_ts, s_limit.as_ref(), e_limit.as_ref()) {
                continue;
            }

            included_messages += 1;

            let (
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                reasoning_tokens,
                total_tokens,
            ) = dedup_token_totals_msg(&mut seen_usage_keys, message, &usage);
            let deduped_source_cost = dedup_source_cost(
                &mut seen_cost_keys,
                &message.session_id,
                message.message_id.as_deref(),
                &message.uuid,
                message.cost_usd,
            );
            let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
            if message.model.is_some() || total_tokens > 0 || deduped_source_cost.is_some() {
                accumulate_model_usage(
                    &mut project_model_usage,
                    &mut project_model_context_usage,
                    &mut project_model_costs,
                    ModelUsageUpdate {
                        model_name,
                        service_tier: usage.service_tier.as_deref(),
                        totals: (
                            input_tokens,
                            output_tokens,
                            cache_creation_tokens,
                            cache_read_tokens,
                            reasoning_tokens,
                            total_tokens,
                        ),
                        cache_creation_tokens_1h: u64::from(
                            usage.cache_creation_input_tokens_1h.unwrap_or(0),
                        ),
                        source_cost: deduped_source_cost,
                    },
                );
            }

            summary.token_distribution.input += input_tokens;
            summary.token_distribution.output += output_tokens;
            summary.token_distribution.cache_creation += cache_creation_tokens;
            summary.token_distribution.cache_read += cache_read_tokens;
            summary.token_distribution.reasoning += reasoning_tokens;

            if let Some(timestamp) = parsed_ts {
                parsed_timestamps.push(timestamp);
                let hour = timestamp.hour() as u8;
                let day = timestamp.weekday().num_days_from_sunday() as u8;
                let date = timestamp.format("%Y-%m-%d").to_string();
                session_dates.insert(date.clone());

                let activity_entry = activity_map.entry((hour, day)).or_insert((0, 0));
                activity_entry.0 += 1;
                activity_entry.1 += total_tokens;

                let daily_entry =
                    daily_stats_map
                        .entry(date.clone())
                        .or_insert_with(|| DailyStats {
                            date,
                            ..Default::default()
                        });
                daily_entry.total_tokens += total_tokens;
                daily_entry.input_tokens += input_tokens;
                daily_entry.output_tokens += output_tokens;
                daily_entry.message_count += 1;
            }

            track_tool_usage(message, &mut tool_usage_map);
        }

        if included_messages == 0 {
            continue;
        }

        summary.total_sessions += 1;
        summary.total_messages += included_messages;

        for date in session_dates {
            let entry = daily_stats_map
                .entry(date.clone())
                .or_insert_with(|| DailyStats {
                    date,
                    ..Default::default()
                });
            entry.session_count += 1;
        }

        let duration = calculate_session_active_minutes(&mut parsed_timestamps);
        if duration > 0 {
            session_durations.push(duration);
        }
    }

    for daily_stat in daily_stats_map.values_mut() {
        daily_stat.active_hours = if daily_stat.message_count > 0 {
            std::cmp::min(24, std::cmp::max(1, daily_stat.message_count / 10))
        } else {
            0
        };
    }

    summary.most_used_tools = build_tool_usage_stats(tool_usage_map);
    summary.model_distribution = build_model_stats(
        provider,
        project_model_usage,
        project_model_context_usage,
        project_model_costs,
    );
    summary.daily_stats = daily_stats_map.into_values().collect();
    summary.daily_stats.sort_by(|a, b| a.date.cmp(&b.date));
    summary.activity_heatmap = activity_map
        .into_iter()
        .map(|((hour, day), (count, tokens))| ActivityHeatmap {
            hour,
            day,
            activity_count: count,
            tokens_used: tokens,
        })
        .collect();

    summary.total_tokens = summary.token_distribution.input
        + summary.token_distribution.output
        + summary.token_distribution.cache_creation
        + summary.token_distribution.cache_read
        + summary.token_distribution.reasoning;
    summary.avg_tokens_per_session = if summary.total_sessions > 0 {
        summary.total_tokens / summary.total_sessions as u64
    } else {
        0
    };
    summary.total_session_duration = session_durations.iter().sum::<u32>();
    summary.avg_session_duration = if session_durations.is_empty() {
        0
    } else {
        summary.total_session_duration / session_durations.len() as u32
    };
    summary.most_active_hour = summary
        .activity_heatmap
        .iter()
        .max_by_key(|a| a.activity_count)
        .map_or(0, |a| a.hour);

    Ok(summary)
}

/// Build session comparison stats for a non-Claude provider.
fn get_provider_session_comparison(
    provider: StatsProvider,
    session_id: &str,
    project_path: &str,
    mode: StatsMode,
    start_date: Option<String>,
    end_date: Option<String>,
) -> Result<SessionComparison, String> {
    if provider == StatsProvider::Antigravity && !is_antigravity_cli_path(project_path) {
        let sessions = load_provider_sessions_for_stats(provider, project_path)?;
        let s_limit = parse_date_limit(start_date, "start_date");
        let e_limit = parse_date_limit(end_date, "end_date");
        let mut all_sessions: Vec<SessionComparisonStats> = Vec::new();

        for session in &sessions {
            let Some((stats, _records)) = build_antigravity_session_token_stats(
                session,
                mode,
                s_limit.as_ref(),
                e_limit.as_ref(),
            )?
            else {
                continue;
            };

            let duration_seconds = match (
                parse_timestamp_utc(&stats.first_message_time),
                parse_timestamp_utc(&stats.last_message_time),
            ) {
                (Some(first), Some(last)) => (last - first).num_seconds(),
                _ => 0,
            };

            all_sessions.push(SessionComparisonStats {
                session_id: session.actual_session_id.clone(),
                total_tokens: stats.total_tokens,
                message_count: stats.message_count,
                duration_seconds,
            });
        }

        let target_session = all_sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .ok_or("Session not found in project")?;

        let total_project_tokens: u64 = all_sessions
            .iter()
            .map(|session| session.total_tokens)
            .sum();
        let total_project_messages: usize = all_sessions
            .iter()
            .map(|session| session.message_count)
            .sum();

        let percentage_of_project_tokens = if total_project_tokens > 0 {
            (target_session.total_tokens as f32 / total_project_tokens as f32) * 100.0
        } else {
            0.0
        };

        let percentage_of_project_messages = if total_project_messages > 0 {
            (target_session.message_count as f32 / total_project_messages as f32) * 100.0
        } else {
            0.0
        };

        let mut sessions_by_tokens = all_sessions.clone();
        sessions_by_tokens.sort_by_key(|s| std::cmp::Reverse(s.total_tokens));
        let rank_by_tokens = sessions_by_tokens
            .iter()
            .position(|session| session.session_id == session_id)
            .unwrap_or(0)
            + 1;

        let mut sessions_by_duration = all_sessions.clone();
        sessions_by_duration.sort_by_key(|s| std::cmp::Reverse(s.duration_seconds));
        let rank_by_duration = sessions_by_duration
            .iter()
            .position(|session| session.session_id == session_id)
            .unwrap_or(0)
            + 1;

        let avg_tokens = if all_sessions.is_empty() {
            0
        } else {
            total_project_tokens / all_sessions.len() as u64
        };

        return Ok(SessionComparison {
            session_id: session_id.to_string(),
            percentage_of_project_tokens,
            percentage_of_project_messages,
            rank_by_tokens,
            rank_by_duration,
            is_above_average: target_session.total_tokens > avg_tokens,
        });
    }

    let sessions = load_provider_sessions_for_stats(provider, project_path)?;
    let mut all_sessions: Vec<SessionComparisonStats> = Vec::new();
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    for session in &sessions {
        let messages = load_provider_messages_for_stats(provider, session)?;
        if messages.is_empty() {
            continue;
        }

        let mut total_tokens: u64 = 0;
        let mut included_message_count = 0usize;
        let mut first_time: Option<DateTime<Utc>> = None;
        let mut last_time: Option<DateTime<Utc>> = None;
        // #283: dedup token usage so each session's `total_tokens` reflects unique
        // assistant turns. `included_message_count` stays per-row (rows displayed)
        // — tokens-per-message in the UI is "tokens per displayed row", not per turn.
        let mut seen_usage_keys: HashSet<String> = HashSet::with_capacity(messages.len());

        for message in &messages {
            if !should_include_stats_message(message, mode) {
                continue;
            }

            let usage = extract_token_usage(message);

            // Per-message date filtering
            let parsed_ts = parse_timestamp_utc(&message.timestamp);
            if !is_within_date_limits(parsed_ts, s_limit.as_ref(), e_limit.as_ref()) {
                continue;
            }

            included_message_count += 1;
            let (_, _, _, _, _, tokens) =
                dedup_token_totals_msg(&mut seen_usage_keys, message, &usage);
            total_tokens += tokens;

            if let Some(ts) = parsed_ts {
                if first_time.map_or(true, |current| ts < current) {
                    first_time = Some(ts);
                }
                if last_time.map_or(true, |current| ts > current) {
                    last_time = Some(ts);
                }
            }
        }
        if included_message_count == 0 {
            continue;
        }

        let duration_seconds = match (first_time.as_ref(), last_time.as_ref()) {
            (Some(first), Some(last)) => (*last - *first).num_seconds(),
            _ => 0,
        };

        all_sessions.push(SessionComparisonStats {
            session_id: session.actual_session_id.clone(),
            total_tokens,
            message_count: included_message_count,
            duration_seconds,
        });
    }

    let target_session = all_sessions
        .iter()
        .find(|s| s.session_id == session_id)
        .ok_or("Session not found in project")?;

    let total_project_tokens: u64 = all_sessions.iter().map(|s| s.total_tokens).sum();
    let total_project_messages: usize = all_sessions.iter().map(|s| s.message_count).sum();

    let percentage_of_project_tokens = if total_project_tokens > 0 {
        (target_session.total_tokens as f32 / total_project_tokens as f32) * 100.0
    } else {
        0.0
    };

    let percentage_of_project_messages = if total_project_messages > 0 {
        (target_session.message_count as f32 / total_project_messages as f32) * 100.0
    } else {
        0.0
    };

    let mut sessions_by_tokens = all_sessions.clone();
    sessions_by_tokens.sort_by_key(|stats| Reverse(stats.total_tokens));
    let rank_by_tokens = sessions_by_tokens
        .iter()
        .position(|s| s.session_id == session_id)
        .unwrap_or(0)
        + 1;

    let mut sessions_by_duration = all_sessions.clone();
    sessions_by_duration.sort_by_key(|stats| Reverse(stats.duration_seconds));
    let rank_by_duration = sessions_by_duration
        .iter()
        .position(|s| s.session_id == session_id)
        .unwrap_or(0)
        + 1;

    let avg_tokens = if all_sessions.is_empty() {
        0
    } else {
        total_project_tokens / all_sessions.len() as u64
    };
    let is_above_average = target_session.total_tokens > avg_tokens;

    Ok(SessionComparison {
        session_id: session_id.to_string(),
        percentage_of_project_tokens,
        percentage_of_project_messages,
        rank_by_tokens,
        rank_by_duration,
        is_above_average,
    })
}

#[tauri::command]
/// Return token stats for a single session.
pub async fn get_session_token_stats(
    session_path: String,
    start_date: Option<String>,
    end_date: Option<String>,
    stats_mode: Option<String>,
) -> Result<SessionTokenStats, String> {
    let start = std::time::Instant::now();
    let mode = parse_stats_mode(stats_mode);
    let provider = detect_session_provider(&session_path);
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    if provider != StatsProvider::Claude {
        if provider == StatsProvider::Antigravity && !is_antigravity_cli_path(&session_path) {
            let session_dir = PathBuf::from(&session_path);
            let session_id = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "Invalid antigravity session path".to_string())?
                .to_string();
            let project_root = session_dir
                .parent()
                .map(|parent| parent.to_string_lossy().to_string())
                .unwrap_or_else(|| session_path.clone());
            let sessions = load_provider_sessions_for_stats(provider, &project_root)?;
            let session = sessions
                .iter()
                .find(|candidate| candidate.actual_session_id == session_id)
                .ok_or_else(|| "Session not found".to_string())?;

            return build_antigravity_session_token_stats(
                session,
                mode,
                s_limit.as_ref(),
                e_limit.as_ref(),
            )?
            .map(|(stats, _records)| stats)
            .ok_or_else(|| "No valid messages found in session".to_string());
        }

        let messages = load_stats_messages(provider, &session_path)?;

        let session_id = messages
            .first()
            .map(|msg| msg.session_id.clone())
            .unwrap_or_else(|| session_path.clone());
        let project_name = resolve_provider_project_name_from_session(provider, &session_path);

        return build_session_token_stats_from_messages(
            SessionTokenStatsOptions {
                provider,
                session_id,
                project_name,
                summary: None,
                mode,
                start_date: s_limit,
                end_date: e_limit,
            },
            &messages,
        )
        .filter(|stats| {
            is_within_date_limits(
                parse_timestamp_utc(&stats.last_message_time),
                s_limit.as_ref(),
                e_limit.as_ref(),
            )
        })
        .ok_or_else(|| "No valid messages found in session".to_string());
    }

    let session_path_buf = PathBuf::from(&session_path);
    let stats = extract_session_token_stats_sync(
        &session_path_buf,
        mode,
        s_limit.as_ref(),
        e_limit.as_ref(),
    )
    .ok_or_else(|| "No valid messages found in session".to_string())?;
    if !is_within_date_limits(
        parse_timestamp_utc(&stats.last_message_time),
        s_limit.as_ref(),
        e_limit.as_ref(),
    ) {
        return Err("No valid messages found in session".to_string());
    }
    let total_time = start.elapsed();

    log::debug!(
        "get_session_token_stats: {} messages, total={}ms",
        stats.message_count,
        total_time.as_millis()
    );

    Ok(stats)
}

/// Paginated response for project token stats
#[derive(Debug, Clone, serde::Serialize)]
pub struct PaginatedTokenStats {
    pub items: Vec<SessionTokenStats>,
    pub total_count: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

/// Extract session token stats from a Claude session file synchronously.
/// Served from the per-file daily-aggregate cache when possible; falls back
/// to the full scan (see the `cache` module design note).
fn extract_session_token_stats_sync(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionTokenStats> {
    if let Some(aggregate) = cache::message_stats_cache().get_or_build(session_path, mode, || {
        cache::build_message_file_aggregate(session_path, mode)
    }) {
        if let cache::Composed::Ready(stats) = cache::compose_session_token(
            &aggregate,
            claude_session_project_name(session_path),
            s_limit,
            e_limit,
        ) {
            return stats;
        }
    }
    scan_session_token_stats(session_path, mode, s_limit, e_limit)
}

/// Synchronous version of session token stats extraction for parallel processing
#[allow(unsafe_code)] // Required for mmap performance optimization
/// Full-scan path for session token stats (cache miss / non-composable filter).
fn scan_session_token_stats(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionTokenStats> {
    let file = fs::File::open(session_path).ok()?;

    // SAFETY: We're only reading the file, and the file handle is kept open
    // for the duration of the mmap's lifetime. Session files are append-only.
    let mmap = unsafe { Mmap::map(&file) }.ok()?;

    let project_name = claude_session_project_name(session_path);

    let mut session_id: Option<String> = None;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_cache_creation_tokens = 0u64;
    let mut total_cache_read_tokens = 0u64;
    let mut total_reasoning_tokens = 0u64;
    let mut message_count = 0usize;
    let mut first_time: Option<String> = None;
    let mut last_time: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut tool_usage: HashMap<String, (u32, u32)> = HashMap::new();
    let mut model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut model_costs: HashMap<String, f64> = HashMap::new();
    let mut included_message_count = 0usize;

    // Use SIMD-accelerated line detection
    let line_ranges = find_line_ranges(&mmap);

    // #283: stream entries with owned-key dedup (no per-file Vec buffering).
    let mut seen_usage_keys: HashSet<String> = HashSet::new();
    let mut seen_cost_keys: HashSet<String> = HashSet::new();

    for (start, end) in line_ranges {
        let mut line_bytes = mmap[start..end].to_vec();
        let Some(log_entry) = parse_raw_log_entry_simd(&mut line_bytes) else {
            continue;
        };
        // Capture summary text before consuming log_entry into ClaudeMessage.
        if log_entry.message_type == "summary" {
            if let Some(s) = &log_entry.summary {
                summary = Some(s.clone());
            }
        }
        let Ok(message) = ClaudeMessage::try_from(log_entry) else {
            continue;
        };

        let parsed_timestamp = parse_timestamp_utc(&message.timestamp);
        if !is_within_date_limits(parsed_timestamp, s_limit, e_limit) {
            continue;
        }

        let usage = extract_token_usage(&message);
        let has_usage = token_usage_has_token_fields(&usage);
        if !should_include_stats_entry(&message.message_type, message.is_sidechain, has_usage, mode)
        {
            continue;
        }

        if session_id.is_none() {
            session_id = Some(message.session_id.clone());
        }

        message_count += 1;
        included_message_count += 1;

        let (
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            reasoning_tokens,
            tokens,
        ) = dedup_token_totals_msg(&mut seen_usage_keys, &message, &usage);
        let deduped_source_cost = dedup_source_cost(
            &mut seen_cost_keys,
            &message.session_id,
            message.message_id.as_deref(),
            &message.uuid,
            message.cost_usd,
        );
        let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
        if message.model.is_some() || tokens > 0 || deduped_source_cost.is_some() {
            accumulate_model_usage(
                &mut model_usage,
                &mut model_context_usage,
                &mut model_costs,
                ModelUsageUpdate {
                    model_name,
                    service_tier: usage.service_tier.as_deref(),
                    totals: (
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                        reasoning_tokens,
                        tokens,
                    ),
                    cache_creation_tokens_1h: u64::from(
                        usage.cache_creation_input_tokens_1h.unwrap_or(0),
                    ),
                    source_cost: deduped_source_cost,
                },
            );
        }
        total_input_tokens += input_tokens;
        total_output_tokens += output_tokens;
        total_cache_creation_tokens += cache_creation_tokens;
        total_cache_read_tokens += cache_read_tokens;
        total_reasoning_tokens += reasoning_tokens;

        if let Some(ts) = parsed_timestamp {
            let should_set_first = first_time
                .as_ref()
                .and_then(|raw| parse_timestamp_utc(raw))
                .map_or(true, |current| ts < current);
            if should_set_first {
                first_time = Some(message.timestamp.clone());
            }

            let should_set_last = last_time
                .as_ref()
                .and_then(|raw| parse_timestamp_utc(raw))
                .map_or(true, |current| ts > current);
            if should_set_last {
                last_time = Some(message.timestamp.clone());
            }
        }

        // Track tool usage
        track_tool_usage(&message, &mut tool_usage);
    }

    let session_id = session_id?;
    if message_count == 0 || included_message_count == 0 {
        return None;
    }

    let total_tokens = total_input_tokens
        + total_output_tokens
        + total_cache_creation_tokens
        + total_cache_read_tokens
        + total_reasoning_tokens;

    Some(SessionTokenStats {
        session_id,
        project_name,
        total_input_tokens,
        total_output_tokens,
        total_cache_creation_tokens,
        total_cache_read_tokens,
        total_reasoning_tokens,
        total_tokens,
        message_count: included_message_count,
        first_message_time: first_time.unwrap_or_else(|| "unknown".to_string()),
        last_message_time: last_time.unwrap_or_else(|| "unknown".to_string()),
        summary,
        model_distribution: build_model_stats(
            StatsProvider::Claude,
            model_usage,
            model_context_usage,
            model_costs,
        ),
        most_used_tools: tool_usage
            .into_iter()
            .map(|(name, (usage, success))| ToolUsageStats {
                tool_name: name,
                usage_count: usage,
                success_rate: if usage > 0 {
                    (success as f32 / usage as f32) * 100.0
                } else {
                    0.0
                },
                avg_execution_time: None,
            })
            .collect(),
    })
}

#[tauri::command]
/// Return paginated token stats for a project.
pub async fn get_project_token_stats(
    project_path: String,
    offset: Option<usize>,
    limit: Option<usize>,
    start_date: Option<String>,
    end_date: Option<String>,
    stats_mode: Option<String>,
) -> Result<PaginatedTokenStats, String> {
    let mode = parse_stats_mode(stats_mode);
    let provider = detect_project_provider(&project_path);
    if provider != StatsProvider::Claude {
        return get_provider_project_token_stats(
            provider,
            &project_path,
            offset.unwrap_or(0),
            limit.unwrap_or(20),
            start_date,
            end_date,
            mode,
        );
    }

    if project_path.trim().is_empty() {
        return Err("project_path is required".to_string());
    }
    let project_path_buf = PathBuf::from(&project_path);
    if !project_path_buf.is_absolute() {
        // detect_project_provider() defaulted this path to Claude because it
        // didn't recognize any known provider's scheme/pattern (e.g. a
        // provider whose virtual-scheme paths, like antigravity-cli://, were
        // never added to that function's checks). That's a real gap in
        // provider coverage, not a reason to hard-crash the whole Insights
        // view for a project the user can otherwise see and click on --
        // degrade to "no data" instead of erroring.
        #[cfg(debug_assertions)]
        eprintln!(
            "⚠️ get_project_token_stats: project_path did not resolve to an absolute path or a recognized provider scheme, returning empty stats: {project_path}"
        );
        return Ok(PaginatedTokenStats {
            items: Vec::new(),
            total_count: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(20),
            has_more: false,
        });
    }

    #[cfg(debug_assertions)]
    let start = std::time::Instant::now();
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(20);

    // Collect all session files
    let session_files: Vec<PathBuf> = WalkDir::new(&project_path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path().to_path_buf())
        .collect();

    #[cfg(debug_assertions)]
    let scan_time = start.elapsed();

    // Parse date limits before parallel processing so per-message filtering is applied
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    // Process all sessions in parallel with per-message date filtering
    let all_stats: Vec<SessionTokenStats> = session_files
        .par_iter()
        .filter_map(|path| {
            extract_session_token_stats_sync(path, mode, s_limit.as_ref(), e_limit.as_ref())
        })
        .collect();

    #[cfg(debug_assertions)]
    let process_time = start.elapsed();

    let total_count = all_stats.len();

    let mut all_stats = all_stats;
    all_stats.sort_by_key(|stats| Reverse(stats.total_tokens));

    // Apply pagination
    let paginated_items: Vec<SessionTokenStats> =
        all_stats.into_iter().skip(offset).take(limit).collect();

    let has_more = offset + paginated_items.len() < total_count;
    #[cfg(debug_assertions)]
    let total_time = start.elapsed();

    #[cfg(debug_assertions)]
    log::debug!(
        "get_project_token_stats: {} sessions ({} after filter), scan={}ms, process={}ms, total={}ms",
        total_count,
        paginated_items.len(),
        scan_time.as_millis(),
        process_time.as_millis(),
        total_time.as_millis()
    );

    Ok(PaginatedTokenStats {
        items: paginated_items,
        total_count,
        offset,
        limit,
        has_more,
    })
}

#[tauri::command]
/// Return an aggregate stats summary for a project.
pub async fn get_project_stats_summary(
    project_path: String,
    start_date: Option<String>,
    end_date: Option<String>,
    stats_mode: Option<String>,
) -> Result<ProjectStatsSummary, String> {
    let mode = parse_stats_mode(stats_mode);
    let provider = detect_project_provider(&project_path);
    if provider != StatsProvider::Claude {
        return get_provider_project_stats_summary(
            provider,
            &project_path,
            start_date,
            end_date,
            mode,
        );
    }

    if project_path.trim().is_empty() {
        return Err("project_path is required".to_string());
    }
    let project_path_buf = PathBuf::from(&project_path);
    if !project_path_buf.is_absolute() {
        // See the matching comment in get_project_token_stats: this means
        // detect_project_provider() couldn't recognize the path as any known
        // provider and defaulted it to Claude, not that the project itself is
        // invalid -- degrade to an empty summary instead of hard-erroring.
        #[cfg(debug_assertions)]
        eprintln!(
            "⚠️ get_project_stats_summary: project_path did not resolve to an absolute path or a recognized provider scheme, returning empty summary: {project_path}"
        );
        return Ok(ProjectStatsSummary::default());
    }

    let start = std::time::Instant::now();
    let project_name = PathBuf::from(&project_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    // Phase 1: Collect all session files
    let session_files: Vec<PathBuf> = WalkDir::new(&project_path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path().to_path_buf())
        .collect();
    let scan_time = start.elapsed();

    // Phase 2: Process all session files in parallel with per-message date filtering
    let file_stats: Vec<ProjectSessionFileStats> = session_files
        .par_iter()
        .filter_map(|path| {
            process_session_file_for_project_stats(path, mode, s_limit.as_ref(), e_limit.as_ref())
        })
        .collect();
    let process_time = start.elapsed();

    // Phase 3: Aggregate results
    let mut summary = ProjectStatsSummary::default();
    summary.project_name = project_name;
    summary.total_sessions = file_stats.len();

    let mut session_durations: Vec<u32> = Vec::new();
    let mut tool_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut skill_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut subagent_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut project_model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut project_model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut project_model_costs: HashMap<String, f64> = HashMap::new();
    let mut daily_stats_map: HashMap<String, DailyStats> = HashMap::new();
    let mut activity_map: HashMap<(u8, u8), (u32, u64)> = HashMap::new();
    let mut session_count_by_date: HashMap<String, usize> = HashMap::new();

    for stats in file_stats {
        summary.total_messages += stats.total_messages as usize;

        // Aggregate token distribution
        summary.token_distribution.input += stats.token_distribution.input;
        summary.token_distribution.output += stats.token_distribution.output;
        summary.token_distribution.cache_creation += stats.token_distribution.cache_creation;
        summary.token_distribution.cache_read += stats.token_distribution.cache_read;
        summary.token_distribution.reasoning += stats.token_distribution.reasoning;
        merge_model_stats(
            &mut project_model_usage,
            &mut project_model_context_usage,
            &mut project_model_costs,
            build_model_stats(
                StatsProvider::Claude,
                stats.model_usage,
                stats.model_context_usage,
                stats.model_costs,
            ),
        );

        // Aggregate tool usage
        for (name, (usage, success)) in stats.tool_usage {
            let entry = tool_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }
        // Aggregate skill / subagent usage (#321)
        for (name, (usage, success)) in stats.skill_usage {
            let entry = skill_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }
        for (name, (usage, success)) in stats.subagent_usage {
            let entry = subagent_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }

        // Aggregate daily stats
        for (date, daily) in stats.daily_stats {
            let entry = daily_stats_map
                .entry(date.clone())
                .or_insert_with(|| DailyStats {
                    date,
                    ..Default::default()
                });
            entry.total_tokens += daily.total_tokens;
            entry.input_tokens += daily.input_tokens;
            entry.output_tokens += daily.output_tokens;
            entry.message_count += daily.message_count;
        }

        // Aggregate activity data
        for ((hour, day), (count, tokens)) in stats.activity_data {
            let entry = activity_map.entry((hour, day)).or_insert((0, 0));
            entry.0 += count;
            entry.1 += tokens;
        }

        // Aggregate per-day session counts from this session's active dates.
        for date in stats.session_dates {
            *session_count_by_date.entry(date).or_insert(0) += 1;
        }

        // Collect session duration
        if stats.session_duration_minutes > 0 {
            session_durations.push(stats.session_duration_minutes);
        }

        // timestamps are preserved for duration calculations only.
    }

    // Phase 4: Finalize daily stats
    for (date, daily_stat) in &mut daily_stats_map {
        daily_stat.session_count = session_count_by_date.get(date).copied().unwrap_or(0);
        daily_stat.active_hours = if daily_stat.message_count > 0 {
            std::cmp::min(24, std::cmp::max(1, daily_stat.message_count / 10))
        } else {
            0
        };
    }

    summary.most_used_tools = tool_usage_map
        .into_iter()
        .map(|(name, (usage, success))| ToolUsageStats {
            tool_name: name,
            usage_count: usage,
            success_rate: if usage > 0 {
                (success as f32 / usage as f32) * 100.0
            } else {
                0.0
            },
            avg_execution_time: None,
        })
        .collect();
    summary
        .most_used_tools
        .sort_by_key(|tool| Reverse(tool.usage_count));
    summary.most_used_skills = build_tool_usage_stats(skill_usage_map);
    summary.most_used_subagents = build_tool_usage_stats(subagent_usage_map);
    let project_model_distribution = build_model_stats(
        StatsProvider::Claude,
        project_model_usage,
        project_model_context_usage,
        project_model_costs,
    );
    summary.model_distribution = project_model_distribution;

    summary.daily_stats = daily_stats_map.into_values().collect();
    summary.daily_stats.sort_by(|a, b| a.date.cmp(&b.date));

    summary.activity_heatmap = activity_map
        .into_iter()
        .map(|((hour, day), (count, tokens))| ActivityHeatmap {
            hour,
            day,
            activity_count: count,
            tokens_used: tokens,
        })
        .collect();

    summary.total_tokens = summary.token_distribution.input
        + summary.token_distribution.output
        + summary.token_distribution.cache_creation
        + summary.token_distribution.cache_read
        + summary.token_distribution.reasoning;
    summary.avg_tokens_per_session = if summary.total_sessions > 0 {
        summary.total_tokens / summary.total_sessions as u64
    } else {
        0
    };
    summary.total_session_duration = session_durations.iter().sum::<u32>();
    summary.avg_session_duration = if session_durations.is_empty() {
        0
    } else {
        summary.total_session_duration / session_durations.len() as u32
    };

    summary.most_active_hour = summary
        .activity_heatmap
        .iter()
        .max_by_key(|a| a.activity_count)
        .map_or(0, |a| a.hour);

    let total_time = start.elapsed();
    log::debug!(
        "get_project_stats_summary: {} sessions, scan={}ms, process={}ms, total={}ms",
        summary.total_sessions,
        scan_time.as_millis(),
        process_time.as_millis(),
        total_time.as_millis()
    );

    Ok(summary)
}

/// Lightweight session stats for comparison (parallel processing)
#[derive(Clone)]
struct SessionComparisonStats {
    session_id: String,
    total_tokens: u64,
    message_count: usize,
    duration_seconds: i64,
}

/// Process a session file into lightweight comparison stats.
/// Served from the per-file daily-aggregate cache when possible; falls back
/// to the full scan (see the `cache` module design note).
fn process_session_file_for_comparison(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionComparisonStats> {
    if let Some(aggregate) = cache::message_stats_cache().get_or_build(session_path, mode, || {
        cache::build_message_file_aggregate(session_path, mode)
    }) {
        if let cache::Composed::Ready(stats) =
            cache::compose_comparison(&aggregate, s_limit, e_limit)
        {
            return stats;
        }
    }
    scan_session_file_for_comparison(session_path, mode, s_limit, e_limit)
}

/// Process a single session file for comparison stats (lightweight)
#[allow(unsafe_code)] // Required for mmap performance optimization
/// Full-scan path for comparison stats (cache miss / non-composable filter).
fn scan_session_file_for_comparison(
    session_path: &PathBuf,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Option<SessionComparisonStats> {
    let file = fs::File::open(session_path).ok()?;

    // SAFETY: We're only reading the file, and the file handle is kept open
    // for the duration of the mmap's lifetime. Session files are append-only.
    let mmap = unsafe { Mmap::map(&file) }.ok()?;

    let mut session_id: Option<String> = None;
    let mut total_tokens: u64 = 0;
    let mut message_count: usize = 0;
    let mut first_time: Option<DateTime<Utc>> = None;
    let mut last_time: Option<DateTime<Utc>> = None;

    // Use SIMD-accelerated line detection
    let line_ranges = find_line_ranges(&mmap);

    // #283: stream entries with owned-key dedup (no per-file Vec buffering).
    let mut seen_usage_keys: HashSet<String> = HashSet::new();

    for (start, end) in line_ranges {
        let mut line_bytes = mmap[start..end].to_vec();
        let Some(log_entry) = parse_raw_log_entry_simd(&mut line_bytes) else {
            continue;
        };
        let Ok(message) = ClaudeMessage::try_from(log_entry) else {
            continue;
        };

        let usage = extract_token_usage(&message);
        let has_usage = token_usage_has_token_fields(&usage);
        if !should_include_stats_entry(&message.message_type, message.is_sidechain, has_usage, mode)
        {
            continue;
        }

        // Per-message date filtering
        let parsed_ts = parse_timestamp_utc(&message.timestamp);
        if !is_within_date_limits(parsed_ts, s_limit, e_limit) {
            continue;
        }

        if session_id.is_none() {
            session_id = Some(message.session_id.clone());
        }

        message_count += 1;

        let (_, _, _, _, _, tokens) =
            dedup_token_totals_msg(&mut seen_usage_keys, &message, &usage);
        total_tokens += tokens;

        if let Some(timestamp) = parsed_ts {
            if first_time
                .as_ref()
                .map_or(true, |current| timestamp < *current)
            {
                first_time = Some(timestamp);
            }
            if last_time
                .as_ref()
                .map_or(true, |current| timestamp > *current)
            {
                last_time = Some(timestamp);
            }
        }
    }

    let duration_seconds = match (first_time.as_ref(), last_time.as_ref()) {
        (Some(first), Some(last)) => (*last - *first).num_seconds(),
        _ => 0,
    };

    Some(SessionComparisonStats {
        session_id: session_id?,
        total_tokens,
        message_count,
        duration_seconds,
    })
}

#[tauri::command]
/// Compare a session against the rest of its project.
pub async fn get_session_comparison(
    session_id: String,
    project_path: String,
    start_date: Option<String>,
    end_date: Option<String>,
    stats_mode: Option<String>,
) -> Result<SessionComparison, String> {
    let mode = parse_stats_mode(stats_mode);
    let provider = detect_project_provider(&project_path);
    if provider != StatsProvider::Claude {
        return get_provider_session_comparison(
            provider,
            &session_id,
            &project_path,
            mode,
            start_date,
            end_date,
        );
    }

    let start = std::time::Instant::now();
    let s_limit = parse_date_limit(start_date, "start_date");
    let e_limit = parse_date_limit(end_date, "end_date");

    // Phase 1: Collect all session files
    let session_files: Vec<PathBuf> = WalkDir::new(&project_path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path().to_path_buf())
        .collect();
    let scan_time = start.elapsed();

    // Phase 2: Process all session files in parallel with per-message date filtering
    let all_sessions: Vec<SessionComparisonStats> = session_files
        .par_iter()
        .filter_map(|path| {
            process_session_file_for_comparison(path, mode, s_limit.as_ref(), e_limit.as_ref())
        })
        .collect();
    let process_time = start.elapsed();

    let target_session = all_sessions
        .iter()
        .find(|s| s.session_id == session_id)
        .ok_or("Session not found in project")?;

    let total_project_tokens: u64 = all_sessions.iter().map(|s| s.total_tokens).sum();
    let total_project_messages: usize = all_sessions.iter().map(|s| s.message_count).sum();

    let percentage_of_project_tokens = if total_project_tokens > 0 {
        (target_session.total_tokens as f32 / total_project_tokens as f32) * 100.0
    } else {
        0.0
    };

    let percentage_of_project_messages = if total_project_messages > 0 {
        (target_session.message_count as f32 / total_project_messages as f32) * 100.0
    } else {
        0.0
    };

    let mut sessions_by_tokens = all_sessions.clone();
    sessions_by_tokens.sort_by_key(|stats| Reverse(stats.total_tokens));

    let rank_by_tokens = sessions_by_tokens
        .iter()
        .position(|s| s.session_id == session_id)
        .unwrap_or(0)
        + 1;

    let mut sessions_by_duration = all_sessions.clone();
    sessions_by_duration.sort_by_key(|stats| Reverse(stats.duration_seconds));

    let rank_by_duration = sessions_by_duration
        .iter()
        .position(|s| s.session_id == session_id)
        .unwrap_or(0)
        + 1;

    let avg_tokens = if all_sessions.is_empty() {
        0
    } else {
        total_project_tokens / all_sessions.len() as u64
    };
    let is_above_average = target_session.total_tokens > avg_tokens;
    let total_time = start.elapsed();

    log::debug!(
        "get_session_comparison: {} sessions, scan={}ms, process={}ms, total={}ms",
        all_sessions.len(),
        scan_time.as_millis(),
        process_time.as_millis(),
        total_time.as_millis()
    );

    Ok(SessionComparison {
        session_id,
        percentage_of_project_tokens,
        percentage_of_project_messages,
        rank_by_tokens,
        rank_by_duration,
        is_above_average,
    })
}

impl TryFrom<RawLogEntry> for ClaudeMessage {
    type Error = String;

    /// Convert a raw log entry into a normalized Claude message.
    fn try_from(log_entry: RawLogEntry) -> Result<Self, Self::Error> {
        if log_entry.message_type == "summary" {
            return Err("Summary entries should be handled separately".to_string());
        }
        if log_entry.session_id.is_none() && log_entry.timestamp.is_none() {
            return Err("Missing session_id and timestamp".to_string());
        }

        let (role, message_id, model, stop_reason, usage) = if let Some(ref msg) = log_entry.message
        {
            (
                Some(msg.role.clone()),
                msg.id.clone(),
                msg.model.clone(),
                msg.stop_reason.clone(),
                msg.usage.clone(),
            )
        } else {
            (None, None, None, None, None)
        };

        Ok(ClaudeMessage {
            uuid: log_entry
                .uuid
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            parent_uuid: log_entry.parent_uuid,
            session_id: log_entry
                .session_id
                .unwrap_or_else(|| "unknown-session".to_string()),
            timestamp: log_entry
                .timestamp
                .unwrap_or_else(|| Utc::now().to_rfc3339()),
            message_type: log_entry.message_type.clone(),
            content: log_entry.message.map(|m| m.content).or(log_entry.content),
            project_name: None,
            tool_use: log_entry.tool_use,
            tool_use_result: log_entry.tool_use_result,
            is_sidechain: log_entry.is_sidechain,
            usage,
            role,
            model,
            stop_reason,
            cost_usd: log_entry.cost_usd,
            duration_ms: log_entry.duration_ms,
            // File history snapshot fields
            message_id: message_id.or(log_entry.message_id),
            snapshot: log_entry.snapshot,
            is_snapshot_update: log_entry.is_snapshot_update,
            // Progress message fields
            data: log_entry.data,
            tool_use_id: log_entry.tool_use_id,
            parent_tool_use_id: log_entry.parent_tool_use_id,
            // Queue operation fields
            operation: log_entry.operation,
            // System message fields
            subtype: log_entry.subtype,
            level: log_entry.level,
            hook_count: log_entry.hook_count,
            hook_infos: log_entry.hook_infos,
            stop_reason_system: log_entry.stop_reason_system,
            prevented_continuation: log_entry.prevented_continuation,
            compact_metadata: log_entry.compact_metadata,
            microcompact_metadata: log_entry.microcompact_metadata,
            provider: None,
        })
    }
}

/// One archived `message` row, joined with its `usage` row if one exists.
struct ArchivedMessageRow {
    id: i64,
    uuid: String,
    parent_uuid: Option<String>,
    message_type: String,
    timestamp: String,
    is_sidechain: bool,
    model: Option<String>,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    message_id: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    cache_creation_tokens_5m: Option<i64>,
    cache_creation_tokens_1h: Option<i64>,
    cache_read_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    service_tier: Option<String>,
}

/// Reconstructs, for one archived Claude session, a synthetic
/// `Vec<ClaudeMessage>` sufficient to drive the exact same aggregation
/// [`build_global_session_file_stats_from_messages`] already uses for
/// every other provider -- so Claude's portion of global stats can be
/// served from `archive_db` instead of re-walking/re-parsing every
/// `.jsonl`, with an identical DTO and identical known quirks. In
/// particular: native Claude `tool_use` content blocks never carry
/// `is_error` (only the later `tool_result` block does), so
/// `track_tool_usage` always counts a tool call as a "success" -- this
/// reconstruction preserves that by only ever synthesizing `tool_use`
/// blocks (never `is_error`), rather than "fixing" it using the more
/// accurate `tool_result.is_error` this archive also has. The goal is
/// transparent equivalence with existing behavior, not a behavior change.
///
/// Full transcript text isn't stored in the archive (only a bounded
/// preview, `message.content_text`), but nothing this aggregation needs
/// depends on it -- only `tool_use` blocks are reconstructed (from
/// `tool_call` rows), never message body text.
fn reconstruct_claude_messages_for_session(
    conn: &rusqlite::Connection,
    session_row_id: i64,
) -> Result<Vec<ClaudeMessage>, String> {
    // One query for every tool_call in the session, grouped by message,
    // rather than one query per message (avoids an N+1 query pattern).
    let mut tool_calls_by_message: HashMap<i64, Vec<serde_json::Value>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT message_id, tool_use_id, tool_name, input_json
                 FROM tool_call WHERE session_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([session_row_id], |row| {
                let message_id: i64 = row.get(0)?;
                let tool_use_id: Option<String> = row.get(1)?;
                let tool_name: String = row.get(2)?;
                let input_json: Option<String> = row.get(3)?;
                Ok((message_id, tool_use_id, tool_name, input_json))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (message_id, tool_use_id, tool_name, input_json) =
                row.map_err(|e| e.to_string())?;
            let input = input_json
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            let mut block = serde_json::json!({
                "type": "tool_use",
                "name": tool_name,
                "input": input,
            });
            if let (Some(id), Some(obj)) = (tool_use_id, block.as_object_mut()) {
                obj.insert("id".to_string(), serde_json::Value::String(id));
            }
            tool_calls_by_message
                .entry(message_id)
                .or_default()
                .push(block);
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.uuid, m.parent_uuid, m.message_type, m.timestamp,
                    m.is_sidechain, m.model, m.cost_usd, m.duration_ms, m.message_id,
                    u.input_tokens, u.output_tokens, u.cache_creation_tokens,
                    u.cache_creation_tokens_5m, u.cache_creation_tokens_1h,
                    u.cache_read_tokens, u.reasoning_tokens, u.service_tier
             FROM message m
             LEFT JOIN usage u ON u.message_id = m.id
             WHERE m.session_id = ?1
             ORDER BY m.id",
        )
        .map_err(|e| e.to_string())?;

    let rows: Vec<ArchivedMessageRow> = stmt
        .query_map([session_row_id], |row| {
            Ok(ArchivedMessageRow {
                id: row.get(0)?,
                uuid: row.get(1)?,
                parent_uuid: row.get(2)?,
                message_type: row.get(3)?,
                timestamp: row.get(4)?,
                is_sidechain: row.get(5)?,
                model: row.get(6)?,
                cost_usd: row.get(7)?,
                duration_ms: row.get(8)?,
                message_id: row.get(9)?,
                input_tokens: row.get(10)?,
                output_tokens: row.get(11)?,
                cache_creation_tokens: row.get(12)?,
                cache_creation_tokens_5m: row.get(13)?,
                cache_creation_tokens_1h: row.get(14)?,
                cache_read_tokens: row.get(15)?,
                reasoning_tokens: row.get(16)?,
                service_tier: row.get(17)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let to_u32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        // `input_tokens` is `NOT NULL` on the `usage` table itself, so it
        // is `Some` if and only if the `LEFT JOIN` matched a usage row --
        // a reliable "does this message have a usage row" signal distinct
        // from "are all its token fields zero."
        let usage = row.input_tokens.map(|input_tokens| TokenUsage {
            input_tokens: Some(to_u32(input_tokens)),
            output_tokens: row.output_tokens.map(to_u32),
            cache_creation_input_tokens: row.cache_creation_tokens.map(to_u32),
            cache_read_input_tokens: row.cache_read_tokens.map(to_u32),
            cache_creation: None,
            cache_creation_input_tokens_5m: row.cache_creation_tokens_5m.map(to_u32),
            cache_creation_input_tokens_1h: row.cache_creation_tokens_1h.map(to_u32),
            reasoning_tokens: row.reasoning_tokens.map(to_u32),
            service_tier: row.service_tier.clone(),
        });

        let content = tool_calls_by_message
            .remove(&row.id)
            .map(serde_json::Value::Array);

        messages.push(ClaudeMessage {
            uuid: row.uuid,
            parent_uuid: row.parent_uuid,
            timestamp: row.timestamp,
            message_type: row.message_type,
            content,
            is_sidechain: Some(row.is_sidechain),
            usage,
            model: row.model,
            cost_usd: row.cost_usd,
            duration_ms: row.duration_ms.map(|d| u64::try_from(d.max(0)).unwrap_or(u64::MAX)),
            message_id: row.message_id,
            provider: Some("claude".to_string()),
            ..Default::default()
        });
    }

    Ok(messages)
}

/// Mirrors `commands::stats::cache::included_buckets`'s day-boundary
/// composability check. The real app serves Claude global stats from a
/// per-file daily-aggregate cache (`process_session_file_for_global_
/// stats` prefers it whenever available), which populates
/// `model_context_usage`; but if an active date filter's boundary lands
/// *inside* a UTC day's data rather than cleanly at its edges, that cache
/// can't compose an answer and the app falls back to a full raw-file
/// scan for that session instead -- which never populates
/// `model_context_usage`, and keys `model_usage`/`model_costs` more
/// simply (no service-tier disambiguation), matching exactly what
/// `build_global_session_file_stats_from_messages` already computes on
/// its own. `messages` should be mode-filtered only (matching
/// `should_include_stats_message`), not date-filtered -- day buckets
/// exist independent of any date range.
fn model_context_usage_is_available(
    messages: &[ClaudeMessage],
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> bool {
    if s_limit.is_none() && e_limit.is_none() {
        return true;
    }
    let mut day_ranges: HashMap<String, (DateTime<Utc>, DateTime<Utc>)> = HashMap::new();
    for message in messages {
        if !should_include_stats_message(message, mode) {
            continue;
        }
        let Some(ts) = parse_timestamp_utc(&message.timestamp) else {
            continue;
        };
        let date = ts.format("%Y-%m-%d").to_string();
        day_ranges
            .entry(date)
            .and_modify(|(first, last)| {
                if ts < *first {
                    *first = ts;
                }
                if ts > *last {
                    *last = ts;
                }
            })
            .or_insert((ts, ts));
    }

    for (first, last) in day_ranges.values() {
        let starts_in = s_limit.map_or(true, |s| *first >= *s);
        let ends_in = e_limit.map_or(true, |e| *last <= *e);
        if starts_in && ends_in {
            continue;
        }
        let fully_before = s_limit.is_some_and(|s| *last < *s);
        let fully_after = e_limit.is_some_and(|e| *first > *e);
        if fully_before || fully_after {
            continue;
        }
        return false;
    }
    true
}

/// Computes per-model usage/cost/context-tier stats for a message list
/// using `accumulate_model_usage` -- the same per-message keying
/// (`model_usage_key`, which includes `service_tier`) and context-tier
/// bucketing (`context_tier_min_tokens`) the cache-backed aggregation
/// path already uses. This is deliberately NOT the same computation
/// `build_global_session_file_stats_from_messages` does for its own
/// `model_usage`/`model_costs` fields (that function keys purely by
/// model name, with no service-tier disambiguation, and never populates
/// `model_context_usage` at all) -- since `process_session_file_for_
/// global_stats` prefers the per-file daily-aggregate cache (which DOES
/// go through `accumulate_model_usage`) whenever it's composable, THAT
/// is the real, user-visible behavior for Claude today, and the one this
/// archive-backed path must match. Call only when
/// [`model_context_usage_is_available`] returns true; otherwise leave
/// `build_global_session_file_stats_from_messages`'s own model-related
/// fields as-is, since that's the full-scan-equivalent behavior.
fn build_model_usage_for_messages(
    messages: &[ClaudeMessage],
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> (
    HashMap<String, ModelUsageAggregate>,
    ModelContextUsageMap,
    HashMap<String, f64>,
) {
    let mut model_usage: HashMap<String, ModelUsageAggregate> = HashMap::new();
    let mut model_context_usage: ModelContextUsageMap = HashMap::new();
    let mut model_costs: HashMap<String, f64> = HashMap::new();
    let mut seen_usage_keys: HashSet<String> = HashSet::new();
    let mut seen_cost_keys: HashSet<String> = HashSet::new();
    let has_date_filter = s_limit.is_some() || e_limit.is_some();

    for message in messages {
        if !should_include_stats_message(message, mode) {
            continue;
        }
        let usage = extract_token_usage(message);

        let parsed_timestamp = parse_timestamp_utc(&message.timestamp);
        if has_date_filter && !is_within_date_limits(parsed_timestamp, s_limit, e_limit) {
            continue;
        }

        let totals = dedup_token_totals_msg(&mut seen_usage_keys, message, &usage);
        let deduped_source_cost = dedup_source_cost(
            &mut seen_cost_keys,
            &message.session_id,
            message.message_id.as_deref(),
            &message.uuid,
            message.cost_usd,
        );
        let tokens = totals.5;

        if message.model.is_some() || tokens > 0 || deduped_source_cost.is_some() {
            let model_name = message.model.as_deref().unwrap_or(UNKNOWN_MODEL_NAME);
            accumulate_model_usage(
                &mut model_usage,
                &mut model_context_usage,
                &mut model_costs,
                ModelUsageUpdate {
                    model_name,
                    service_tier: usage.service_tier.as_deref(),
                    totals,
                    cache_creation_tokens_1h: u64::from(
                        usage.cache_creation_input_tokens_1h.unwrap_or(0),
                    ),
                    source_cost: deduped_source_cost,
                },
            );
        }
    }

    (model_usage, model_context_usage, model_costs)
}

/// Builds Claude's portion of global stats entirely from Grid's own
/// normalized archive (spec §20-21's `archive_db`), one `SessionFileStats`
/// per archived Claude session. Returns `Ok(vec![])` if the archive has no
/// Claude sessions yet (not backfilled, or a fresh install) -- callers
/// must treat that the same as "fall back to the raw-file-walk path,"
/// never as "the user has zero Claude history."
///
/// Resolves the real `~/.grid-local/archive.db` via
/// `archive_db::open_connection()` in production. Tests should call
/// [`build_global_stats_from_connection`] directly with an injected
/// (e.g. in-memory) connection instead -- resolving the real path here
/// unconditionally would make this function silently read the real
/// developer machine's own archive during a test run, the same class of
/// risk already caught and fixed for the backfill functions.
fn build_global_stats_from_archive_db(
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Result<Vec<SessionFileStats>, String> {
    let conn = crate::archive_db::open_connection()?;
    build_global_stats_from_connection(&conn, mode, s_limit, e_limit)
}

/// Core of [`build_global_stats_from_archive_db`], parametrized on the
/// connection so it never has to resolve a real filesystem path itself.
fn build_global_stats_from_connection(
    conn: &rusqlite::Connection,
    mode: StatsMode,
    s_limit: Option<&DateTime<Utc>>,
    e_limit: Option<&DateTime<Utc>>,
) -> Result<Vec<SessionFileStats>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.file_path FROM session s
             JOIN project p ON p.id = s.project_id
             JOIN provider pr ON pr.id = p.provider_id
             WHERE pr.provider_key = 'claude'
               AND s.is_subagent = 0",
        )
        .map_err(|e| e.to_string())?;
    let sessions: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut file_stats = Vec::with_capacity(sessions.len());
    for (session_row_id, file_path) in sessions {
        let messages = reconstruct_claude_messages_for_session(conn, session_row_id)?;

        // Same project-name derivation the raw-file-walk path uses (the
        // session file's parent directory name) -- NOT this archive's own
        // `project.display_name` (a more sophisticated verified-folder/
        // cwd-based derivation used elsewhere) -- so project groupings
        // match exactly, including edge cases where the two derivations
        // would otherwise disagree.
        let project_name = claude_session_project_name(&PathBuf::from(&file_path));

        if messages.is_empty() {
            // Matches the raw-walk path's `scan_session_file_for_global_stats`,
            // which always returns `Some(stats)` for a discovered file even
            // if it has zero parseable messages -- an empty/malformed
            // session file still counts as one session.
            file_stats.push(SessionFileStats {
                project_name,
                provider: StatsProvider::Claude,
                ..Default::default()
            });
            continue;
        }

        if let Some(mut stats) = build_global_session_file_stats_from_messages(
            StatsProvider::Claude,
            project_name,
            &messages,
            mode,
            s_limit,
            e_limit,
        ) {
            // Replace (not supplement) the model-related fields with the
            // cache-aggregation-equivalent computation, but ONLY when the
            // real app's cache-composed path would actually have been
            // usable for this date range -- otherwise the real app falls
            // back to a full scan, whose model bookkeeping is exactly
            // what `build_global_session_file_stats_from_messages`
            // already computed above, so leave it untouched.
            if model_context_usage_is_available(&messages, mode, s_limit, e_limit) {
                let (model_usage, model_context_usage, model_costs) =
                    build_model_usage_for_messages(&messages, mode, s_limit, e_limit);
                stats.model_usage = model_usage;
                stats.model_context_usage = model_context_usage;
                stats.model_costs = model_costs;
            }
            file_stats.push(stats);
        }
    }
    Ok(file_stats)
}

#[tauri::command]
/// Return an aggregate stats summary across all selected providers.
pub async fn get_global_stats_summary(
    claude_path: String,
    active_providers: Option<Vec<String>>,
    stats_mode: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    custom_claude_paths: Option<Vec<crate::commands::multi_provider::CustomClaudePathParam>>,
) -> Result<GlobalStatsSummary, String> {
    let mode = parse_stats_mode(stats_mode);
    let providers_to_include = parse_active_stats_providers(active_providers);
    let s_limit = parse_date_limit(start_date, "global start_date");
    let e_limit = parse_date_limit(end_date, "global end_date");
    // Temporary phase-level profiling to find the actual bottleneck behind
    // slow global stats loads -- debug-only, remove once the real cost is
    // identified and fixed (not a permanent instrumentation decision).
    let global_stats_start = std::time::Instant::now();

    // Phase 1: Collect all session files and their project names from the default
    // Claude root AND any user-configured custom Claude directories (#362). Without
    // the custom roots the global summary undercounts everything for users who added
    // extra Claude directories, even though the project list and search honor them.
    let mut session_files: Vec<PathBuf> = Vec::new();
    let mut project_names: HashSet<String> = HashSet::new();
    let mut file_stats: Vec<SessionFileStats> = Vec::new();

    let collect_claude_base = |projects_path: &Path,
                               session_files: &mut Vec<PathBuf>,
                               project_names: &mut HashSet<String>| {
        if !projects_path.exists() {
            return;
        }
        match fs::read_dir(projects_path) {
            Ok(entries) => {
                for project_entry in entries {
                    let project_entry = match project_entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            log::warn!("Skipping unreadable Claude project entry: {e}");
                            continue;
                        }
                    };
                    let project_path = project_entry.path();

                    if !project_path.is_dir() {
                        continue;
                    }

                    let project_name = project_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    project_names.insert(format!("claude:{project_name}"));

                    for entry in WalkDir::new(&project_path)
                        .into_iter()
                        .filter_map(std::result::Result::ok)
                        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
                    {
                        session_files.push(entry.path().to_path_buf());
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to read Claude projects directory: {e}");
            }
        }
    };

    // Phase 2: Process all session files in parallel
    let s_ref = s_limit.as_ref();
    let e_ref = e_limit.as_ref();

    // Claude: prefer Grid's own normalized archive (spec §20-21)
    // over re-walking/re-parsing every `.jsonl`, when it's actually
    // populated. Custom Claude directories are NOT yet ingested into the
    // archive (only the default `~/.claude`), so their presence always
    // falls back to the existing raw-walk path -- silently dropping a
    // user's custom-directory sessions from global stats would be a real
    // regression, worse than the archive's performance win.
    let claude_phase_start = std::time::Instant::now();
    let mut claude_handled_by_archive = false;
    if providers_to_include.contains(&StatsProvider::Claude)
        && custom_claude_paths.as_ref().map_or(true, Vec::is_empty)
    {
        match build_global_stats_from_archive_db(mode, s_ref, e_ref) {
            Ok(claude_stats) if !claude_stats.is_empty() => {
                for stats in &claude_stats {
                    project_names.insert(format!("claude:{}", stats.project_name));
                }
                file_stats.extend(claude_stats);
                claude_handled_by_archive = true;
            }
            Ok(_) => {
                // Empty: not backfilled yet (or genuinely no Claude
                // history) -- fall back to the raw walk below, which will
                // also find nothing in the "genuinely empty" case.
            }
            Err(e) => {
                log::warn!(
                    "Failed to read Claude global stats from Grid archive, falling back to raw scan: {e}"
                );
            }
        }
    }

    if providers_to_include.contains(&StatsProvider::Claude) && !claude_handled_by_archive {
        collect_claude_base(
            &PathBuf::from(&claude_path).join("projects"),
            &mut session_files,
            &mut project_names,
        );

        if let Some(ref custom_paths) = custom_claude_paths {
            for custom in custom_paths {
                let base = PathBuf::from(&custom.path);
                if let Err(e) = crate::utils::validate_custom_claude_path(&base) {
                    log::warn!("Skipping invalid custom Claude path for global stats: {e}");
                    continue;
                }
                collect_claude_base(
                    &base.join("projects"),
                    &mut session_files,
                    &mut project_names,
                );
            }
        }
    }

    file_stats.extend(
        session_files
            .par_iter()
            .filter_map(|path| process_session_file_for_global_stats(path, mode, s_ref, e_ref))
            .collect::<Vec<_>>(),
    );

    #[cfg(debug_assertions)]
    eprintln!(
        "⏱ get_global_stats_summary: Claude phase took {:?} (via {}), {} session files, {} file_stats so far",
        claude_phase_start.elapsed(),
        if claude_handled_by_archive { "archive_db" } else { "raw walk" },
        session_files.len(),
        file_stats.len()
    );

    // Every non-Claude provider previously ran its own full directory
    // walk + parse sequentially, one after another -- with ~28 possible
    // providers, most contributing zero sessions, that serialized scan
    // was the dominant cost of a global stats load (each an independent
    // filesystem scan with no data dependency on the others). Run them
    // all through rayon's thread pool at once instead, the same way the
    // Claude session files above already are.
    let providers_to_scan: Vec<StatsProvider> = [
        StatsProvider::Codebuddy,
        StatsProvider::Codex,
        StatsProvider::ForgeCode,
        StatsProvider::OpenCode,
        StatsProvider::Grok,
        StatsProvider::Cursor,
        StatsProvider::Kimi,
        StatsProvider::Antigravity,
        StatsProvider::Copilot,
        StatsProvider::Ompi,
        StatsProvider::Pi,
        StatsProvider::Gemini,
        StatsProvider::Aider,
        StatsProvider::AmazonQ,
        StatsProvider::Cline,
        StatsProvider::Continue,
        StatsProvider::Crush,
        StatsProvider::CursorAgent,
        StatsProvider::Goose,
        StatsProvider::Kiro,
        StatsProvider::Llm,
        StatsProvider::OpenHands,
        StatsProvider::OpenInterpreter,
        StatsProvider::PearAI,
        StatsProvider::Qwen,
        StatsProvider::Trae,
        StatsProvider::Vibe,
        StatsProvider::Zed,
    ]
    .into_iter()
    .filter(|provider| providers_to_include.contains(provider))
    .collect();

    let provider_scan_start = std::time::Instant::now();
    let provider_scan_results: Vec<(StatsProvider, Vec<SessionFileStats>, HashSet<String>, std::time::Duration)> =
        providers_to_scan
            .par_iter()
            .map(|&provider| {
                let provider_start = std::time::Instant::now();
                let (stats, projects) = collect_provider_global_file_stats(provider, mode, s_ref, e_ref);
                (provider, stats, projects, provider_start.elapsed())
            })
            .collect();

    #[cfg(debug_assertions)]
    {
        eprintln!(
            "⏱ get_global_stats_summary: provider scan (all {} providers, parallel) took {:?} wall-clock",
            providers_to_scan.len(),
            provider_scan_start.elapsed()
        );
        for (provider, stats, _, duration) in &provider_scan_results {
            if !stats.is_empty() || duration.as_millis() > 50 {
                eprintln!(
                    "⏱   provider {provider:?}: {duration:?} ({} sessions)",
                    stats.len()
                );
            }
        }
    }

    for (_, provider_stats, provider_projects, _) in provider_scan_results {
        project_names.extend(provider_projects);
        file_stats.extend(provider_stats);
    }

    let aggregation_start = std::time::Instant::now();

    // When date filtering is active, exclude sessions that ended up with zero messages
    if s_ref.is_some() || e_ref.is_some() {
        file_stats.retain(|s| s.total_messages > 0);
    }
    let file_stats_count = file_stats.len();

    let active_project_keys: HashSet<String> = file_stats
        .iter()
        .map(|stats| {
            format!(
                "{}:{}",
                stats_provider_id(stats.provider),
                stats.project_name
            )
        })
        .collect();

    // Phase 3: Aggregate results
    let mut summary = GlobalStatsSummary::default();
    summary.total_projects = active_project_keys.len() as u32;
    summary.total_sessions = file_stats.len() as u32;

    let mut tool_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut skill_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut subagent_usage_map: HashMap<String, (u32, u32)> = HashMap::new();
    let mut daily_stats_map: HashMap<String, DailyStats> = HashMap::new();
    let mut activity_map: HashMap<(u8, u8), (u32, u64)> = HashMap::new();
    let mut model_usage_map: HashMap<(StatsProvider, String), ModelUsageAggregate> = HashMap::new();
    let mut model_context_map: HashMap<(StatsProvider, String), HashMap<u64, ModelContextStats>> =
        HashMap::new();
    let mut model_cost_map: HashMap<(StatsProvider, String), f64> = HashMap::new();
    let mut project_stats_map: HashMap<String, (u32, u32, u64)> = HashMap::new();
    let mut provider_stats_map: HashMap<StatsProvider, (u32, u32, u64)> = HashMap::new();
    let mut provider_projects_map: HashMap<StatsProvider, HashSet<String>> = HashMap::new();
    let mut global_first_message: Option<DateTime<Utc>> = None;
    let mut global_last_message: Option<DateTime<Utc>> = None;

    for stats in file_stats {
        let provider = stats.provider;
        let project_name = stats.project_name.clone();

        summary.total_messages += stats.total_messages;
        summary.total_tokens += stats.total_tokens;
        summary.total_session_duration_minutes += stats.session_duration_minutes;

        // Aggregate token distribution
        summary.token_distribution.input += stats.token_distribution.input;
        summary.token_distribution.output += stats.token_distribution.output;
        summary.token_distribution.cache_creation += stats.token_distribution.cache_creation;
        summary.token_distribution.cache_read += stats.token_distribution.cache_read;
        summary.token_distribution.reasoning += stats.token_distribution.reasoning;

        // Aggregate tool usage
        for (name, (usage, success)) in stats.tool_usage {
            let entry = tool_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }
        // Aggregate skill / subagent usage (#321)
        for (name, (usage, success)) in stats.skill_usage {
            let entry = skill_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }
        for (name, (usage, success)) in stats.subagent_usage {
            let entry = subagent_usage_map.entry(name).or_insert((0, 0));
            entry.0 += usage;
            entry.1 += success;
        }

        // Aggregate daily stats
        for (date, daily) in stats.daily_stats {
            let entry = daily_stats_map
                .entry(date.clone())
                .or_insert_with(|| DailyStats {
                    date,
                    ..Default::default()
                });
            entry.total_tokens += daily.total_tokens;
            entry.input_tokens += daily.input_tokens;
            entry.output_tokens += daily.output_tokens;
            entry.message_count += daily.message_count;
        }

        // Aggregate activity data
        for ((hour, day), (count, tokens)) in stats.activity_data {
            let entry = activity_map.entry((hour, day)).or_insert((0, 0));
            entry.0 += count;
            entry.1 += tokens;
        }

        // Aggregate model usage
        for (model, (msg_count, total, input, output, cache_create, cache_read, reasoning)) in
            stats.model_usage
        {
            if let Some(cost_usd) = stats.model_costs.get(&model) {
                *model_cost_map
                    .entry((provider, model.clone()))
                    .or_insert(0.0) += cost_usd;
            }
            let entry = model_usage_map
                .entry((provider, model))
                .or_insert((0, 0, 0, 0, 0, 0, 0));
            entry.0 += msg_count;
            entry.1 += total;
            entry.2 += input;
            entry.3 += output;
            entry.4 += cache_create;
            entry.5 += cache_read;
            entry.6 += reasoning;
        }
        for (model, buckets) in stats.model_context_usage {
            let target = model_context_map.entry((provider, model)).or_default();
            for (min_context_tokens, values) in buckets {
                target
                    .entry(min_context_tokens)
                    .and_modify(|entry| {
                        entry.token_count += values.token_count;
                        entry.input_tokens += values.input_tokens;
                        entry.output_tokens += values.output_tokens;
                        entry.cache_creation_tokens += values.cache_creation_tokens;
                        entry.cache_creation_tokens_5m += values.cache_creation_tokens_5m;
                        entry.cache_creation_tokens_1h += values.cache_creation_tokens_1h;
                        entry.cache_read_tokens += values.cache_read_tokens;
                        entry.reasoning_tokens += values.reasoning_tokens;
                    })
                    .or_insert(values);
            }
        }

        // Aggregate provider stats
        let provider_entry = provider_stats_map.entry(provider).or_insert((0, 0, 0));
        provider_entry.0 += 1; // sessions
        provider_entry.1 += stats.total_messages; // messages
        provider_entry.2 += stats.total_tokens; // tokens

        provider_projects_map
            .entry(provider)
            .or_default()
            .insert(project_name.clone());

        // Aggregate project stats
        let project_entry = project_stats_map.entry(project_name).or_insert((0, 0, 0));
        project_entry.0 += 1; // sessions
        project_entry.1 += stats.total_messages; // messages
        project_entry.2 += stats.total_tokens; // tokens

        // Track global first/last message
        if let Some(first) = stats.first_message {
            if global_first_message.is_none() || first < global_first_message.unwrap() {
                global_first_message = Some(first);
            }
        }
        if let Some(last) = stats.last_message {
            if global_last_message.is_none() || last > global_last_message.unwrap() {
                global_last_message = Some(last);
            }
        }
    }
    // Phase 4: Build final summary structures
    summary.most_used_tools = tool_usage_map
        .into_iter()
        .map(|(name, (usage, success))| ToolUsageStats {
            tool_name: name,
            usage_count: usage,
            success_rate: if usage > 0 {
                (success as f32 / usage as f32) * 100.0
            } else {
                0.0
            },
            avg_execution_time: None,
        })
        .collect();
    summary
        .most_used_tools
        .sort_by_key(|tool| Reverse(tool.usage_count));
    summary.most_used_skills = build_tool_usage_stats(skill_usage_map);
    summary.most_used_subagents = build_tool_usage_stats(subagent_usage_map);

    summary.provider_distribution = provider_stats_map
        .into_iter()
        .map(
            |(provider, (sessions, messages, tokens))| ProviderUsageStats {
                provider_id: stats_provider_id(provider).to_string(),
                projects: provider_projects_map
                    .get(&provider)
                    .map(|projects| projects.len() as u32)
                    .unwrap_or(0),
                sessions,
                messages,
                tokens,
            },
        )
        .collect();
    summary
        .provider_distribution
        .sort_by_key(|provider| Reverse(provider.tokens));

    summary.model_distribution = model_usage_map
        .into_iter()
        .map(
            |(
                (provider, model_key),
                (
                    message_count,
                    token_count,
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    reasoning_tokens,
                ),
            )| {
                let (model_name, service_tier) = split_model_usage_key(&model_key);
                let mut context_breakdown = model_context_map
                    .get(&(provider, model_key.clone()))
                    .map(|buckets| buckets.values().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                context_breakdown.sort_by_key(|bucket| bucket.min_context_tokens);
                let cost_usd = model_cost_map.get(&(provider, model_key.clone())).copied();
                ModelStats {
                    provider_id: Some(stats_provider_id(provider).to_string()),
                    model_name: model_name.to_string(),
                    service_tier: service_tier.map(str::to_string),
                    message_count,
                    token_count,
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    reasoning_tokens,
                    cost_usd,
                    context_breakdown,
                }
            },
        )
        .collect();
    summary
        .model_distribution
        .sort_by_key(|model| Reverse(model.token_count));

    summary.top_projects = project_stats_map
        .into_iter()
        .map(
            |(project_name, (sessions, messages, tokens))| ProjectRanking {
                project_name,
                sessions,
                messages,
                tokens,
            },
        )
        .collect();
    summary
        .top_projects
        .sort_by_key(|project| Reverse(project.tokens));
    summary.top_projects.truncate(10);

    summary.daily_stats = daily_stats_map.into_values().collect();
    summary.daily_stats.sort_by(|a, b| a.date.cmp(&b.date));

    summary.activity_heatmap = activity_map
        .into_iter()
        .map(|((hour, day), (count, tokens))| ActivityHeatmap {
            hour,
            day,
            activity_count: count,
            tokens_used: tokens,
        })
        .collect();

    if let (Some(first), Some(last)) = (global_first_message, global_last_message) {
        summary.date_range.first_message = Some(first.to_rfc3339());
        summary.date_range.last_message = Some(last.to_rfc3339());
        summary.date_range.days_span = (last - first).num_days() as u32;
    }

    #[cfg(debug_assertions)]
    eprintln!(
        "⏱ get_global_stats_summary: aggregation phase took {:?} over {} file_stats; TOTAL {:?}",
        aggregation_start.elapsed(),
        file_stats_count,
        global_stats_start.elapsed()
    );

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.as_ref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn make_test_message(
        provider: Option<&str>,
        message_type: &str,
        usage: Option<TokenUsage>,
    ) -> ClaudeMessage {
        ClaudeMessage {
            uuid: "test-uuid".to_string(),
            parent_uuid: None,
            session_id: "session-123".to_string(),
            timestamp: "2025-06-26T10:00:00Z".to_string(),
            message_type: message_type.to_string(),
            content: None,
            project_name: None,
            tool_use: None,
            tool_use_result: None,
            is_sidechain: Some(false),
            usage,
            role: None,
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: provider.map(std::string::ToString::to_string),
        }
    }

    #[test]
    /// #321: Skill (`input.skill`) and Agent (`input.subagent_type`) invocations
    /// are aggregated by their input value, not collapsed into one bucket.
    fn test_track_skill_and_subagent_usage() {
        let mut msg = make_test_message(None, "assistant", None);
        msg.content = Some(json!([
            { "type": "tool_use", "name": "Skill", "input": { "skill": "triage", "args": "x" } },
            { "type": "tool_use", "name": "Skill", "input": { "skill": "triage" } },
            { "type": "tool_use", "name": "Skill", "input": { "skill": "loop" } },
            { "type": "tool_use", "name": "Agent", "input": { "subagent_type": "Explore", "prompt": "p" } },
            { "type": "tool_use", "name": "Read", "input": { "file_path": "/a" } },
        ]));

        let mut skills: HashMap<String, (u32, u32)> = HashMap::new();
        let mut subagents: HashMap<String, (u32, u32)> = HashMap::new();
        track_skill_and_subagent_usage(&msg, &mut skills, &mut subagents);

        assert_eq!(skills.get("triage"), Some(&(2, 2)));
        assert_eq!(skills.get("loop"), Some(&(1, 1)));
        assert_eq!(skills.len(), 2);
        assert!(!skills.contains_key("Read"));
        assert_eq!(subagents.get("Explore"), Some(&(1, 1)));
        assert_eq!(subagents.len(), 1);
    }

    #[test]
    /// #321: only assistant messages are scanned, and a Skill call missing the
    /// `skill` key is skipped (no empty-named bucket).
    fn test_skill_usage_ignores_user_and_missing_key() {
        let mut skills: HashMap<String, (u32, u32)> = HashMap::new();
        let mut subagents: HashMap<String, (u32, u32)> = HashMap::new();

        let mut user = make_test_message(None, "user", None);
        user.content = Some(json!([
            { "type": "tool_use", "name": "Skill", "input": { "skill": "x" } }
        ]));
        track_skill_and_subagent_usage(&user, &mut skills, &mut subagents);
        assert!(skills.is_empty());

        let mut asst = make_test_message(None, "assistant", None);
        asst.content = Some(json!([
            { "type": "tool_use", "name": "Skill", "input": {} }
        ]));
        track_skill_and_subagent_usage(&asst, &mut skills, &mut subagents);
        assert!(skills.is_empty());
    }

    #[test]
    /// Verify try from raw log entry user message.
    fn test_try_from_raw_log_entry_user_message() {
        let raw = RawLogEntry {
            uuid: Some("test-uuid".to_string()),
            parent_uuid: Some("parent-uuid".to_string()),
            session_id: Some("session-123".to_string()),
            timestamp: Some("2025-06-26T10:00:00Z".to_string()),
            message_type: "user".to_string(),
            summary: None,
            leaf_uuid: None,
            message: Some(MessageContent {
                role: "user".to_string(),
                content: json!("Hello, Claude!"),
                id: None,
                model: None,
                stop_reason: None,
                usage: None,
            }),
            tool_use: None,
            tool_use_result: None,
            is_sidechain: Some(false),
            cwd: Some("/home/user/project".to_string()),
            entrypoint: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };

        let result = ClaudeMessage::try_from(raw);
        assert!(result.is_ok());

        let msg = result.unwrap();
        assert_eq!(msg.uuid, "test-uuid");
        assert_eq!(msg.session_id, "session-123");
        assert_eq!(msg.message_type, "user");
        assert_eq!(msg.role, Some("user".to_string()));
    }

    #[test]
    /// Verify try from raw log entry assistant message.
    fn test_try_from_raw_log_entry_assistant_message() {
        let raw = RawLogEntry {
            uuid: Some("assistant-uuid".to_string()),
            parent_uuid: None,
            session_id: Some("session-123".to_string()),
            timestamp: Some("2025-06-26T10:01:00Z".to_string()),
            message_type: "assistant".to_string(),
            summary: None,
            leaf_uuid: None,
            message: Some(MessageContent {
                role: "assistant".to_string(),
                content: json!([{"type": "text", "text": "Hello!"}]),
                id: Some("msg_123".to_string()),
                model: Some("claude-opus-4-20250514".to_string()),
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: Some(100),
                    output_tokens: Some(50),
                    cache_creation_input_tokens: Some(20),
                    cache_read_input_tokens: Some(10),
                    reasoning_tokens: None,
                    service_tier: Some("standard".to_string()),
                    ..Default::default()
                }),
            }),
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            cwd: None,
            entrypoint: None,
            cost_usd: Some(0.005),
            duration_ms: Some(1500),
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };

        let result = ClaudeMessage::try_from(raw);
        assert!(result.is_ok());

        let msg = result.unwrap();
        assert_eq!(msg.message_type, "assistant");
        assert_eq!(msg.model, Some("claude-opus-4-20250514".to_string()));
        assert_eq!(msg.stop_reason, Some("end_turn".to_string()));
        assert_eq!(msg.cost_usd, Some(0.005));
        assert_eq!(msg.duration_ms, Some(1500));

        let usage = msg.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(50));
    }

    #[test]
    /// Verify try from raw log entry summary fails.
    fn test_try_from_raw_log_entry_summary_fails() {
        let raw = RawLogEntry {
            uuid: None,
            parent_uuid: None,
            session_id: None,
            timestamp: None,
            message_type: "summary".to_string(),
            summary: Some("This is a summary".to_string()),
            leaf_uuid: Some("leaf-123".to_string()),
            message: None,
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            cwd: None,
            entrypoint: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };

        let result = ClaudeMessage::try_from(raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Summary"));
    }

    #[test]
    /// Verify try from raw log entry missing session and timestamp fails.
    fn test_try_from_raw_log_entry_missing_session_and_timestamp_fails() {
        let raw = RawLogEntry {
            uuid: Some("uuid".to_string()),
            parent_uuid: None,
            session_id: None,
            timestamp: None,
            message_type: "user".to_string(),
            summary: None,
            leaf_uuid: None,
            message: Some(MessageContent {
                role: "user".to_string(),
                content: json!("Hello"),
                id: None,
                model: None,
                stop_reason: None,
                usage: None,
            }),
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            cwd: None,
            entrypoint: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };

        let result = ClaudeMessage::try_from(raw);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing"));
    }

    #[test]
    /// Verify try from raw log entry with only timestamp.
    fn test_try_from_raw_log_entry_with_only_timestamp() {
        let raw = RawLogEntry {
            uuid: None,
            parent_uuid: None,
            session_id: None,
            timestamp: Some("2025-06-26T10:00:00Z".to_string()),
            message_type: "user".to_string(),
            summary: None,
            leaf_uuid: None,
            message: Some(MessageContent {
                role: "user".to_string(),
                content: json!("Hello"),
                id: None,
                model: None,
                stop_reason: None,
                usage: None,
            }),
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            cwd: None,
            entrypoint: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };

        // Should succeed with timestamp even without session_id
        let result = ClaudeMessage::try_from(raw);
        assert!(result.is_ok());

        let msg = result.unwrap();
        assert_eq!(msg.session_id, "unknown-session");
    }

    #[test]
    /// Verify extract token usage from usage field.
    fn test_extract_token_usage_from_usage_field() {
        let msg = ClaudeMessage {
            uuid: "uuid".to_string(),
            parent_uuid: None,
            session_id: "session".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message_type: "assistant".to_string(),
            content: None,
            project_name: None,
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            usage: Some(TokenUsage {
                input_tokens: Some(100),
                output_tokens: Some(50),
                cache_creation_input_tokens: Some(20),
                cache_read_input_tokens: Some(10),
                reasoning_tokens: None,
                service_tier: Some("standard".to_string()),
                ..Default::default()
            }),
            role: Some("assistant".to_string()),
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: None,
        };

        let usage = extract_token_usage(&msg);
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(50));
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(10));
    }

    #[test]
    /// Verify extract token usage from content.
    fn test_extract_token_usage_from_content() {
        let msg = ClaudeMessage {
            uuid: "uuid".to_string(),
            parent_uuid: None,
            session_id: "session".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message_type: "assistant".to_string(),
            content: Some(json!({
                "usage": {
                    "input_tokens": 200,
                    "output_tokens": 100,
                    "service_tier": "premium"
                }
            })),
            project_name: None,
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            usage: None,
            role: None,
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: None,
        };

        let usage = extract_token_usage(&msg);
        assert_eq!(usage.input_tokens, Some(200));
        assert_eq!(usage.output_tokens, Some(100));
        assert_eq!(usage.service_tier, Some("premium".to_string()));
    }

    #[test]
    /// Verify extract token usage from tool use result.
    fn test_extract_token_usage_from_tool_use_result() {
        let msg = ClaudeMessage {
            uuid: "uuid".to_string(),
            parent_uuid: None,
            session_id: "session".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message_type: "user".to_string(),
            content: None,
            project_name: None,
            tool_use: None,
            tool_use_result: Some(json!({
                "usage": {
                    "input_tokens": 150,
                    "output_tokens": 75
                }
            })),
            is_sidechain: None,
            usage: None,
            role: None,
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: None,
        };

        let usage = extract_token_usage(&msg);
        assert_eq!(usage.input_tokens, Some(150));
        assert_eq!(usage.output_tokens, Some(75));
    }

    #[test]
    /// Verify extract token usage from total tokens.
    fn test_extract_token_usage_from_total_tokens() {
        let msg = ClaudeMessage {
            uuid: "uuid".to_string(),
            parent_uuid: None,
            session_id: "session".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message_type: "assistant".to_string(),
            content: None,
            project_name: None,
            tool_use: None,
            tool_use_result: Some(json!({
                "totalTokens": 500
            })),
            is_sidechain: None,
            usage: None,
            role: None,
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: None,
        };

        let usage = extract_token_usage(&msg);
        // For assistant messages, totalTokens goes to output_tokens
        assert_eq!(usage.output_tokens, Some(500));
    }

    #[test]
    /// Verify extract token usage empty.
    fn test_extract_token_usage_empty() {
        let msg = ClaudeMessage {
            uuid: "uuid".to_string(),
            parent_uuid: None,
            session_id: "session".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message_type: "user".to_string(),
            content: None,
            project_name: None,
            tool_use: None,
            tool_use_result: None,
            is_sidechain: None,
            usage: None,
            role: None,
            model: None,
            stop_reason: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            provider: None,
        };

        let usage = extract_token_usage(&msg);
        assert!(usage.input_tokens.is_none());
        assert!(usage.output_tokens.is_none());
    }

    #[test]
    /// Verify detect project provider from virtual prefix.
    fn test_detect_project_provider_from_virtual_prefix() {
        assert_eq!(
            detect_project_provider("codex:///Users/jack/workspace"),
            StatsProvider::Codex
        );
        assert_eq!(
            detect_project_provider("forgecode://workspace/workspace-alpha"),
            StatsProvider::ForgeCode
        );
        assert_eq!(
            detect_project_provider("opencode://project_123"),
            StatsProvider::OpenCode
        );
        assert_eq!(
            detect_project_provider("grok:///Users/jack/.grok/sessions/%2FUsers%2Fjack%2Frepo"),
            StatsProvider::Grok
        );
        assert_eq!(
            detect_project_provider(
                "cursor:///Users/jack/Library/Application Support/Cursor/User/workspaceStorage/hash"
            ),
            StatsProvider::Cursor
        );
        assert_eq!(
            detect_project_provider("kimi:///Users/jack/.kimi/sessions/project-hash"),
            StatsProvider::Kimi
        );
        assert_eq!(
            detect_project_provider("copilot-cli:///Users/jack/workspace"),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_project_provider("copilot-desktop:///Users/jack/workspace"),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_project_provider("copilot:///Users/jack/workspace"),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_project_provider(
                "vscode:///Users/jack/Library/Application Support/Code/User/workspaceStorage/hash"
            ),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_project_provider("/Users/jack/.claude/projects/my-project"),
            StatsProvider::Claude
        );
        if let Some(root) = crate::commands::antigravity::get_antigravity_root() {
            let antigravity_path = root
                .join(".token-monitor")
                .join("rpc-cache")
                .join("v1")
                .to_string_lossy()
                .to_string();
            assert_eq!(
                detect_project_provider(&antigravity_path),
                StatsProvider::Antigravity
            );
        }
    }

    #[test]
    /// Verify detect session provider from path pattern.
    fn test_detect_session_provider_from_path_pattern() {
        assert_eq!(
            detect_session_provider("forgecode://workspace/ws-1/conversation/conv-1"),
            StatsProvider::ForgeCode
        );
        assert_eq!(
            detect_session_provider("forgecode-db://workspace/ws-1/conversation/conv-1"),
            StatsProvider::ForgeCode
        );
        assert_eq!(
            detect_session_provider("opencode://project/ses_abc"),
            StatsProvider::OpenCode
        );
        assert_eq!(
            detect_session_provider("cursor://composer-id-abc"),
            StatsProvider::Cursor
        );
        if let Some(root) = providers::grok::get_base_path() {
            let grok_session = PathBuf::from(root)
                .join("sessions")
                .join("%2FUsers%2Fjack%2Frepo")
                .join("session-id")
                .to_string_lossy()
                .to_string();
            assert_eq!(detect_session_provider(&grok_session), StatsProvider::Grok);
        }
        if let Some(root) = providers::kimi::get_base_path() {
            let kimi_session = PathBuf::from(root)
                .join("sessions")
                .join("project-hash")
                .join("session-id")
                .to_string_lossy()
                .to_string();
            assert_eq!(detect_session_provider(&kimi_session), StatsProvider::Kimi);
        }
        assert_eq!(
            detect_session_provider("/Users/jack/.copilot/session-state/abcd-1234/events.jsonl"),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_session_provider(
                r"C:\Users\jack\AppData\Roaming\Code\User\workspaceStorage\hash\chatSessions\session.jsonl"
            ),
            StatsProvider::Copilot
        );
        assert_eq!(
            detect_session_provider(
                "/Users/jack/.codex/sessions/2026/02/20/rollout-2026-02-20T11-04-52-1234.jsonl"
            ),
            StatsProvider::Codex
        );
        assert_eq!(
            detect_session_provider(
                "/Users/jack/.claude/projects/-Users-jack-client-repo/1234-5678-90ab.jsonl"
            ),
            StatsProvider::Claude
        );
        if let Some(root) = crate::commands::antigravity::get_antigravity_root() {
            let antigravity_session = root
                .join(".token-monitor")
                .join("rpc-cache")
                .join("v1")
                .join("session-abc")
                .to_string_lossy()
                .to_string();
            assert_eq!(
                detect_session_provider(&antigravity_session),
                StatsProvider::Antigravity
            );
        }
    }

    #[test]
    #[serial]
    fn detect_session_provider_uses_copilot_cli_home_env() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp
            .path()
            .join("session-state")
            .join("abcd-1234")
            .join("events.jsonl");
        fs::create_dir_all(events_path.parent().unwrap()).unwrap();
        fs::write(&events_path, "").unwrap();
        let _env_guard = EnvVarGuard::set("COPILOT_CLI_HOME", tmp.path());

        assert_eq!(
            detect_session_provider(&events_path.to_string_lossy()),
            StatsProvider::Copilot
        );
    }

    #[test]
    /// Verify parse active stats providers defaults to all.
    fn test_parse_active_stats_providers_defaults_to_all() {
        let providers = parse_active_stats_providers(None);
        assert!(providers.contains(&StatsProvider::Claude));
        assert!(providers.contains(&StatsProvider::Codex));
        assert!(providers.contains(&StatsProvider::ForgeCode));
        assert!(providers.contains(&StatsProvider::OpenCode));
        assert!(providers.contains(&StatsProvider::Grok));
        assert!(providers.contains(&StatsProvider::Kimi));
        assert!(providers.contains(&StatsProvider::Antigravity));
        assert!(providers.contains(&StatsProvider::Copilot));
        assert!(providers.contains(&StatsProvider::Cursor));
    }

    #[test]
    fn test_parse_active_stats_providers_covers_every_supported_provider() {
        let supported = all_stats_providers();
        let ids = supported
            .iter()
            .map(|provider| stats_provider_id(*provider).to_string())
            .collect::<Vec<_>>();
        let parsed = parse_active_stats_providers(Some(ids));

        assert_eq!(parsed, supported);
        assert_eq!(supported.len(), 29);
    }

    #[test]
    /// Verify parse active stats providers filters unknown values.
    fn test_parse_active_stats_providers_filters_unknown_values() {
        let providers =
            parse_active_stats_providers(Some(vec!["claude".to_string(), "unknown".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::Claude));
    }

    #[test]
    /// Verify parse active stats providers returns empty for unknown only values.
    fn test_parse_active_stats_providers_returns_empty_for_unknown_only_values() {
        let providers = parse_active_stats_providers(Some(vec!["invalid".to_string()]));
        assert!(providers.is_empty());
    }

    #[test]
    /// Verify parse active stats providers returns empty for empty list.
    fn test_parse_active_stats_providers_returns_empty_for_empty_list() {
        let providers = parse_active_stats_providers(Some(vec![]));
        assert!(providers.is_empty());
    }

    #[test]
    /// Verify parse active stats providers supports forgecode.
    fn test_parse_active_stats_providers_supports_forgecode() {
        let providers = parse_active_stats_providers(Some(vec!["forgecode".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::ForgeCode));
    }

    #[test]
    /// Verify parse active stats providers supports Grok.
    fn test_parse_active_stats_providers_supports_grok() {
        let providers = parse_active_stats_providers(Some(vec!["grok".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::Grok));
    }

    #[test]
    /// Verify parse active stats providers supports Cursor.
    fn test_parse_active_stats_providers_supports_cursor() {
        let providers = parse_active_stats_providers(Some(vec!["cursor".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::Cursor));
    }

    #[test]
    /// Verify parse active stats providers supports Kimi.
    fn test_parse_active_stats_providers_supports_kimi() {
        let providers = parse_active_stats_providers(Some(vec!["kimi".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::Kimi));
    }

    #[test]
    /// Verify parse active stats providers supports Copilot.
    fn test_parse_active_stats_providers_supports_copilot() {
        let providers = parse_active_stats_providers(Some(vec!["copilot".to_string()]));
        assert_eq!(providers.len(), 1);
        assert!(providers.contains(&StatsProvider::Copilot));
    }

    #[test]
    /// Verify parse stats mode defaults and unknown.
    fn test_parse_stats_mode_defaults_and_unknown() {
        assert_eq!(parse_stats_mode(None), StatsMode::BillingTotal);
        assert_eq!(
            parse_stats_mode(Some("billing_total".to_string())),
            StatsMode::BillingTotal
        );
        assert_eq!(
            parse_stats_mode(Some("conversation_only".to_string())),
            StatsMode::ConversationOnly
        );
        assert_eq!(
            parse_stats_mode(Some("invalid_mode".to_string())),
            StatsMode::BillingTotal
        );
    }

    #[test]
    /// Verify should include stats entry sidechain mode switch.
    fn test_should_include_stats_entry_sidechain_mode_switch() {
        assert!(should_include_stats_entry(
            "assistant",
            Some(true),
            true,
            StatsMode::BillingTotal
        ));
        assert!(!should_include_stats_entry(
            "assistant",
            Some(true),
            true,
            StatsMode::ConversationOnly
        ));
        assert!(!should_include_stats_entry(
            "summary",
            Some(false),
            true,
            StatsMode::BillingTotal
        ));
        assert!(!should_include_stats_entry(
            "progress",
            Some(false),
            false,
            StatsMode::BillingTotal
        ));
        assert!(should_include_stats_entry(
            "progress",
            Some(false),
            true,
            StatsMode::BillingTotal
        ));
        assert!(should_include_stats_entry(
            "system",
            Some(false),
            true,
            StatsMode::BillingTotal
        ));
        assert!(!should_include_stats_entry(
            "system",
            Some(false),
            true,
            StatsMode::ConversationOnly
        ));
        assert!(!should_include_stats_entry(
            "tool_result",
            Some(false),
            true,
            StatsMode::ConversationOnly
        ));
        assert!(!should_include_stats_entry(
            "tool",
            Some(false),
            false,
            StatsMode::ConversationOnly
        ));
        assert!(!should_include_stats_entry(
            "tool",
            Some(false),
            false,
            StatsMode::BillingTotal
        ));
    }

    #[tokio::test]
    #[serial]
    async fn get_project_stats_summary_accepts_grok_virtual_path() {
        let temp = TempDir::new().expect("temp dir");
        let encoded = "%2FUsers%2Ftest%2Fdemo";
        let session_id = "019fa555-791c-71e2-8c92-ff2e6fa26d6e";
        let project_dir = temp.path().join("sessions").join(encoded);
        let session_dir = project_dir.join(session_id);
        fs::create_dir_all(&session_dir).unwrap();

        fs::write(
            session_dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": session_id, "cwd": "/Users/test/demo" },
                "generated_title": "Demo",
                "created_at": "2026-07-27T20:47:50Z",
                "updated_at": "2026-07-27T21:11:25Z",
                "num_chat_messages": 2,
                "current_model_id": "grok-4.5"
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            session_dir.join("chat_history.jsonl"),
            r#"{"type":"user","content":"hello"}
{"type":"assistant","content":"hi","model_id":"grok-4.5"}
"#,
        )
        .unwrap();
        fs::write(
            session_dir.join("signals.json"),
            serde_json::json!({
                "contextTokensUsed": 42,
                "primaryModelId": "grok-4.5",
                "toolCallCount": 0
            })
            .to_string(),
        )
        .unwrap();

        let _env = EnvVarGuard::set("GROK_HOME", temp.path());
        let project_path = format!("grok://{}", project_dir.to_string_lossy());

        assert_eq!(detect_project_provider(&project_path), StatsProvider::Grok);

        let summary = get_project_stats_summary(project_path.clone(), None, None, None)
            .await
            .expect("grok virtual project path should load stats");
        assert_eq!(summary.project_name, "demo");
        assert!(summary.total_sessions >= 1);
        assert!(summary.total_messages >= 1);
        assert_eq!(summary.total_tokens, 42);

        let global = get_global_stats_summary(
            "/tmp".to_string(),
            Some(vec!["grok".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("grok should contribute to global stats");
        assert!(
            global
                .provider_distribution
                .iter()
                .any(|provider| provider.provider_id == "grok" && provider.tokens >= 42),
            "expected grok in provider_distribution: {:?}",
            global.provider_distribution
        );
        assert!(
            global
                .model_distribution
                .iter()
                .any(|model| model.model_name.contains("grok") && model.token_count >= 42),
            "expected grok model in model_distribution: {:?}",
            global.model_distribution
        );
        assert!(
            global
                .top_projects
                .iter()
                .any(|project| project.project_name.contains("demo")),
            "expected grok project in top_projects: {:?}",
            global.top_projects
        );
    }

    /// Antigravity CLI shares `StatsProvider::Antigravity` with the desktop
    /// app (see `providers::antigravity_cli`'s module docs), but its sessions
    /// carry no external usage-log file, so the bespoke desktop stats logic
    /// (keyed to that file) always found zero records for CLI sessions and
    /// silently returned an empty summary. This must route through the
    /// generic, message-based stats path instead, the same as every other
    /// provider without rich usage data.
    ///
    /// `#[cfg(unix)]`: relies on a HOME override reaching `dirs::home_dir()`,
    /// which this codebase's tests don't trust on Windows -- matches the
    /// precedent set by `commands::session::load`'s `with_home` tests.
    #[cfg(unix)]
    #[tokio::test]
    #[serial]
    async fn get_project_stats_summary_routes_antigravity_cli_through_generic_path() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = EnvVarGuard::set("HOME", temp.path());
        let root = temp.path().join(".gemini").join("antigravity-cli");
        fs::create_dir_all(&root).unwrap();

        fs::write(
            root.join("history.jsonl"),
            r#"{"display": "Fix the parser", "timestamp": 1750500000000, "workspace": "/tmp/proj-a", "conversationId": "conv-aaa"}"#,
        )
        .unwrap();

        let session_dir = root.join("brain").join("conv-aaa");
        let logs_dir = session_dir.join(".system_generated").join("logs");
        fs::create_dir_all(&logs_dir).unwrap();
        fs::write(
            logs_dir.join("transcript_full.jsonl"),
            [
                r#"{"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "content": "Fix the parser", "created_at": "2026-06-21T10:00:00Z"}"#,
                r#"{"step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "content": "Here is my plan", "created_at": "2026-06-21T10:00:05Z"}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let project_path = "antigravity-cli:///tmp/proj-a".to_string();
        assert_eq!(
            detect_project_provider(&project_path),
            StatsProvider::Antigravity
        );
        assert!(is_antigravity_cli_path(&project_path));

        let summary = get_project_stats_summary(project_path, None, None, None)
            .await
            .expect("antigravity-cli project path should load real stats, not an empty summary");
        assert_eq!(summary.total_sessions, 1);
        assert_eq!(summary.total_messages, 2);

        let session_path = session_dir.to_string_lossy().to_string();
        assert_eq!(
            detect_session_provider(&session_path),
            StatsProvider::Antigravity
        );
        assert!(is_antigravity_cli_path(&session_path));

        let session_stats = get_session_token_stats(session_path, None, None, None)
            .await
            .expect("antigravity-cli session path should load real stats");
        assert_eq!(session_stats.message_count, 2);
    }

    #[tokio::test]
    #[serial]
    async fn get_project_stats_summary_accepts_cursor_virtual_path() {
        let temp = TempDir::new().expect("temp dir");
        let user_dir = temp.path().join("Cursor").join("User");
        let ws_path = user_dir.join("workspaceStorage").join("hash-demo");
        fs::create_dir_all(&ws_path).unwrap();
        fs::create_dir_all(user_dir.join("globalStorage")).unwrap();

        fs::write(
            ws_path.join("workspace.json"),
            r#"{"folder":"file:///Users/test/demo"}"#,
        )
        .unwrap();

        let ws_conn = rusqlite::Connection::open(ws_path.join("state.vscdb")).unwrap();
        ws_conn
            .execute(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value TEXT)",
                [],
            )
            .unwrap();
        let composers = json!({
            "allComposers": [{
                "composerId": "comp-demo",
                "name": "Demo chat",
                "createdAt": 1_700_000_000_000u64,
                "lastUpdatedAt": 1_700_000_100_000u64,
                "isArchived": false,
                "unifiedMode": "agent"
            }]
        })
        .to_string();
        ws_conn
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES ('composer.composerData', ?1)",
                [&composers],
            )
            .unwrap();

        let global_conn =
            rusqlite::Connection::open(user_dir.join("globalStorage").join("state.vscdb")).unwrap();
        global_conn
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value TEXT)",
                [],
            )
            .unwrap();
        let composer_data = json!({
            "fullConversationHeadersOnly": [
                { "bubbleId": "b1", "type": 1 },
                { "bubbleId": "b2", "type": 2 }
            ],
            "promptTokenBreakdown": { "totalUsedTokens": 4200 }
        })
        .to_string();
        global_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params!["composerData:comp-demo", composer_data],
            )
            .unwrap();
        global_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    "bubbleId:comp-demo:b1",
                    json!({
                        "bubbleId": "b1",
                        "type": 1,
                        "text": "hello cursor",
                        "createdAt": "2026-07-27T20:00:00Z"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        global_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    "bubbleId:comp-demo:b2",
                    json!({
                        "bubbleId": "b2",
                        "type": 2,
                        "text": "hi from composer",
                        "createdAt": "2026-07-27T20:01:00Z"
                    })
                    .to_string()
                ],
            )
            .unwrap();

        let _env = EnvVarGuard::set("CURSOR_USER_DIR", &user_dir);
        let project_path = format!("cursor://{}", ws_path.to_string_lossy());

        assert_eq!(
            detect_project_provider(&project_path),
            StatsProvider::Cursor
        );

        let summary = get_project_stats_summary(project_path.clone(), None, None, None)
            .await
            .expect("cursor virtual project path should load stats");
        assert_eq!(summary.project_name, "demo");
        assert!(summary.total_sessions >= 1);
        assert!(summary.total_messages >= 1);

        let global = get_global_stats_summary(
            "/tmp".to_string(),
            Some(vec!["cursor".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("cursor should contribute to global stats");
        assert!(
            global
                .provider_distribution
                .iter()
                .any(|provider| provider.provider_id == "cursor" && provider.sessions >= 1),
            "expected cursor in provider_distribution: {:?}",
            global.provider_distribution
        );
        assert!(
            global
                .model_distribution
                .iter()
                .any(|model| model.model_name == "cursor" && model.token_count >= 4200),
            "expected cursor in model_distribution: {:?}",
            global.model_distribution
        );
        assert!(
            global
                .provider_distribution
                .iter()
                .any(|provider| provider.provider_id == "cursor" && provider.tokens >= 4200),
            "expected cursor tokens in provider_distribution: {:?}",
            global.provider_distribution
        );
    }

    #[test]
    fn test_kimi_project_name_resolves_from_session_parent_directory() {
        let session_path = "/tmp/kimi/sessions/project-hash/session-1";

        assert_eq!(
            resolve_provider_project_name_from_session(StatsProvider::Kimi, session_path),
            "project-hash"
        );
    }

    #[test]
    fn test_antigravity_conversation_breakdown_uses_chat_message_tokens() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        // resolve_usage_jsonl_path validates the canonical session_path is
        // under a marker-rooted antigravity root before reading. Create the
        // marker so this loose-fixture test goes through the same security
        // path as production callers.
        fs::create_dir_all(
            temp_dir
                .path()
                .join(".token-monitor")
                .join("rpc-cache")
                .join("v1"),
        )
        .expect("failed to create antigravity marker");
        let session_dir = temp_dir.path().join("session-123");
        fs::create_dir_all(&session_dir).expect("failed to create session dir");

        let usage_record = json!({
            "recordType": "usage",
            "sessionId": "session-123",
            "sequence": 0,
            "model": "claude-sonnet-4-6",
            "inputTokens": 1000,
            "outputTokens": 200,
            "cacheReadTokens": 600,
            "cacheWriteTokens": 100,
            "reasoningTokens": 50,
            "totalTokens": 1950,
            "raw": {
                "chatModel": {
                    "chatStartMetadata": {
                        "createdAt": "2026-04-14T16:28:44Z",
                        "contextWindowMetadata": {
                            "tokenBreakdown": {
                                "groups": [
                                    {
                                        "name": "System Prompt",
                                        "type": "TOKEN_TYPE_SYSTEM_PROMPT",
                                        "numTokens": 300
                                    },
                                    {
                                        "name": "Tools",
                                        "type": "TOKEN_TYPE_TOOLS",
                                        "numTokens": 300
                                    },
                                    {
                                        "name": "Chat Messages",
                                        "type": "TOKEN_TYPE_CHAT_MESSAGES",
                                        "numTokens": 400
                                    }
                                ],
                                "totalTokens": 1000
                            }
                        }
                    }
                }
            }
        });

        fs::write(session_dir.join("usage.jsonl"), format!("{usage_record}\n"))
            .expect("failed to write usage file");

        let session = crate::models::ClaudeSession {
            session_id: "session-123".to_string(),
            actual_session_id: "session-123".to_string(),
            file_path: session_dir.to_string_lossy().to_string(),
            project_name: "Antigravity".to_string(),
            message_count: 1,
            first_message_time: "2026-04-14T16:28:44Z".to_string(),
            last_message_time: "2026-04-14T16:28:44Z".to_string(),
            last_modified: "2026-04-14T16:28:44Z".to_string(),
            has_tool_use: true,
            has_errors: false,
            summary: None,
            is_renamed: false,
            provider: Some("antigravity".to_string()),
            storage_type: None,
            entrypoint: None,
        };

        let (billing_stats, _) =
            build_antigravity_session_token_stats(&session, StatsMode::BillingTotal, None, None)
                .expect("billing stats should parse")
                .expect("billing stats should exist");
        let (conversation_stats, _) = build_antigravity_session_token_stats(
            &session,
            StatsMode::ConversationOnly,
            None,
            None,
        )
        .expect("conversation stats should parse")
        .expect("conversation stats should exist");

        assert_eq!(billing_stats.total_tokens, 1950);
        assert_eq!(conversation_stats.total_input_tokens, 400);
        assert_eq!(conversation_stats.total_cache_read_tokens, 240);
        assert_eq!(conversation_stats.total_cache_creation_tokens, 40);
        assert_eq!(conversation_stats.total_output_tokens, 200);
        assert_eq!(conversation_stats.total_reasoning_tokens, 50);
        assert_eq!(conversation_stats.total_tokens, 930);
        assert!(conversation_stats.total_tokens < billing_stats.total_tokens);
    }

    #[test]
    fn test_should_include_stats_message_skips_synthetic_antigravity_prompt() {
        let synthetic_prompt = make_test_message(Some("antigravity"), "user", None);
        assert!(!should_include_stats_message(
            &synthetic_prompt,
            StatsMode::BillingTotal
        ));

        let usage_message = make_test_message(
            Some("antigravity"),
            "assistant",
            Some(TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_tokens: None,
                service_tier: None,
                ..Default::default()
            }),
        );
        assert!(should_include_stats_message(
            &usage_message,
            StatsMode::BillingTotal
        ));
    }

    #[tokio::test]
    /// End-to-end through the command: repeat `get_global_stats_summary`
    /// calls (including a date-filtered one) are served from the per-file
    /// cache, and a file append is detected and re-parsed.
    async fn test_global_stats_summary_serves_cache_and_detects_file_changes() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        // Isolate archive_db_path() from this dev machine's real
        // ~/.grid-local/archive.db -- without this, the Claude phase
        // prefers the (real, populated) archive over this test's own
        // fixture files and asserts against unrelated live data.
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp_dir.path());
        let claude_path = temp_dir.path();
        let project_dir = claude_path.join("projects").join("cache-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");
        let session_path = project_dir.join("session-cache.jsonl");

        let mut file = File::create(&session_path).expect("failed to create session file");
        let day1 = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2025-01-01T10:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"day1"}],"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":100,"output_tokens":10}},"isSidechain":false}"#;
        let day2 = r#"{"uuid":"u2","sessionId":"s1","timestamp":"2025-01-02T10:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"day2"}],"id":"m2","model":"claude-sonnet-4","usage":{"input_tokens":20,"output_tokens":2}},"isSidechain":false}"#;
        writeln!(file, "{day1}").expect("failed to write day1");
        writeln!(file, "{day2}").expect("failed to write day2");
        drop(file);

        let claude_path_str = claude_path.to_string_lossy().to_string();
        let first = get_global_stats_summary(
            claude_path_str.clone(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("first global summary");
        assert_eq!(first.total_tokens, 132);
        assert_eq!(cache::test_build_count(&session_path), 1);

        // Unchanged file: the repeat call and a day-filtered call both
        // compose from the cached daily buckets without re-parsing.
        let second = get_global_stats_summary(
            claude_path_str.clone(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("second global summary");
        assert_eq!(second.total_tokens, first.total_tokens);
        let filtered = get_global_stats_summary(
            claude_path_str.clone(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            Some("2025-01-02T00:00:00Z".to_string()),
            Some("2025-01-02T23:59:59.999Z".to_string()),
            None,
        )
        .await
        .expect("filtered global summary");
        assert_eq!(filtered.total_tokens, 22);
        assert_eq!(filtered.total_messages, 1);
        assert_eq!(
            cache::test_build_count(&session_path),
            1,
            "unchanged file must be served from cache"
        );

        // Append a new message: (size, mtime) changes force a re-parse.
        let mut appender = fs::OpenOptions::new()
            .append(true)
            .open(&session_path)
            .expect("failed to open session for append");
        let day3 = r#"{"uuid":"u3","sessionId":"s1","timestamp":"2025-01-03T10:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"day3"}],"id":"m3","model":"claude-sonnet-4","usage":{"input_tokens":5,"output_tokens":3}},"isSidechain":false}"#;
        writeln!(appender, "{day3}").expect("failed to append day3");
        drop(appender);

        let third = get_global_stats_summary(
            claude_path_str,
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("third global summary");
        assert_eq!(third.total_tokens, 140);
        assert_eq!(third.total_messages, 3);
        assert_eq!(
            cache::test_build_count(&session_path),
            2,
            "appended file must be re-parsed exactly once"
        );
    }

    #[tokio::test]
    async fn test_global_model_distribution_preserves_source_cost() {
        let temp_dir = TempDir::new().expect("temp dir");
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp_dir.path());
        let project_dir = temp_dir.path().join("projects").join("cost-project");
        fs::create_dir_all(&project_dir).expect("project dir");
        let session_path = project_dir.join("session-cost.jsonl");
        let entry = json!({
            "uuid": "cost-row",
            "sessionId": "cost-session",
            "timestamp": "2026-08-01T10:00:00Z",
            "type": "assistant",
            "costUSD": 0.005,
            "isSidechain": false,
            "message": {
                "role": "assistant",
                "id": "cost-message",
                "model": "claude-sonnet-4-6",
                "content": [{"type": "text", "text": "priced"}],
                "usage": {"input_tokens": 100, "output_tokens": 50}
            }
        });
        // Claude may repeat the complete source cost on split rows for one
        // assistant turn. The duplicate uuid must not make billing double.
        let mut duplicate = entry.clone();
        duplicate["uuid"] = json!("cost-row-duplicate");
        fs::write(&session_path, format!("{entry}\n{duplicate}\n")).expect("session file");

        let summary = get_global_stats_summary(
            temp_dir.path().to_string_lossy().to_string(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("global summary");

        let model = summary
            .model_distribution
            .iter()
            .find(|model| model.model_name == "claude-sonnet-4-6")
            .expect("model distribution entry");
        assert_eq!(model.provider_id.as_deref(), Some("claude"));
        assert_eq!(model.token_count, 150);
        assert_eq!(model.cost_usd, Some(0.005));
    }

    #[tokio::test]
    /// Verify project summary session count matches token list in conversation mode.
    async fn test_project_summary_session_count_matches_token_list_in_conversation_mode() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let claude_path = temp_dir.path();
        let project_dir = claude_path.join("projects").join("demo-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");

        let session_main = project_dir.join("session-main.jsonl");
        let session_sidechain = project_dir.join("session-sidechain.jsonl");

        let mut main_file = File::create(&session_main).expect("failed to create main session");
        let main_line = r#"{"uuid":"u1","sessionId":"s-main","timestamp":"2025-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"main"}],"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":50,"output_tokens":5}},"isSidechain":false}"#;
        writeln!(main_file, "{main_line}").expect("failed to write main line");

        let mut sidechain_file =
            File::create(&session_sidechain).expect("failed to create sidechain session");
        let sidechain_line = r#"{"uuid":"u2","sessionId":"s-side","timestamp":"2025-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"side"}],"id":"m2","model":"claude-sonnet-4","usage":{"input_tokens":70,"output_tokens":7}},"isSidechain":true}"#;
        writeln!(sidechain_file, "{sidechain_line}").expect("failed to write sidechain line");

        let project_path_str = project_dir.to_string_lossy().to_string();

        let project_summary = get_project_stats_summary(
            project_path_str.clone(),
            None,
            None,
            Some("conversation_only".to_string()),
        )
        .await
        .expect("failed to get project summary");

        let token_list = get_project_token_stats(
            project_path_str.clone(),
            Some(0),
            Some(20),
            None,
            None,
            Some("conversation_only".to_string()),
        )
        .await
        .expect("failed to get project token stats");

        assert_eq!(
            project_summary.total_sessions as usize,
            token_list.total_count
        );
        assert_eq!(project_summary.total_sessions, 1);
        assert_eq!(token_list.items.len(), 1);
        assert_eq!(token_list.items[0].session_id, "s-main");
    }

    #[tokio::test]
    /// Verify stats mode reconciles global project and session totals.
    async fn test_stats_mode_reconciles_global_project_and_session_totals() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp_dir.path());
        let claude_path = temp_dir.path();
        let project_dir = claude_path.join("projects").join("demo-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");
        let session_path = project_dir.join("session-1.jsonl");

        let mut file = File::create(&session_path).expect("failed to create session file");
        let line1 = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2025-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"main"}],"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":100,"output_tokens":10}},"isSidechain":false}"#;
        let line2 = r#"{"uuid":"u2","sessionId":"s1","timestamp":"2025-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"sidechain"}],"id":"m2","model":"claude-sonnet-4","usage":{"input_tokens":200,"output_tokens":20}},"isSidechain":true}"#;
        writeln!(file, "{line1}").expect("failed to write line1");
        writeln!(file, "{line2}").expect("failed to write line2");

        let claude_path_str = claude_path.to_string_lossy().to_string();
        let project_path_str = project_dir.to_string_lossy().to_string();
        let session_path_str = session_path.to_string_lossy().to_string();

        let global_billing = get_global_stats_summary(
            claude_path_str.clone(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("failed to get global billing stats");
        let global_conversation = get_global_stats_summary(
            claude_path_str,
            Some(vec!["claude".to_string()]),
            Some("conversation_only".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("failed to get global conversation stats");

        assert_eq!(global_billing.total_tokens, 330);
        assert_eq!(global_conversation.total_tokens, 110);

        let project_billing = get_project_stats_summary(
            project_path_str.clone(),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get project billing stats");
        let project_conversation = get_project_stats_summary(
            project_path_str.clone(),
            None,
            None,
            Some("conversation_only".to_string()),
        )
        .await
        .expect("failed to get project conversation stats");

        assert_eq!(project_billing.total_tokens, global_billing.total_tokens);
        assert_eq!(
            project_conversation.total_tokens,
            global_conversation.total_tokens
        );

        let project_token_billing = get_project_token_stats(
            project_path_str.clone(),
            Some(0),
            Some(20),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get project token billing stats");
        let project_token_conversation = get_project_token_stats(
            project_path_str,
            Some(0),
            Some(20),
            None,
            None,
            Some("conversation_only".to_string()),
        )
        .await
        .expect("failed to get project token conversation stats");

        let total_project_token_billing: u64 = project_token_billing
            .items
            .iter()
            .map(|s| s.total_tokens)
            .sum();
        let total_project_token_conversation: u64 = project_token_conversation
            .items
            .iter()
            .map(|s| s.total_tokens)
            .sum();
        assert_eq!(total_project_token_billing, global_billing.total_tokens);
        assert_eq!(
            total_project_token_conversation,
            global_conversation.total_tokens
        );

        let session_billing = get_session_token_stats(
            session_path_str.clone(),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get session billing stats");
        let session_conversation = get_session_token_stats(
            session_path_str,
            None,
            None,
            Some("conversation_only".to_string()),
        )
        .await
        .expect("failed to get session conversation stats");

        assert_eq!(session_billing.total_tokens, global_billing.total_tokens);
        assert_eq!(
            session_conversation.total_tokens,
            global_conversation.total_tokens
        );
    }

    #[tokio::test]
    /// Verify session token stats respects date filter.
    async fn test_session_token_stats_respects_date_filter() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_dir = temp_dir.path().join("projects").join("demo-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");
        let session_path = project_dir.join("session-date-filter.jsonl");

        let mut file = File::create(&session_path).expect("failed to create session file");
        let day1 = r#"{"uuid":"u1","sessionId":"s-date","timestamp":"2025-01-01T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"day1"}],"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":10,"output_tokens":1}},"isSidechain":false}"#;
        let day2 = r#"{"uuid":"u2","sessionId":"s-date","timestamp":"2025-01-02T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"day2"}],"id":"m2","model":"claude-sonnet-4","usage":{"input_tokens":20,"output_tokens":2}},"isSidechain":false}"#;
        writeln!(file, "{day1}").expect("failed to write day1");
        writeln!(file, "{day2}").expect("failed to write day2");

        // Per-message filtering: only day2 (Jan 2) is in range.
        let stats = get_session_token_stats(
            session_path.to_string_lossy().to_string(),
            Some("2025-01-02T00:00:00Z".to_string()),
            Some("2025-01-02T23:59:59.999Z".to_string()),
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get filtered session stats");

        assert_eq!(stats.message_count, 1);
        assert_eq!(stats.total_input_tokens, 20);
        assert_eq!(stats.total_output_tokens, 2);
        assert_eq!(stats.total_tokens, 22);

        // Per-message filtering: only day1 (Jan 1) is in range.
        let day1_stats = get_session_token_stats(
            session_path.to_string_lossy().to_string(),
            Some("2025-01-01T00:00:00Z".to_string()),
            Some("2025-01-01T23:59:59.999Z".to_string()),
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get day1 filtered session stats");

        assert_eq!(day1_stats.message_count, 1);
        assert_eq!(day1_stats.total_input_tokens, 10);
        assert_eq!(day1_stats.total_output_tokens, 1);
        assert_eq!(day1_stats.total_tokens, 11);

        // No messages in range → error.
        let filtered_out = get_session_token_stats(
            session_path.to_string_lossy().to_string(),
            Some("2024-12-01T00:00:00Z".to_string()),
            Some("2024-12-31T23:59:59.999Z".to_string()),
            Some("billing_total".to_string()),
        )
        .await;
        assert!(filtered_out.is_err());
    }

    #[tokio::test]
    /// Verify session comparison respects date filter.
    async fn test_session_comparison_respects_date_filter() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_dir = temp_dir.path().join("projects").join("demo-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");

        let session_a = project_dir.join("session-a.jsonl");
        let session_b = project_dir.join("session-b.jsonl");

        let mut file_a = File::create(&session_a).expect("failed to create session a");
        let line_a = r#"{"uuid":"ua","sessionId":"s-a","timestamp":"2025-01-01T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a"}],"id":"ma","model":"claude-sonnet-4","usage":{"input_tokens":10,"output_tokens":1}},"isSidechain":false}"#;
        writeln!(file_a, "{line_a}").expect("failed to write session a");

        let mut file_b = File::create(&session_b).expect("failed to create session b");
        let line_b = r#"{"uuid":"ub","sessionId":"s-b","timestamp":"2025-01-02T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"b"}],"id":"mb","model":"claude-sonnet-4","usage":{"input_tokens":20,"output_tokens":2}},"isSidechain":false}"#;
        writeln!(file_b, "{line_b}").expect("failed to write session b");

        let project_path = project_dir.to_string_lossy().to_string();

        let comparison = get_session_comparison(
            "s-b".to_string(),
            project_path.clone(),
            Some("2025-01-02T00:00:00Z".to_string()),
            Some("2025-01-02T23:59:59.999Z".to_string()),
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get filtered comparison");
        assert_eq!(comparison.session_id, "s-b");
        assert_eq!(comparison.rank_by_tokens, 1);

        let filtered_out = get_session_comparison(
            "s-a".to_string(),
            project_path,
            Some("2025-01-02T00:00:00Z".to_string()),
            Some("2025-01-02T23:59:59.999Z".to_string()),
            Some("billing_total".to_string()),
        )
        .await;
        assert!(filtered_out.is_err());
    }

    #[tokio::test]
    /// Verify project summary daily session count tracks multiple sessions on same day.
    async fn test_project_summary_daily_session_count_tracks_multiple_sessions_on_same_day() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_dir = temp_dir.path().join("projects").join("demo-project");
        fs::create_dir_all(&project_dir).expect("failed to create project dir");

        let session_a = project_dir.join("session-a.jsonl");
        let session_b = project_dir.join("session-b.jsonl");

        let mut file_a = File::create(&session_a).expect("failed to create session a");
        let line_a = r#"{"uuid":"ua","sessionId":"s-a","timestamp":"2025-01-01T08:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a"}],"id":"ma","model":"claude-sonnet-4","usage":{"input_tokens":10,"output_tokens":1}},"isSidechain":false}"#;
        writeln!(file_a, "{line_a}").expect("failed to write session a");

        let mut file_b = File::create(&session_b).expect("failed to create session b");
        let line_b = r#"{"uuid":"ub","sessionId":"s-b","timestamp":"2025-01-01T20:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"b"}],"id":"mb","model":"claude-sonnet-4","usage":{"input_tokens":20,"output_tokens":2}},"isSidechain":false}"#;
        writeln!(file_b, "{line_b}").expect("failed to write session b");

        let summary = get_project_stats_summary(
            project_dir.to_string_lossy().to_string(),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get project summary");

        assert_eq!(summary.total_sessions, 2);
        let jan1 = summary
            .daily_stats
            .iter()
            .find(|daily| daily.date == "2025-01-01")
            .expect("missing jan1 daily stat");
        assert_eq!(jan1.session_count, 2);
    }

    #[test]
    /// Verify `track_antigravity_tool_usage` honors the `start_date` / `end_date` window.
    fn test_track_antigravity_tool_usage_respects_date_filter() {
        let mk = |timestamp: &str, tool: &str| {
            let mut msg = make_test_message(Some("antigravity"), "assistant", None);
            msg.content = Some(json!([
                { "type": "text", "text": "preamble" },
                { "type": "tool_use", "id": "t-1", "name": tool, "input": {} }
            ]));
            msg.timestamp = timestamp.to_string();
            msg
        };

        let messages = vec![
            mk("2026-01-01T10:00:00Z", "BrowserClick"),
            mk("2026-01-05T10:00:00Z", "BrowserGetDom"),
        ];

        // No filter → both tools tracked.
        let mut all = HashMap::new();
        track_antigravity_tool_usage(&messages, None, None, &mut all);
        assert_eq!(all.len(), 2);

        // Window covering only the second message → only its tool is tracked.
        let s = parse_date_limit(Some("2026-01-03T00:00:00Z".to_string()), "start_date");
        let e = parse_date_limit(Some("2026-01-31T00:00:00Z".to_string()), "end_date");
        let mut filtered = HashMap::new();
        track_antigravity_tool_usage(&messages, s.as_ref(), e.as_ref(), &mut filtered);
        assert_eq!(filtered.len(), 1);
        assert!(filtered.contains_key("BrowserGetDom"));
        assert!(!filtered.contains_key("BrowserClick"));

        // Window excluding both messages → empty.
        let s = parse_date_limit(Some("2026-02-01T00:00:00Z".to_string()), "start_date");
        let mut none = HashMap::new();
        track_antigravity_tool_usage(&messages, s.as_ref(), None, &mut none);
        assert!(none.is_empty());

        // Unparseable timestamp with an active filter is rejected (defensive).
        let mut bad_msg = make_test_message(Some("antigravity"), "assistant", None);
        bad_msg.content = Some(json!([
            { "type": "tool_use", "id": "t-2", "name": "BrowserClick", "input": {} }
        ]));
        bad_msg.timestamp = "not-a-timestamp".to_string();
        let s = parse_date_limit(Some("2020-01-01T00:00:00Z".to_string()), "start_date");
        let mut rejected = HashMap::new();
        track_antigravity_tool_usage(&[bad_msg], s.as_ref(), None, &mut rejected);
        assert!(rejected.is_empty());
    }

    #[test]
    fn test_antigravity_provider_project_summary_uses_mode_adjusted_daily_tokens() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root = temp_dir.path();
        let session_dir = root
            .join(".token-monitor")
            .join("rpc-cache")
            .join("v1")
            .join("session-123");
        fs::create_dir_all(&session_dir).expect("failed to create antigravity session dir");
        fs::create_dir_all(root.join("brain").join("session-123"))
            .expect("failed to create antigravity brain dir");

        let usage_record = json!({
            "recordType": "usage",
            "sessionId": "session-123",
            "sequence": 0,
            "model": "claude-sonnet-4-6",
            "inputTokens": 1000,
            "outputTokens": 200,
            "cacheReadTokens": 600,
            "cacheWriteTokens": 100,
            "reasoningTokens": 50,
            "totalTokens": 1950,
            "raw": {
                "chatModel": {
                    "chatStartMetadata": {
                        "createdAt": "2026-04-14T16:28:44Z",
                        "contextWindowMetadata": {
                            "tokenBreakdown": {
                                "groups": [
                                    {
                                        "name": "System Prompt",
                                        "type": "TOKEN_TYPE_SYSTEM_PROMPT",
                                        "numTokens": 300
                                    },
                                    {
                                        "name": "Tools",
                                        "type": "TOKEN_TYPE_TOOLS",
                                        "numTokens": 300
                                    },
                                    {
                                        "name": "Chat Messages",
                                        "type": "TOKEN_TYPE_CHAT_MESSAGES",
                                        "numTokens": 400
                                    }
                                ],
                                "totalTokens": 1000
                            }
                        }
                    }
                }
            }
        });

        fs::write(session_dir.join("usage.jsonl"), format!("{usage_record}\n"))
            .expect("failed to write antigravity usage file");

        let summary = get_provider_project_stats_summary(
            StatsProvider::Antigravity,
            &root.to_string_lossy(),
            None,
            None,
            StatsMode::ConversationOnly,
        )
        .expect("failed to build antigravity project summary");

        assert_eq!(summary.total_tokens, 930);
        assert_eq!(summary.token_distribution.input, 400);
        assert_eq!(summary.token_distribution.output, 200);

        let day = summary
            .daily_stats
            .iter()
            .find(|daily| daily.date == "2026-04-14")
            .expect("missing daily summary");
        assert_eq!(day.total_tokens, 930);
        assert_eq!(day.input_tokens, 400);
        assert_eq!(day.output_tokens, 200);

        let heatmap = summary
            .activity_heatmap
            .iter()
            .find(|entry| entry.hour == 16 && entry.day == 2)
            .expect("missing activity heatmap entry");
        assert_eq!(heatmap.tokens_used, 930);
    }

    #[test]
    /// `load_antigravity_usage_records` mirrors the rpc-cache fallback used by
    /// `providers::antigravity::load_messages` so a brain/-only session whose
    /// `usage.jsonl` lives in the rpc-cache contributes records (and therefore
    /// tokens) to per-session / project / global stats.
    fn test_load_antigravity_usage_records_falls_back_to_rpc_cache() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root = temp_dir.path();
        let rpc_v1 = root
            .join(".token-monitor")
            .join("rpc-cache")
            .join("v1")
            .join("session-brain-only");
        fs::create_dir_all(&rpc_v1).expect("failed to create rpc-cache session dir");

        // Brain/-only session — no in-place usage.jsonl.
        let brain_dir = root.join("brain").join("session-brain-only");
        fs::create_dir_all(&brain_dir).expect("failed to create brain dir");

        // The rpc-cache carries the actual usage record.
        let usage_record = json!({
            "recordType": "usage",
            "sessionId": "session-brain-only",
            "sequence": 0,
            "model": "claude-sonnet-4-6",
            "inputTokens": 1000,
            "outputTokens": 200,
            "cacheReadTokens": 600,
            "cacheWriteTokens": 100,
            "reasoningTokens": 50,
            "totalTokens": 1950,
            "raw": {
                "chatModel": {
                    "chatStartMetadata": {
                        "createdAt": "2026-04-14T16:28:44Z"
                    }
                }
            }
        });
        fs::write(rpc_v1.join("usage.jsonl"), format!("{usage_record}\n"))
            .expect("failed to write rpc-cache usage file");

        let records = load_antigravity_usage_records(&brain_dir.to_string_lossy())
            .expect("expected fallback to surface rpc-cache records");

        assert_eq!(
            records.len(),
            1,
            "fallback should surface the rpc-cache record"
        );
        let record = &records[0];
        assert_eq!(record.input_tokens, 1000);
        assert_eq!(record.output_tokens, 200);
        assert_eq!(record.total_tokens, 1950);
    }

    #[test]
    /// When neither in-session nor rpc-cache `usage.jsonl` exists, the helper
    /// returns `Ok(vec![])` (legacy behaviour preserved).
    fn test_load_antigravity_usage_records_returns_empty_when_missing() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root = temp_dir.path();
        fs::create_dir_all(root.join(".token-monitor").join("rpc-cache").join("v1"))
            .expect("failed to create rpc-cache root");

        let brain_dir = root.join("brain").join("session-none");
        fs::create_dir_all(&brain_dir).expect("failed to create brain dir");

        let records = load_antigravity_usage_records(&brain_dir.to_string_lossy())
            .expect("expected empty result");
        assert!(records.is_empty());
    }

    #[tokio::test]
    /// Verify global summary total projects respects date filter.
    async fn test_global_summary_total_projects_respects_date_filter() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp_dir.path());
        let claude_path = temp_dir.path();
        let project_a = claude_path.join("projects").join("demo-a");
        let project_b = claude_path.join("projects").join("demo-b");
        fs::create_dir_all(&project_a).expect("failed to create project a");
        fs::create_dir_all(&project_b).expect("failed to create project b");

        let session_a = project_a.join("session-a.jsonl");
        let session_b = project_b.join("session-b.jsonl");

        let mut file_a = File::create(&session_a).expect("failed to create session a");
        let line_a = r#"{"uuid":"ua","sessionId":"s-a","timestamp":"2025-01-01T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a"}],"id":"ma","model":"claude-sonnet-4","usage":{"input_tokens":10,"output_tokens":1}},"isSidechain":false}"#;
        writeln!(file_a, "{line_a}").expect("failed to write session a");

        let mut file_b = File::create(&session_b).expect("failed to create session b");
        let line_b = r#"{"uuid":"ub","sessionId":"s-b","timestamp":"2025-01-10T12:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"b"}],"id":"mb","model":"claude-sonnet-4","usage":{"input_tokens":20,"output_tokens":2}},"isSidechain":false}"#;
        writeln!(file_b, "{line_b}").expect("failed to write session b");

        let summary = get_global_stats_summary(
            claude_path.to_string_lossy().to_string(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            Some("2025-01-10T00:00:00Z".to_string()),
            Some("2025-01-10T23:59:59.999Z".to_string()),
            None,
        )
        .await
        .expect("failed to get filtered global summary");

        assert_eq!(summary.total_projects, 1);
        assert_eq!(summary.total_sessions, 1);
        assert_eq!(summary.total_tokens, 22);
    }

    /// Write one assistant session line with the given token counts under
    /// `<base>/projects/<project>/session.jsonl`.
    fn write_claude_session(base: &Path, project: &str, input: u32, output: u32) {
        let dir = base.join("projects").join(project);
        fs::create_dir_all(&dir).expect("failed to create project dir");
        let mut file = File::create(dir.join("session.jsonl")).expect("failed to create session");
        let line = format!(
            r#"{{"uuid":"u-{project}","sessionId":"s-{project}","timestamp":"2025-01-05T12:00:00Z","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"x"}}],"id":"m-{project}","model":"claude-sonnet-4","usage":{{"input_tokens":{input},"output_tokens":{output}}}}},"isSidechain":false}}"#
        );
        writeln!(file, "{line}").expect("failed to write session");
    }

    #[tokio::test]
    /// Global summary must aggregate custom Claude directories, not just the default
    /// root (#362) — and must NOT when no custom paths are supplied.
    async fn test_global_summary_includes_custom_claude_paths() {
        let default_dir = TempDir::new().expect("default tempdir");
        let _home_override_guard =
            EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", default_dir.path());
        let custom_dir = TempDir::new().expect("custom tempdir");
        write_claude_session(default_dir.path(), "proj-default", 10, 1);
        write_claude_session(custom_dir.path(), "proj-custom", 20, 2);

        let customs = Some(vec![
            crate::commands::multi_provider::CustomClaudePathParam {
                path: custom_dir.path().to_string_lossy().to_string(),
                label: Some("Personal".to_string()),
            },
        ]);

        let with_custom = get_global_stats_summary(
            default_dir.path().to_string_lossy().to_string(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            customs,
        )
        .await
        .expect("failed to get global summary with custom paths");
        assert_eq!(with_custom.total_projects, 2);
        assert_eq!(with_custom.total_sessions, 2);
        assert_eq!(with_custom.total_tokens, 11 + 22);

        // Control: without custom paths, only the default root is aggregated.
        let default_only = get_global_stats_summary(
            default_dir.path().to_string_lossy().to_string(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("failed to get default-only global summary");
        assert_eq!(default_only.total_projects, 1);
        assert_eq!(default_only.total_tokens, 11);
    }

    #[tokio::test]
    /// An invalid custom Claude path (no projects/ dir) is skipped, not fatal.
    async fn test_global_summary_skips_invalid_custom_claude_path() {
        let default_dir = TempDir::new().expect("default tempdir");
        let bogus_dir = TempDir::new().expect("bogus tempdir"); // exists but has no projects/
        write_claude_session(default_dir.path(), "proj-default", 10, 1);

        let customs = Some(vec![
            crate::commands::multi_provider::CustomClaudePathParam {
                path: bogus_dir.path().to_string_lossy().to_string(),
                label: None,
            },
        ]);

        let summary = get_global_stats_summary(
            default_dir.path().to_string_lossy().to_string(),
            Some(vec!["claude".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            customs,
        )
        .await
        .expect("invalid custom path must not be fatal");
        assert_eq!(summary.total_projects, 1);
        assert_eq!(summary.total_tokens, 11);
    }

    #[tokio::test]
    #[serial]
    /// Verify global summary accumulates `token_distribution.reasoning` from
    /// providers that emit reasoning tokens (Antigravity). Pre-fix, the
    /// aggregation loop dropped reasoning even though every other distribution
    /// field was carried through — leaving the UI's reasoning breakdown at 0
    /// no matter how many reasoning tokens the underlying sessions reported.
    async fn test_global_summary_aggregates_reasoning_tokens() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp_dir.path());
        let home = temp_dir.path();

        // Override HOME so resolve_antigravity_root() points at our fixture.
        // env::set_var is process-global → this test must be `#[serial]` so
        // it cannot race with other HOME-touching tests.
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);

        let antigravity_root = home.join(".gemini").join("antigravity");
        let rpc_session = antigravity_root
            .join(".token-monitor")
            .join("rpc-cache")
            .join("v1")
            .join("session-reasoning");
        fs::create_dir_all(&rpc_session).expect("failed to create rpc-cache session dir");

        let usage_record = json!({
            "recordType": "usage",
            "sessionId": "session-reasoning",
            "sequence": 0,
            "model": "claude-sonnet-4-6",
            "inputTokens": 100,
            "outputTokens": 50,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
            "reasoningTokens": 1234,
            "totalTokens": 1384,
            "raw": {
                "chatModel": {
                    "chatStartMetadata": { "createdAt": "2026-05-14T10:00:00Z" }
                }
            }
        });
        fs::write(rpc_session.join("usage.jsonl"), format!("{usage_record}\n"))
            .expect("failed to write antigravity usage file");

        // claude_path is required but the Claude projects subtree is empty —
        // we are only exercising the Antigravity branch of the global summary.
        let summary = get_global_stats_summary(
            home.to_string_lossy().to_string(),
            Some(vec!["antigravity".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("failed to get global summary");

        assert_eq!(
            summary.token_distribution.reasoning, 1234,
            "reasoning tokens must reach the global summary, not get dropped during aggregation"
        );
        // Sanity: the rest of the distribution still aggregates correctly.
        assert_eq!(summary.token_distribution.input, 100);
        assert_eq!(summary.token_distribution.output, 50);

        if let Some(value) = original_home {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }
    }

    /// Write a temporary `ForgeCode` database used by stats tests.
    fn write_forgecode_test_db(base_dir: &std::path::Path) {
        let db_path = base_dir.join(".forge.db");
        let conn = rusqlite::Connection::open(db_path).expect("create forgecode stats test db");
        conn.execute_batch(
            "CREATE TABLE conversations (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                title TEXT,
                context TEXT,
                metrics TEXT,
                created_at TEXT,
                updated_at TEXT
            );",
        )
        .expect("create forge conversations table");

        conn.execute(
            "INSERT INTO conversations (id, workspace_id, title, context, metrics, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "conv-001",
                "workspace-alpha",
                "Forge stats session",
                serde_json::to_string(&json!({
                    "conversation_id": "conv-001",
                    "cwd": "/Users/christian/projects/banana-prompting-service",
                    "messages": [
                        {
                            "Text": {
                                "role": "user",
                                "content": "Inspect src/main.rs",
                                "timestamp": "2026-01-10T08:00:00Z"
                            }
                        },
                        {
                            "message": {
                                "text": {
                                    "role": "assistant",
                                    "content": [
                                        { "type": "text", "text": "Done" },
                                        { "type": "tool_use", "id": "tool-456", "name": "Write", "input": { "file_path": "/tmp/out.rs" } }
                                    ],
                                    "model": "forge-model-v1",
                                    "usage": {
                                        "prompt_tokens": 120,
                                        "completion_tokens": 45,
                                        "cached_tokens": 30,
                                        "cost": 0.125
                                    },
                                    "timestamp": "2026-01-10T08:00:10Z"
                                }
                            }
                        }
                    ]
                }))
                .unwrap(),
                serde_json::to_string(&json!({
                    "session_start_time": "2026-01-10T08:00:00Z",
                    "file_operations": 1
                }))
                .unwrap(),
                "2026-01-10T08:00:00Z",
                "2026-01-10T08:00:10Z"
            ],
        )
        .expect("insert forge conversation");
    }

    #[tokio::test]
    #[serial]
    /// Verify forgecode stats commands use provider paths.
    async fn test_forgecode_stats_commands_use_provider_paths() {
        let forge_dir = TempDir::new().expect("failed to create forge temp dir");
        write_forgecode_test_db(forge_dir.path());

        let original_forge_config = std::env::var("FORGE_CONFIG").ok();
        std::env::set_var("FORGE_CONFIG", forge_dir.path());

        let project_path = "forgecode://workspace/workspace-alpha".to_string();
        let session_path =
            "forgecode-db://workspace/workspace-alpha/conversation/conv-001".to_string();

        let session_stats = get_session_token_stats(
            session_path.clone(),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get forgecode session stats");
        assert_eq!(session_stats.session_id, "conv-001");
        assert_eq!(session_stats.project_name, "banana-prompting-service");
        assert_eq!(session_stats.total_tokens, 165);
        assert_eq!(session_stats.message_count, 2);

        let project_stats = get_project_token_stats(
            project_path.clone(),
            Some(0),
            Some(20),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get forgecode project stats");
        assert_eq!(project_stats.total_count, 1);
        assert_eq!(
            project_stats.items[0].project_name,
            "banana-prompting-service"
        );
        assert_eq!(project_stats.items[0].total_tokens, 165);

        let summary = get_project_stats_summary(
            project_path.clone(),
            None,
            None,
            Some("billing_total".to_string()),
        )
        .await
        .expect("failed to get forgecode project summary");
        assert_eq!(summary.project_name, "banana-prompting-service");
        assert_eq!(summary.total_sessions, 1);
        assert_eq!(summary.total_tokens, 165);

        let global_summary = get_global_stats_summary(
            forge_dir.path().to_string_lossy().to_string(),
            Some(vec!["forgecode".to_string()]),
            Some("billing_total".to_string()),
            None,
            None,
            None,
        )
        .await
        .expect("failed to get forgecode global summary");
        assert_eq!(global_summary.total_projects, 1);
        assert_eq!(global_summary.total_sessions, 1);
        assert_eq!(global_summary.total_tokens, 165);
        assert_eq!(global_summary.provider_distribution.len(), 1);
        assert_eq!(
            global_summary.provider_distribution[0].provider_id,
            "forgecode"
        );

        if let Some(value) = original_forge_config {
            std::env::set_var("FORGE_CONFIG", value);
        } else {
            std::env::remove_var("FORGE_CONFIG");
        }
    }

    #[test]
    /// Verify calculate session active minutes handles long gaps.
    fn test_calculate_session_active_minutes_handles_long_gaps() {
        let mut timestamps = vec![
            DateTime::parse_from_rfc3339("2026-02-20T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            DateTime::parse_from_rfc3339("2026-02-20T10:20:00Z")
                .unwrap()
                .with_timezone(&Utc),
            DateTime::parse_from_rfc3339("2026-02-20T14:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            DateTime::parse_from_rfc3339("2026-02-20T14:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ];

        // 10:00~10:20(20분) + 14:00~14:30(30분) = 50분
        assert_eq!(calculate_session_active_minutes(&mut timestamps), 50);
    }

    // -----------------------------------------------------------------------
    // #283: token usage dedup tests
    //
    // Claude assistant turns split content (thinking/tool_use/text) across
    // multiple JSONL rows that share the same `message.id` and embed the same
    // `usage` payload. Aggregators must count rows but only add usage once.
    // -----------------------------------------------------------------------

    fn make_assistant_message(
        uuid: &str,
        session_id: &str,
        message_id: Option<&str>,
        timestamp: &str,
        usage: TokenUsage,
    ) -> ClaudeMessage {
        let raw = RawLogEntry {
            uuid: Some(uuid.to_string()),
            parent_uuid: None,
            session_id: Some(session_id.to_string()),
            timestamp: Some(timestamp.to_string()),
            message_type: "assistant".to_string(),
            summary: None,
            leaf_uuid: None,
            message: Some(MessageContent {
                role: "assistant".to_string(),
                content: json!([{"type": "text", "text": "ok"}]),
                id: message_id.map(str::to_string),
                model: Some("claude-opus-4-7".to_string()),
                stop_reason: None,
                usage: Some(usage),
            }),
            tool_use: None,
            tool_use_result: None,
            is_sidechain: Some(false),
            cwd: None,
            entrypoint: None,
            cost_usd: None,
            duration_ms: None,
            message_id: None,
            snapshot: None,
            is_snapshot_update: None,
            data: None,
            tool_use_id: None,
            parent_tool_use_id: None,
            operation: None,
            subtype: None,
            level: None,
            hook_count: None,
            hook_infos: None,
            stop_reason_system: None,
            prevented_continuation: None,
            compact_metadata: None,
            microcompact_metadata: None,
            content: None,
            is_meta: None,
        };
        ClaudeMessage::try_from(raw).expect("test message construction")
    }

    fn sample_usage() -> TokenUsage {
        TokenUsage {
            input_tokens: Some(6),
            output_tokens: Some(222),
            cache_creation_input_tokens: Some(28644),
            cache_read_input_tokens: Some(14732),
            reasoning_tokens: None,
            service_tier: Some("standard".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_dedup_global_stats_same_message_id_counts_usage_once() {
        // Two rows representing one assistant turn split across thinking + text
        // content blocks. They share message.id but have distinct uuids.
        let messages = vec![
            make_assistant_message(
                "uuid-thinking",
                "sess-1",
                Some("msg_shared"),
                "2026-04-27T10:00:00Z",
                sample_usage(),
            ),
            make_assistant_message(
                "uuid-text",
                "sess-1",
                Some("msg_shared"),
                "2026-04-27T10:00:01Z",
                sample_usage(),
            ),
        ];

        let stats = build_global_session_file_stats_from_messages(
            StatsProvider::Claude,
            "test-project".to_string(),
            &messages,
            StatsMode::BillingTotal,
            None,
            None,
        )
        .expect("stats");

        // Rows still counted as 2 messages.
        assert_eq!(stats.total_messages, 2);

        // Usage counted once: 6 + 222 + 28644 + 14732 = 43604
        assert_eq!(stats.token_distribution.input, 6);
        assert_eq!(stats.token_distribution.output, 222);
        assert_eq!(stats.token_distribution.cache_creation, 28644);
        assert_eq!(stats.token_distribution.cache_read, 14732);
        assert_eq!(stats.total_tokens, 6 + 222 + 28644 + 14732);

        // model.msg_count counts rows; model token totals are deduped.
        let model_entry = stats
            .model_usage
            .get("claude-opus-4-7")
            .expect("model entry");
        assert_eq!(model_entry.0, 2, "msg_count counts rows");
        assert_eq!(model_entry.2, 6, "model input tokens deduped");
        assert_eq!(model_entry.3, 222, "model output tokens deduped");
    }

    #[test]
    fn test_dedup_global_stats_distinct_message_ids_summed() {
        // Two rows representing two different assistant turns with same usage.
        let messages = vec![
            make_assistant_message(
                "uuid-a",
                "sess-1",
                Some("msg_a"),
                "2026-04-27T10:00:00Z",
                sample_usage(),
            ),
            make_assistant_message(
                "uuid-b",
                "sess-1",
                Some("msg_b"),
                "2026-04-27T10:00:01Z",
                sample_usage(),
            ),
        ];

        let stats = build_global_session_file_stats_from_messages(
            StatsProvider::Claude,
            "test-project".to_string(),
            &messages,
            StatsMode::BillingTotal,
            None,
            None,
        )
        .expect("stats");

        assert_eq!(stats.total_messages, 2);
        // Distinct ids → summed twice.
        assert_eq!(stats.token_distribution.input, 12);
        assert_eq!(stats.token_distribution.output, 444);
        assert_eq!(stats.total_tokens, 2 * (6 + 222 + 28644 + 14732));
    }

    #[test]
    fn test_dedup_global_stats_missing_message_id_counted_per_row() {
        // Older logs / providers without message.id: fall back to uuid keys
        // so each distinct row still contributes once.
        let messages = vec![
            make_assistant_message(
                "uuid-a",
                "sess-1",
                None,
                "2026-04-27T10:00:00Z",
                sample_usage(),
            ),
            make_assistant_message(
                "uuid-b",
                "sess-1",
                None,
                "2026-04-27T10:00:01Z",
                sample_usage(),
            ),
        ];

        let stats = build_global_session_file_stats_from_messages(
            StatsProvider::Claude,
            "test-project".to_string(),
            &messages,
            StatsMode::BillingTotal,
            None,
            None,
        )
        .expect("stats");

        assert_eq!(stats.total_messages, 2);
        assert_eq!(stats.total_tokens, 2 * (6 + 222 + 28644 + 14732));
    }

    #[test]
    fn test_global_model_distribution_keeps_token_usage_without_model_name() {
        let message = make_test_message(
            Some("qwen"),
            "assistant",
            Some(TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_tokens: Some(7),
                service_tier: None,
                ..Default::default()
            }),
        );

        let stats = build_global_session_file_stats_from_messages(
            StatsProvider::Qwen,
            "qwen-project".to_string(),
            &[message],
            StatsMode::BillingTotal,
            None,
            None,
        )
        .expect("stats");

        let model_entry = stats
            .model_usage
            .get(UNKNOWN_MODEL_NAME)
            .expect("unknown model");
        assert_eq!(model_entry.0, 1);
        assert_eq!(model_entry.1, 22);
        assert_eq!(model_entry.6, 7);
        assert_eq!(stats.total_tokens, 22);
    }

    #[test]
    fn test_dedup_token_totals_returns_full_when_first_seen() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let result = dedup_token_totals(&mut seen, "sess-1", Some("msg_a"), "uuid-1", &usage);
        assert_eq!(result, (6, 222, 28644, 14732, 0, 6 + 222 + 28644 + 14732));
    }

    #[test]
    fn test_dedup_token_totals_returns_zero_when_duplicate() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let _ = dedup_token_totals(&mut seen, "sess-1", Some("msg_a"), "uuid-1", &usage);
        let result = dedup_token_totals(&mut seen, "sess-1", Some("msg_a"), "uuid-2", &usage);
        assert_eq!(result, (0, 0, 0, 0, 0, 0), "duplicate by message_id");
    }

    #[test]
    fn test_dedup_token_totals_distinct_ids_summed_separately() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let r1 = dedup_token_totals(&mut seen, "sess-1", Some("msg_a"), "uuid-1", &usage);
        let r2 = dedup_token_totals(&mut seen, "sess-1", Some("msg_b"), "uuid-2", &usage);
        assert_eq!(r1, r2, "both should return full totals");
        assert_ne!(r1, (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn test_dedup_token_totals_missing_message_id_falls_back_to_uuid() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        // Two distinct uuids with no message_id → both counted (distinct fallback keys).
        let r1 = dedup_token_totals(&mut seen, "sess-1", None, "uuid-1", &usage);
        let r2 = dedup_token_totals(&mut seen, "sess-1", None, "uuid-2", &usage);
        assert_eq!(r1.0, 6);
        assert_eq!(r2.0, 6);
        // Same uuid repeated → second is deduped.
        let r3 = dedup_token_totals(&mut seen, "sess-1", None, "uuid-1", &usage);
        assert_eq!(r3, (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn test_dedup_token_totals_empty_message_id_falls_back_to_uuid() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let r1 = dedup_token_totals(&mut seen, "sess-1", Some(""), "uuid-1", &usage);
        let r2 = dedup_token_totals(&mut seen, "sess-1", Some(""), "uuid-1", &usage);
        assert_ne!(r1, (0, 0, 0, 0, 0, 0));
        assert_eq!(r2, (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn test_dedup_token_totals_cross_session_isolation() {
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let r1 = dedup_token_totals(&mut seen, "sess-1", Some("msg_a"), "uuid-1", &usage);
        let r2 = dedup_token_totals(&mut seen, "sess-2", Some("msg_a"), "uuid-2", &usage);
        assert_ne!(r1, (0, 0, 0, 0, 0, 0));
        assert_ne!(r2, (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn test_dedup_token_totals_no_identity_always_counts() {
        // Defensive: a row with neither message_id nor uuid (malformed/legacy log)
        // has no identity to dedup by. Each such row must contribute its usage
        // rather than collapse to a shared empty key.
        let mut seen: HashSet<String> = HashSet::new();
        let usage = sample_usage();
        let r1 = dedup_token_totals(&mut seen, "", None, "", &usage);
        let r2 = dedup_token_totals(&mut seen, "", None, "", &usage);
        assert_ne!(r1, (0, 0, 0, 0, 0, 0), "first unkeyable row counts");
        assert_ne!(r2, (0, 0, 0, 0, 0, 0), "second unkeyable row also counts");
        assert_eq!(r1, r2, "both contribute full totals");
    }

    #[test]
    fn test_dedup_session_token_stats_same_message_id_counts_once() {
        let messages = vec![
            make_assistant_message(
                "uuid-thinking",
                "sess-1",
                Some("msg_shared"),
                "2026-04-27T10:00:00Z",
                sample_usage(),
            ),
            make_assistant_message(
                "uuid-text",
                "sess-1",
                Some("msg_shared"),
                "2026-04-27T10:00:01Z",
                sample_usage(),
            ),
        ];

        let stats = build_session_token_stats_from_messages(
            SessionTokenStatsOptions {
                provider: StatsProvider::Claude,
                session_id: "sess-1".to_string(),
                project_name: "test-project".to_string(),
                summary: None,
                mode: StatsMode::BillingTotal,
                start_date: None,
                end_date: None,
            },
            &messages,
        )
        .expect("stats");

        assert_eq!(stats.total_input_tokens, 6, "input deduped");
        assert_eq!(stats.total_output_tokens, 222, "output deduped");
        assert_eq!(stats.total_cache_creation_tokens, 28644);
        assert_eq!(stats.total_cache_read_tokens, 14732);
        assert_eq!(stats.total_tokens, 6 + 222 + 28644 + 14732);
    }

    /// `CodeBuddy` provider detection must be anchored under
    /// `~/.codebuddy/projects`, not a substring match. Otherwise paths like
    /// `/work/foo.codebuddy-test/...` get routed to `CodeBuddy` loaders that
    /// then return empty / error, breaking stats for the actual provider.
    /// Uses an injected home so the assertion is meaningful regardless of
    /// the runner's environment.
    #[test]
    fn is_codebuddy_path_rejects_substring_lookalikes() {
        let home = Path::new("/test-home/user");
        // Substring-style matches that the OLD `path.contains(".codebuddy")`
        // logic would have accepted — all must be rejected by the anchored
        // version.
        assert!(
            !is_codebuddy_path_under("/work/foo.codebuddy-test/projects/abc.jsonl", home),
            "name suffix lookalike must not match"
        );
        assert!(
            !is_codebuddy_path_under("/Users/dev/notes/.codebuddy-clone/data.jsonl", home),
            "hidden-dir lookalike must not match"
        );
        assert!(
            !is_codebuddy_path_under("/tmp/sample.codebuddy.jsonl", home),
            "filename containing the substring must not match"
        );
    }

    /// Real-shaped `CodeBuddy` paths must still be detected. Mirrors the
    /// runtime layout: `~/.codebuddy/projects/<project>/<session>.jsonl`.
    /// Uses an injected home so the test does not silently skip on runners
    /// without `$HOME` and does not depend on the actual user's filesystem.
    #[test]
    fn is_codebuddy_path_accepts_real_layout() {
        let home = Path::new("/test-home/user");
        let real = home
            .join(".codebuddy")
            .join("projects")
            .join("my-project")
            .join("session-1.jsonl");
        assert!(
            is_codebuddy_path_under(real.to_string_lossy().as_ref(), home),
            "anchored detection must accept ~/.codebuddy/projects/.../*.jsonl"
        );
    }

    /// `oh-my-pi` provider detection must be anchored under
    /// `~/.omp/agent/sessions`, not a substring match, so lookalike paths
    /// (e.g. `/work/foo.omp-agent-test`) do not get routed to the ompi
    /// loader.
    #[test]
    fn is_ompi_path_rejects_substring_lookalikes() {
        let home = Path::new("/test-home/user");
        assert!(
            !is_ompi_path_under("/work/foo.omp-agent-test/abc.jsonl", home),
            "name suffix lookalike must not match"
        );
        assert!(
            !is_ompi_path_under("/Users/dev/notes/.omp-clone/data.jsonl", home),
            "hidden-dir lookalike must not match"
        );
        assert!(
            !is_ompi_path_under("/tmp/sample.omp.jsonl", home),
            "filename containing the substring must not match"
        );
    }

    /// Real-shaped oh-my-pi / Pi paths must be detected. Mirrors the runtime
    /// layout: `~/.omp/agent/sessions/<escaped-cwd>/<session>.jsonl`.
    #[test]
    fn is_ompi_path_accepts_real_layout() {
        let home = Path::new("/test-home/user");
        let real = home
            .join(".omp")
            .join("agent")
            .join("sessions")
            .join("--Users-justin--")
            .join("2026-08-01T00-00-00-000Z_019f0000-0000-7000-0000-000000000000.jsonl");
        assert!(
            is_ompi_path_under(real.to_string_lossy().as_ref(), home),
            "anchored detection must accept ~/.omp/agent/sessions/.../*.jsonl"
        );
        assert!(
            is_pi_path_under(
                home.join(".pi")
                    .join("agent")
                    .join("sessions")
                    .join("proj")
                    .join("s.jsonl")
                    .to_string_lossy()
                    .as_ref(),
                home
            ),
            "anchored detection must accept ~/.pi/agent/sessions/.../*.jsonl"
        );
    }

    /// `parse_active_stats_providers` must accept the ompi/pi/gemini ids that
    /// the frontend sends after the user selects those provider tabs,
    /// instead of silently dropping them (which zeroed all stats).
    #[test]
    fn parse_active_stats_providers_accepts_ompi_pi_gemini() {
        let parsed = parse_active_stats_providers(Some(vec![
            "ompi".to_string(),
            "pi".to_string(),
            "gemini".to_string(),
            "codex".to_string(),
        ]));
        assert!(parsed.contains(&StatsProvider::Ompi));
        assert!(parsed.contains(&StatsProvider::Pi));
        assert!(parsed.contains(&StatsProvider::Gemini));
        assert!(parsed.contains(&StatsProvider::Codex));
        assert_eq!(parsed.len(), 4);
    }

    /// `detect_project_provider` must route gemini virtual project keys and
    /// ompi/pi on-disk project dirs to their own providers (not Claude).
    /// Uses an injected HOME so the assertion is meaningful regardless of
    /// the runner's environment.
    #[test]
    #[serial]
    fn detect_project_provider_routes_new_providers() {
        let temp = TempDir::new().expect("tempdir");
        let _guard = EnvVarGuard::set("HOME", temp.path());
        // `HOME` alone never reached `dirs::home_dir()` on Windows --
        // every provider's own `get_base_path()` now calls
        // `crate::utils::resolve_home_dir()` instead, which checks this
        // var first.
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp.path());
        let home = temp.path().to_string_lossy().to_string();

        let ompi_proj = format!("{home}/.omp/agent/sessions/-tmp");
        let pi_proj = format!("{home}/.pi/agent/sessions/-tmp");
        let claude_proj = format!("{home}/.claude/projects/-Users-justin");

        assert_eq!(
            detect_project_provider(&format!("gemini://{home}/.gemini/tmp/proj-a")),
            StatsProvider::Gemini
        );
        assert_eq!(detect_project_provider(&ompi_proj), StatsProvider::Ompi);
        assert_eq!(detect_project_provider(&pi_proj), StatsProvider::Pi);
        assert_eq!(detect_project_provider(&claude_proj), StatsProvider::Claude);
    }

    /// `detect_session_provider` must route ompi/pi/gemini session files to
    /// their own providers (not Claude), so token stats stop reporting
    /// "No valid messages found in session". Uses an injected HOME so the
    /// assertion is meaningful regardless of the runner's environment.
    #[test]
    #[serial]
    fn detect_session_provider_routes_new_providers() {
        let temp = TempDir::new().expect("tempdir");
        let _guard = EnvVarGuard::set("HOME", temp.path());
        // See the sibling `detect_project_provider_routes_new_providers`
        // test's own comment -- same reason, same fix.
        let _home_override_guard = EnvVarGuard::set("GRID_LOCAL_HOME_OVERRIDE", temp.path());
        let gemini_home = TempDir::new_in(".").expect("relative Gemini home");
        let _gemini_guard = EnvVarGuard::set("GEMINI_HOME", gemini_home.path());
        // Plain `fs::canonicalize()` returns Windows' extended-length UNC
        // form (`\\?\C:\...`), which `path_from_current_dir()` (used by
        // `is_gemini_path`) never produces for its own `current_dir().join()`
        // fallback -- the mismatched prefix made `.starts_with()` fail and
        // silently fell through to the `Claude` default.
        let gemini_home_absolute =
            crate::utils::test_support::canonicalize_for_fixture(gemini_home.path());
        let home = temp.path().to_string_lossy().to_string();

        assert_eq!(
            detect_session_provider(&format!(
                "{home}/.omp/agent/sessions/-tmp/2026-08-01T00-00-00-000Z_x.jsonl"
            )),
            StatsProvider::Ompi
        );
        assert_eq!(
            detect_session_provider(&format!(
                "{home}/.pi/agent/sessions/-tmp/2026-08-01T00-00-00-000Z_x.jsonl"
            )),
            StatsProvider::Pi
        );
        assert_eq!(
            detect_session_provider(
                &gemini_home_absolute
                    .join("tmp/abcd/chats/chat-1.jsonl")
                    .to_string_lossy(),
            ),
            StatsProvider::Gemini
        );
        // A codex rollout still routes to Codex.
        assert_eq!(
            detect_session_provider(&format!("{home}/.codex/sessions/2026/rollout-x.jsonl")),
            StatsProvider::Codex
        );
        // Claude files still route to Claude.
        assert_eq!(
            detect_session_provider(&format!("{home}/.claude/projects/-u/s.jsonl")),
            StatsProvider::Claude
        );
    }

    /// Provider ids emitted by `stats_provider_id` match the ids the frontend
    /// sends in `active_providers` for the new providers.
    #[test]
    fn stats_provider_id_matches_frontend_ids() {
        assert_eq!(stats_provider_id(StatsProvider::Ompi), "ompi");
        assert_eq!(stats_provider_id(StatsProvider::Pi), "pi");
        assert_eq!(stats_provider_id(StatsProvider::Gemini), "gemini");
        assert_eq!(stats_provider_id(StatsProvider::Claude), "claude");
    }

    /// Canary against silent drift: `FILE_BASED_STATS_PROVIDERS` must stay
    /// exactly the 17 providers verified to be genuinely file-based --
    /// adding a new `StatsProvider`
    /// variant, or a DB-based/hybrid one being accidentally swept in, must
    /// fail this test and force a deliberate review, not pass silently.
    #[test]
    fn file_based_stats_providers_is_exactly_the_verified_17() {
        let expected_keys: std::collections::HashSet<&str> = [
            "aider",
            "antigravity",
            "cline",
            "codebuddy",
            "codex",
            "continue",
            "copilot",
            "cursor-agent",
            "gemini",
            "grok",
            "kimi",
            "ompi",
            "openinterpreter",
            "pearai",
            "pi",
            "qwen",
            "vibe",
        ]
        .into_iter()
        .collect();

        assert_eq!(FILE_BASED_STATS_PROVIDERS.len(), 17);
        let actual_keys: std::collections::HashSet<&str> = FILE_BASED_STATS_PROVIDERS
            .iter()
            .map(|&p| stats_provider_id(p))
            .collect();
        assert_eq!(actual_keys, expected_keys);

        // Explicitly confirm every STILL-deferred provider is NOT in the
        // list -- a silent inclusion here would be exactly the scope-creep
        // this allowlist exists to prevent. OpenHands is deferred alongside
        // the DB-based/hybrid providers despite being storage-wise
        // file-based -- its session_id/file_path are both synthetic, not a
        // real stat-able path (see this const's own doc comment). Copilot
        // was ALSO originally in this list (excluded as "hybrid") but was
        // moved into FILE_BASED_STATS_PROVIDERS once its real blocker (an
        // unstable merged-project key, not its session-level staleness
        // model) was fixed -- see this const's own doc comment.
        let deferred = [
            StatsProvider::AmazonQ,
            StatsProvider::Crush,
            StatsProvider::Cursor,
            StatsProvider::ForgeCode,
            StatsProvider::Goose,
            StatsProvider::Kiro,
            StatsProvider::Llm,
            StatsProvider::Trae,
            StatsProvider::Zed,
            StatsProvider::OpenCode,
            StatsProvider::OpenHands,
        ];
        for provider in deferred {
            assert!(
                !FILE_BASED_STATS_PROVIDERS.contains(&provider),
                "{provider:?} is DB-based/hybrid and must stay deferred, not ingested in v1"
            );
        }
    }

    #[test]
    fn model_stats_preserve_context_buckets_and_normalize_fast_tier() {
        assert_eq!(
            context_tier_min_tokens("openai/gpt-5.6-terra-2026-01-01", 272_001),
            272_001
        );
        assert_eq!(context_tier_min_tokens("gpt-5.6-terra", 272_000), 0);

        let model_key = model_usage_key("gpt-5.6-terra", Some("priority"));
        let mut model_usage = HashMap::new();
        model_usage.insert(model_key.clone(), (1, 300, 200, 100, 0, 0, 0));
        let mut context_usage = HashMap::new();
        context_usage.insert(
            model_key,
            HashMap::from([(
                272_001,
                ModelContextStats {
                    min_context_tokens: 272_001,
                    token_count: 300,
                    input_tokens: 200,
                    output_tokens: 100,
                    ..Default::default()
                },
            )]),
        );

        let models = build_model_stats(
            StatsProvider::Codex,
            model_usage,
            context_usage,
            HashMap::new(),
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].service_tier.as_deref(), Some("fast"));
        assert_eq!(models[0].context_breakdown[0].min_context_tokens, 272_001);
    }

    #[test]
    fn nested_anthropic_cache_usage_is_flattened_with_ttl_split() {
        let usage: TokenUsage = serde_json::from_value(json!({
            "input_tokens": 100,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 10,
                "ephemeral_1h_input_tokens": 5
            },
            "serviceTier": "standard"
        }))
        .expect("nested usage should deserialize");
        let usage = normalize_token_usage(usage);

        assert_eq!(usage.cache_creation_input_tokens, Some(15));
        assert_eq!(usage.cache_creation_input_tokens_5m, Some(10));
        assert_eq!(usage.cache_creation_input_tokens_1h, Some(5));
        assert_eq!(usage.service_tier.as_deref(), Some("standard"));
    }

    // ---------------------------------------------------------------
    // DTO-compatibility between the raw-file-walk path and the
    // archive_db-backed path for Claude's global stats: the same
    // fixture data, read two different ways, must produce a
    // field-for-field identical `SessionFileStats`.
    // ---------------------------------------------------------------

    const STEP5_TOOL_USE_LINE: &str = r#"{"uuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":100,"output_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":7,"ephemeral_1h_input_tokens":0}}}}"#;
    const STEP5_ERROR_RESULT_LINE: &str = r#"{"uuid":"u2","parentUuid":"u1","sessionId":"s1","timestamp":"2026-01-01T00:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"FAILED: exit 1","is_error":true}]}}"#;
    const STEP5_TOOL_USE_LINE_2: &str = r#"{"uuid":"u3","parentUuid":"u2","sessionId":"s1","timestamp":"2026-01-01T00:05:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"/a/b.rs"}}],"model":"claude-x","usage":{"input_tokens":50,"output_tokens":10}}}"#;
    const STEP5_RESULT_LINE_2: &str = r#"{"uuid":"u4","parentUuid":"u3","sessionId":"s1","timestamp":"2026-01-01T00:05:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"file contents","is_error":false}]}}"#;

    fn step5_fixture_lines() -> String {
        format!(
            "{STEP5_TOOL_USE_LINE}\n{STEP5_ERROR_RESULT_LINE}\n{STEP5_TOOL_USE_LINE_2}\n{STEP5_RESULT_LINE_2}\n"
        )
    }

    #[tokio::test]
    async fn archive_backed_global_stats_match_raw_file_walk_for_claude() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path().join("projects").join("-fixture-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session1.jsonl");
        fs::write(&session_path, step5_fixture_lines()).unwrap();

        // Path A: today's raw-file-walk aggregation, unchanged.
        let expected = process_session_file_for_global_stats(
            &session_path,
            StatsMode::BillingTotal,
            None,
            None,
        )
        .expect("raw-walk scan should produce stats for a real fixture file");

        // Path B: ingest the same fixture into an in-memory archive, then
        // read it back through the new archive-backed aggregation.
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();
        crate::archive_db::ingest::ingest_claude_project(
            &mut conn,
            &project_dir.to_string_lossy(),
        )
        .await
        .unwrap();

        let mut actual = build_global_stats_from_connection(
            &conn,
            StatsMode::BillingTotal,
            None,
            None,
        )
        .unwrap();
        assert_eq!(actual.len(), 1, "exactly one archived session expected");
        let actual = actual.remove(0);

        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn archive_backed_global_stats_match_raw_file_walk_in_conversation_only_mode() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path().join("projects").join("-fixture-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session1.jsonl");
        fs::write(&session_path, step5_fixture_lines()).unwrap();

        let expected = process_session_file_for_global_stats(
            &session_path,
            StatsMode::ConversationOnly,
            None,
            None,
        )
        .expect("raw-walk scan should produce stats for a real fixture file");

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();
        crate::archive_db::ingest::ingest_claude_project(
            &mut conn,
            &project_dir.to_string_lossy(),
        )
        .await
        .unwrap();

        let mut actual = build_global_stats_from_connection(
            &conn,
            StatsMode::ConversationOnly,
            None,
            None,
        )
        .unwrap();
        assert_eq!(actual.len(), 1);
        let actual = actual.remove(0);

        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn archive_backed_global_stats_respect_date_range_filter() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path().join("projects").join("-fixture-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session1.jsonl");
        fs::write(&session_path, step5_fixture_lines()).unwrap();

        // Window that only covers the first tool_use/tool_result pair.
        let s_limit = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let e_limit = DateTime::parse_from_rfc3339("2026-01-01T00:02:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let expected = process_session_file_for_global_stats(
            &session_path,
            StatsMode::BillingTotal,
            Some(&s_limit),
            Some(&e_limit),
        )
        .expect("raw-walk scan should produce stats for a real fixture file");

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();
        crate::archive_db::ingest::ingest_claude_project(
            &mut conn,
            &project_dir.to_string_lossy(),
        )
        .await
        .unwrap();

        let mut actual = build_global_stats_from_connection(
            &conn,
            StatsMode::BillingTotal,
            Some(&s_limit),
            Some(&e_limit),
        )
        .unwrap();
        assert_eq!(actual.len(), 1);
        let actual = actual.remove(0);

        assert_eq!(actual, expected);
        // Sanity: the filter actually excluded something, otherwise this
        // test wouldn't be exercising date filtering at all.
        assert_eq!(actual.total_messages, 2);
    }

    #[tokio::test]
    async fn archive_backed_global_stats_empty_when_not_backfilled() {
        // Migrated (so the schema exists) but never ingested into --
        // callers must treat an empty result as "fall back to the
        // raw-file-walk path," not as "the user has zero Claude history."
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();

        let result =
            build_global_stats_from_connection(&conn, StatsMode::BillingTotal, None, None)
                .unwrap();
        assert!(result.is_empty());
    }

    /// A correlated subagent transcript is ingested as its own `session`
    /// row (`is_subagent = 1`) -- global stats must not double-count it
    /// as if it were a real top-level session.
    #[tokio::test]
    async fn archive_backed_global_stats_exclude_subagent_sessions() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path().join("projects").join("-fixture-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session1.jsonl");
        let parent_lines = [
            r#"{"uuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:00:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_agent1","name":"Agent","input":{"subagent_type":"general-purpose"}}],"model":"claude-x","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"{"uuid":"p2","parentUuid":"p1","sessionId":"parent","timestamp":"2026-01-01T00:05:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent1","content":"done","is_error":false}]}}"#,
        ];
        fs::write(&session_path, parent_lines.join("\n") + "\n").unwrap();

        let subagents_dir = project_dir.join("session1").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.jsonl"),
            format!("{STEP5_TOOL_USE_LINE}\n{STEP5_ERROR_RESULT_LINE}\n"),
        )
        .unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_agent1"}"#,
        )
        .unwrap();

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::archive_db::migrate::migrate(&mut conn).unwrap();
        crate::archive_db::ingest::ingest_claude_project(
            &mut conn,
            &project_dir.to_string_lossy(),
        )
        .await
        .unwrap();

        let subagent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session WHERE is_subagent = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(subagent_count, 1);

        let result =
            build_global_stats_from_connection(&conn, StatsMode::BillingTotal, None, None)
                .unwrap();
        assert_eq!(result.len(), 1, "the subagent session must not appear in global stats");
    }
}
