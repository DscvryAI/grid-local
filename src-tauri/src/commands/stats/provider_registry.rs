//! Provider identity, path-classification/routing, and the common
//! project/session/message dispatch table used by the stats commands.
//! Split out because the largest backend modules were concentrating too
//! much logic in one file.
//!
//! Extracted first, as the lowest-risk piece of `stats.rs` (~9,300 lines
//! before this split) to pull out on its own, guided by a simple rule:
//! split by domain boundary only after provider contract tests exist to
//! protect behavior during the split -- not before. Everything in this
//! file is a pure function of a `&str`/`&Path`/enum value with no shared
//! mutable aggregation state, already has solid existing test coverage
//! (routing/classification, not computation), and has a narrow,
//! already-`pub(crate)` external surface (`archive_db::backfill`,
//! `archive_db::ingest::provider`, `commands::history`, `commands::
//! history::cache` all depend on `StatsProvider`/`FILE_BASED_STATS_
//! PROVIDERS`/a handful of these functions -- see each one's own doc).
//! The riskier buckets (aggregation math, per-provider dispatch mixed
//! with generic aggregation in `get_provider_project_stats_summary`)
//! stay in `stats.rs` untouched, deliberately deferred until real
//! provider contract tests exist for the ~20 providers that don't have
//! any today.
//!
//! Re-exported via `pub(crate) use provider_registry::*;` at the top of
//! `stats.rs`, so every existing call site in `stats.rs` itself and in
//! its own `mod tests` block keeps working completely unqualified,
//! exactly as before this file existed -- this split changed WHERE this
//! code lives, not its visibility or behavior.

use crate::models::ClaudeMessage;
use crate::providers;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// `pub(crate)` (not private): `commands::history` dispatches through this
// exact provider registry rather than maintaining a second one, so a
// provider added/fixed here can't silently drift out of sync between the
// two features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum StatsProvider {
    #[default]
    Claude,
    Aider,
    AmazonQ,
    Cline,
    Codebuddy,
    Codex,
    Continue,
    ForgeCode,
    OpenCode,
    OpenHands,
    OpenInterpreter,
    PearAI,
    Qwen,
    Trae,
    Vibe,
    Zed,
    Crush,
    CursorAgent,
    Goose,
    Kiro,
    Llm,
    Grok,
    Kimi,
    Antigravity,
    Copilot,
    Ompi,
    Pi,
    Gemini,
    Cursor,
}

/// Return the stable identifier for a stats provider.
pub(crate) fn stats_provider_id(provider: StatsProvider) -> &'static str {
    match provider {
        StatsProvider::Claude => "claude",
        StatsProvider::Aider => "aider",
        StatsProvider::AmazonQ => "amazonq",
        StatsProvider::Cline => "cline",
        StatsProvider::Codebuddy => "codebuddy",
        StatsProvider::Codex => "codex",
        StatsProvider::Continue => "continue",
        StatsProvider::ForgeCode => "forgecode",
        StatsProvider::OpenCode => "opencode",
        StatsProvider::OpenHands => "openhands",
        StatsProvider::OpenInterpreter => "openinterpreter",
        StatsProvider::PearAI => "pearai",
        StatsProvider::Qwen => "qwen",
        StatsProvider::Trae => "trae",
        StatsProvider::Vibe => "vibe",
        StatsProvider::Zed => "zed",
        StatsProvider::Crush => "crush",
        StatsProvider::CursorAgent => "cursor-agent",
        StatsProvider::Goose => "goose",
        StatsProvider::Kiro => "kiro",
        StatsProvider::Llm => "llm",
        StatsProvider::Grok => "grok",
        StatsProvider::Kimi => "kimi",
        StatsProvider::Antigravity => "antigravity",
        StatsProvider::Copilot => "copilot",
        StatsProvider::Ompi => "ompi",
        StatsProvider::Pi => "pi",
        StatsProvider::Gemini => "gemini",
        StatsProvider::Cursor => "cursor",
    }
}

