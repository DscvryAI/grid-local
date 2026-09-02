//! DDL for Grid's own normalized archive database (spec §20-21).
//!
//! Each entry in [`MIGRATIONS`] is applied exactly once, in order, by
//! [`crate::archive_db::migrate::migrate`]. Append new entries for future
//! schema changes -- never edit an entry that has already shipped.

/// Migration 1: initial schema. Twelve entities per spec §21, plus
/// incremental-ingest bookkeeping columns on `session` (mirrors the
/// `(size, mtime, last_byte_offset)` pattern already used by the
/// pre-existing `.session_cache.json` cache).
///
/// `source_record` is one row per ingested JSONL *line* (matching the
/// line-boundary computation already done in
/// `commands::session::load::find_line_starts`/`find_line_ranges`), not
/// one row per derived fact -- every other derived row traces back to
/// evidence transitively via `message_id -> message.source_record_id`.
const MIGRATION_0001_INITIAL_SCHEMA: &str = r"
CREATE TABLE provider (
  id INTEGER PRIMARY KEY,
  provider_key TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  tier TEXT NOT NULL CHECK (tier IN ('A','B','C')),
  parser_version INTEGER NOT NULL,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL
);

CREATE TABLE project (
  id INTEGER PRIMARY KEY,
  provider_id INTEGER NOT NULL REFERENCES provider(id),
  project_key TEXT NOT NULL,
  display_name TEXT NOT NULL,
  actual_path TEXT,
  git_worktree_type TEXT,
  git_main_project_path TEXT,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  UNIQUE(provider_id, project_key)
);
CREATE INDEX idx_project_provider ON project(provider_id);

CREATE TABLE session (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id),
  session_key TEXT NOT NULL,
  actual_session_id TEXT,
  file_path TEXT NOT NULL,
  first_message_time TEXT,
  last_message_time TEXT,
  last_modified TEXT,
  message_count INTEGER NOT NULL DEFAULT 0,
  has_tool_use INTEGER NOT NULL DEFAULT 0,
  has_errors INTEGER NOT NULL DEFAULT 0,
  summary TEXT,
  entrypoint TEXT,
  total_input_tokens INTEGER NOT NULL DEFAULT 0,
  total_output_tokens INTEGER NOT NULL DEFAULT 0,
  total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  total_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  duration_minutes INTEGER,
  file_size INTEGER NOT NULL,
  file_mtime INTEGER NOT NULL,
  last_byte_offset INTEGER NOT NULL DEFAULT 0,
  parser_version INTEGER NOT NULL,
  last_ingested_at TEXT NOT NULL,
  UNIQUE(project_id, session_key)
);
CREATE INDEX idx_session_project ON session(project_id);
CREATE INDEX idx_session_first_message_time ON session(first_message_time);
CREATE INDEX idx_session_total_tokens ON session(total_tokens DESC);
CREATE INDEX idx_session_file_path ON session(file_path);

CREATE TABLE message (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id),
  uuid TEXT NOT NULL,
  parent_uuid TEXT,
  role TEXT,
  message_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  is_sidechain INTEGER NOT NULL DEFAULT 0,
  model TEXT,
  stop_reason TEXT,
  cost_usd REAL,
  duration_ms INTEGER,
  content_text TEXT,
  source_record_id INTEGER REFERENCES source_record(id),
  UNIQUE(session_id, uuid)
);
CREATE INDEX idx_message_session ON message(session_id);
CREATE INDEX idx_message_timestamp ON message(timestamp);

CREATE TABLE usage (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES message(id),
  session_id INTEGER NOT NULL REFERENCES session(id),
  model TEXT,
  service_tier TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens_5m INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens_1h INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL
);
CREATE INDEX idx_usage_session ON usage(session_id);
CREATE INDEX idx_usage_model ON usage(model);

CREATE TABLE tool_call (
  id INTEGER PRIMARY KEY,
  message_id INTEGER NOT NULL REFERENCES message(id),
  session_id INTEGER NOT NULL REFERENCES session(id),
  tool_use_id TEXT,
  tool_name TEXT NOT NULL,
  input_json TEXT,
  parent_tool_use_id TEXT,
  is_subagent_task INTEGER NOT NULL DEFAULT 0,
  subagent_type TEXT,
  skill_name TEXT
);
CREATE INDEX idx_toolcall_session ON tool_call(session_id);
CREATE INDEX idx_toolcall_name ON tool_call(tool_name);
CREATE INDEX idx_toolcall_message ON tool_call(message_id);

