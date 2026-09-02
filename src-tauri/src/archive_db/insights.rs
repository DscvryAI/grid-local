//! Home/Insights aggregation queries over Grid's own normalized archive
//! (spec §9/§10/§17). Originally Claude-only; universal-provider ingestion
//! later made `archive_db` genuinely multi-provider, and added an optional
//! `provider_key` filter to the 5 "worth looking at" functions
//! specifically for this -- functions here without that filter (Home's
//! own summaries) aggregate across every ingested provider, not just
//! Claude.
//!
//! **Known, accepted limitations, not silently papered over:**
//! - `agent_run.started_at`/`ended_at`/`tool_call_count`/
//!   `child_session_id`/`parent_agent_run_id`/`subagent_type` are now
//!   populated whenever a launched subagent's own transcript can be found
//!   and correlated (`ingest::claude::ingest_subagent_tree`, called
//!   recursively after each session file finishes ingesting).
//!   [`get_agent_run_tree`]'s recursive CTE returns genuinely multi-level
//!   trees against real data now, not just a shape ready for future data.
//!   These fields stay unset only when no correlatable subagent transcript
//!   exists for a given launch (an `Agent` `tool_use` with no matching
//!   `.meta.json`/`agentId`, or an older session predating that
//!   correlation key) -- a real, disclosed "unlinked" case, not a blanket
//!   ingest gap. Session rows created from subagent transcripts are
//!   flagged `session.is_subagent = 1`; the functions in this module that
//!   count or list top-level sessions still need that filter added (not
//!   yet done as of this comment) -- until then they may double-count
//!   subagent sessions alongside their parents.
//! - `error.occurred_at` is likewise never populated at ingest; every
//!   query needing an error's timestamp falls back to its owning
//!   message's `timestamp` instead.

use crate::models::{
    AgentRunDetail, AgentRunNode, AgentRunToolUsage, AgentRunTree, ErrorOccurrence,
    HighTokenSessionCard, InsightCard, LargeAgentRunCard, PersonalBaseline, ProblemTrend,
    ProviderTokenShare, RepeatedCommandFailureCard, RepeatedErrorCard, SessionListItem,
    SimilarErrorResolution, SinceLastVisitSummary, ThisWeekSummary, VerificationGapCard,
};
use once_cell::sync::Lazy;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

/// Raw failing-command/error occurrences are fetched (most-recent-first)
/// up to this many rows before being normalized and grouped in Rust --
/// bounded so a very large real archive can't make one Problems-tab load
/// scan its entire history. Generous for realistic single-user local
/// usage (this dev machine's own real archive, with 100+ real projects
/// across many providers, is nowhere near this many failures/errors);
/// revisit if a real install is ever seen hitting it.
const MAX_RAW_PROBLEM_OCCURRENCES: usize = 5000;

/// Replaces variable-looking tokens (paths, numbers, quoted strings,
/// UUIDs) with placeholders so semantically-the-same command invoked
/// against different arguments groups together -- the previous `GROUP BY
/// c.shell_command` was exact-text and fragile, never grouping `pytest
/// tests/test_foo.py` with `pytest tests/test_bar.py` at all.
/// Deliberately simple, token-level heuristics, not a shell parser --
/// good enough to catch the common "same command, different
/// file/number/id" case without pulling in a real shlex dependency for
/// this.
fn normalize_command_template(raw: &str) -> String {
    raw.split_whitespace()
        .map(normalize_command_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_command_token(token: &str) -> String {
    if is_uuid_like(token) {
        return "<UUID>".to_string();
    }
    if !token.is_empty() && token.chars().all(|c| c.is_ascii_digit()) {
        return "<N>".to_string();
    }
    if token.len() >= 2
        && ((token.starts_with('"') && token.ends_with('"'))
            || (token.starts_with('\'') && token.ends_with('\'')))
    {
        return "<STR>".to_string();
    }
    if token.contains('/') || token.contains('\\') {
        return "<PATH>".to_string();
    }
    token.to_string()
}

fn is_uuid_like(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// Last-7-days-vs-7-days-before comparison -- see [`ProblemTrend`]'s own
/// doc for why this is a fixed lookback independent of the caller's
/// `window_start` filter. `occurrences` need not be sorted.
fn compute_trend(occurrences: &[String], now: chrono::DateTime<chrono::Utc>) -> ProblemTrend {
    let recent_cutoff = now - chrono::Duration::days(7);
    let prior_cutoff = now - chrono::Duration::days(14);

    let first = occurrences.iter().min();
    if let Some(first) = first {
        if crate::utils::parse_rfc3339_utc(first).is_some_and(|t| t >= recent_cutoff) {
            return ProblemTrend::New;
        }
    }

    let (mut recent_count, mut prior_count) = (0usize, 0usize);
    for occurred_at in occurrences {
        let Some(t) = crate::utils::parse_rfc3339_utc(occurred_at) else {
            continue;
        };
        if t >= recent_cutoff {
            recent_count += 1;
        } else if t >= prior_cutoff {
            prior_count += 1;
        }
    }

    // A small buffer (+1) against single-occurrence noise flipping the
    // label back and forth -- e.g. prior=0, recent=1 reads as "steady
    // background noise," not a real trend, until it's at least 2 vs 0/1.
    if recent_count > prior_count + 1 {
        ProblemTrend::Increasing
    } else if prior_count > recent_count + 1 {
        ProblemTrend::Decreasing
    } else {
        ProblemTrend::Steady
    }
}

fn dismissed_signatures(conn: &Connection, kind: &str) -> Result<HashSet<String>, String> {
    let mut stmt = conn
        .prepare("SELECT signature FROM dismissed_problem WHERE kind = ?1")
        .map_err(|e| format!("Failed to prepare dismissed-problem query: {e}"))?;
    let rows = stmt
        .query_map([kind], |row| row.get(0))
        .map_err(|e| format!("Failed to run dismissed-problem query: {e}"))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|e| format!("Failed to read dismissed-problem row: {e}"))
}

/// Marks a "worth looking at" card as handled so it stops resurfacing,
/// via local dismiss/resolve state. `kind` is `"command_failure"` or
/// `"error"`; `signature` is the card's own `template`/`error_signature`.
/// Idempotent (re-dismissing an already-dismissed card is a no-op, not an
/// error).
pub fn dismiss_problem(conn: &Connection, kind: &str, signature: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO dismissed_problem (kind, signature, dismissed_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(kind, signature) DO NOTHING",
        rusqlite::params![kind, signature, chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| format!("Failed to dismiss problem: {e}"))?;
    Ok(())
}

// ============================================================================
// "Things worth looking at" (spec §10)
// ============================================================================

/// Shell commands that failed at least `min_failures` times, grouped by
/// their normalized `template` (see `normalize_command_template`'s own
/// doc for why this replaced the previous exact-text `GROUP BY`, which
/// never grouped `pytest tests/test_foo.py` with `pytest
/// tests/test_bar.py` at all). Grouping happens
/// in Rust, not SQL, since the normalization itself is a string
/// transform SQL can't express -- raw occurrences are fetched
/// most-recent-first, bounded by [`MAX_RAW_PROBLEM_OCCURRENCES`].
/// `shell_command` on the returned card is the most recent RAW
/// occurrence's own text (a real example), not the abstract `template`.
/// `project_key` (the same value `history.rs`'s own query filters on --
/// a Claude project's raw session-storage path, see `ingest::
/// ingest_claude_project`) narrows to one project when `Some`; `None`
/// means global. `provider_key` (universal-provider-ingestion plan, Step
/// 6) narrows to one provider's own data the same way -- `None` stays
/// global across every ingested provider. Dismissed templates (see
/// [`dismiss_problem`]) are excluded.
pub fn repeated_command_failures(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    min_failures: usize,
    limit: usize,
) -> Result<Vec<RepeatedCommandFailureCard>, String> {
    let sql = "
        SELECT c.shell_command, m.timestamp, c.session_id, s.file_path
        FROM command c
        JOIN tool_call tc ON tc.id = c.tool_call_id
        JOIN tool_result r ON r.tool_call_id = c.tool_call_id
        JOIN message m ON m.id = tc.message_id
        JOIN session s ON s.id = c.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE r.is_error = 1
          AND s.is_subagent = 0
          AND (?1 IS NULL OR m.timestamp >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY m.timestamp DESC
        LIMIT ?4";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare repeated_command_failures query: {e}"))?;
    let max_rows = i64::try_from(MAX_RAW_PROBLEM_OCCURRENCES).unwrap_or(i64::MAX);
    let raw_rows: Vec<(String, String, i64, String)> = stmt
        .query_map(
            rusqlite::params![window_start, project_key, provider_key, max_rows],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("Failed to run repeated_command_failures query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read repeated_command_failures row: {e}"))?;

    struct CommandGroup {
        sample_shell_command: String,
        sample_session_file_path: String,
        occurrences: Vec<String>,
        session_ids: HashSet<i64>,
    }

    let mut groups: HashMap<String, CommandGroup> = HashMap::new();
    for (shell_command, timestamp, session_row_id, session_file_path) in raw_rows {
        let template = normalize_command_template(&shell_command);
        // Rows arrive most-recent-first, so the group's FIRST occurrence
        // (the only time `or_insert_with` actually runs) is always its
        // most recent -- no separate max-tracking needed.
        let group = groups.entry(template).or_insert_with(|| CommandGroup {
            sample_shell_command: shell_command.clone(),
            sample_session_file_path: session_file_path.clone(),
            occurrences: Vec::new(),
            session_ids: HashSet::new(),
        });
        group.occurrences.push(timestamp);
        group.session_ids.insert(session_row_id);
    }

    let dismissed = dismissed_signatures(conn, "command_failure")?;
    let now = chrono::Utc::now();
    let mut cards: Vec<RepeatedCommandFailureCard> = groups
        .into_iter()
        .filter(|(template, g)| g.occurrences.len() >= min_failures && !dismissed.contains(template))
        .map(|(template, g)| {
            let first_occurred_at = g.occurrences.iter().min().cloned().unwrap_or_default();
            let last_occurred_at = g.occurrences.iter().max().cloned().unwrap_or_default();
            RepeatedCommandFailureCard {
                shell_command: g.sample_shell_command,
                trend: compute_trend(&g.occurrences, now),
                template,
                failure_count: g.occurrences.len(),
                session_count: g.session_ids.len(),
                first_occurred_at,
                last_occurred_at,
                sample_session_id: g.sample_session_file_path,
            }
        })
        .collect();

    cards.sort_by(|a, b| {
        b.failure_count
            .cmp(&a.failure_count)
            .then_with(|| b.last_occurred_at.cmp(&a.last_occurred_at))
    });
    cards.truncate(limit);
    Ok(cards)
}

/// Error signatures that recurred across at least `min_sessions` distinct
/// sessions -- a cross-session signal (systemic/environment issue), a
/// different kind of "worth looking at" than one session repeating the
/// same failure many times ([`repeated_command_failures`]'s job).
/// `error_signature` is already a normalized grouping key (computed at
/// ingest time by `ingest::claude::truncate_error_signature`), so unlike
/// commands this groups directly on it -- no template step needed. Raw
/// occurrences are fetched most-recent-first, bounded by
/// [`MAX_RAW_PROBLEM_OCCURRENCES`], same reasoning as
/// [`repeated_command_failures`]. Dismissed signatures (see
/// [`dismiss_problem`]) are excluded.
pub fn repeated_errors(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    min_sessions: usize,
    limit: usize,
) -> Result<Vec<RepeatedErrorCard>, String> {
    let sql = "
        SELECT e.error_signature, m.timestamp, e.session_id, s.file_path
        FROM error e
        JOIN session s ON s.id = e.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        LEFT JOIN message m ON m.id = e.message_id
        WHERE s.is_subagent = 0
          AND (?1 IS NULL OR m.timestamp IS NULL OR m.timestamp >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY m.timestamp DESC
        LIMIT ?4";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare repeated_errors query: {e}"))?;
    let max_rows = i64::try_from(MAX_RAW_PROBLEM_OCCURRENCES).unwrap_or(i64::MAX);
    let raw_rows: Vec<(String, Option<String>, i64, String)> = stmt
        .query_map(
            rusqlite::params![window_start, project_key, provider_key, max_rows],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("Failed to run repeated_errors query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read repeated_errors row: {e}"))?;

    struct ErrorGroup {
        sample_session_file_path: String,
        timestamps: Vec<String>,
        session_ids: HashSet<i64>,
        occurrence_count: usize,
    }

    let mut groups: HashMap<String, ErrorGroup> = HashMap::new();
    for (error_signature, timestamp, session_row_id, session_file_path) in raw_rows {
        let group = groups.entry(error_signature).or_insert_with(|| ErrorGroup {
            sample_session_file_path: session_file_path.clone(),
            timestamps: Vec::new(),
            session_ids: HashSet::new(),
            occurrence_count: 0,
        });
        if let Some(ts) = timestamp {
            group.timestamps.push(ts);
        }
        group.session_ids.insert(session_row_id);
        group.occurrence_count += 1;
    }

    let dismissed = dismissed_signatures(conn, "error")?;
    let now = chrono::Utc::now();
    let mut cards: Vec<RepeatedErrorCard> = groups
        .into_iter()
        .filter(|(sig, g)| g.session_ids.len() >= min_sessions && !dismissed.contains(sig))
        .map(|(error_signature, g)| {
            let first_occurred_at = g.timestamps.iter().min().cloned().unwrap_or_default();
            let last_occurred_at = g.timestamps.iter().max().cloned().unwrap_or_default();
            RepeatedErrorCard {
                error_signature,
                trend: compute_trend(&g.timestamps, now),
                occurrence_count: g.occurrence_count,
                session_count: g.session_ids.len(),
                first_occurred_at,
                last_occurred_at,
                sample_session_id: g.sample_session_file_path,
            }
        })
        .collect();

    cards.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| b.occurrence_count.cmp(&a.occurrence_count))
    });
    cards.truncate(limit);
    Ok(cards)
}