/// The 17 providers whose primary session/message content is genuinely
/// file-based AND whose `session.file_path` is a real, stat-able path
/// (confirmed by reading each one's actual `scan_stats_projects`/
/// `load_stats_sessions`/`load_stats_messages` implementation, not
/// import-grepped) -- the safe v1 scope for universal provider ingestion
/// into `archive_db` (see the plan file referenced in `archive_db::ingest`'s
/// module doc). A `(file_size, file_mtime)` staleness check on a session's
/// own content file works for all 17 (with a documented weaker guarantee
/// for `grok`/`kimi`/`vibe` -- see below); it does NOT for the DB-based
/// (`amazon_q`, `crush`, `cursor`, `forgecode`, `goose`, `kiro`, `llm`,
/// `trae`, `zed`) or hybrid/multi-backend (`opencode`) providers, which are
/// deliberately excluded here and deferred to a separate, later pass.
///
/// `OpenHands` was ORIGINALLY classified file-based (its session content
/// genuinely is files on disk) but is excluded here after direct
/// verification found `ClaudeSession.session_id`/`.file_path` are BOTH the
/// synthetic string `"openhands://<sid>"` (`providers/openhands.rs`'s own
/// session-building code), not a real filesystem path at all -- the real
/// per-session directory (`dir.join(sid)`) is never surfaced on the
/// struct. `stat_signature` would error on every `OpenHands` session with
/// today's orchestrator. Fixing this needs a bespoke staleness key (that
/// provider's own `dir_mtime_rfc3339` helper already exists and could
/// back one), which is real, scoped follow-up work, not something to
/// silently paper over here.
///
/// `Copilot` was ALSO originally excluded as "hybrid/multi-backend" (it
/// aggregates 3 sub-sources -- CLI, Desktop, VS Code Copilot Chat --
/// behind one synthetic `copilot://<base64>` project URL), but direct
/// verification found its SESSION-level staleness is actually fine: every
/// sub-source's `session.file_path` is a real, stat-able file and
/// `session_id == file_path`, matching the default convention every other
/// provider here already uses. The real risk was at the PROJECT-key
/// level -- `providers::copilot::merge_projects` built its `sources` list
/// (baked into the project key `archive_db::ingest::upsert_project` treats
/// as an exact-match idempotency key) from an ordering that wasn't
/// provably stable across separate scans. Fixed by sorting `sources`
/// deterministically before encoding (see that function's own doc
/// comment) -- once fixed, Copilot needed no bespoke ingestion model at
/// all, just the same generic orchestrator every other provider uses.
///
/// `grok`/`kimi`/`vibe` store one session as a DIRECTORY of files, not a
/// single file -- `stat_signature` on a directory still succeeds (so
/// ingestion works), but a directory's mtime only changes when entries
/// are added/removed, not when an existing file inside it is appended
/// to. A session that grows purely via in-place appends to an existing
/// file could be silently missed by the staleness check and not
/// re-ingested on its next real change. Documented, accepted gap for v1
/// (matches this codebase's "flag the limitation, don't silently
/// misrepresent" house style) -- not a correctness bug in what DOES get
/// ingested, just a possible staleness lag for these 3 providers
/// specifically.
///
/// Deliberately an explicit allowlist, not "every variant minus a
/// deny-list" -- a future new `StatsProvider` variant must be reviewed and
/// added here on purpose, not silently swept into ingestion.
///
/// Consumed by `archive_db::backfill::run_full_backfill`/`rebuild_index`.
pub(crate) const FILE_BASED_STATS_PROVIDERS: &[StatsProvider] = &[
    StatsProvider::Aider,
    StatsProvider::Antigravity,
    StatsProvider::Cline,
    StatsProvider::Codebuddy,
    StatsProvider::Codex,
    StatsProvider::Continue,
    StatsProvider::Copilot,
    StatsProvider::CursorAgent,
    StatsProvider::Gemini,
    StatsProvider::Grok,
    StatsProvider::Kimi,
    StatsProvider::Ompi,
    StatsProvider::OpenInterpreter,
    StatsProvider::PearAI,
    StatsProvider::Pi,
    StatsProvider::Qwen,
    StatsProvider::Vibe,
];