CREATE TABLE command (
  id INTEGER PRIMARY KEY,
  tool_call_id INTEGER NOT NULL REFERENCES tool_call(id),
  session_id INTEGER NOT NULL REFERENCES session(id),
  shell_command TEXT NOT NULL,
  description TEXT,
  timeout_ms INTEGER
);
CREATE INDEX idx_command_session ON command(session_id);
CREATE INDEX idx_command_text ON command(shell_command);

CREATE TABLE tool_result (
  id INTEGER PRIMARY KEY,
  tool_call_id INTEGER NOT NULL REFERENCES tool_call(id),
  session_id INTEGER NOT NULL REFERENCES session(id),
  is_error INTEGER NOT NULL DEFAULT 0,
  result_summary TEXT,
  duration_ms INTEGER
);
CREATE INDEX idx_toolresult_session ON tool_result(session_id);
CREATE INDEX idx_toolresult_error ON tool_result(session_id, is_error) WHERE is_error = 1;

CREATE TABLE file_event (
  id INTEGER PRIMARY KEY,
  tool_call_id INTEGER NOT NULL REFERENCES tool_call(id),
  session_id INTEGER NOT NULL REFERENCES session(id),
  event_type TEXT NOT NULL,
  file_path TEXT NOT NULL
);
CREATE INDEX idx_fileevent_session ON file_event(session_id);
CREATE INDEX idx_fileevent_path ON file_event(file_path);

CREATE TABLE agent_run (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id),
  parent_tool_call_id INTEGER REFERENCES tool_call(id),
  parent_agent_run_id INTEGER REFERENCES agent_run(id),
  child_session_id INTEGER REFERENCES session(id),
  subagent_type TEXT,
  started_at TEXT,
  ended_at TEXT,
  tool_call_count INTEGER NOT NULL DEFAULT 0,
  status TEXT
);
CREATE INDEX idx_agentrun_session ON agent_run(session_id);
CREATE INDEX idx_agentrun_duration ON agent_run(started_at, ended_at);

CREATE TABLE error (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id),
  tool_result_id INTEGER REFERENCES tool_result(id),
  message_id INTEGER REFERENCES message(id),
  occurred_at TEXT,
  error_signature TEXT NOT NULL,
  raw_text TEXT
);
CREATE INDEX idx_error_session ON error(session_id);
CREATE INDEX idx_error_signature ON error(error_signature);