/// Every real occurrence of one specific `error_signature`, backing
/// "selecting an error shows occurrences": the individual, unaggregated rows behind a
/// `RepeatedErrorCard`'s own `occurrence_count`/`session_count`, for a
/// user who's already looking at one specific recurring error and wants
/// to see exactly where. Same table/join shape as [`repeated_errors`],
/// just filtered to one signature and returned as a raw list instead of
/// grouped -- no `min_sessions` threshold, since every real occurrence
/// of an already-identified recurring error is worth showing, not just
/// ones crossing a "is this worth surfacing AT ALL" bar.
pub fn error_occurrences(
    conn: &Connection,
    error_signature: &str,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    limit: usize,
) -> Result<Vec<ErrorOccurrence>, String> {
    let sql = "
        SELECT s.file_path, p.display_name, m.timestamp
        FROM error e
        JOIN session s ON s.id = e.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        LEFT JOIN message m ON m.id = e.message_id
        WHERE e.error_signature = ?1
          AND s.is_subagent = 0
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY m.timestamp DESC
        LIMIT ?4";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare error_occurrences query: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![error_signature, project_key, provider_key, limit],
            |row| {
                Ok(ErrorOccurrence {
                    session_id: row.get(0)?,
                    project_name: row.get(1)?,
                    occurred_at: row.get(2)?,
                })
            },
        )
        .map_err(|e| format!("Failed to run error_occurrences query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read error_occurrences row: {e}"))
}

/// How many sessions sharing an `error_signature` are inspected for a
/// later passing verification, for cross-session reusable-solution
/// retrieval. Kept small and separate from [`MAX_RAW_PROBLEM_OCCURRENCES`]
/// -- this is an
/// on-demand, user-triggered lookup (one command per candidate session),
/// not a background aggregate, so it deliberately doesn't scan thousands
/// of rows the way the "worth looking at" queries are allowed to.
const MAX_SIMILAR_ERROR_CANDIDATES: usize = 50;

/// Deterministic "was this same error later followed by a passing
/// verification, possibly in a different project?" -- a reusable-solution
/// insight: "A similar dependency issue was resolved in another
/// project.".
///
/// **What "similar" means here, and its honest limit**: `error_signature`
/// (`truncate_error_signature` at ingest) is the raw first line of the
/// error text, capped at 200 chars -- NOT normalized the way
/// [`normalize_command_template`] strips paths/numbers/UUIDs. Exact
/// string matching therefore works well for generic, path-free errors
/// (e.g. missing-dependency/module errors) and
/// rarely matches stack-trace-style errors carrying absolute paths that
/// differ between projects. This is disclosed, not silently papered
/// over -- a caller finding no results for a path-heavy error signature
/// is not a bug.
///
/// **What "resolved" means here, and its honest limit**: a later,
/// later-timestamped `command` row in the SAME candidate session that
/// matches [`VERIFICATION_COMMAND_PATTERN`] and passed (`tool_result.
/// is_error = 0`). A passing verification after an error is real,
/// checkable evidence -- it is NOT proof the error was caused by, or
/// fixed by, anything in particular; the two could be unrelated. Callers
/// must render this evidentially ("later followed by a passing
/// verification"), never as a conclusion ("resolved" or "the fix").
///
/// `exclude_project_key`, when set, omits that project from candidates
/// so a caller already looking at one project's own error only sees
/// evidence from genuinely OTHER projects (matching the review's own
/// "in another project" framing); pass `None` when browsing globally,
/// where any project's evidence is relevant.
pub fn similar_error_resolutions(
    conn: &Connection,
    error_signature: &str,
    exclude_project_key: Option<&str>,
    limit: usize,
) -> Result<Vec<SimilarErrorResolution>, String> {
    let candidate_sql = "
        SELECT s.id, s.file_path, p.display_name, m.timestamp
        FROM error e
        JOIN session s ON s.id = e.session_id
        JOIN project p ON p.id = s.project_id
        LEFT JOIN message m ON m.id = e.message_id
        WHERE e.error_signature = ?1
          AND s.is_subagent = 0
          AND (?2 IS NULL OR p.project_key != ?2)
        ORDER BY m.timestamp DESC
        LIMIT ?3";
    let mut candidate_stmt = conn
        .prepare(candidate_sql)
        .map_err(|e| format!("Failed to prepare similar_error_resolutions candidate query: {e}"))?;
    #[allow(clippy::type_complexity)]
    let candidates: Vec<(i64, String, String, Option<String>)> = candidate_stmt
        .query_map(
            rusqlite::params![
                error_signature,
                exclude_project_key,
                i64::try_from(MAX_SIMILAR_ERROR_CANDIDATES).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| format!("Failed to run similar_error_resolutions candidate query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read similar_error_resolutions candidate row: {e}"))?;

    let verification_sql = "
        SELECT c.shell_command, m.timestamp, r.is_error
        FROM command c
        JOIN tool_call tc ON tc.id = c.tool_call_id
        JOIN tool_result r ON r.tool_call_id = c.tool_call_id
        JOIN message m ON m.id = tc.message_id
        WHERE c.session_id = ?1 AND m.timestamp > ?2
        ORDER BY m.timestamp ASC";
    let mut verification_stmt = conn
        .prepare(verification_sql)
        .map_err(|e| format!("Failed to prepare similar_error_resolutions verification query: {e}"))?;

    let mut resolutions = Vec::new();
    for (session_row_id, file_path, project_name, error_occurred_at) in candidates {
        let Some(error_timestamp) = error_occurred_at.clone() else {
            continue;
        };
        let verification_rows: Vec<(String, String, bool)> = verification_stmt
            .query_map(rusqlite::params![session_row_id, error_timestamp], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| {
                format!("Failed to run similar_error_resolutions verification query: {e}")
            })?
            .collect::<Result<_, _>>()
            .map_err(|e| {
                format!("Failed to read similar_error_resolutions verification row: {e}")
            })?;

        let first_passing_verification = verification_rows
            .into_iter()
            .find(|(shell_command, _, is_error)| {
                !is_error && VERIFICATION_COMMAND_PATTERN.is_match(shell_command)
            });

        if let Some((verification_command, verification_occurred_at, _)) =
            first_passing_verification
        {
            resolutions.push(SimilarErrorResolution {
                session_id: file_path,
                project_name,
                error_occurred_at,
                verification_occurred_at,
                verification_command,
            });
            if resolutions.len() >= limit {
                break;
            }
        }
    }

    Ok(resolutions)
}

/// Sessions with at least `min_subagents` agent-run rows. `subagent_count`
/// is real (`agent_run` rows themselves ARE inserted correctly per
/// subagent launch); `session_started_at` intentionally comes from
/// `session.first_message_time` rather than any single `agent_run.
/// started_at` -- this card aggregates across potentially many agent runs
/// in one session, so a single run's own timing wouldn't represent the
/// group even now that `agent_run.started_at` can be populated (via
/// `ingest::claude::ingest_subagent_tree`).
pub fn large_agent_runs(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    min_subagents: usize,
    limit: usize,
) -> Result<Vec<LargeAgentRunCard>, String> {
    let sql = "
        SELECT s.file_path, p.display_name, s.summary,
               COUNT(ar.id) AS subagent_count, s.first_message_time
        FROM agent_run ar
        JOIN session s ON s.id = ar.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND (?1 IS NULL OR s.first_message_time >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        GROUP BY ar.session_id
        HAVING COUNT(ar.id) >= ?4
        ORDER BY subagent_count DESC
        LIMIT ?5";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare large_agent_runs query: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![window_start, project_key, provider_key, min_subagents, limit],
            |row| {
                Ok(LargeAgentRunCard {
                    session_id: row.get(0)?,
                    project_name: row.get(1)?,
                    session_summary: row.get(2)?,
                    subagent_count: {
                        let count: i64 = row.get(3)?;
                        usize::try_from(count).unwrap_or(0)
                    },
                    session_started_at: row.get(4)?,
                })
            },
        )
        .map_err(|e| format!("Failed to run large_agent_runs query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read large_agent_runs row: {e}"))
}

/// Top sessions by `session.total_tokens` (already denormalized at
/// ingest, same convention `History`'s own query relies on).
pub fn high_token_sessions(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    limit: usize,
) -> Result<Vec<HighTokenSessionCard>, String> {
    let sql = "
        SELECT s.file_path, p.display_name, s.summary, s.total_tokens, s.last_message_time
        FROM session s
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND (?1 IS NULL OR s.first_message_time >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY s.total_tokens DESC
        LIMIT ?4";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare high_token_sessions query: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![window_start, project_key, provider_key, limit],
            |row| {
            Ok(HighTokenSessionCard {
                session_id: row.get(0)?,
                project_name: row.get(1)?,
                session_summary: row.get(2)?,
                total_tokens: row.get(3)?,
                last_message_time: row.get(4)?,
            })
        })
        .map_err(|e| format!("Failed to run high_token_sessions query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read high_token_sessions row: {e}"))
}

/// This user's own historical average total tokens and session duration
/// for one provider, used for personal-baseline anomaly explanations.
/// Scoped per-provider, never
/// global -- providers report token usage on incomparable scales
/// (confirmed by reading real fixture/provider data), so a global
/// average would produce a misleading ratio.
///
/// `duration_minutes` is deliberately NOT read from `session.
/// duration_minutes` -- that column is declared in the schema but never
/// populated by any ingest path (confirmed via grep across
/// `archive_db::ingest`), a dead column, not a real signal. Duration is
/// derived at query time from `first_message_time`/`last_message_time`
/// instead, the same approach the frontend's own `SessionIntelligence
/// Header` already uses client-side.
///
/// `exclude_session_id`, when set, omits that one session (by its
/// `file_path`) from the average so a caller comparing the CURRENTLY
/// OPEN session against its own personal baseline doesn't have that
/// same session skew the baseline it's being measured against --
/// especially important with a small sample size.
pub fn personal_baseline(
    conn: &Connection,
    provider_key: &str,
    exclude_session_id: Option<&str>,
) -> Result<PersonalBaseline, String> {
    let sql = "
        SELECT AVG(s.total_tokens),
               AVG((julianday(s.last_message_time) - julianday(s.first_message_time)) * 24 * 60),
               COUNT(*)
        FROM session s
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND pr.provider_key = ?1
          AND (?2 IS NULL OR s.file_path != ?2)
          AND s.first_message_time IS NOT NULL
          AND s.last_message_time IS NOT NULL";

    conn.query_row(sql, rusqlite::params![provider_key, exclude_session_id], |row| {
        Ok(PersonalBaseline {
            average_total_tokens: row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
            average_duration_minutes: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            session_count: row.get(2)?,
        })
    })
    .map_err(|e| format!("Failed to compute personal_baseline: {e}"))
}

/// A small, deliberately narrow allowlist of unambiguous test/build
/// command shapes -- the exact same commands the frontend's own
/// `deriveVerificationStatus` (`sessionIntelligence.ts`) recognizes,
/// ported here so the cross-session/archive-level signal agrees with the
/// per-session one. A command outside this list is never assumed to be a
/// verification step.
pub static VERIFICATION_COMMAND_PATTERN: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(
        r"\b(?:npm|pnpm|yarn)\s+(?:run\s+)?test\b|\bcargo\s+test\b|\bpytest\b|\bgo\s+test\b|\bmvn\s+test\b|\bgradle\s+test\b",
    )
    .expect("VERIFICATION_COMMAND_PATTERN is a fixed, valid regex")
});