/// Return the complete set of providers supported by stats commands.
pub(crate) fn all_stats_providers() -> HashSet<StatsProvider> {
    [
        StatsProvider::Claude,
        StatsProvider::Aider,
        StatsProvider::AmazonQ,
        StatsProvider::Cline,
        StatsProvider::Codebuddy,
        StatsProvider::Codex,
        StatsProvider::Continue,
        StatsProvider::ForgeCode,
        StatsProvider::OpenCode,
        StatsProvider::OpenHands,
        StatsProvider::OpenInterpreter,
        StatsProvider::PearAI,
        StatsProvider::Qwen,
        StatsProvider::Trae,
        StatsProvider::Vibe,
        StatsProvider::Zed,
        StatsProvider::Crush,
        StatsProvider::CursorAgent,
        StatsProvider::Goose,
        StatsProvider::Kiro,
        StatsProvider::Llm,
        StatsProvider::Grok,
        StatsProvider::Kimi,
        StatsProvider::Antigravity,
        StatsProvider::Copilot,
        StatsProvider::Ompi,
        StatsProvider::Pi,
        StatsProvider::Gemini,
        StatsProvider::Cursor,
    ]
    .into_iter()
    .collect()
}

/// Parse the requested provider filter for stats commands.
pub(crate) fn parse_active_stats_providers(
    active_providers: Option<Vec<String>>,
) -> HashSet<StatsProvider> {
    let Some(raw_providers) = active_providers else {
        return all_stats_providers();
    };

    let mut unknown = Vec::new();
    let parsed: HashSet<StatsProvider> = raw_providers
        .into_iter()
        .filter_map(|provider| match provider.as_str() {
            "claude" => Some(StatsProvider::Claude),
            "aider" => Some(StatsProvider::Aider),
            "amazonq" => Some(StatsProvider::AmazonQ),
            "cline" => Some(StatsProvider::Cline),
            "codebuddy" => Some(StatsProvider::Codebuddy),
            "codex" => Some(StatsProvider::Codex),
            "continue" => Some(StatsProvider::Continue),
            "forgecode" => Some(StatsProvider::ForgeCode),
            "opencode" => Some(StatsProvider::OpenCode),
            "openhands" => Some(StatsProvider::OpenHands),
            "openinterpreter" => Some(StatsProvider::OpenInterpreter),
            "pearai" => Some(StatsProvider::PearAI),
            "qwen" => Some(StatsProvider::Qwen),
            "trae" => Some(StatsProvider::Trae),
            "vibe" => Some(StatsProvider::Vibe),
            "zed" => Some(StatsProvider::Zed),
            "crush" => Some(StatsProvider::Crush),
            "cursor-agent" => Some(StatsProvider::CursorAgent),
            "goose" => Some(StatsProvider::Goose),
            "kiro" => Some(StatsProvider::Kiro),
            "llm" => Some(StatsProvider::Llm),
            "grok" => Some(StatsProvider::Grok),
            "kimi" => Some(StatsProvider::Kimi),
            "antigravity" => Some(StatsProvider::Antigravity),
            "copilot" => Some(StatsProvider::Copilot),
            "ompi" => Some(StatsProvider::Ompi),
            "pi" => Some(StatsProvider::Pi),
            "gemini" => Some(StatsProvider::Gemini),
            "cursor" => Some(StatsProvider::Cursor),
            _ => {
                unknown.push(provider);
                None
            }
        })
        .collect();

    if !unknown.is_empty() {
        log::warn!(
            "Ignoring unknown providers in active_providers: {}",
            unknown.join(", ")
        );
    }

    parsed
}

