//! Tauri commands for Claude Code settings management
//!
//! This module provides read-only commands for inspecting Claude Code settings
//! across different scopes (user, project, local, managed) and MCP server
//! configurations. Grid Local never writes into a provider's own config files
//! (read-only guarantee, spec §22/§46) — this module intentionally has no
//! save/write commands.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// All settings scopes in a single structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllSettings {
    pub user: Option<String>,
    pub project: Option<String>,
    pub local: Option<String>,
    pub managed: Option<String>,
}

/// MCP servers from both settings.json and .mcp.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServers {
    pub servers: serde_json::Value,
}

/// All MCP servers across all scopes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllMCPServers {
    /// User-level from settings.json mcpServers (legacy)
    pub user_settings: Option<serde_json::Value>,
    /// User-level from ~/.claude/.mcp.json (legacy)
    pub user_mcp_file: Option<serde_json::Value>,
    /// Project-level from .mcp.json (in project root)
    pub project_mcp_file: Option<serde_json::Value>,
    /// User-scoped MCP from ~/.claude.json → mcpServers (official)
    pub user_claude_json: Option<serde_json::Value>,
    /// Local/Project-scoped MCP from `~/.claude.json` → `projects.<path>.mcpServers` (official)
    pub local_claude_json: Option<serde_json::Value>,
}

/// Get the user settings path (~/.claude/settings.json)
fn get_user_settings_path() -> Result<PathBuf, String> {
    let home = crate::utils::resolve_home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".claude").join("settings.json"))
}

/// Get the user MCP settings path (~/.claude/.mcp.json)
fn get_user_mcp_path() -> Result<PathBuf, String> {
    let home = crate::utils::resolve_home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".claude").join(".mcp.json"))
}

/// Get the main Claude config path (~/.claude.json) - the official config file
fn get_claude_json_path() -> Result<PathBuf, String> {
    let home = crate::utils::resolve_home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(".claude.json"))
}

/// Validate project path to prevent path traversal attacks
///
/// # Security
/// - Ensures path is absolute
/// - Prevents ".." path traversal components
/// - Canonicalizes existing paths
///
/// # Arguments
/// * `path` - Project path to validate
///
/// # Returns
/// Validated `PathBuf` or error message
fn validate_project_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);

    if !path.is_absolute() {
        return Err("Project path must be absolute".to_string());
    }

    // Check for path traversal
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Project path cannot contain '..' components".to_string());
    }

    // Canonicalize if exists, otherwise return as-is
    if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Failed to canonicalize path: {e}"))
    } else {
        Ok(path)
    }
}

/// Get the project MCP settings path (`<project>/.mcp.json`)
fn get_project_mcp_path(project_path: &str) -> Result<PathBuf, String> {
    let validated = validate_project_path(project_path)?;
    Ok(validated.join(".mcp.json"))
}

/// Get the managed settings path (macOS only)
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
fn get_managed_settings_path() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        "/Library/Application Support/ClaudeCode/managed-settings.json",
    ))
}

#[cfg(not(target_os = "macos"))]
fn get_managed_settings_path() -> Result<PathBuf, String> {
    Err("Managed settings are only available on macOS".to_string())
}

/// Get settings path for a specific scope
fn get_settings_path(scope: &str, project_path: Option<&str>) -> Result<PathBuf, String> {
    match scope {
        "user" => get_user_settings_path(),
        "project" => {
            let path = project_path.ok_or("project_path required for 'project' scope")?;
            let validated = validate_project_path(path)?;
            Ok(validated.join(".claude").join("settings.json"))
        }
        "local" => {
            let path = project_path.ok_or("project_path required for 'local' scope")?;
            let validated = validate_project_path(path)?;
            Ok(validated.join(".claude").join("settings.local.json"))
        }
        "managed" => get_managed_settings_path(),
        _ => Err(format!("Invalid scope: {scope}")),
    }
}

/// Read a settings file, returns JSON string or empty object if not exists
fn read_settings_file(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok("{}".to_string());
    }

    fs::read_to_string(path).map_err(|e| format!("Failed to read settings file: {e}"))
}