CREATE TABLE source_record (
  id INTEGER PRIMARY KEY,
  provider_id INTEGER NOT NULL REFERENCES provider(id),
  file_path TEXT NOT NULL,
  byte_offset_start INTEGER NOT NULL,
  byte_offset_end INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  ingested_at TEXT NOT NULL,
  parser_version INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_sourcerecord_path_offset ON source_record(file_path, byte_offset_start);
";

/// Ordered migrations, applied starting from `PRAGMA user_version + 1`.
/// Migration 2: adds `message.message_id` -- the Anthropic API's per-turn
/// message id (`RawLogEntry.message.id`, distinct from `message.uuid`,
/// which is per-*log-line*). Needed to replicate `commands::stats`'
/// existing dedup-by-message-id logic (a single conversational turn can
/// span multiple JSONL lines that all repeat the same cumulative
/// usage/cost for that turn) since `get_global_stats_summary`'s Claude
/// portion reads from this archive instead of re-parsing raw files.
const MIGRATION_0002_MESSAGE_ID: &str = "ALTER TABLE message ADD COLUMN message_id TEXT;";

/// Migration 3: adds `session.dominant_model` (the most-frequent model
/// across a session's messages, denormalized at ingest time -- same
/// reasoning as `message_count`/`has_tool_use`/`total_tokens` already
/// being denormalized onto `session`: a live per-query join over `message`
/// would scale with total message count across all history on every
/// History-surface list call, the wrong cost curve for a browsing list)
/// and an index on `last_message_time` (History's own sort/bucket key --
/// only `first_message_time` had an index before this). Both additive;
/// `dominant_model` backfills lazily as each session is next re-ingested
/// (see `ingest::claude::CLAUDE_PARSER_VERSION`'s 2->3 bump).
const MIGRATION_0003_HISTORY_SESSION_MODEL: &str = r"
ALTER TABLE session ADD COLUMN dominant_model TEXT;
CREATE INDEX idx_session_last_message_time ON session(last_message_time);
";

/// Migration 4: a tiny generic key/value table to hold Grid's own
/// bookkeeping that belongs in the archive, not the frontend's
/// `user-data.json` -- starting with `last_visit_at` (the "Since you were
/// last here" summary), whose only consumer is
/// `archive_db::insights::since_last_visit_summary`. Generic `app_state`
/// rather than a dedicated single-purpose table/column so a future
/// similar need doesn't require yet another migration.
const MIGRATION_0004_APP_STATE: &str = "
CREATE TABLE app_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
";

/// Migration 5: adds `session.is_subagent`. Subagent transcript files
/// (previously never ingested at
/// all -- only top-level, user-facing sessions were) now get their own
/// `session` row, needed so `agent_run.child_session_id` (a real FK to
/// `session`, present since migration 1 but never populated) has something
/// valid to point at and multi-level agent-run trees can reuse this
/// table's own per-session token/tool/error rollups instead of
/// reimplementing them. Every existing user-facing query that lists or
/// counts sessions must add `is_subagent = 0` to stay correct -- see
/// `archive_db::ingest::claude`'s subagent-ingest doc comment for the full
/// list of query sites this was audited against.
const MIGRATION_0005_SUBAGENT_SESSIONS: &str =
    "ALTER TABLE session ADD COLUMN is_subagent INTEGER NOT NULL DEFAULT 0;";

/// Migration 6: a single unified FTS5 virtual table backing search-on-FTS
/// global search, replacing the raw-file-walk
/// scan (`commands::session::search::search_messages`) as the PRIMARY
/// search path -- that raw scan stays only as a fallback for whatever it
/// covers that `archive_db` doesn't yet.
///
/// One table with a `kind` column (rather than one table per facet) so a
/// single `MATCH` query ranks across every facet at once -- the
/// "command/error/file/tool-output/agent-instruction" facets map
/// directly onto existing normalized columns:
/// - `message`    -- `message.content_text` (already `flatten_text_
///   preview`-truncated at ingest, same recall ceiling the raw-scan path
///   already has -- not a new limitation).
/// - `command`    -- `command.shell_command`.
/// - `tool_result`-- `tool_result.result_summary` (same truncation note).
/// - `file`       -- `file_event.file_path`.
/// - `error`      -- `error.raw_text`, falling back to `error_signature`.
/// - `agent_instruction` -- an `Agent` `tool_call`'s own launch input,
///   preferring its `prompt` field then `description` (mirrors
///   `archive_db::insights::extract_agent_launch_purpose`'s own
///   precedence, done here in SQL via `json_extract` since a trigger body
///   can't call a Rust function), falling back to the raw JSON so a
///   launch with neither field is still findable rather than silently
///   unindexed.
///
/// `session_id`/`ref_id`/`occurred_at` are `UNINDEXED` (metadata carried
/// alongside the match, not tokenized) -- `ref_id` is the source row's own
/// `id` in its facet's table, letting a result trace back to exactly the
/// row it came from (needed since multiple facets can share the same
/// `session_id`).
///
/// Kept in sync automatically by one `AFTER INSERT`/`AFTER DELETE` trigger
/// pair per source table -- deliberately NOT wired into each ingest
/// function in Rust: every existing and future insert path (Claude,
/// every file-based provider, any later provider) already writes through
/// these same 6 tables, and `archive_db::ingest::delete_session_rows`
/// already deletes from every one of them on re-ingest/removal (see its
/// own `tables_referencing_session` list) -- the `AFTER DELETE` triggers
/// ride along on that existing delete path for free, no Rust change
/// needed there either. The initial `INSERT ... SELECT` backfills every
/// row that existed before this migration ran (the triggers only cover
/// rows inserted AFTER it).
const MIGRATION_0006_SEARCH_FTS: &str = r"
CREATE VIRTUAL TABLE search_fts USING fts5(
  kind UNINDEXED,
  body,
  session_id UNINDEXED,
  ref_id UNINDEXED,
  occurred_at UNINDEXED
);

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'message', content_text, session_id, id, timestamp FROM message
WHERE content_text IS NOT NULL AND content_text != '';

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'command', shell_command, session_id, id, NULL FROM command
WHERE shell_command IS NOT NULL AND shell_command != '';

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'tool_result', result_summary, session_id, id, NULL FROM tool_result
WHERE result_summary IS NOT NULL AND result_summary != '';

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'error', COALESCE(raw_text, error_signature), session_id, id, occurred_at FROM error
WHERE error_signature IS NOT NULL;

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'file', file_path, session_id, id, NULL FROM file_event
WHERE file_path IS NOT NULL AND file_path != '';

INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
SELECT 'agent_instruction',
       COALESCE(json_extract(input_json, '$.prompt'), json_extract(input_json, '$.description'), input_json),
       session_id, id, NULL
FROM tool_call
WHERE tool_name = 'Agent' AND input_json IS NOT NULL AND input_json != '';

CREATE TRIGGER search_fts_message_ai AFTER INSERT ON message BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'message', NEW.content_text, NEW.session_id, NEW.id, NEW.timestamp
  WHERE NEW.content_text IS NOT NULL AND NEW.content_text != '';
END;
CREATE TRIGGER search_fts_message_ad AFTER DELETE ON message BEGIN
  DELETE FROM search_fts WHERE kind = 'message' AND ref_id = OLD.id;
END;

CREATE TRIGGER search_fts_command_ai AFTER INSERT ON command BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'command', NEW.shell_command, NEW.session_id, NEW.id, NULL
  WHERE NEW.shell_command IS NOT NULL AND NEW.shell_command != '';
END;
CREATE TRIGGER search_fts_command_ad AFTER DELETE ON command BEGIN
  DELETE FROM search_fts WHERE kind = 'command' AND ref_id = OLD.id;
END;

CREATE TRIGGER search_fts_tool_result_ai AFTER INSERT ON tool_result BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'tool_result', NEW.result_summary, NEW.session_id, NEW.id, NULL
  WHERE NEW.result_summary IS NOT NULL AND NEW.result_summary != '';
END;
CREATE TRIGGER search_fts_tool_result_ad AFTER DELETE ON tool_result BEGIN
  DELETE FROM search_fts WHERE kind = 'tool_result' AND ref_id = OLD.id;
END;

CREATE TRIGGER search_fts_error_ai AFTER INSERT ON error BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'error', COALESCE(NEW.raw_text, NEW.error_signature), NEW.session_id, NEW.id, NEW.occurred_at
  WHERE NEW.error_signature IS NOT NULL;
END;
CREATE TRIGGER search_fts_error_ad AFTER DELETE ON error BEGIN
  DELETE FROM search_fts WHERE kind = 'error' AND ref_id = OLD.id;
END;

CREATE TRIGGER search_fts_file_event_ai AFTER INSERT ON file_event BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'file', NEW.file_path, NEW.session_id, NEW.id, NULL
  WHERE NEW.file_path IS NOT NULL AND NEW.file_path != '';
END;
CREATE TRIGGER search_fts_file_event_ad AFTER DELETE ON file_event BEGIN
  DELETE FROM search_fts WHERE kind = 'file' AND ref_id = OLD.id;
END;

CREATE TRIGGER search_fts_agent_instruction_ai AFTER INSERT ON tool_call WHEN NEW.tool_name = 'Agent' BEGIN
  INSERT INTO search_fts(kind, body, session_id, ref_id, occurred_at)
  SELECT 'agent_instruction',
         COALESCE(json_extract(NEW.input_json, '$.prompt'), json_extract(NEW.input_json, '$.description'), NEW.input_json),
         NEW.session_id, NEW.id, NULL
  WHERE NEW.input_json IS NOT NULL AND NEW.input_json != '';
END;
CREATE TRIGGER search_fts_agent_instruction_ad AFTER DELETE ON tool_call WHEN OLD.tool_name = 'Agent' BEGIN
  DELETE FROM search_fts WHERE kind = 'agent_instruction' AND ref_id = OLD.id;
END;
";

/// Migration 7: local dismiss/resolve state for "things worth looking
/// at" cards -- a user acting on a repeated
/// command failure or repeated error (fixed it, or doesn't care) can
/// dismiss that card so it stops resurfacing. `(kind, signature)` keys
/// scope the two card kinds' signature spaces independently (a command's
/// normalized `template` and an error's `error_signature` could
/// otherwise collide as bare strings). Deliberately local-only, no sync
/// -- this is per-machine housekeeping state, not data worth backing up.
const MIGRATION_0007_DISMISSED_PROBLEMS: &str = "
CREATE TABLE dismissed_problem (
  kind TEXT NOT NULL,
  signature TEXT NOT NULL,
  dismissed_at TEXT NOT NULL,
  PRIMARY KEY (kind, signature)
);
";

/// Index 0 corresponds to `user_version` 1, etc. -- see
/// [`crate::archive_db::migrate::migrate`].
pub const MIGRATIONS: &[&str] = &[
    MIGRATION_0001_INITIAL_SCHEMA,
    MIGRATION_0002_MESSAGE_ID,
    MIGRATION_0003_HISTORY_SESSION_MODEL,
    MIGRATION_0004_APP_STATE,
    MIGRATION_0005_SUBAGENT_SESSIONS,
    MIGRATION_0006_SEARCH_FTS,
    MIGRATION_0007_DISMISSED_PROBLEMS,
];

/// Every table name this schema defines, in creation order -- used by
/// tests to assert the full schema landed, and (later) by `backfill` to
/// truncate rows for a specific provider on a full rebuild.
pub const TABLE_NAMES: &[&str] = &[
    "provider",
    "project",
    "session",
    "message",
    "usage",
    "tool_call",
    "command",
    "tool_result",
    "file_event",
    "agent_run",
    "error",
    "source_record",
    "app_state",
    "search_fts",
    "dismissed_problem",
];