/// Detect the provider encoded in a project path.
pub(crate) fn detect_project_provider(project_path: &str) -> StatsProvider {
    if project_path.starts_with("aider://") {
        StatsProvider::Aider
    } else if project_path.starts_with("amazonq://") {
        StatsProvider::AmazonQ
    } else if project_path.starts_with("cline://") {
        StatsProvider::Cline
    } else if project_path.starts_with("continue://") {
        StatsProvider::Continue
    } else if project_path.starts_with("crush://") {
        StatsProvider::Crush
    } else if project_path.starts_with("goose://") {
        StatsProvider::Goose
    } else if project_path.starts_with("kiro://") {
        StatsProvider::Kiro
    } else if project_path.starts_with("llm://") {
        StatsProvider::Llm
    } else if project_path.starts_with("openhands://") {
        StatsProvider::OpenHands
    } else if project_path.starts_with("openinterpreter://") {
        StatsProvider::OpenInterpreter
    } else if project_path.starts_with("pearai://") {
        StatsProvider::PearAI
    } else if project_path.starts_with("qwen://") {
        StatsProvider::Qwen
    } else if project_path.starts_with("trae://") {
        StatsProvider::Trae
    } else if project_path.starts_with("vibe://") {
        StatsProvider::Vibe
    } else if project_path.starts_with("zed://") {
        StatsProvider::Zed
    } else if project_path.starts_with("codex://") {
        StatsProvider::Codex
    } else if project_path.starts_with("forgecode://") {
        StatsProvider::ForgeCode
    } else if project_path.starts_with("opencode://") {
        StatsProvider::OpenCode
    } else if project_path.starts_with("grok://") {
        StatsProvider::Grok
    } else if project_path.starts_with("kimi://") {
        StatsProvider::Kimi
    } else if project_path.starts_with("gemini://") {
        StatsProvider::Gemini
    } else if project_path.starts_with("cursor://") {
        StatsProvider::Cursor
    } else if is_antigravity_path(project_path) {
        StatsProvider::Antigravity
    } else if is_ompi_path(project_path) {
        StatsProvider::Ompi
    } else if is_pi_path(project_path) {
        StatsProvider::Pi
    } else if is_codebuddy_path(project_path) {
        StatsProvider::Codebuddy
    } else if path_under_root(project_path, providers::cursor_agent::get_base_path()) {
        StatsProvider::CursorAgent
    } else if project_path.starts_with("copilot://")
        || project_path.starts_with("copilot-cli://")
        || project_path.starts_with("copilot-desktop://")
        || project_path.starts_with("vscode://")
    {
        StatsProvider::Copilot
    } else {
        StatsProvider::Claude
    }
}