/// Normalizes errors, commands, and validation outcomes into
/// deterministic insight types, for the "Verification gap" row ("Four
/// files changed after the last passing test."). For each session, finds
/// the most recent command
/// matching [`VERIFICATION_COMMAND_PATTERN`] that PASSED (`tool_result.
/// is_error = 0`), then counts `file_event` rows in that same session
/// with a later timestamp. A session with zero such file events
/// afterward has nothing worth flagging -- its verification is current,
/// not stale.
///
/// A session whose most recent matching command FAILED is deliberately
/// NOT flagged here -- a different, narrower signal than this card
/// answers (a failing session is better surfaced via
/// [`repeated_command_failures`]/[`repeated_errors`] if the failure
/// recurs, not fabricated into a "gap" claim this card doesn't mean).
/// Mirrors the frontend's own per-session `deriveVerificationStatus`'s
/// "stale" state at the cross-session/archive level, so a gap shows up
/// in Insights/Home even before a user happens to open that session.
pub fn verification_gaps(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
    limit: usize,
) -> Result<Vec<VerificationGapCard>, String> {
    let max_rows = i64::try_from(MAX_RAW_PROBLEM_OCCURRENCES).unwrap_or(i64::MAX);

    let command_sql = "
        SELECT c.session_id, c.shell_command, m.timestamp, r.is_error, s.file_path, p.display_name, s.summary
        FROM command c
        JOIN tool_call tc ON tc.id = c.tool_call_id
        JOIN tool_result r ON r.tool_call_id = c.tool_call_id
        JOIN message m ON m.id = tc.message_id
        JOIN session s ON s.id = c.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND (?1 IS NULL OR m.timestamp >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY m.timestamp DESC
        LIMIT ?4";
    let mut command_stmt = conn
        .prepare(command_sql)
        .map_err(|e| format!("Failed to prepare verification_gaps command query: {e}"))?;
    #[allow(clippy::type_complexity)]
    let command_rows: Vec<(i64, String, String, bool, String, String, Option<String>)> =
        command_stmt
            .query_map(
                rusqlite::params![window_start, project_key, provider_key, max_rows],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .map_err(|e| format!("Failed to run verification_gaps command query: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read verification_gaps command row: {e}"))?;

    struct LastVerification {
        shell_command: String,
        timestamp: String,
        passed: bool,
        session_file_path: String,
        project_name: String,
        session_summary: Option<String>,
    }
    let mut last_verification: HashMap<i64, LastVerification> = HashMap::new();
    for (session_id, shell_command, timestamp, is_error, session_file_path, project_name, session_summary) in
        command_rows
    {
        if !VERIFICATION_COMMAND_PATTERN.is_match(&shell_command) {
            continue;
        }
        // Rows arrive most-recent-first -- the first match per session IS
        // its most recent verification command (same trick
        // `repeated_command_failures` already relies on).
        last_verification.entry(session_id).or_insert(LastVerification {
            shell_command,
            timestamp,
            passed: !is_error,
            session_file_path,
            project_name,
            session_summary,
        });
    }

    if last_verification.is_empty() {
        return Ok(Vec::new());
    }

    let file_event_sql = "
        SELECT fe.session_id, m.timestamp
        FROM file_event fe
        JOIN tool_call tc ON tc.id = fe.tool_call_id
        JOIN message m ON m.id = tc.message_id
        JOIN session s ON s.id = fe.session_id
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND (?1 IS NULL OR m.timestamp >= ?1)
          AND (?2 IS NULL OR p.project_key = ?2)
          AND (?3 IS NULL OR pr.provider_key = ?3)
        ORDER BY m.timestamp DESC
        LIMIT ?4";
    let mut file_event_stmt = conn
        .prepare(file_event_sql)
        .map_err(|e| format!("Failed to prepare verification_gaps file_event query: {e}"))?;
    let file_event_rows: Vec<(i64, String)> = file_event_stmt
        .query_map(
            rusqlite::params![window_start, project_key, provider_key, max_rows],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Failed to run verification_gaps file_event query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read verification_gaps file_event row: {e}"))?;

    let mut files_changed_since: HashMap<i64, usize> = HashMap::new();
    for (session_id, timestamp) in file_event_rows {
        if let Some(verification) = last_verification.get(&session_id) {
            if verification.passed && timestamp > verification.timestamp {
                *files_changed_since.entry(session_id).or_insert(0) += 1;
            }
        }
    }

    let mut cards: Vec<VerificationGapCard> = files_changed_since
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .filter_map(|(session_id, count)| {
            let verification = last_verification.get(&session_id)?;
            Some(VerificationGapCard {
                session_id: verification.session_file_path.clone(),
                project_name: verification.project_name.clone(),
                session_summary: verification.session_summary.clone(),
                files_changed_since: count,
                last_verified_command: verification.shell_command.clone(),
                last_verified_at: verification.timestamp.clone(),
            })
        })
        .collect();

    cards.sort_by(|a, b| {
        b.files_changed_since
            .cmp(&a.files_changed_since)
            .then_with(|| b.last_verified_at.cmp(&a.last_verified_at))
    });
    cards.truncate(limit);
    Ok(cards)
}

/// Default thresholds/caps for [`things_worth_looking_at`] -- deliberately
/// conservative (2 occurrences/sessions is the smallest number that still
/// means "repeated", not "happened once"). Each card kind is capped
/// independently so one noisy kind can't crowd out the others.
const DEFAULT_MIN_COMMAND_FAILURES: usize = 2;
const DEFAULT_MIN_ERROR_SESSIONS: usize = 2;
const DEFAULT_MIN_SUBAGENTS: usize = 2;
const PER_KIND_LIMIT: usize = 5;

/// Merges the five card kinds above into one ranked list (a 5th,
/// `VerificationGap`, normalizes errors, commands, and validation
/// outcomes into deterministic insight types). Ranking is deliberately
/// simple and explainable rather than
/// a contrived scoring formula: repeated failures/errors are a real
/// signal something is actually wrong, ranked first; large agent runs,
/// high-token sessions, and verification gaps are "notable," ranked
/// after. Within each kind, the query's own ORDER BY already ranks by
/// severity/recency. `project_key` narrows to one project when `Some`,
/// matching the underlying functions' own semantics -- Home's own caller
/// passes `None` (this surface stays global there, unchanged from before
/// this parameter existed). `provider_key` narrows to one provider the
/// same way, threaded straight through to all five underlying functions.
pub fn things_worth_looking_at(
    conn: &Connection,
    window_start: Option<&str>,
    project_key: Option<&str>,
    provider_key: Option<&str>,
) -> Result<Vec<InsightCard>, String> {
    let mut cards = Vec::new();
    cards.extend(
        repeated_command_failures(
            conn,
            window_start,
            project_key,
            provider_key,
            DEFAULT_MIN_COMMAND_FAILURES,
            PER_KIND_LIMIT,
        )?
        .into_iter()
        .map(InsightCard::RepeatedCommandFailure),
    );
    cards.extend(
        repeated_errors(
            conn,
            window_start,
            project_key,
            provider_key,
            DEFAULT_MIN_ERROR_SESSIONS,
            PER_KIND_LIMIT,
        )?
        .into_iter()
        .map(InsightCard::RepeatedError),
    );
    cards.extend(
        large_agent_runs(
            conn,
            window_start,
            project_key,
            provider_key,
            DEFAULT_MIN_SUBAGENTS,
            PER_KIND_LIMIT,
        )?
        .into_iter()
        .map(InsightCard::LargeAgentRun),
    );
    cards.extend(
        high_token_sessions(conn, window_start, project_key, provider_key, PER_KIND_LIMIT)?
            .into_iter()
            .map(InsightCard::HighTokenSession),
    );
    cards.extend(
        verification_gaps(conn, window_start, project_key, provider_key, PER_KIND_LIMIT)?
            .into_iter()
            .map(InsightCard::VerificationGap),
    );
    Ok(cards)
}

// ============================================================================
// Since last visit / this week (spec §9)
// ============================================================================

/// Reads the last stored `app_state` value for `last_visit_at`. `None` on
/// a fresh install (no visit has ever been recorded) -- callers must
/// treat that as "nothing to summarize yet," not as an error.
pub fn get_last_visit_at(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM app_state WHERE key = 'last_visit_at'",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| format!("Failed to read last_visit_at: {e}"))
}

/// Records `timestamp` as the new `last_visit_at`. Callers must call this
/// AFTER reading the previous value via [`get_last_visit_at`] (e.g. for
/// [`since_last_visit_summary`]'s own `since` boundary) -- recording first
/// would overwrite the very value the summary needs to compare against.
pub fn record_visit(conn: &Connection, timestamp: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES ('last_visit_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [timestamp],
    )
    .map_err(|e| format!("Failed to record last_visit_at: {e}"))?;
    Ok(())
}

/// Everything active since `since` (a session counts if its recency --
/// `last_message_time`, falling back to `last_modified`, matching
/// History's own convention -- falls on or after `since`).
pub fn since_last_visit_summary(
    conn: &Connection,
    since: &str,
) -> Result<SinceLastVisitSummary, String> {
    let (session_count, message_count, total_tokens, error_count, project_count, tool_call_count, agent_run_count): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT
                COUNT(DISTINCT s.id),
                COALESCE(SUM(s.message_count), 0),
                COALESCE(SUM(s.total_tokens), 0),
                COALESCE((
                    SELECT COUNT(*) FROM error e
                    JOIN session se ON se.id = e.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) >= ?1
                ), 0),
                COUNT(DISTINCT s.project_id),
                COALESCE((
                    SELECT COUNT(*) FROM tool_call tc
                    JOIN session se ON se.id = tc.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) >= ?1
                ), 0),
                COALESCE((
                    SELECT COUNT(*) FROM agent_run ar
                    JOIN session se ON se.id = ar.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) >= ?1
                ), 0)
             FROM session s
             WHERE s.is_subagent = 0
               AND COALESCE(s.last_message_time, s.last_modified) >= ?1",
            [since],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|e| format!("Failed to compute since_last_visit_summary: {e}"))?;

    Ok(SinceLastVisitSummary {
        since: since.to_string(),
        session_count: usize::try_from(session_count).unwrap_or(0),
        message_count: usize::try_from(message_count).unwrap_or(0),
        total_tokens,
        error_count: usize::try_from(error_count).unwrap_or(0),
        project_count: usize::try_from(project_count).unwrap_or(0),
        tool_call_count: usize::try_from(tool_call_count).unwrap_or(0),
        agent_run_count: usize::try_from(agent_run_count).unwrap_or(0),
        primary_projects: primary_projects_since(conn, since, 3)?,
    })
}