/// Get settings for a specific scope
///
/// # Arguments
/// * `scope` - One of: "user", "project", "local", "managed"
/// * `project_path` - Required for "project" and "local" scopes (must be absolute path)
///
/// # Returns
/// JSON string of settings, or empty object "{}" if file doesn't exist
#[tauri::command]
pub async fn get_settings_by_scope(
    scope: String,
    project_path: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = get_settings_path(&scope, project_path.as_deref())?;
        read_settings_file(&path)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Get all settings scopes at once
///
/// # Arguments
/// * `project_path` - Optional project path for project/local settings (must be absolute)
///
/// # Returns
/// `AllSettings` struct with all 4 scopes (each is `Option<String>`)
#[tauri::command]
pub async fn get_all_settings(project_path: Option<String>) -> Result<AllSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let user = get_user_settings_path()
            .ok()
            .and_then(|p| read_settings_file(&p).ok());

        let project = project_path
            .as_deref()
            .and_then(|pp| get_settings_path("project", Some(pp)).ok())
            .and_then(|p| read_settings_file(&p).ok());

        let local = project_path
            .as_deref()
            .and_then(|pp| get_settings_path("local", Some(pp)).ok())
            .and_then(|p| read_settings_file(&p).ok());

        let managed = get_managed_settings_path()
            .ok()
            .and_then(|p| read_settings_file(&p).ok());

        Ok(AllSettings {
            user,
            project,
            local,
            managed,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Get MCP servers from both settings.json (mcpServers field) and .mcp.json
///
/// # Returns
/// `MCPServers` struct with merged servers from both sources
#[tauri::command]
pub async fn get_mcp_servers() -> Result<MCPServers, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut merged = serde_json::Map::new();

        // Read from ~/.claude/settings.json (mcpServers field)
        if let Ok(user_path) = get_user_settings_path() {
            if let Ok(content) = read_settings_file(&user_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(mcp_servers) = json.get("mcpServers") {
                        if let Some(obj) = mcp_servers.as_object() {
                            merged.extend(obj.clone());
                        }
                    }
                }
            }
        }

        // Read from ~/.claude/.mcp.json
        if let Ok(mcp_path) = get_user_mcp_path() {
            if let Ok(content) = read_settings_file(&mcp_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    // Check if it has mcpServers key or is the servers object directly
                    if let Some(mcp_servers) = json.get("mcpServers") {
                        if let Some(obj) = mcp_servers.as_object() {
                            merged.extend(obj.clone());
                        }
                    } else if let Some(obj) = json.as_object() {
                        // .mcp.json might be servers directly without mcpServers wrapper
                        merged.extend(obj.clone());
                    }
                }
            }
        }

        Ok(MCPServers {
            servers: serde_json::Value::Object(merged),
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Get all MCP servers from all sources (user settings, user .mcp.json, project .mcp.json, ~/.claude.json)
///
/// # Arguments
/// * `project_path` - Optional project path for project-level .mcp.json and local scope in ~/.claude.json
///
/// # Returns
/// `AllMCPServers` struct with servers from each source separately
#[tauri::command]
pub async fn get_all_mcp_servers(project_path: Option<String>) -> Result<AllMCPServers, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // User settings.json mcpServers (legacy)
        let user_settings = get_user_settings_path().ok().and_then(|p| {
            read_settings_file(&p).ok().and_then(|content| {
                serde_json::from_str::<serde_json::Value>(&content)
                    .ok()
                    .and_then(|json| json.get("mcpServers").cloned())
            })
        });

        // User .mcp.json (legacy)
        let user_mcp_file = get_user_mcp_path().ok().and_then(|p| {
            if !p.exists() {
                return None;
            }
            read_settings_file(&p).ok().and_then(|content| {
                serde_json::from_str::<serde_json::Value>(&content)
                    .ok()
                    .map(|json| {
                        // Check if it has mcpServers key or is servers directly
                        if let Some(servers) = json.get("mcpServers") {
                            servers.clone()
                        } else {
                            json
                        }
                    })
            })
        });

        // Project .mcp.json
        let project_mcp_file = project_path.as_deref().and_then(|pp| {
            let p = get_project_mcp_path(pp).ok()?;
            if !p.exists() {
                return None;
            }
            read_settings_file(&p).ok().and_then(|content| {
                serde_json::from_str::<serde_json::Value>(&content)
                    .ok()
                    .map(|json| {
                        // Check if it has mcpServers key or is servers directly
                        if let Some(servers) = json.get("mcpServers") {
                            servers.clone()
                        } else {
                            json
                        }
                    })
            })
        });

        // Read ~/.claude.json (official config file)
        let claude_json = get_claude_json_path().ok().and_then(|p| {
            if !p.exists() {
                return None;
            }
            read_settings_file(&p)
                .ok()
                .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        });

        // User-scoped MCP from ~/.claude.json → mcpServers
        let user_claude_json = claude_json
            .as_ref()
            .and_then(|json| json.get("mcpServers").cloned());

        // Local/Project-scoped MCP from ~/.claude.json → projects.<path>.mcpServers
        let local_claude_json = project_path.as_deref().and_then(|pp| {
            claude_json.as_ref().and_then(|json| {
                json.get("projects")
                    .and_then(|projects| projects.get(pp))
                    .and_then(|project| project.get("mcpServers").cloned())
            })
        });

        Ok(AllMCPServers {
            user_settings,
            user_mcp_file,
            project_mcp_file,
            user_claude_json,
            local_claude_json,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Claude.json configuration structure for reading
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeJsonConfig {
    /// Full raw JSON content
    pub raw: serde_json::Value,
    /// User-scoped MCP servers
    pub mcp_servers: Option<serde_json::Value>,
    /// Project settings from `projects.<path>`
    pub project_settings: Option<serde_json::Value>,
    /// File path for reference
    pub file_path: String,
}

/// Get the full ~/.claude.json configuration
///
/// # Arguments
/// * `project_path` - Optional project path to extract project-specific settings
///
/// # Returns
/// `ClaudeJsonConfig` with raw JSON and extracted fields
#[tauri::command]
pub async fn get_claude_json_config(
    project_path: Option<String>,
) -> Result<ClaudeJsonConfig, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = get_claude_json_path()?;
        let file_path = path.to_string_lossy().to_string();

        if !path.exists() {
            return Ok(ClaudeJsonConfig {
                raw: serde_json::json!({}),
                mcp_servers: None,
                project_settings: None,
                file_path,
            });
        }

        let content = read_settings_file(&path)?;
        let raw: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse claude.json: {e}"))?;

        let mcp_servers = raw.get("mcpServers").cloned();

        let project_settings = project_path.and_then(|pp| {
            raw.get("projects")
                .and_then(|projects| projects.get(&pp).cloned())
        });

        Ok(ClaudeJsonConfig {
            raw,
            mcp_servers,
            project_settings,
            file_path,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Every provider module exposing a simple `fn() -> Option<String>` base-
/// path getter -- this enumeration centralizes all provider roots into
/// one registry. `cursor`/`vscode` are handled separately in
/// [`restricted_write_prefixes`] since they return `PathBuf`/`Vec<PathBuf>`
/// instead. **Still not exhaustive**: `aider`/`antigravity`/`cline`
/// resolve multiple base directories via private, provider-specific
/// helpers rather than one public getter, and `kiro` is a SQLite-backed
/// provider with no filesystem base-path concept at all -- these 4 remain
/// a real, disclosed gap, not silently claimed as covered.
const STRING_BASE_PATH_PROVIDERS: &[fn() -> Option<String>] = &[
    crate::providers::amazon_q::get_base_path,
    crate::providers::claude::get_base_path,
    crate::providers::codebuddy::get_base_path,
    crate::providers::codex::get_base_path,
    crate::providers::continue_dev::get_base_path,
    crate::providers::copilot_cli::get_base_path,
    crate::providers::crush::get_base_path,
    crate::providers::cursor_agent::get_base_path,
    crate::providers::forgecode::get_base_path,
    crate::providers::gemini::get_base_path,
    crate::providers::goose::get_base_path,
    crate::providers::grok::get_base_path,
    crate::providers::kimi::get_base_path,
    crate::providers::llm::get_base_path,
    crate::providers::ompi::get_base_path,
    crate::providers::opencode::get_base_path,
    crate::providers::openhands::get_base_path,
    crate::providers::openinterpreter::get_base_path,
    crate::providers::pearai::get_base_path,
    crate::providers::pi::get_base_path,
    crate::providers::qwen::get_base_path,
    crate::providers::trae::get_base_path,
    crate::providers::vibe::get_base_path,
    crate::providers::zed::get_base_path,
];

/// Directories a dialog-chosen write must never target, regardless of what
/// path a (possibly compromised or buggy) frontend caller supplies:
/// `write_text_file`/`save_screenshot` accept an absolute path from the
/// frontend with no proof it actually came from the native save dialog
/// the UI shows, only basic shape validation. Recreates the exact
/// prefix-list pattern the (now-removed) `restore_file` command's own
/// `restricted_restore_prefixes` used, and covers 26 provider roots
/// (every provider in [`STRING_BASE_PATH_PROVIDERS`] plus
/// `cursor`/`vscode`'s own `PathBuf`-returning getters) -- see that
/// constant's own doc comment for the 4 providers still not covered and
/// why.
fn restricted_write_prefixes() -> Vec<PathBuf> {
    let mut prefixes = Vec::new();

    if let Some(home) = crate::utils::resolve_home_dir() {
        prefixes.push(home.join(".grid-local"));
    }
    for get_base_path in STRING_BASE_PATH_PROVIDERS {
        if let Some(base_path) = get_base_path() {
            prefixes.push(PathBuf::from(base_path));
        }
    }
    if let Some(cursor_base) = crate::providers::cursor::get_base_path() {
        prefixes.push(cursor_base);
    }
    if let Some(vscode_base) = crate::providers::vscode::get_base_path() {
        prefixes.push(vscode_base);
    }
    prefixes.extend(crate::providers::vscode::get_base_paths());

    if cfg!(windows) {
        for var in ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(value) = std::env::var(var) {
                if !value.is_empty() {
                    prefixes.push(PathBuf::from(value));
                }
            }
        }
    } else {
        for p in ["/etc", "/usr", "/bin", "/sbin", "/boot", "/System", "/Library"] {
            prefixes.push(PathBuf::from(p));
        }
    }

    prefixes
}

/// `None` if `path` is safe to write into; `Some(reason)` otherwise.
/// Windows paths are compared case-insensitively (the filesystem itself
/// is); Unix paths case-sensitively, matching each platform's own
/// semantics.
fn restricted_write_reason(path: &Path) -> Option<&'static str> {
    let matches = |candidate: &Path, prefix: &Path| {
        if cfg!(windows) {
            candidate
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&prefix.to_string_lossy().to_lowercase())
        } else {
            candidate.starts_with(prefix)
        }
    };

    for prefix in restricted_write_prefixes() {
        if matches(path, &prefix) {
            return Some("Invalid file path: refusing to write into a Grid, provider, or system directory");
        }
    }
    None
}

/// Validate a path chosen by user via native file dialog.
///
/// Checks: absolute path, no `..` traversal, parent directory exists, and
/// (for writes) not inside a directory this app must never write into --
/// see [`restricted_write_prefixes`].
///
/// Used by [`write_text_file`] and [`save_screenshot`].
pub(crate) fn validate_dialog_path(path: &Path) -> Result<(), String> {
    if !crate::utils::looks_like_absolute_path(&path.to_string_lossy()) {
        return Err("Path must be absolute".to_string());
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Path cannot contain '..' components".to_string());
    }
    if let Some(reason) = restricted_write_reason(path) {
        return Err(reason.to_string());
    }
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(format!(
                "Parent directory does not exist: {}",
                parent.display()
            ));
        }
        let metadata = parent.symlink_metadata().map_err(|e| {
            format!(
                "Failed to read metadata for parent directory {}: {}",
                parent.display(),
                e
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err("Symlink parent directories are not allowed".to_string());
        }
    }
    Ok(())
}

/// Write text content to a file chosen by user via native dialog.
///
/// Path is validated for basic safety (absolute, no traversal, parent exists).
/// Directory allowlisting for `WebUI` callers is enforced at the HTTP handler layer.
///
/// # Arguments
/// * `path` - Absolute path chosen by user via save dialog
/// * `content` - Text content to write
#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = PathBuf::from(path);

        validate_dialog_path(&path)?;

        // Atomic write: write to temp file then rename
        let temp_path = path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temp file {}: {}", temp_path.display(), e))?;
        file.write_all(content.as_bytes()).map_err(|e| {
            format!(
                "Failed to write to temp file {}: {}",
                temp_path.display(),
                e
            )
        })?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp file: {e}"))?;
        super::fs_utils::atomic_rename(&temp_path, &path)?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Save screenshot binary data to a user-selected path.
///
/// The path is expected to come from a native save dialog and must be absolute.
/// Directory allowlisting for `WebUI` callers is enforced at the HTTP handler layer.
///
/// # Arguments
/// * `path` - Absolute path chosen by user via save dialog
/// * `data` - Base64-encoded PNG data
#[tauri::command]
pub async fn save_screenshot(path: String, data: String) -> Result<(), String> {
    use base64::Engine;
    tauri::async_runtime::spawn_blocking(move || {
        let path = PathBuf::from(&path);
        validate_dialog_path(&path)?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&data)
            .map_err(|e| format!("Base64 decode error: {e}"))?;

        // Atomic write: temp file + rename
        let temp_path = path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temp file {}: {}", temp_path.display(), e))?;
        file.write_all(&bytes)
            .map_err(|e| format!("Failed to write temp file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp file: {e}"))?;
        super::fs_utils::atomic_rename(&temp_path, &path)?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    /// Sets up a test environment with a temporary HOME directory.
    /// `env::set_var("HOME", ...)` alone never worked on Windows (`dirs::
    /// home_dir()` reads `USERPROFILE`, not `HOME`) -- kept here for Unix
    /// compatibility, but now paired with `home_override_guard`, which
    /// `crate::utils::resolve_home_dir()` (what this module's own
    /// production code actually calls) checks first, on every platform.
    /// NOTE: Tests using this MUST run with --test-threads=1 because both
    /// env vars are process-global and not thread-safe.
    fn setup_test_env() -> (TempDir, crate::archive_db::test_support::EnvVarGuard) {
        let temp_dir = TempDir::new().unwrap();
        env::set_var("HOME", temp_dir.path());
        let guard = crate::utils::test_support::home_override_guard(temp_dir.path());
        (temp_dir, guard)
    }

    #[test]
    fn test_get_user_settings_path() {
        let (temp, _guard) = setup_test_env();
        let path = get_user_settings_path().unwrap();
        assert!(path.to_string_lossy().contains(".claude"));
        assert!(path.to_string_lossy().ends_with("settings.json"));
        drop(temp);
    }

    #[test]
    fn test_read_nonexistent_settings() {
        let (temp, _guard) = setup_test_env();
        let path = temp.path().join("nonexistent.json");
        let result = read_settings_file(&path).unwrap();
        assert_eq!(result, "{}");
        drop(temp);
    }

    #[test]
    fn test_get_settings_path_invalid_scope() {
        let result = get_settings_path("invalid", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid scope"));
    }

    #[test]
    fn test_get_settings_path_project_without_path() {
        let result = get_settings_path("project", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("project_path required"));
    }

    #[tokio::test]
    async fn test_get_settings_by_scope_user() {
        let (temp, _guard) = setup_test_env();
        let claude_dir = temp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        let settings_path = claude_dir.join("settings.json");
        fs::write(&settings_path, r#"{"user":"test"}"#).unwrap();

        let result = get_settings_by_scope("user".to_string(), None).await;
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains("user"));

        drop(temp);
    }

    #[tokio::test]
    async fn test_get_all_settings_empty() {
        let (temp, _guard) = setup_test_env();
        let result = get_all_settings(None).await;
        assert!(result.is_ok());

        let all = result.unwrap();
        // User settings file doesn't exist yet
        assert_eq!(all.user, Some("{}".to_string()));
        assert!(all.project.is_none());
        assert!(all.local.is_none());

        drop(temp);
    }

    #[tokio::test]
    async fn test_get_mcp_servers_empty() {
        let (temp, _guard) = setup_test_env();
        let result = get_mcp_servers().await;
        assert!(result.is_ok());

        let mcp = result.unwrap();
        assert!(mcp.servers.is_object());
        assert_eq!(mcp.servers.as_object().unwrap().len(), 0);

        drop(temp);
    }

    #[tokio::test]
    async fn test_get_mcp_servers_merges_sources() {
        let (temp, _guard) = setup_test_env();
        let claude_dir = temp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Create settings.json with mcpServers
        let settings_path = claude_dir.join("settings.json");
        fs::write(
            &settings_path,
            r#"{"mcpServers":{"server1":{"command":"cmd1"}}}"#,
        )
        .unwrap();

        // Create .mcp.json
        let mcp_path = claude_dir.join(".mcp.json");
        fs::write(&mcp_path, r#"{"server2":{"command":"cmd2"}}"#).unwrap();

        let result = get_mcp_servers().await;
        assert!(result.is_ok());

        let mcp = result.unwrap();
        let servers = mcp.servers.as_object().unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains_key("server1"));
        assert!(servers.contains_key("server2"));

        drop(temp);
    }

    #[tokio::test]
    async fn test_mcp_json_overrides_settings_json() {
        let (temp, _guard) = setup_test_env();
        let claude_dir = temp.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();

        // Both define "server1" - .mcp.json should win
        let settings_path = claude_dir.join("settings.json");
        fs::write(
            &settings_path,
            r#"{"mcpServers":{"server1":{"priority":"low"}}}"#,
        )
        .unwrap();

        let mcp_path = claude_dir.join(".mcp.json");
        fs::write(&mcp_path, r#"{"server1":{"priority":"high"}}"#).unwrap();

        let result = get_mcp_servers().await;
        assert!(result.is_ok());

        let mcp = result.unwrap();
        let servers = mcp.servers.as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers["server1"]["priority"], "high");

        drop(temp);
    }

    #[test]
    fn test_validate_dialog_path_absolute_accepted() {
        let (temp, _guard) = setup_test_env();
        let path = temp.path().join("test.txt");
        assert!(validate_dialog_path(&path).is_ok());
        drop(temp);
    }

    #[test]
    fn test_validate_dialog_path_relative_rejected() {
        let path = Path::new("relative/path.txt");
        let result = validate_dialog_path(path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn test_validate_dialog_path_parent_dir_rejected() {
        let path = Path::new("/some/path/../escape.txt");
        let result = validate_dialog_path(path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("'..'"));
    }

    #[test]
    fn test_validate_dialog_path_nonexistent_parent_rejected() {
        let path = Path::new("/nonexistent_dir_abc123/file.txt");
        let result = validate_dialog_path(path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_dialog_path_symlink_parent_rejected() {
        let (temp, _guard) = setup_test_env();
        let real_dir = temp.path().join("real_dir");
        fs::create_dir_all(&real_dir).unwrap();
        let symlink_dir = temp.path().join("symlink_dir");
        std::os::unix::fs::symlink(&real_dir, &symlink_dir).unwrap();
        let file_path = symlink_dir.join("test.txt");

        let result = validate_dialog_path(&file_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Symlink"));
        drop(temp);
    }

    #[tokio::test]
    async fn test_write_text_file_to_temp_dir() {
        let (temp, _guard) = setup_test_env();
        let file_path = temp.path().join("export-test.md");
        let content = "# Test Export\nHello world".to_string();

        let result =
            write_text_file(file_path.to_string_lossy().to_string(), content.clone()).await;
        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), content);
        drop(temp);
    }

    #[tokio::test]
    async fn test_write_text_file_relative_path_rejected() {
        let result = write_text_file("relative/path.txt".to_string(), "content".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    // Deliberately does NOT use `setup_test_env()`'s `HOME`-mocking pattern:
    // `dirs::home_dir()` does not reliably honor a test-mocked `HOME` on
    // Windows (an established gap elsewhere in this codebase). Calling
    // `dirs::home_dir()` directly and building the expected blocked path
    // FROM its real return value works identically cross-platform and
    // never depends on environment mocking at all.
    #[test]
    fn test_restricted_write_reason_blocks_grid_local_dir() {
        let home = dirs::home_dir().expect("a home directory must be resolvable in test env");
        let target = home.join(".grid-local").join("user-data.json");
        assert!(restricted_write_reason(&target).is_some());
    }

    #[test]
    fn test_restricted_write_reason_allows_an_ordinary_export_path() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("export.json");
        assert!(restricted_write_reason(&target).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_restricted_write_reason_blocks_os_critical_dir() {
        let system_root =
            std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let target = PathBuf::from(system_root).join("System32");
        assert!(restricted_write_reason(&target).is_some());
    }

    #[tokio::test]
    async fn test_write_text_file_rejects_a_path_inside_grid_local() {
        // Safe to point at the REAL home dir without mocking: the
        // restricted-prefix check rejects this before any file I/O is
        // attempted, so nothing is ever actually written.
        let home = dirs::home_dir().expect("a home directory must be resolvable in test env");
        let target = home
            .join(".grid-local")
            .join("write-hardening-test-should-never-be-created.json");
        let result =
            write_text_file(target.to_string_lossy().to_string(), "{}".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("refusing to write"));
        assert!(!target.exists(), "the rejected write must never touch disk");
    }
}