/// Detect the provider encoded in a session path.
pub(crate) fn detect_session_provider(session_path: &str) -> StatsProvider {
    if session_path.starts_with("aider://") || session_path.ends_with(".aider.chat.history.md") {
        return StatsProvider::Aider;
    }
    if session_path.starts_with("amazonq://") {
        return StatsProvider::AmazonQ;
    }
    if session_path.starts_with("cline://") {
        return StatsProvider::Cline;
    }
    if session_path.starts_with("continue://") {
        return StatsProvider::Continue;
    }
    if session_path.starts_with("crush://") {
        return StatsProvider::Crush;
    }
    if session_path.starts_with("goose://") {
        return StatsProvider::Goose;
    }
    if session_path.starts_with("kiro://") {
        return StatsProvider::Kiro;
    }
    if session_path.starts_with("llm://") {
        return StatsProvider::Llm;
    }
    if session_path.starts_with("openhands://") {
        return StatsProvider::OpenHands;
    }
    if session_path.starts_with("openinterpreter://") {
        return StatsProvider::OpenInterpreter;
    }
    if session_path.starts_with("pearai://") {
        return StatsProvider::PearAI;
    }
    if session_path.starts_with("qwen://") {
        return StatsProvider::Qwen;
    }
    if session_path.starts_with("trae://") {
        return StatsProvider::Trae;
    }
    if session_path.starts_with("vibe://") {
        return StatsProvider::Vibe;
    }
    if session_path.starts_with("zed://") {
        return StatsProvider::Zed;
    }
    if path_under_root(session_path, providers::cursor_agent::get_base_path()) {
        return StatsProvider::CursorAgent;
    }
    if path_under_root(session_path, providers::continue_dev::get_base_path()) {
        return StatsProvider::Continue;
    }
    if path_under_root(session_path, providers::pearai::get_base_path()) {
        return StatsProvider::PearAI;
    }
    if path_under_root(session_path, providers::qwen::get_base_path()) {
        return StatsProvider::Qwen;
    }
    if path_under_root(session_path, providers::vibe::get_base_path()) {
        return StatsProvider::Vibe;
    }
    if path_under_root(session_path, providers::openinterpreter::get_base_path()) {
        return StatsProvider::OpenInterpreter;
    }
    if session_path.starts_with("opencode://") {
        return StatsProvider::OpenCode;
    }

    if session_path.starts_with("cursor://") {
        return StatsProvider::Cursor;
    }

    if is_grok_path(session_path) {
        return StatsProvider::Grok;
    }

    if is_kimi_path(session_path) {
        return StatsProvider::Kimi;
    }

    if is_antigravity_path(session_path) {
        return StatsProvider::Antigravity;
    }

    if is_gemini_path(session_path) {
        return StatsProvider::Gemini;
    }

    if is_ompi_path(session_path) {
        return StatsProvider::Ompi;
    }

    if is_pi_path(session_path) {
        return StatsProvider::Pi;
    }

    if session_path.starts_with("forgecode://") || session_path.starts_with("forgecode-db://") {
        return StatsProvider::ForgeCode;
    }
    if is_copilot_cli_session_path(session_path)
        || session_path.contains("/.copilot/session-state/")
        || session_path.contains("\\.copilot\\session-state\\")
    {
        return StatsProvider::Copilot;
    }
    if (session_path.contains("/workspaceStorage/")
        || session_path.contains("\\workspaceStorage\\"))
        && (session_path.contains("/chatSessions/") || session_path.contains("\\chatSessions\\"))
        && PathBuf::from(session_path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
    {
        return StatsProvider::Copilot;
    }

    // CodeBuddy: path is anchored under ~/.codebuddy/projects (not just substring
    // match, which would misclassify paths like "/work/foo.codebuddy-test").
    if is_codebuddy_path(session_path) {
        return StatsProvider::Codebuddy;
    }

    let is_rollout = PathBuf::from(session_path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("rollout-")
                && std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
        });

    if is_rollout {
        StatsProvider::Codex
    } else {
        StatsProvider::Claude
    }
}

pub(crate) fn path_under_root(path: &str, root: Option<String>) -> bool {
    root.is_some_and(|root| Path::new(path).starts_with(Path::new(&root)))
}