/// Up to `limit` project display names most recently active on/after
/// `since`, most-recent first -- spec §9.1's "Primary projects" list.
pub fn primary_projects_since(
    conn: &Connection,
    since: &str,
    limit: usize,
) -> Result<Vec<String>, String> {
    let sql = "
        SELECT p.display_name
        FROM session s
        JOIN project p ON p.id = s.project_id
        WHERE s.is_subagent = 0
          AND COALESCE(s.last_message_time, s.last_modified) >= ?1
        GROUP BY p.id
        ORDER BY MAX(COALESCE(s.last_message_time, s.last_modified)) DESC
        LIMIT ?2";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare primary_projects_since query: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since, limit], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to run primary_projects_since query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read primary_projects_since row: {e}"))
}

/// Same shape as [`since_last_visit_summary`], bounded on both ends.
pub fn this_week_summary(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> Result<ThisWeekSummary, String> {
    let (session_count, message_count, total_tokens, error_count, project_count, tool_call_count, agent_run_count): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT
                COUNT(DISTINCT s.id),
                COALESCE(SUM(s.message_count), 0),
                COALESCE(SUM(s.total_tokens), 0),
                COALESCE((
                    SELECT COUNT(*) FROM error e
                    JOIN session se ON se.id = e.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) BETWEEN ?1 AND ?2
                ), 0),
                COUNT(DISTINCT s.project_id),
                COALESCE((
                    SELECT COUNT(*) FROM tool_call tc
                    JOIN session se ON se.id = tc.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) BETWEEN ?1 AND ?2
                ), 0),
                COALESCE((
                    SELECT COUNT(*) FROM agent_run ar
                    JOIN session se ON se.id = ar.session_id
                    WHERE se.is_subagent = 0
                      AND COALESCE(se.last_message_time, se.last_modified) BETWEEN ?1 AND ?2
                ), 0)
             FROM session s
             WHERE s.is_subagent = 0
               AND COALESCE(s.last_message_time, s.last_modified) BETWEEN ?1 AND ?2",
            [window_start, window_end],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|e| format!("Failed to compute this_week_summary: {e}"))?;

    let peak_agents_in_session: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(cnt), 0) FROM (
                SELECT COUNT(*) as cnt FROM agent_run ar
                JOIN session se ON se.id = ar.session_id
                WHERE se.is_subagent = 0
                  AND COALESCE(se.last_message_time, se.last_modified) BETWEEN ?1 AND ?2
                GROUP BY ar.session_id
             )",
            [window_start, window_end],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to compute peak_agents_in_session: {e}"))?;

    Ok(ThisWeekSummary {
        window_start: window_start.to_string(),
        window_end: window_end.to_string(),
        session_count: usize::try_from(session_count).unwrap_or(0),
        message_count: usize::try_from(message_count).unwrap_or(0),
        total_tokens,
        error_count: usize::try_from(error_count).unwrap_or(0),
        project_count: usize::try_from(project_count).unwrap_or(0),
        tool_call_count: usize::try_from(tool_call_count).unwrap_or(0),
        agent_run_count: usize::try_from(agent_run_count).unwrap_or(0),
        peak_agents_in_session: usize::try_from(peak_agents_in_session).unwrap_or(0),
        provider_breakdown: provider_token_breakdown_in_window(conn, window_start, window_end)?,
    })
}

/// Token share per provider active in `[start, end]`, most-tokens-first,
/// providers with zero tokens in the window omitted -- spec §9.2's
/// "Claude Code 71% / Codex 24% / Cursor 5%" breakdown. Percentages are
/// deliberately NOT computed here; see [`ProviderTokenShare`]'s own doc.
pub fn provider_token_breakdown_in_window(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> Result<Vec<ProviderTokenShare>, String> {
    let sql = "
        SELECT pr.provider_key, pr.display_name, COALESCE(SUM(s.total_tokens), 0) as tokens
        FROM session s
        JOIN project p ON p.id = s.project_id
        JOIN provider pr ON pr.id = p.provider_id
        WHERE s.is_subagent = 0
          AND COALESCE(s.last_message_time, s.last_modified) BETWEEN ?1 AND ?2
        GROUP BY pr.id
        HAVING tokens > 0
        ORDER BY tokens DESC";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare provider_token_breakdown_in_window query: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![window_start, window_end], |row| {
            Ok(ProviderTokenShare {
                provider_key: row.get(0)?,
                display_name: row.get(1)?,
                total_tokens: row.get(2)?,
            })
        })
        .map_err(|e| format!("Failed to run provider_token_breakdown_in_window query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read provider_token_breakdown_in_window row: {e}"))
}

/// Shared drill-down: every session active in `[start, end]`, most recent
/// first. Backs both `since_last_visit_summary` and `this_week_summary`'s
/// clickable counts (spec §9's "every number must drill down" rule).
pub fn list_sessions_in_window(
    conn: &Connection,
    start: &str,
    end: &str,
    limit: usize,
) -> Result<Vec<SessionListItem>, String> {
    let sql = "
        SELECT s.file_path, p.display_name, s.summary,
               s.last_message_time, s.total_tokens, s.message_count
        FROM session s
        JOIN project p ON p.id = s.project_id
        WHERE s.is_subagent = 0
          AND COALESCE(s.last_message_time, s.last_modified) BETWEEN ?1 AND ?2
        ORDER BY COALESCE(s.last_message_time, s.last_modified) DESC
        LIMIT ?3";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare list_sessions_in_window query: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![start, end, limit], |row| {
            Ok(SessionListItem {
                session_id: row.get(0)?,
                project_name: row.get(1)?,
                summary: row.get(2)?,
                last_message_time: row.get(3)?,
                total_tokens: row.get(4)?,
                message_count: {
                    let count: i64 = row.get(5)?;
                    usize::try_from(count).unwrap_or(0)
                },
            })
        })
        .map_err(|e| format!("Failed to run list_sessions_in_window query: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read list_sessions_in_window row: {e}"))
}

// ============================================================================
// Agent-run tree (spec §17)
// ============================================================================

struct AgentRunRow {
    id: i64,
    parent_agent_run_id: Option<i64>,
    subagent_type: Option<String>,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    tool_call_count: usize,
    child_session_id: Option<String>,
}

fn build_forest(rows: Vec<AgentRunRow>) -> Vec<AgentRunNode> {
    let mut children_by_parent: HashMap<Option<i64>, Vec<AgentRunRow>> = HashMap::new();
    for row in rows {
        children_by_parent
            .entry(row.parent_agent_run_id)
            .or_default()
            .push(row);
    }

    fn assemble(
        parent: Option<i64>,
        children_by_parent: &mut HashMap<Option<i64>, Vec<AgentRunRow>>,
    ) -> Vec<AgentRunNode> {
        let Some(rows) = children_by_parent.remove(&parent) else {
            return Vec::new();
        };
        rows.into_iter()
            .map(|row| {
                let children = assemble(Some(row.id), children_by_parent);
                AgentRunNode {
                    agent_run_id: row.id,
                    subagent_type: row.subagent_type,
                    status: row.status,
                    started_at: row.started_at,
                    ended_at: row.ended_at,
                    tool_call_count: row.tool_call_count,
                    child_session_id: row.child_session_id,
                    children,
                }
            })
            .collect()
    }

    assemble(None, &mut children_by_parent)
}

/// Every agent run belonging to `session_id` (a `file_path`), assembled
/// into a tree via `parent_agent_run_id`. Returns an empty tree (not an
/// error) when the session doesn't exist or has no agent runs -- an
/// ordinary, common case for most sessions.
///
/// A genuinely MULTI-LEVEL tree
/// (an agent run whose own launches live in a DIFFERENT session's rows --
/// a correlated subagent transcript, `archive_db::ingest::claude::
/// ingest_subagent_tree`) needs the fetch itself to walk `child_session_id`
/// chains, not just the in-memory `build_forest` grouping below (which was
/// already fully general, just previously starved of any rows beyond the
/// root session's own). `WITH RECURSIVE` seeds with the root session's own
/// `agent_run` rows, then repeatedly pulls in rows belonging to whatever
/// session an already-included row's `child_session_id` points at.
pub fn get_agent_run_tree(conn: &Connection, session_id: &str) -> Result<AgentRunTree, String> {
    let sql = "
        WITH RECURSIVE tree(id, parent_agent_run_id, subagent_type, status,
                             started_at, ended_at, tool_call_count, child_session_id) AS (
            SELECT ar.id, ar.parent_agent_run_id, ar.subagent_type, ar.status,
                   ar.started_at, ar.ended_at, ar.tool_call_count, ar.child_session_id
            FROM agent_run ar
            JOIN session s ON s.id = ar.session_id
            WHERE s.file_path = ?1
            UNION ALL
            SELECT ar2.id, ar2.parent_agent_run_id, ar2.subagent_type, ar2.status,
                   ar2.started_at, ar2.ended_at, ar2.tool_call_count, ar2.child_session_id
            FROM agent_run ar2
            JOIN tree ON ar2.session_id = tree.child_session_id
        )
        SELECT tree.id, tree.parent_agent_run_id, tree.subagent_type, tree.status,
               tree.started_at, tree.ended_at, tree.tool_call_count, cs.file_path
        FROM tree
        LEFT JOIN session cs ON cs.id = tree.child_session_id
        ORDER BY tree.id";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare get_agent_run_tree query: {e}"))?;
    let rows: Vec<AgentRunRow> = stmt
        .query_map([session_id], |row| {
            Ok(AgentRunRow {
                id: row.get(0)?,
                parent_agent_run_id: row.get(1)?,
                subagent_type: row.get(2)?,
                status: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                tool_call_count: {
                    let count: i64 = row.get(6)?;
                    usize::try_from(count).unwrap_or(0)
                },
                child_session_id: row.get(7)?,
            })
        })
        .map_err(|e| format!("Failed to run get_agent_run_tree query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read get_agent_run_tree row: {e}"))?;

    let total_count = rows.len();
    let roots = build_forest(rows);

    Ok(AgentRunTree {
        session_id: session_id.to_string(),
        roots,
        total_count,
    })
}

/// Bounded preview of an error's own signature list -- a detail panel, not
/// a full report (mirrors `command`/`error` preview-length conventions
/// elsewhere in this module).
const MAX_DETAIL_ERRORS: usize = 10;

/// A single agent run's own detail, keyed by its internal row id (from a
/// tree node's `agent_run_id`).
///
/// Shows purpose, duration, model, tokens, tools, files, errors, and
/// source transcript on selection. `purpose` comes from the
/// PARENT's own `tool_call.input_json` (the `Agent`/Task tool's launch
/// instructions); `model`/`total_tokens`/`tools_used`/`files_touched`/
/// `errors` all come from the CHILD (subagent) session's own already-
/// computed rollups and rows -- reusing the exact same per-session
/// aggregation this archive already does for every real session, not new
/// aggregation logic. All of these stay `None`/empty when
/// `child_session_id` is `None` (an unlinked or leaf-less run) rather than
/// guessing.
/// Raw row shape for `get_agent_run_detail`'s main query -- a private
/// struct rather than an 11-element tuple, purely for readability.
struct AgentRunDetailRow {
    session_id: String,
    subagent_type: Option<String>,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    tool_call_count: i64,
    purpose_input_json: Option<String>,
    child_session_id: Option<String>,
    child_row_id: Option<i64>,
    model: Option<String>,
    total_tokens: Option<i64>,
}

