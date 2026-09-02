import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * Config-shape security tests ("a CSP exists," "these specific permissions
 * are present") don't assert the actual promise that no new mutating
 * command can be registered unnoticed -- a new one could be added and
 * every existing test would still pass. This test enumerates the COMPLETE,
 * real `tauri::generate_handler![...]` command list from `lib.rs` and
 * asserts it exactly equals a reviewed, checked-in list. Any future
 * addition or removal MUST show up as an explicit diff to this list --
 * there is no way to add a new registered command (mutating or not)
 * without this test failing until the list below is updated on purpose.
 */
describe("registered Tauri commands: complete, reviewed allowlist", () => {
  const libRs = readFileSync(
    resolve(__dirname, "../../src-tauri/src/lib.rs"),
    "utf-8"
  );

  function extractRegisteredCommands(source: string): string[] {
    const marker = "tauri::generate_handler![";
    const start = source.indexOf(marker);
    if (start === -1) throw new Error("generate_handler! block not found");
    const openIdx = start + marker.length;
    let depth = 1;
    let i = openIdx;
    while (depth > 0 && i < source.length) {
      if (source[i] === "[") depth++;
      else if (source[i] === "]") depth--;
      i++;
    }
    const block = source.slice(openIdx, i - 1);
    // Strip `//` line comments without assuming a line-ending style --
    // `[^\r\n]*` stops before any carriage return or newline, unlike a
    // naive per-line `.replace(/\/\/.*$/, "")` which silently fails to
    // match (and leaves the comment text glued onto the next command) if
    // the file has trailing `\r` characters `.` doesn't consume.
    return block
      .replace(/\/\/[^\r\n]*/g, "")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
  }

  const EXPECTED_COMMANDS = [
    "crate::cli::get_startup_session_hint",
    "get_claude_folder_path",
    "validate_claude_folder",
    "validate_custom_claude_dir",
    "detect_claude_config_dir",
    "scan_projects",
    "get_git_log",
    "load_project_sessions",
    "load_project_sessions_page",
    "load_session_messages",
    "load_session_messages_paginated",
    "get_session_message_count",
    "search_messages",
    "get_session_subagents",
    "get_session_token_stats",
    "get_project_token_stats",
    "get_project_stats_summary",
    "get_session_comparison",
    "get_global_stats_summary",
    "list_history_sessions",
    "get_repeated_command_failures",
    "get_repeated_errors",
    "get_error_occurrences",
    "get_similar_error_resolutions",
    "get_personal_baseline",
    "get_large_agent_runs",
    "get_high_token_sessions",
    "get_things_worth_looking_at",
    "get_since_last_visit_summary",
    "record_visit",
    "get_this_week_summary",
    "list_sessions_in_window",
    "get_agent_run_tree",
    "get_agent_run_detail",
    "search_archive_fts",
    "dismiss_problem",
    "send_feedback",
    "get_system_info",
    "get_third_party_notices",
    "open_github_issues",
    "get_metadata_folder_path",
    "load_user_metadata",
    "save_user_metadata",
    "update_session_metadata",
    "update_project_metadata",
    "update_user_settings",
    "is_project_hidden",
    "get_session_display_name",
    "record_diagnostics_event",
    "get_diagnostics_snapshot",
    "save_preset",
    "load_presets",
    "get_preset",
    "delete_preset",
    "save_mcp_preset",
    "load_mcp_presets",
    "get_mcp_preset",
    "delete_mcp_preset",
    "save_unified_preset",
    "load_unified_presets",
    "get_unified_preset",
    "delete_unified_preset",
    "get_settings_by_scope",
    "get_all_settings",
    "get_mcp_servers",
    "get_all_mcp_servers",
    "get_claude_json_config",
    "write_text_file",
    "save_screenshot",
    "start_file_watcher",
    "stop_file_watcher",
    "detect_providers",
    "scan_all_projects",
    "load_provider_sessions",
    "load_provider_sessions_page",
    "load_provider_messages",
    "load_provider_messages_paginated",
    "get_provider_message_offset",
    "search_all_providers",
    "get_provider_tiers",
    "sync_grid_index",
    "rebuild_grid_index",
    "run_first_index",
    "cancel_first_index",
    "get_archive_db_status",
    "delete_grid_local_data",
    "detect_wsl_distros",
    "is_wsl_available",
    "load_antigravity_state",
    "get_antigravity_session",
    "get_antigravity_project_summary",
    "force_quit_and_relaunch",
  ];

  // Every command that writes/deletes real data, or otherwise mutates
  // state beyond an in-memory cache. `write_text_file`/`save_screenshot`
  // write only to a path the frontend supplies (hardened against Grid's
  // own/Claude's/OS-critical directories via
  // `commands::claude_settings::restricted_write_reason`, but NOT proven
  // to originate from the dialog -- a disclosed, narrower-than-ideal fix).
  // Read-only commands (the majority) are everything NOT in this list.
  const KNOWN_MUTATING_COMMANDS = new Set([
    "save_user_metadata",
    "update_session_metadata",
    "update_project_metadata",
    "update_user_settings",
    "record_diagnostics_event",
    "save_preset",
    "delete_preset",
    "save_mcp_preset",
    "delete_mcp_preset",
    "save_unified_preset",
    "delete_unified_preset",
    "write_text_file",
    "save_screenshot",
    "start_file_watcher",
    "stop_file_watcher",
    "sync_grid_index",
    "rebuild_grid_index",
    "run_first_index",
    "cancel_first_index",
    "delete_grid_local_data",
    "record_visit",
    "dismiss_problem",
    "force_quit_and_relaunch",
    "send_feedback",
    "open_github_issues",
  ]);

  it("matches the reviewed, complete command list exactly", () => {
    const actual = extractRegisteredCommands(libRs);
    expect(actual).toEqual(EXPECTED_COMMANDS);
  });

  it("has a name for every command in the reviewed mutating-or-not classification", () => {
    // Every mutating command name must actually be registered -- catches
    // the classification list itself drifting from reality (a typo, or a
    // renamed/removed command left behind in KNOWN_MUTATING_COMMANDS).
    for (const command of KNOWN_MUTATING_COMMANDS) {
      expect(EXPECTED_COMMANDS).toContain(command);
    }
  });

  it("never re-registers a command this project has already reviewed and removed", () => {
    // Real, confirmed removals from this project's own P0 remediation work
    // -- regression guard against reintroducing any of them.
    const REMOVED_COMMANDS = [
      "restore_file",
      "open_resume_in_terminal",
      "get_recent_edits",
      "read_text_file",
      "save_settings",
      "save_mcp_servers",
    ];
    const actual = extractRegisteredCommands(libRs);
    for (const removed of REMOVED_COMMANDS) {
      expect(actual).not.toContain(removed);
    }
  });
});