pub(crate) fn is_copilot_cli_session_path(session_path: &str) -> bool {
    if !Path::new(session_path)
        .file_name()
        .is_some_and(|name| name == "events.jsonl")
    {
        return false;
    }
    let Some(base) = providers::copilot_cli::get_base_path() else {
        return false;
    };
    let root = Path::new(&base).join("session-state");
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = Path::new(session_path).canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

pub(crate) fn is_antigravity_path(path: &str) -> bool {
    // Antigravity CLI project paths carry the `antigravity-cli://` scheme
    // (see providers::antigravity_cli), but its session paths are raw
    // absolute directories under the CLI root with no scheme prefix --
    // owns_session_path is the only way to recognize those.
    if path.starts_with(providers::antigravity_cli::SCHEME) {
        return true;
    }
    if providers::antigravity_cli::owns_session_path(path) {
        return true;
    }
    crate::commands::antigravity::resolve_antigravity_root()
        .map(|root| Path::new(path).starts_with(root.as_path()))
        .unwrap_or(false)
}

/// Whether `path` is an Antigravity CLI path (project, scheme-prefixed, or a
/// raw session directory under the CLI root) -- as opposed to the desktop
/// app's own layout, which the two share under one `StatsProvider::Antigravity`
/// variant (see `providers::antigravity_cli`'s module docs for why). Bespoke
/// Antigravity stats logic below is keyed to the desktop app's external
/// usage-log format and does not apply to CLI data, which carries standard
/// `ClaudeMessage`s instead -- callers use this to route CLI paths to the
/// generic, message-based stats path used by every other provider.
pub(crate) fn is_antigravity_cli_path(path: &str) -> bool {
    path.starts_with(providers::antigravity_cli::SCHEME)
        || providers::antigravity_cli::owns_session_path(path)
}

/// Whether `path` lies under `~/.codebuddy/projects/`. Anchored detection avoids
/// false positives from arbitrary substrings (e.g. `/work/foo.codebuddy-test`).
pub(crate) fn is_codebuddy_path(path: &str) -> bool {
    let Some(home) = crate::utils::resolve_home_dir() else {
        return false;
    };
    is_codebuddy_path_under(path, &home)
}

/// Implementation of [`is_codebuddy_path`] parameterized by the home dir,
/// so tests can drive the anchored check with a fixed home and not depend
/// on whether the CI runner has a HOME env at all.
pub(crate) fn is_codebuddy_path_under(path: &str, home: &Path) -> bool {
    let root = home.join(".codebuddy").join("projects");
    Path::new(path).starts_with(root)
}

pub(crate) fn is_kimi_path(path: &str) -> bool {
    providers::kimi::get_base_path()
        .map(|root| Path::new(path).starts_with(root))
        .unwrap_or(false)
}

/// Whether `path` lies under the oh-my-pi sessions store root
/// (`~/.omp/agent/sessions/`). Anchored detection avoids false positives
/// from arbitrary substrings (e.g. `/work/foo.omp-agent-test`).
pub(crate) fn is_ompi_path(path: &str) -> bool {
    let Some(home) = crate::utils::resolve_home_dir() else {
        return false;
    };
    is_ompi_path_under(path, &home)
}

/// Implementation of [`is_ompi_path`] parameterized by the home dir, so
/// tests can drive the anchored check with a fixed home.
pub(crate) fn is_ompi_path_under(path: &str, home: &Path) -> bool {
    let root = home.join(".omp").join("agent").join("sessions");
    Path::new(path).starts_with(root)
}

/// Whether `path` lies under the Pi sessions store root
/// (`~/.pi/agent/sessions/`).
pub(crate) fn is_pi_path(path: &str) -> bool {
    let Some(home) = crate::utils::resolve_home_dir() else {
        return false;
    };
    is_pi_path_under(path, &home)
}

/// Implementation of [`is_pi_path`] parameterized by the home dir, so
/// tests can drive the anchored check with a fixed home.
pub(crate) fn is_pi_path_under(path: &str, home: &Path) -> bool {
    let root = home.join(".pi").join("agent").join("sessions");
    Path::new(path).starts_with(root)
}

/// Whether `path` lies under the Gemini CLI sessions root
/// (`<base>/tmp/` — the directory that holds per-project `chats/` dirs).
/// Anchored on the `tmp` subtree (not the whole `~/.gemini`) so paths under
/// other Gemini sub-trees (e.g. the Antigravity store at `~/.gemini/antigravity`)
/// are not misrouted to the Gemini provider.
pub(crate) fn path_from_current_dir(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(&path))
            .unwrap_or(path)
    }
}

pub(crate) fn is_gemini_path(path: &str) -> bool {
    providers::gemini::get_base_path()
        .map(|root| {
            path_from_current_dir(PathBuf::from(path))
                .starts_with(path_from_current_dir(PathBuf::from(root).join("tmp")))
        })
        .unwrap_or(false)
}

pub(crate) fn is_grok_path(path: &str) -> bool {
    providers::grok::get_base_path()
        .map(|root| Path::new(path).starts_with(root))
        .unwrap_or(false)
}