pub fn get_agent_run_detail(conn: &Connection, agent_run_id: i64) -> Result<AgentRunDetail, String> {
    let row: AgentRunDetailRow = conn
        .query_row(
            "SELECT s.file_path, ar.subagent_type, ar.status,
                    ar.started_at, ar.ended_at, ar.tool_call_count,
                    ptc.input_json, cs.file_path, cs.id, cs.dominant_model, cs.total_tokens
             FROM agent_run ar
             JOIN session s ON s.id = ar.session_id
             LEFT JOIN tool_call ptc ON ptc.id = ar.parent_tool_call_id
             LEFT JOIN session cs ON cs.id = ar.child_session_id
             WHERE ar.id = ?1",
            [agent_run_id],
            |r| {
                Ok(AgentRunDetailRow {
                    session_id: r.get(0)?,
                    subagent_type: r.get(1)?,
                    status: r.get(2)?,
                    started_at: r.get(3)?,
                    ended_at: r.get(4)?,
                    tool_call_count: r.get(5)?,
                    purpose_input_json: r.get(6)?,
                    child_session_id: r.get(7)?,
                    child_row_id: r.get(8)?,
                    model: r.get(9)?,
                    total_tokens: r.get(10)?,
                })
            },
        )
        .map_err(|e| format!("Failed to load agent run {agent_run_id}: {e}"))?;

    let purpose = row
        .purpose_input_json
        .as_deref()
        .and_then(extract_agent_launch_purpose);

    let (tools_used, files_touched, error_count, errors) = match row.child_row_id {
        Some(child_id) => (
            query_tool_usage_for_session(conn, child_id)?,
            query_files_touched_for_session(conn, child_id)?,
            query_error_count_for_session(conn, child_id)?,
            query_error_previews_for_session(conn, child_id)?,
        ),
        None => (Vec::new(), Vec::new(), 0, Vec::new()),
    };

    Ok(AgentRunDetail {
        agent_run_id,
        session_id: row.session_id,
        subagent_type: row.subagent_type,
        status: row.status,
        started_at: row.started_at,
        ended_at: row.ended_at,
        tool_call_count: usize::try_from(row.tool_call_count).unwrap_or(0),
        purpose,
        child_session_id: row.child_session_id,
        model: row.model,
        total_tokens: row.total_tokens,
        tools_used,
        files_touched,
        error_count,
        errors,
    })
}

/// Extracts an `Agent`/Task `tool_use`'s own launch instructions from its
/// stored `input_json` -- the fuller `prompt` field when present (the
/// actual task instructions), else the shorter `description`.
fn extract_agent_launch_purpose(input_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input_json).ok()?;
    value
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("description").and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn query_tool_usage_for_session(
    conn: &Connection,
    session_row_id: i64,
) -> Result<Vec<AgentRunToolUsage>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, COUNT(*) FROM tool_call
             WHERE session_id = ?1
             GROUP BY tool_name
             ORDER BY COUNT(*) DESC, tool_name ASC",
        )
        .map_err(|e| format!("Failed to prepare tool usage query: {e}"))?;
    let rows = stmt
        .query_map([session_row_id], |row| {
            let count: i64 = row.get(1)?;
            Ok(AgentRunToolUsage {
                tool_name: row.get(0)?,
                count: usize::try_from(count).unwrap_or(0),
            })
        })
        .map_err(|e| format!("Failed to run tool usage query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read tool usage row: {e}"));
    rows
}

fn query_files_touched_for_session(
    conn: &Connection,
    session_row_id: i64,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT file_path FROM file_event
             WHERE session_id = ?1
             ORDER BY file_path",
        )
        .map_err(|e| format!("Failed to prepare files-touched query: {e}"))?;
    let rows = stmt
        .query_map([session_row_id], |row| row.get(0))
        .map_err(|e| format!("Failed to run files-touched query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read files-touched row: {e}"));
    rows
}

fn query_error_count_for_session(conn: &Connection, session_row_id: i64) -> Result<usize, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM error WHERE session_id = ?1",
            [session_row_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count errors for session: {e}"))?;
    Ok(usize::try_from(count).unwrap_or(0))
}