pub(crate) fn grok_virtual_paths_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left_path = left.strip_prefix("grok://").unwrap_or(left);
    let right_path = right.strip_prefix("grok://").unwrap_or(right);
    match (
        Path::new(left_path).canonicalize(),
        Path::new(right_path).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn cursor_virtual_paths_match(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left_path = left.strip_prefix("cursor://").unwrap_or(left);
    let right_path = right.strip_prefix("cursor://").unwrap_or(right);
    match (
        Path::new(left_path).canonicalize(),
        Path::new(right_path).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Dispatch the common project/session/message interface used by global and
/// project stats. Keeping this table in one place prevents a provider from
/// being visible in the project tree but silently disappearing from stats.
pub(crate) fn scan_stats_projects(
    provider: StatsProvider,
) -> Result<Vec<crate::models::ClaudeProject>, String> {
    match provider {
        StatsProvider::Aider => providers::aider::scan_projects(),
        StatsProvider::AmazonQ => providers::amazon_q::scan_projects(),
        StatsProvider::Cline => providers::cline::scan_projects(),
        StatsProvider::Codebuddy => providers::codebuddy::scan_projects(),
        StatsProvider::Codex => providers::codex::scan_projects(),
        StatsProvider::Continue => providers::continue_dev::scan_projects(),
        StatsProvider::ForgeCode => providers::forgecode::scan_projects(),
        StatsProvider::OpenCode => providers::opencode::scan_projects(),
        StatsProvider::OpenHands => providers::openhands::scan_projects(),
        StatsProvider::OpenInterpreter => providers::openinterpreter::scan_projects(),
        StatsProvider::PearAI => providers::pearai::scan_projects(),
        StatsProvider::Qwen => providers::qwen::scan_projects(),
        StatsProvider::Trae => providers::trae::scan_projects(),
        StatsProvider::Vibe => providers::vibe::scan_projects(),
        StatsProvider::Zed => providers::zed::scan_projects(),
        StatsProvider::Crush => providers::crush::scan_projects(),
        StatsProvider::CursorAgent => providers::cursor_agent::scan_projects(),
        StatsProvider::Goose => providers::goose::scan_projects(),
        StatsProvider::Kiro => providers::kiro::scan_projects(),
        StatsProvider::Llm => providers::llm::scan_projects(),
        StatsProvider::Grok => providers::grok::scan_projects(),
        StatsProvider::Kimi => providers::kimi::scan_projects(),
        StatsProvider::Antigravity => providers::antigravity::scan_projects(),
        StatsProvider::Copilot => providers::copilot::scan_projects(),
        StatsProvider::Ompi => providers::ompi::scan_projects(),
        StatsProvider::Pi => providers::pi::scan_projects(),
        StatsProvider::Gemini => providers::gemini::scan_projects(),
        StatsProvider::Cursor => providers::cursor::scan_projects(),
        StatsProvider::Claude => Ok(Vec::new()),
    }
}

pub(crate) fn load_stats_sessions(
    provider: StatsProvider,
    project_path: &str,
) -> Result<Vec<crate::models::ClaudeSession>, String> {
    match provider {
        StatsProvider::Aider => providers::aider::load_sessions(project_path, false),
        StatsProvider::AmazonQ => providers::amazon_q::load_sessions(project_path, false),
        StatsProvider::Cline => providers::cline::load_sessions(project_path, false),
        StatsProvider::Codebuddy => providers::codebuddy::load_sessions(project_path, false),
        StatsProvider::Codex => providers::codex::load_sessions(project_path, false),
        StatsProvider::Continue => providers::continue_dev::load_sessions(project_path, false),
        StatsProvider::ForgeCode => providers::forgecode::load_sessions(project_path, false),
        StatsProvider::OpenCode => providers::opencode::load_sessions(project_path, false),
        StatsProvider::OpenHands => providers::openhands::load_sessions(project_path, false),
        StatsProvider::OpenInterpreter => {
            providers::openinterpreter::load_sessions(project_path, false)
        }
        StatsProvider::PearAI => providers::pearai::load_sessions(project_path, false),
        StatsProvider::Qwen => providers::qwen::load_sessions(project_path, false),
        StatsProvider::Trae => providers::trae::load_sessions(project_path, false),
        StatsProvider::Vibe => providers::vibe::load_sessions(project_path, false),
        StatsProvider::Zed => providers::zed::load_sessions(project_path, false),
        StatsProvider::Crush => providers::crush::load_sessions(project_path, false),
        StatsProvider::CursorAgent => providers::cursor_agent::load_sessions(project_path, false),
        StatsProvider::Goose => providers::goose::load_sessions(project_path, false),
        StatsProvider::Kiro => providers::kiro::load_sessions(project_path, false),
        StatsProvider::Llm => providers::llm::load_sessions(project_path, false),
        StatsProvider::Grok => providers::grok::load_sessions(project_path, false),
        StatsProvider::Kimi => providers::kimi::load_sessions(project_path, false),
        StatsProvider::Antigravity => providers::antigravity::load_sessions(project_path, false),
        StatsProvider::Copilot => providers::copilot::load_sessions(project_path, false),
        StatsProvider::Ompi => providers::ompi::load_sessions(project_path, false),
        StatsProvider::Pi => providers::pi::load_sessions(project_path, false),
        StatsProvider::Gemini => providers::gemini::load_sessions(project_path, false),
        StatsProvider::Cursor => providers::cursor::load_sessions(project_path, false),
        StatsProvider::Claude => Ok(Vec::new()),
    }
}

pub(crate) fn load_stats_messages(
    provider: StatsProvider,
    session_path: &str,
) -> Result<Vec<ClaudeMessage>, String> {
    match provider {
        StatsProvider::Aider => providers::aider::load_messages(session_path),
        StatsProvider::AmazonQ => providers::amazon_q::load_messages(session_path),
        StatsProvider::Cline => providers::cline::load_messages(session_path),
        StatsProvider::Codebuddy => providers::codebuddy::load_messages(session_path),
        StatsProvider::Codex => providers::codex::load_messages(session_path),
        StatsProvider::Continue => providers::continue_dev::load_messages(session_path),
        StatsProvider::ForgeCode => providers::forgecode::load_messages(session_path),
        StatsProvider::OpenCode => providers::opencode::load_messages(session_path),
        StatsProvider::OpenHands => providers::openhands::load_messages(session_path),
        StatsProvider::OpenInterpreter => providers::openinterpreter::load_messages(session_path),
        StatsProvider::PearAI => providers::pearai::load_messages(session_path),
        StatsProvider::Qwen => providers::qwen::load_messages(session_path),
        StatsProvider::Trae => providers::trae::load_messages(session_path),
        StatsProvider::Vibe => providers::vibe::load_messages(session_path),
        StatsProvider::Zed => providers::zed::load_messages(session_path),
        StatsProvider::Crush => providers::crush::load_messages(session_path),
        StatsProvider::CursorAgent => providers::cursor_agent::load_messages(session_path),
        StatsProvider::Goose => providers::goose::load_messages(session_path),
        StatsProvider::Kiro => providers::kiro::load_messages(session_path),
        StatsProvider::Llm => providers::llm::load_messages(session_path),
        StatsProvider::Grok => providers::grok::load_messages(session_path),
        StatsProvider::Kimi => providers::kimi::load_messages(session_path),
        StatsProvider::Antigravity => providers::antigravity::load_messages(session_path),
        StatsProvider::Copilot => providers::copilot::load_messages(session_path),
        StatsProvider::Ompi => providers::ompi::load_messages(session_path),
        StatsProvider::Pi => providers::pi::load_messages(session_path),
        StatsProvider::Gemini => providers::gemini::load_messages(session_path),
        StatsProvider::Cursor => providers::cursor::load_stats_messages(session_path),
        StatsProvider::Claude => Ok(Vec::new()),
    }
}