fn query_error_previews_for_session(
    conn: &Connection,
    session_row_id: i64,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT error_signature FROM error
             WHERE session_id = ?1
             ORDER BY occurred_at ASC
             LIMIT ?2",
        )
        .map_err(|e| format!("Failed to prepare error-preview query: {e}"))?;
    let limit = i64::try_from(MAX_DETAIL_ERRORS).unwrap_or(i64::MAX);
    let rows = stmt
        .query_map(rusqlite::params![session_row_id, limit], |row| row.get(0))
        .map_err(|e| format!("Failed to run error-preview query: {e}"))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("Failed to read error-preview row: {e}"));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_db::backfill::run_full_backfill;
    use crate::archive_db::migrate::migrate;
    use std::fs;
    use tempfile::TempDir;

    fn write_fixture_project(
        claude_base: &std::path::Path,
        project_dir_name: &str,
        session_file_name: &str,
        session_lines: &str,
    ) -> String {
        let project_dir = claude_base.join("projects").join(project_dir_name);
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join(session_file_name), session_lines).unwrap();
        claude_base.to_string_lossy().to_string()
    }

    #[test]
    fn normalize_command_template_replaces_paths_numbers_strings_and_uuids() {
        assert_eq!(
            normalize_command_template("pytest tests/test_foo.py"),
            "pytest <PATH>"
        );
        assert_eq!(
            normalize_command_template("docker logs 12345"),
            "docker logs <N>"
        );
        assert_eq!(
            normalize_command_template(r#"grep "TODO" src/file.txt"#),
            "grep <STR> <PATH>"
        );
        assert_eq!(
            normalize_command_template("kubectl get pod 550e8400-e29b-41d4-a716-446655440000"),
            "kubectl get pod <UUID>"
        );
        assert_eq!(
            normalize_command_template("npm test"),
            "npm test",
            "a command with no variable-looking tokens is its own template"
        );
    }

    #[test]
    fn compute_trend_classifies_new_increasing_decreasing_and_steady() {
        let now = chrono::Utc::now();
        let days_ago = |n: i64| (now - chrono::Duration::days(n)).to_rfc3339();

        assert_eq!(compute_trend(&[days_ago(1)], now), ProblemTrend::New);

        assert_eq!(
            compute_trend(&[days_ago(10), days_ago(1), days_ago(2), days_ago(3)], now),
            ProblemTrend::Increasing
        );

        assert_eq!(
            compute_trend(
                &[days_ago(10), days_ago(11), days_ago(12), days_ago(1)],
                now
            ),
            ProblemTrend::Decreasing
        );

        assert_eq!(
            compute_trend(&[days_ago(10), days_ago(2)], now),
            ProblemTrend::Steady
        );

        assert_eq!(
            compute_trend(&[], now),
            ProblemTrend::Steady,
            "no occurrences at all is a degenerate case, not a crash"
        );
    }

    fn failed_command_line(uuid: &str, ts: &str, command: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}-a","sessionId":"s1","timestamp":"{ts}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid}","name":"Bash","input":{{"command":"{command}"}}}}],"model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}}}}}}
{{"uuid":"{uuid}-b","sessionId":"s1","timestamp":"{ts}","type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{uuid}","content":"FAILED","is_error":true}}]}}}}"#
        )
    }

    fn passing_command_line(uuid: &str, ts: &str, command: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}-a","sessionId":"s1","timestamp":"{ts}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid}","name":"Bash","input":{{"command":"{command}"}}}}],"model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}}}}}}
{{"uuid":"{uuid}-b","sessionId":"s1","timestamp":"{ts}","type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{uuid}","content":"ok","is_error":false}}]}}}}"#
        )
    }

    fn edit_file_line(uuid: &str, ts: &str, file_path: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}-a","sessionId":"s1","timestamp":"{ts}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid}","name":"Edit","input":{{"file_path":"{file_path}"}}}}],"model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}}}}}}
{{"uuid":"{uuid}-b","sessionId":"s1","timestamp":"{ts}","type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{uuid}","content":"ok"}}]}}}}"#
        )
    }

    fn agent_launch_line(uuid: &str, ts: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}-a","sessionId":"s1","timestamp":"{ts}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{uuid}","name":"Agent","input":{{"subagent_type":"Explore"}}}}],"model":"claude-x","usage":{{"input_tokens":10,"output_tokens":5}}}}}}
{{"uuid":"{uuid}-b","sessionId":"s1","timestamp":"{ts}","type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{uuid}","content":"done"}}]}}}}"#
        )
    }

    /// Relabels an already-ingested Claude project as belonging to a
    /// different, synthetic provider -- used by the `_respects_provider_key`
    /// tests below. No non-Claude provider populates `command`/`error`/
    /// `agent_run` with realistic content via its own real parser today
    /// (see `commands::stats::FILE_BASED_STATS_PROVIDERS`'s own doc comment
    /// on sparse `command`/`agent_run` tables for most providers), so these
    /// tests exercise the `provider_key` SQL filter itself directly rather
    /// than depending on a second provider's real ingest pipeline --
    /// per-provider ingest correctness is already covered separately by
    /// each provider's own fixture test in `archive_db::ingest::provider`.
    fn relabel_project_provider(conn: &Connection, project_key: &str, provider_key: &str) {
        let provider_id =
            crate::archive_db::ingest::upsert_provider(conn, provider_key, provider_key, "B", 1)
                .unwrap();
        conn.execute(
            "UPDATE project SET provider_id = ?1 WHERE project_key = ?2",
            rusqlite::params![provider_id, project_key],
        )
        .unwrap();
    }

    fn plain_message_line(uuid: &str, ts: &str) -> String {
        format!(
            r#"{{"uuid":"{uuid}","sessionId":"s1","timestamp":"{ts}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}],"model":"claude-x","usage":{{"input_tokens":1000,"output_tokens":500}}}}}}"#
        )
    }

    /// Every caller of this helper (and `seeded_conn_multi_session` below)
    /// must be `#[serial_test::serial]` -- `run_full_backfill` now also
    /// scans every file-based provider (universal-provider-ingestion
    /// plan, Step 5), and this guard mutates the process-global
    /// `CODEX_HOME` env var to avoid parsing this machine's real,
    /// substantial `~/.codex` data on every test (see
    /// `archive_db::test_support`'s own doc comment for the full
    /// explanation, including why a handful of real Antigravity sessions
    /// may still incidentally appear and why every assertion in this file
    /// scopes to Claude-specific queries/thresholds rather than relying
    /// on nothing else ever being ingested).
    async fn seeded_conn(lines: &str) -> Connection {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", lines);
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        conn
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_groups_by_identical_template_and_applies_threshold() {
        let lines = format!(
            "{}\n{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
            failed_command_line("f2", "2026-01-01T00:01:00Z", "pytest -q"),
            failed_command_line("f3", "2026-01-01T00:02:00Z", "npm test"),
        );
        let conn = seeded_conn(&lines).await;

        let two_plus = repeated_command_failures(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(two_plus.len(), 1);
        assert_eq!(two_plus[0].shell_command, "pytest -q");
        assert_eq!(two_plus[0].template, "pytest -q");
        assert_eq!(two_plus[0].failure_count, 2);
        assert_eq!(two_plus[0].session_count, 1);
        assert_eq!(two_plus[0].first_occurred_at, "2026-01-01T00:00:00Z");
        assert!(two_plus[0].last_occurred_at.starts_with("2026-01-01T00:01:00"));
        assert!(!two_plus[0].sample_session_id.is_empty());

        let one_plus = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(one_plus.len(), 2, "both distinct templates qualify at threshold 1");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_groups_different_paths_under_one_template() {
        // The whole point of normalizing before grouping -- two genuinely
        // different exact commands (different test files) must count as
        // ONE repeated failure, which the previous exact-text `GROUP BY`
        // could never do.
        let lines = format!(
            "{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest tests/test_foo.py"),
            failed_command_line("f2", "2026-01-01T00:01:00Z", "pytest tests/test_bar.py"),
        );
        let conn = seeded_conn(&lines).await;

        let results = repeated_command_failures(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(results.len(), 1, "both normalize to the same template");
        assert_eq!(results[0].template, "pytest <PATH>");
        assert_eq!(
            results[0].shell_command, "pytest tests/test_bar.py",
            "shell_command shows the most recent RAW example, not the template"
        );
        assert_eq!(results[0].failure_count, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_dismiss_hides_it_from_future_results() {
        let lines = format!(
            "{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
            failed_command_line("f2", "2026-01-01T00:01:00Z", "pytest -q"),
        );
        let conn = seeded_conn(&lines).await;

        let before = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(before.len(), 1);

        dismiss_problem(&conn, "command_failure", &before[0].template).unwrap();
        let after = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert!(after.is_empty(), "a dismissed template must not resurface");

        // Idempotent: dismissing an already-dismissed template again must
        // not error.
        dismiss_problem(&conn, "command_failure", &before[0].template).unwrap();

        // Dismissal is scoped by kind -- the same signature under the
        // "error" kind must be unaffected.
        let dismissed_as_error = dismissed_signatures(&conn, "error").unwrap();
        assert!(!dismissed_as_error.contains(&before[0].template));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_respects_window_start() {
        let lines = format!(
            "{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
            failed_command_line("f2", "2026-06-01T00:00:00Z", "pytest -q"),
        );
        let conn = seeded_conn(&lines).await;

        let all = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(all[0].failure_count, 2);

        let windowed =
            repeated_command_failures(&conn, Some("2026-03-01T00:00:00Z"), None, None, 1, 10).unwrap();
        assert_eq!(windowed[0].failure_count, 1, "only the June failure is in-window");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_respects_project_key() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base_a = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base_a).await.unwrap();

        // Two distinct projects, each with a single matching failure --
        // global (None) sees both project's sessions contributing to the
        // same "pytest -q" signature; scoping to project A's own key must
        // narrow the count down to just that project's one occurrence.
        let global = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(global[0].failure_count, 2, "both projects share the same command text");

        let project_a_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-a%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let scoped = repeated_command_failures(&conn, None, Some(&project_a_key), None, 1, 10).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].failure_count, 1, "only project A's own failure counts");

        let no_such_project =
            repeated_command_failures(&conn, None, Some("no-such-project"), None, 1, 10).unwrap();
        assert!(
            no_such_project.is_empty(),
            "an unknown project_key must match nothing, not fall back to global"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_command_failures_respects_provider_key() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        relabel_project_provider(&conn, &project_b_key, "gemini");

        // provider_key = None (the Tauri layer's only value in this plan's
        // scope) must stay a true no-op -- both providers' failures still
        // merge into one global count.
        let global = repeated_command_failures(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(global[0].failure_count, 2, "both providers share the same command text");

        let scoped = repeated_command_failures(&conn, None, None, Some("claude"), 1, 10).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].failure_count, 1, "only claude's own failure counts");

        let no_such_provider =
            repeated_command_failures(&conn, None, None, Some("no-such-provider"), 1, 10).unwrap();
        assert!(
            no_such_provider.is_empty(),
            "an unknown provider_key must match nothing, not fall back to global"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_errors_thresholds_on_distinct_session_count() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base_a = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base_a).await.unwrap();

        // Two sessions, each with one failure sharing the same signature
        // ("FAILED" -> identical error_signature both times).
        let two_sessions = repeated_errors(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(two_sessions.len(), 1);
        assert_eq!(two_sessions[0].session_count, 2);
        assert_eq!(two_sessions[0].occurrence_count, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn repeated_errors_respects_provider_key() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        relabel_project_provider(&conn, &project_b_key, "gemini");

        let global = repeated_errors(&conn, None, None, None, 1, 10).unwrap();
        assert_eq!(global[0].session_count, 2, "both providers' sessions share the signature");

        let scoped = repeated_errors(&conn, None, None, Some("claude"), 1, 10).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].session_count, 1, "only claude's own session counts");

        let no_such_provider =
            repeated_errors(&conn, None, None, Some("no-such-provider"), 1, 10).unwrap();
        assert!(
            no_such_provider.is_empty(),
            "an unknown provider_key must match nothing, not fall back to global"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn error_occurrences_returns_every_raw_occurrence_for_one_signature() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let grouped = repeated_errors(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(grouped.len(), 1);
        let signature = &grouped[0].error_signature;

        // No min-sessions threshold: every raw occurrence of an already
        // identified recurring error comes back, not just ones crossing a
        // "worth surfacing" bar.
        let occurrences = error_occurrences(&conn, signature, None, None, 10).unwrap();
        assert_eq!(occurrences.len(), 2);
        assert!(occurrences.iter().all(|o| o.occurred_at.is_some()));
        let project_names: std::collections::HashSet<_> =
            occurrences.iter().map(|o| o.project_name.clone()).collect();
        assert_eq!(project_names.len(), 2, "both fixture projects are represented");

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let scoped = error_occurrences(&conn, signature, Some(&project_b_key), None, 10).unwrap();
        assert_eq!(scoped.len(), 1, "project_key narrows to just that project's occurrence");

        let unknown_signature = error_occurrences(&conn, "no-such-signature", None, None, 10).unwrap();
        assert!(unknown_signature.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn similar_error_resolutions_finds_a_later_passing_verification_in_another_project() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        // Project A: the same error, later followed by a passing test --
        // this is the "reusable solution" evidence.
        let lines_a = format!(
            "{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
            passing_command_line("t1", "2026-01-01T00:05:00Z", "pytest -q"),
        );
        let claude_base = write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &lines_a);
        // Project B: the same error, no later verification at all -- this
        // is the "currently viewed" project, with nothing to offer itself.
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &failed_command_line("f2", "2026-01-01T00:00:01Z", "pytest -q"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let project_a_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-a%'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Excluding project B (the "current" project) surfaces project A's
        // real later-passing verification.
        let found = similar_error_resolutions(&conn, "FAILED", Some(&project_b_key), 10).unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0].session_id.contains("project-a"),
            "the resolution must come from project A's own session, not project B's"
        );
        assert_eq!(found[0].verification_command, "pytest -q");
        assert!(found[0].error_occurred_at.is_some());

        // Excluding project A (the one WITH the resolution) leaves nothing
        // -- project B never got a later passing verification.
        let excluded = similar_error_resolutions(&conn, "FAILED", Some(&project_a_key), 10).unwrap();
        assert!(excluded.is_empty());

        // No exclusion (global view) still finds project A's evidence.
        let global = similar_error_resolutions(&conn, "FAILED", None, 10).unwrap();
        assert_eq!(global.len(), 1);

        let unknown = similar_error_resolutions(&conn, "no-such-signature", None, 10).unwrap();
        assert!(unknown.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn large_agent_runs_counts_agent_run_rows_per_session_with_threshold() {
        let lines = format!(
            "{}\n{}\n{}\n",
            agent_launch_line("a1", "2026-01-01T00:00:00Z"),
            agent_launch_line("a2", "2026-01-01T00:01:00Z"),
            agent_launch_line("a3", "2026-01-01T00:02:00Z"),
        );
        let conn = seeded_conn(&lines).await;

        let results = large_agent_runs(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subagent_count, 3);
        assert!(results[0].session_started_at.is_some());

        let none = large_agent_runs(&conn, None, None, None, 4, 10).unwrap();
        assert!(none.is_empty(), "threshold above the real count must exclude the session");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn large_agent_runs_respects_provider_key() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &format!(
                "{}\n{}\n",
                agent_launch_line("a1", "2026-01-01T00:00:00Z"),
                agent_launch_line("a2", "2026-01-01T00:01:00Z"),
            ),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &format!(
                "{}\n{}\n",
                agent_launch_line("a3", "2026-01-02T00:00:00Z"),
                agent_launch_line("a4", "2026-01-02T00:01:00Z"),
            ),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        relabel_project_provider(&conn, &project_b_key, "gemini");

        let global = large_agent_runs(&conn, None, None, None, 2, 10).unwrap();
        assert_eq!(global.len(), 2, "both providers' sessions qualify at the threshold");

        let scoped = large_agent_runs(&conn, None, None, Some("claude"), 2, 10).unwrap();
        assert_eq!(scoped.len(), 1, "only claude's own session counts");

        let no_such_provider =
            large_agent_runs(&conn, None, None, Some("no-such-provider"), 2, 10).unwrap();
        assert!(
            no_such_provider.is_empty(),
            "an unknown provider_key must match nothing, not fall back to global"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn high_token_sessions_orders_by_total_tokens_descending() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-small",
            "session1.jsonl",
            &plain_message_line("m1", "2026-01-01T00:00:00Z"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-big",
            "session1.jsonl",
            &format!(
                "{}\n{}\n",
                plain_message_line("m2", "2026-01-01T00:00:00Z"),
                plain_message_line("m3", "2026-01-01T00:01:00Z"),
            ),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        // Scoped to our own two fixture projects, matched by `session_id`
        // (the raw absolute file path, unlike `project_name` which is
        // DECODED from the Claude directory-name convention and does NOT
        // literally contain the fixture dir name -- confirmed live, not
        // assumed, after this exact filter failed once using
        // `project_name` directly) -- a handful of real Antigravity
        // sessions may incidentally appear on this machine (see
        // archive_db::test_support's own doc comment) and must not make
        // this assertion flaky.
        let results = high_token_sessions(&conn, None, None, None, 50).unwrap();
        let ours: Vec<_> = results
            .iter()
            .filter(|r| r.session_id.contains("fixture-project-small") || r.session_id.contains("fixture-project-big"))
            .collect();
        assert_eq!(ours.len(), 2);
        let big = ours.iter().find(|r| r.session_id.contains("fixture-project-big")).unwrap();
        let small = ours.iter().find(|r| r.session_id.contains("fixture-project-small")).unwrap();
        assert!(big.total_tokens > small.total_tokens);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn personal_baseline_averages_tokens_and_duration_excluding_the_given_session() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        // Session A: 1 message (1500 tokens), single timestamp -> 0 minutes.
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &plain_message_line("m1", "2026-01-01T00:00:00Z"),
        );
        // Session B: 2 messages (3000 tokens), 2 minutes apart.
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &format!(
                "{}\n{}\n",
                plain_message_line("m2", "2026-01-01T00:00:00Z"),
                plain_message_line("m3", "2026-01-01T00:02:00Z"),
            ),
        );
        // Session C: 2 messages (3000 tokens), 10 minutes apart -- excluded
        // below, standing in for "the session currently being viewed."
        write_fixture_project(
            dir.path(),
            "-fixture-project-c",
            "session1.jsonl",
            &format!(
                "{}\n{}\n",
                plain_message_line("m4", "2026-01-01T00:00:00Z"),
                plain_message_line("m5", "2026-01-01T00:10:00Z"),
            ),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let session_c_path: String = conn
            .query_row(
                "SELECT file_path FROM session WHERE file_path LIKE '%fixture-project-c%'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let all = personal_baseline(&conn, "claude", None).unwrap();
        assert_eq!(all.session_count, 3);
        assert!((all.average_total_tokens - 2500.0).abs() < 0.01);
        assert!((all.average_duration_minutes - 4.0).abs() < 0.01);

        let excluding_c = personal_baseline(&conn, "claude", Some(&session_c_path)).unwrap();
        assert_eq!(excluding_c.session_count, 2);
        assert!((excluding_c.average_total_tokens - 2250.0).abs() < 0.01);
        assert!((excluding_c.average_duration_minutes - 1.0).abs() < 0.01);

        let unknown_provider = personal_baseline(&conn, "no-such-provider", None).unwrap();
        assert_eq!(unknown_provider.session_count, 0);
        assert!(unknown_provider.average_total_tokens.abs() < 0.01);
        assert!(unknown_provider.average_duration_minutes.abs() < 0.01);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn high_token_sessions_respects_provider_key() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-a",
            "session1.jsonl",
            &plain_message_line("m1", "2026-01-01T00:00:00Z"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-b",
            "session1.jsonl",
            &plain_message_line("m2", "2026-01-02T00:00:00Z"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let project_b_key: String = conn
            .query_row(
                "SELECT project_key FROM project WHERE project_key LIKE '%project-b%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        relabel_project_provider(&conn, &project_b_key, "gemini");

        let global: Vec<_> = high_token_sessions(&conn, None, None, None, 50)
            .unwrap()
            .into_iter()
            .filter(|r| r.session_id.contains("fixture-project-a") || r.session_id.contains("fixture-project-b"))
            .collect();
        assert_eq!(global.len(), 2, "both providers' sessions are visible globally");

        let scoped: Vec<_> = high_token_sessions(&conn, None, None, Some("claude"), 50)
            .unwrap()
            .into_iter()
            .filter(|r| r.session_id.contains("fixture-project-a") || r.session_id.contains("fixture-project-b"))
            .collect();
        assert_eq!(scoped.len(), 1, "only claude's own session counts");
        assert!(scoped[0].session_id.contains("fixture-project-a"));

        let no_such_provider: Vec<_> = high_token_sessions(&conn, None, None, Some("no-such-provider"), 50)
            .unwrap()
            .into_iter()
            .filter(|r| r.session_id.contains("fixture-project-a") || r.session_id.contains("fixture-project-b"))
            .collect();
        assert!(
            no_such_provider.is_empty(),
            "an unknown provider_key must match nothing, not fall back to global"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn verification_gaps_flags_files_changed_after_the_last_passing_test() {
        let lines = format!(
            "{}\n{}\n{}\n",
            passing_command_line("t1", "2026-01-01T00:00:00Z", "pnpm test"),
            edit_file_line("e1", "2026-01-01T00:01:00Z", "/src/a.ts"),
            edit_file_line("e2", "2026-01-01T00:02:00Z", "/src/b.ts"),
        );
        let conn = seeded_conn(&lines).await;

        let gaps = verification_gaps(&conn, None, None, None, 10).unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].files_changed_since, 2);
        assert_eq!(gaps[0].last_verified_command, "pnpm test");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn verification_gaps_is_silent_when_nothing_changed_since_the_last_passing_test() {
        let lines = format!(
            "{}\n{}\n",
            edit_file_line("e1", "2026-01-01T00:00:00Z", "/src/a.ts"),
            passing_command_line("t1", "2026-01-01T00:01:00Z", "cargo test"),
        );
        let conn = seeded_conn(&lines).await;

        let gaps = verification_gaps(&conn, None, None, None, 10).unwrap();
        assert!(
            gaps.is_empty(),
            "the test ran AFTER the only file change -- verification is current, not stale"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn verification_gaps_does_not_flag_a_session_whose_last_test_failed() {
        let lines = format!(
            "{}\n{}\n",
            failed_command_line("t1", "2026-01-01T00:00:00Z", "pytest -q"),
            edit_file_line("e1", "2026-01-01T00:01:00Z", "/src/a.ts"),
        );
        let conn = seeded_conn(&lines).await;

        let gaps = verification_gaps(&conn, None, None, None, 10).unwrap();
        assert!(
            gaps.is_empty(),
            "a failing verification is a different signal (repeated_command_failures'/repeated_errors' job), not a fabricated 'gap' claim"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn verification_gaps_ignores_commands_outside_the_honest_allowlist() {
        let lines = format!(
            "{}\n{}\n",
            passing_command_line("t1", "2026-01-01T00:00:00Z", "ls -la"),
            edit_file_line("e1", "2026-01-01T00:01:00Z", "/src/a.ts"),
        );
        let conn = seeded_conn(&lines).await;

        let gaps = verification_gaps(&conn, None, None, None, 10).unwrap();
        assert!(
            gaps.is_empty(),
            "a command outside the test/build allowlist must never be treated as verification"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn things_worth_looking_at_merges_every_card_kind() {
        let lines = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            failed_command_line("f1", "2026-01-01T00:00:00Z", "pytest -q"),
            failed_command_line("f2", "2026-01-01T00:01:00Z", "pytest -q"),
            agent_launch_line("a1", "2026-01-01T00:02:00Z"),
            agent_launch_line("a2", "2026-01-01T00:03:00Z"),
            passing_command_line("t1", "2026-01-01T00:04:00Z", "pnpm test"),
            edit_file_line("e1", "2026-01-01T00:05:00Z", "/src/a.ts"),
        );
        let conn = seeded_conn(&lines).await;

        let cards = things_worth_looking_at(&conn, None, None, None).unwrap();
        assert!(
            cards
                .iter()
                .any(|c| matches!(c, InsightCard::RepeatedCommandFailure(_))),
            "expected a repeated-command-failure card"
        );
        assert!(
            cards.iter().any(|c| matches!(c, InsightCard::LargeAgentRun(_))),
            "expected a large-agent-run card"
        );
        assert!(
            cards.iter().any(|c| matches!(c, InsightCard::VerificationGap(_))),
            "expected a verification-gap card"
        );
        assert!(
            cards
                .iter()
                .any(|c| matches!(c, InsightCard::HighTokenSession(_))),
            "high_token_sessions has no threshold, so it always contributes when any session exists"
        );
    }

    /// Seeds N separate sessions (one per project fixture, matching
    /// `high_token_sessions_orders_by_total_tokens_descending`'s own
    /// pattern) -- unlike `seeded_conn`, which puts every line into ONE
    /// session file, sharing a `sessionId` and therefore one summed
    /// `total_tokens`. Window-boundary tests need genuinely separate
    /// sessions with distinct recency times, not messages within one.
    async fn seeded_conn_multi_session(lines: &[(&str, &str)]) -> Connection {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let mut claude_base = String::new();
        for (i, (uuid, ts)) in lines.iter().enumerate() {
            claude_base = write_fixture_project(
                dir.path(),
                &format!("-fixture-project-{i}"),
                "session1.jsonl",
                &plain_message_line(uuid, ts),
            );
        }
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();
        conn
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn visit_tracking_round_trips_and_summary_counts_activity_since() {
        // since_last_visit_summary is deliberately NOT provider-scoped (see
        // this module's Step 6 doc note -- Home's "everything you did" is a
        // unified cross-provider summary by design), so this test can't
        // scope its assertions the way the provider_key-filterable
        // functions do. Instead it dodges this machine's real incidental
        // provider data (bounded by *actual* usage, i.e. never later than
        // today) entirely by using far-future fixture/visit timestamps --
        // no real session can possibly have a `last_message_time` past
        // 2099.
        let conn = seeded_conn_multi_session(&[
            ("m1", "2099-01-01T00:00:00Z"),
            ("m2", "2099-06-01T00:00:00Z"),
        ])
        .await;

        assert_eq!(get_last_visit_at(&conn).unwrap(), None, "no visit recorded yet");

        record_visit(&conn, "2099-03-01T00:00:00Z").unwrap();
        assert_eq!(
            get_last_visit_at(&conn).unwrap(),
            Some("2099-03-01T00:00:00Z".to_string())
        );

        let summary = since_last_visit_summary(&conn, "2099-03-01T00:00:00Z").unwrap();
        assert_eq!(summary.session_count, 1, "only the June session is after the visit");
        assert_eq!(summary.total_tokens, 1500);
        assert_eq!(summary.project_count, 1, "the June session's own fixture project");
        assert_eq!(
            summary.primary_projects.len(),
            1,
            "only the one project active since the visit"
        );

        // Re-recording overwrites, not duplicates (ON CONFLICT upsert).
        record_visit(&conn, "2099-07-01T00:00:00Z").unwrap();
        assert_eq!(
            get_last_visit_at(&conn).unwrap(),
            Some("2099-07-01T00:00:00Z".to_string())
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn this_week_summary_bounds_both_ends() {
        let conn = seeded_conn_multi_session(&[
            ("m1", "2025-12-01T00:00:00Z"),
            ("m2", "2026-01-15T00:00:00Z"),
            ("m3", "2026-03-01T00:00:00Z"),
        ])
        .await;

        let summary =
            this_week_summary(&conn, "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z").unwrap();
        assert_eq!(summary.session_count, 1, "only the mid-January session is in-window");
        assert_eq!(summary.project_count, 1, "the mid-January session's own fixture project");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn this_week_summary_reports_tool_calls_agent_runs_and_provider_breakdown() {
        // Far-future timestamps, same reasoning as
        // visit_tracking_round_trips_and_summary_counts_activity_since --
        // since_last_visit_summary/this_week_summary are deliberately NOT
        // provider-scoped, so this dodges any of this machine's real
        // incidental data landing in the query window.
        let ts = "2099-04-01T00:00:00Z";
        let lines = format!(
            "{}\n{}\n{}",
            agent_launch_line("u1", ts),
            agent_launch_line("u2", ts),
            failed_command_line("u3", ts, "pytest -q"),
        );
        let conn = seeded_conn(&lines).await;

        let summary = this_week_summary(
            &conn,
            "2099-01-01T00:00:00Z",
            "2099-12-31T23:59:59Z",
        )
        .unwrap();

        assert_eq!(
            summary.tool_call_count, 3,
            "2 Agent tool_use blocks + 1 Bash tool_use block"
        );
        assert_eq!(summary.agent_run_count, 2, "one agent_run row per Agent tool_result");
        assert_eq!(
            summary.peak_agents_in_session, 2,
            "both agent runs landed in the one fixture session"
        );
        assert_eq!(
            summary.provider_breakdown.len(),
            1,
            "the fixture ingests as a single Claude project"
        );
        assert_eq!(summary.provider_breakdown[0].provider_key, "claude");
        assert_eq!(summary.provider_breakdown[0].total_tokens, summary.total_tokens);

        let since_summary = since_last_visit_summary(&conn, "2099-01-01T00:00:00Z").unwrap();
        assert_eq!(since_summary.tool_call_count, 3);
        assert_eq!(since_summary.agent_run_count, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn primary_projects_since_orders_most_recently_active_first_and_respects_limit() {
        // seeded_conn_multi_session gives each line its own fixture project
        // (-fixture-project-0/1/2), so this also exercises project_count
        // across 3 distinct projects, not just 1. Display names are looked
        // up from the `project` table, ordered by each project's OWN
        // session timestamp -- not hardcoded, and NOT ordered by
        // `project.id`/insertion order (the backfill's directory scan
        // doesn't guarantee it walks the fixture projects in the same
        // order they were written, so `id` isn't a reliable proxy for
        // chronological order here -- confirmed the hard way when a first
        // draft of this test assumed it was and failed). Claude's
        // directory-name decoding isn't this test's concern, only that
        // `primary_projects_since` returns the REAL display names in the
        // right order.
        //
        // `since_last_visit_summary`/`primary_projects_since` are
        // deliberately NOT provider-scoped (Step 6 doc note), so this test
        // dodges this machine's real incidental provider data (bounded by
        // actual usage, never later than today) with far-future fixture
        // timestamps -- no real session can have a `last_message_time`
        // past 2099.
        let conn = seeded_conn_multi_session(&[
            ("m1", "2099-01-01T00:00:00Z"),
            ("m2", "2099-01-02T00:00:00Z"),
            ("m3", "2099-01-03T00:00:00Z"),
        ])
        .await;

        // Scoped to our own future-dated fixture sessions -- this raw query
        // has no other project/provider filter available, and would
        // otherwise also pick up any real (non-future-dated) Antigravity
        // project's own session on this machine.
        let mut stmt = conn
            .prepare(
                "SELECT p.display_name FROM project p
                 JOIN session s ON s.project_id = p.id
                 WHERE COALESCE(s.last_message_time, s.last_modified) >= '2099-01-01T00:00:00Z'
                 ORDER BY COALESCE(s.last_message_time, s.last_modified) ASC",
            )
            .unwrap();
        let display_names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(display_names.len(), 3, "one project per fixture line");
        let (oldest, middle, newest) = (&display_names[0], &display_names[1], &display_names[2]);

        let summary = since_last_visit_summary(&conn, "2099-01-01T00:00:00Z").unwrap();
        assert_eq!(summary.project_count, 3);
        assert_eq!(
            summary.primary_projects,
            vec![newest.clone(), middle.clone(), oldest.clone()],
            "most recently active project first, all 3 fit within the default limit"
        );

        let limited = primary_projects_since(&conn, "2099-01-01T00:00:00Z", 2).unwrap();
        assert_eq!(
            limited,
            vec![newest.clone(), middle.clone()],
            "limit is respected"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn list_sessions_in_window_orders_most_recent_first() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let claude_base = write_fixture_project(
            dir.path(),
            "-fixture-project-early",
            "session1.jsonl",
            &plain_message_line("m1", "2026-01-01T00:00:00Z"),
        );
        write_fixture_project(
            dir.path(),
            "-fixture-project-late",
            "session1.jsonl",
            &plain_message_line("m2", "2026-01-02T00:00:00Z"),
        );

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let results = list_sessions_in_window(
            &conn,
            "2025-12-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            50,
        )
        .unwrap();
        // Scoped to our own two fixture projects, matched by `session_id`
        // (the raw absolute file path, unlike `project_name` which is
        // DECODED from the Claude directory-name convention and does NOT
        // literally contain the fixture dir name) -- a handful of real
        // Antigravity sessions may incidentally fall in this window on
        // this machine (see archive_db::test_support's own doc comment)
        // and must not make this assertion flaky.
        let ours: Vec<_> = results
            .iter()
            .filter(|r| r.session_id.contains("fixture-project-early") || r.session_id.contains("fixture-project-late"))
            .collect();
        assert_eq!(ours.len(), 2);
        assert!(ours[0].last_message_time.as_deref().unwrap() > ours[1].last_message_time.as_deref().unwrap());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_run_tree_stays_flat_with_no_subagent_transcripts_and_empty_for_no_agent_runs() {
        // `agent_launch_line` fixtures create no `subagents/` directory, so
        // `ingest_subagent_tree` correctly finds
        // nothing to correlate -- this is the "unlinked launch" case, not a
        // blanket statement about ingest in general. See
        // `agent_run_tree_and_detail_read_back_a_real_multi_level_subagent_correlation`
        // below for the genuinely multi-level case.
        let lines = format!(
            "{}\n{}\n",
            agent_launch_line("a1", "2026-01-01T00:00:00Z"),
            agent_launch_line("a2", "2026-01-01T00:01:00Z"),
        );
        let conn = seeded_conn(&lines).await;

        // Scoped to our own fixture session by path -- an unfiltered
        // `LIMIT 1` could otherwise land on a real Antigravity session on
        // this machine (see archive_db::test_support's own doc comment).
        let session_id: String = conn
            .query_row(
                "SELECT file_path FROM session WHERE file_path LIKE '%fixture-project-a%' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let tree = get_agent_run_tree(&conn, &session_id).unwrap();
        assert_eq!(tree.total_count, 2);
        assert_eq!(
            tree.roots.len(),
            2,
            "no subagent transcript exists to correlate against, so parent_agent_run_id \
             stays unset and every run is a root"
        );
        assert!(tree.roots.iter().all(|r| r.children.is_empty()));

        let empty = get_agent_run_tree(&conn, "no-such-session").unwrap();
        assert_eq!(empty.total_count, 0);
        assert!(empty.roots.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_run_detail_loads_a_single_run_by_id() {
        // No `subagents/` directory in this fixture -- see the sibling
        // tree test's comment above for why `subagent_type`/`child_session_id`
        // stay unset here rather than being a general ingest limitation.
        let conn = seeded_conn(&agent_launch_line("a1", "2026-01-01T00:00:00Z")).await;

        let agent_run_id: i64 = conn
            .query_row("SELECT id FROM agent_run LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let detail = get_agent_run_detail(&conn, agent_run_id).unwrap();
        assert_eq!(detail.agent_run_id, agent_run_id);
        assert_eq!(
            detail.subagent_type.as_deref(),
            None,
            "no correlated subagent transcript in this fixture"
        );
        assert_eq!(detail.status.as_deref(), Some("completed"));
        assert_eq!(detail.child_session_id, None);
        assert!(detail.tools_used.is_empty());

        let missing = get_agent_run_detail(&conn, 9_999_999);
        assert!(missing.is_err());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_run_tree_and_detail_read_back_a_real_multi_level_subagent_correlation() {
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let parent_lines = format!("{}\n", agent_launch_line("a1", "2026-01-01T00:00:00Z"));
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &parent_lines);

        // Subagent transcript at `{parent-stem}/subagents/agent-sub1.jsonl`,
        // matching `find_subagent_files`'s native on-disk layout exactly --
        // see `archive_db::ingest::claude`'s own real-fixture test for the
        // convention this mirrors.
        let subagents_dir = std::path::Path::new(&claude_base)
            .join("projects")
            .join("-fixture-project-a")
            .join("session1")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        let sub_lines = [
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_read1","name":"Read","input":{"file_path":"/x/file.rs"}}],"model":"claude-x","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            r#"{"uuid":"s2","parentUuid":"s1","sessionId":"sub1","timestamp":"2026-01-01T00:02:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_read1","content":"contents","is_error":false}]}}"#,
            r#"{"uuid":"s3","parentUuid":"s2","sessionId":"sub1","timestamp":"2026-01-01T00:03:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_bash1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":4,"output_tokens":2}}}"#,
            r#"{"uuid":"s4","parentUuid":"s3","sessionId":"sub1","timestamp":"2026-01-01T00:04:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bash1","content":"FAILED","is_error":true}]}}"#,
        ];
        fs::write(subagents_dir.join("agent-sub1.jsonl"), sub_lines.join("\n") + "\n").unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_a1"}"#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        let session_id: String = conn
            .query_row(
                "SELECT file_path FROM session WHERE file_path LIKE '%fixture-project-a%session1.jsonl'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let tree = get_agent_run_tree(&conn, &session_id).unwrap();
        assert_eq!(tree.total_count, 1);
        assert_eq!(tree.roots.len(), 1);
        let root = &tree.roots[0];
        assert_eq!(root.subagent_type.as_deref(), Some("Explore"));
        assert!(root.children.is_empty(), "sub1 launched no Agent tool_use of its own");
        assert!(root.child_session_id.as_deref().unwrap().contains("sub1"));

        let detail = get_agent_run_detail(&conn, root.agent_run_id).unwrap();
        assert_eq!(detail.status.as_deref(), Some("completed"));
        assert_eq!(detail.model.as_deref(), Some("claude-x"));
        assert_eq!(detail.total_tokens, Some(16));
        assert_eq!(
            detail.tools_used,
            vec![
                AgentRunToolUsage { tool_name: "Bash".to_string(), count: 1 },
                AgentRunToolUsage { tool_name: "Read".to_string(), count: 1 },
            ]
        );
        assert_eq!(detail.files_touched, vec!["/x/file.rs".to_string()]);
        assert_eq!(detail.error_count, 1);
        assert_eq!(detail.errors.len(), 1);
    }

    /// A real subagent session (`is_subagent = 1`) must
    /// never surface in the Home/Insights/History/Stats queries that count
    /// or list top-level sessions -- only in `get_agent_run_tree`/
    /// `get_agent_run_detail`, which are its intended consumers. The
    /// subagent session here is deliberately given MORE tokens, a failing
    /// command, an error, and its own nested (unlinked) `agent_run` than the
    /// parent, so a missing filter would make it outrank or otherwise leak
    /// into every one of these results, not just silently tie.
    #[tokio::test]
    #[serial_test::serial]
    async fn is_subagent_sessions_are_excluded_from_hardened_home_and_insights_queries() {
        // Far-future timestamps throughout (same reasoning as
        // `visit_tracking_round_trips_and_summary_counts_activity_since`):
        // `since_last_visit_summary`/`this_week_summary`/
        // `list_sessions_in_window` are deliberately NOT provider-scoped,
        // and this machine has substantial real cross-provider data (not
        // just the Codex/Antigravity `test_support` already calls out --
        // a first attempt at this test, scoped to real dates with no
        // window filter, actually observed `high_token_sessions` return 10
        // real sessions instead of 1). No real session can have a
        // timestamp past 2099, so window-scoping every call to `[2099-01-
        // 01, 2099-01-03]` dodges all of it, for every function here --
        // including the four that DO take `window_start` but where `None`
        // was tried first and found insufficient.
        let _codex_guard = crate::archive_db::test_support::empty_codex_home_guard();
        let dir = TempDir::new().unwrap();
        let parent_lines = format!("{}\n", agent_launch_line("a1", "2099-01-02T00:00:00Z"));
        let claude_base =
            write_fixture_project(dir.path(), "-fixture-project-a", "session1.jsonl", &parent_lines);

        let subagents_dir = std::path::Path::new(&claude_base)
            .join("projects")
            .join("-fixture-project-a")
            .join("session1")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        let sub_lines = [
            r#"{"uuid":"s1","sessionId":"sub1","timestamp":"2099-01-02T00:01:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_read1","name":"Read","input":{"file_path":"/x/file.rs"}}],"model":"claude-x","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            r#"{"uuid":"s2","parentUuid":"s1","sessionId":"sub1","timestamp":"2099-01-02T00:02:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_read1","content":"contents","is_error":false}]}}"#,
            r#"{"uuid":"s3","parentUuid":"s2","sessionId":"sub1","timestamp":"2099-01-02T00:03:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_bash1","name":"Bash","input":{"command":"pytest -q"}}],"model":"claude-x","usage":{"input_tokens":4,"output_tokens":2}}}"#,
            r#"{"uuid":"s4","parentUuid":"s3","sessionId":"sub1","timestamp":"2099-01-02T00:04:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bash1","content":"FAILED","is_error":true}]}}"#,
            r#"{"uuid":"s5","parentUuid":"s4","sessionId":"sub1","timestamp":"2099-01-02T00:05:00Z","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_nested1","name":"Agent","input":{"subagent_type":"nested"}}],"model":"claude-x","usage":{"input_tokens":20,"output_tokens":10}}}"#,
            r#"{"uuid":"s6","parentUuid":"s5","sessionId":"sub1","timestamp":"2099-01-02T00:06:00Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_nested1","content":"nested done","is_error":false}]}}"#,
        ];
        fs::write(subagents_dir.join("agent-sub1.jsonl"), sub_lines.join("\n") + "\n").unwrap();
        fs::write(
            subagents_dir.join("agent-sub1.meta.json"),
            r#"{"toolUseId":"toolu_a1"}"#,
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        run_full_backfill(&mut conn, &claude_base).await.unwrap();

        // Sanity: sub1 really did land with more tokens, a failing command,
        // an error, and its own agent_run row -- so every assertion below
        // is a real exclusion, not an accidental non-difference.
        let (sub1_tokens, sub1_is_subagent): (i64, i64) = conn
            .query_row(
                "SELECT total_tokens, is_subagent FROM session WHERE file_path LIKE '%sub1%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sub1_is_subagent, 1);
        assert!(sub1_tokens > 15, "sub1 should out-token the parent (got {sub1_tokens})");

        let window_start = Some("2099-01-01T00:00:00Z");
        assert!(
            repeated_command_failures(&conn, window_start, None, None, 1, 10).unwrap().is_empty(),
            "the only failing command lives in the excluded subagent session"
        );
        assert!(
            repeated_errors(&conn, window_start, None, None, 1, 10).unwrap().is_empty(),
            "the only error lives in the excluded subagent session"
        );

        let large_runs = large_agent_runs(&conn, window_start, None, None, 1, 10).unwrap();
        assert_eq!(large_runs.len(), 1, "sub1's own nested agent_run must not count here");
        assert!(large_runs[0].session_id.contains("session1.jsonl"));

        let high_token = high_token_sessions(&conn, window_start, None, None, 10).unwrap();
        assert_eq!(high_token.len(), 1, "sub1 must not leak in despite out-ranking the parent");
        assert!(high_token[0].session_id.contains("session1.jsonl"));
        assert_eq!(high_token[0].total_tokens, 15);

        let since = since_last_visit_summary(&conn, "2099-01-01T00:00:00Z").unwrap();
        assert_eq!(since.session_count, 1);
        assert_eq!(since.tool_call_count, 1, "only the parent's own Agent tool_use");
        assert_eq!(since.agent_run_count, 1, "only the parent's own agent_run row");

        let window = this_week_summary(&conn, "2099-01-01T00:00:00Z", "2099-01-03T00:00:00Z").unwrap();
        assert_eq!(window.session_count, 1);
        assert_eq!(window.tool_call_count, 1);
        assert_eq!(window.agent_run_count, 1);
        assert_eq!(window.peak_agents_in_session, 1);
        assert_eq!(window.provider_breakdown.len(), 1);
        assert_eq!(window.provider_breakdown[0].total_tokens, 15);

        let sessions = list_sessions_in_window(&conn, "2099-01-01T00:00:00Z", "2099-01-03T00:00:00Z", 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].session_id.contains("session1.jsonl"));
    }
}
