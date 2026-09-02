pub mod archive_db;
pub mod cli;
pub mod cli_args;
pub mod commands;
pub mod export;
pub mod models;
pub mod providers;
pub mod utils;
pub mod wsl;

#[cfg(test)]
pub mod test_utils;

use crate::commands::antigravity::{
    get_antigravity_project_summary, get_antigravity_session, load_antigravity_state,
};
use crate::commands::{
    archive_db::{
        cancel_first_index, delete_grid_local_data, get_archive_db_status, rebuild_grid_index,
        run_first_index, sync_grid_index, FirstIndexCancelFlag,
    },
    claude_settings::{
        get_all_mcp_servers, get_all_settings, get_claude_json_config, get_mcp_servers,
        get_settings_by_scope, save_screenshot, write_text_file,
    },
    diagnostics::{get_diagnostics_snapshot, record_diagnostics_event, DiagnosticsState},
    feedback::{get_system_info, get_third_party_notices, open_github_issues, send_feedback},
    history::list_history_sessions,
    insights::{
        dismiss_problem, get_agent_run_detail, get_agent_run_tree, get_error_occurrences,
        get_high_token_sessions, get_large_agent_runs, get_personal_baseline,
        get_repeated_command_failures, get_repeated_errors, get_similar_error_resolutions,
        get_since_last_visit_summary, get_things_worth_looking_at, get_this_week_summary,
        list_sessions_in_window, record_visit,
    },
    mcp_presets::{delete_mcp_preset, get_mcp_preset, load_mcp_presets, save_mcp_preset},
    metadata::{
        get_metadata_folder_path, get_session_display_name, is_project_hidden, load_user_metadata,
        save_user_metadata, update_project_metadata, update_session_metadata, update_user_settings,
        MetadataState,
    },
    multi_provider::{
        detect_providers, get_provider_message_offset, get_provider_tiers, load_provider_messages,
        load_provider_messages_paginated, load_provider_sessions, load_provider_sessions_page,
        scan_all_projects, search_all_providers,
    },
    project::{
        detect_claude_config_dir, get_claude_folder_path, get_git_log, scan_projects,
        validate_claude_folder, validate_custom_claude_dir,
    },
    search::search_archive_fts,
    session::{
        get_session_message_count, get_session_subagents,
        load_project_sessions, load_project_sessions_page, load_session_messages,
        load_session_messages_paginated, search_messages,
    },
    settings::{delete_preset, get_preset, load_presets, save_preset},
    stats::{
        get_global_stats_summary, get_project_stats_summary, get_project_token_stats,
        get_session_comparison, get_session_token_stats,
    },
    unified_presets::{
        delete_unified_preset, get_unified_preset, load_unified_presets, save_unified_preset,
    },
    update::force_quit_and_relaunch,
    watcher::{start_file_watcher, stop_file_watcher},
    wsl::{detect_wsl_distros, is_wsl_available},
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Headless session export (issue #343): `--export <id|path> [--format html|json]
    // [--output <file>]`. Handled before any GUI/webview so it works over SSH/CI
    // with no display.
    {
        let args: Vec<String> = std::env::args().collect();
        if args
            .iter()
            .any(|a| a == "--export" || a.starts_with("--export="))
        {
            std::process::exit(export::run_export(&args));
        }
    }

    run_tauri();
}

/// Run the normal Tauri desktop application.
fn run_tauri() {
    configure_linux_ime_environment();

    // Workaround for WebKitGTK GPU-process crashes on Linux.
    //
    // AppImage: bundled Ubuntu-compiled EGL/Mesa libs conflict with the system
    // WebKitGPUProcess (which inherits LD_LIBRARY_PATH), causing EGL_BAD_ALLOC
    // on distros with newer Mesa (e.g. Arch Linux). The CI pipeline removes
    // conflicting EGL libs from the AppImage (primary fix).
    //
    // Plain binaries: NVIDIA proprietary/open drivers on Wayland can crash at
    // startup in the DMA-BUF renderer path with "Gdk-Message: Error 71
    // (Protocol error) dispatching to Wayland display" (seen with WebKitGTK
    // 2.52 + NVIDIA 610 + GNOME Wayland), so this is not gated on AppImage.
    //
    // See: https://github.com/jhlee0409/claude-code-history-viewer/issues/186
    // See: https://github.com/tauri-apps/tauri/issues/11988
    // Note: std::env::set_var becomes unsafe in Rust edition 2024.
    // This is safe here because no threads exist yet at this point in startup.
    // Only set if not already configured by the user.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    use std::sync::{Arc, Mutex};
    use tauri::{Emitter, Manager};

    // Parse CLI args for a session preload hint (e.g. `--session <uuid>`).
    // A missing or unrecognized value yields None; the GUI then runs as usual.
    let startup_session_hint = cli::StartupSessionHint(cli::parse_session_hint(
        &std::env::args().collect::<Vec<_>>(),
    ));

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        // Single-instance plugin MUST be registered first so the second
        // invocation is intercepted before any other plugin does any work.
        // The callback receives the second process's argv; we re-parse it
        // for a session hint and forward to the live window. Any panic in
        // the callback is caught so a malformed argv cannot freeze the
        // already-running window.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Re-focus the main window regardless of hint presence so users
                // get visible feedback that the second launch was intercepted.
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
                if let Some(hint) = cli::parse_session_hint(&argv) {
                    // Frontend listens on this event (see App.tsx).
                    let _ = app.emit("cli-session-hint", hint);
                }
            }));
            if result.is_err() {
                log::error!("single_instance callback panicked; argv dropped");
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        // This crate was already a declared dependency but was never
        // actually registered here, so every
        // `log::error!`/`log::warn!`/`log::info!` call in this backend was
        // a silent no-op in any real build -- a user hitting a bug in
        // production had nothing to send for support beyond the
        // diagnostics counters (which deliberately carry no messages) and
        // the frontend's own client-side ErrorBoundary. Default targets
        // (stdout + the platform's standard app-log directory) are exactly
        // what's needed; only the default level (`Trace`, very noisy) is
        // overridden to `Info` for a sane production log size.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        );

    builder
        .manage(MetadataState::default())
        .manage(DiagnosticsState::default())
        .manage(startup_session_hint)
        .manage(FirstIndexCancelFlag::default())
        .manage(Arc::new(Mutex::new(None))
            as Arc<
                Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
            >)
        .invoke_handler(tauri::generate_handler![
            crate::cli::get_startup_session_hint,
            get_claude_folder_path,
            validate_claude_folder,
            validate_custom_claude_dir,
            detect_claude_config_dir,
            scan_projects,
            get_git_log,
            load_project_sessions,
            load_project_sessions_page,
            load_session_messages,
            load_session_messages_paginated,
            get_session_message_count,
            search_messages,
            get_session_subagents,
            get_session_token_stats,
            get_project_token_stats,
            get_project_stats_summary,
            get_session_comparison,
            get_global_stats_summary,
            list_history_sessions,
            get_repeated_command_failures,
            get_repeated_errors,
            get_error_occurrences,
            get_similar_error_resolutions,
            get_personal_baseline,
            get_large_agent_runs,
            get_high_token_sessions,
            get_things_worth_looking_at,
            get_since_last_visit_summary,
            record_visit,
            get_this_week_summary,
            list_sessions_in_window,
            get_agent_run_tree,
            get_agent_run_detail,
            search_archive_fts,
            dismiss_problem,
            send_feedback,
            get_system_info,
            get_third_party_notices,
            open_github_issues,
            // Metadata commands
            get_metadata_folder_path,
            load_user_metadata,
            save_user_metadata,
            update_session_metadata,
            update_project_metadata,
            update_user_settings,
            is_project_hidden,
            get_session_display_name,
            // Diagnostics commands
            record_diagnostics_event,
            get_diagnostics_snapshot,
            // Settings preset commands
            save_preset,
            load_presets,
            get_preset,
            delete_preset,
            // MCP preset commands
            save_mcp_preset,
            load_mcp_presets,
            get_mcp_preset,
            delete_mcp_preset,
            // Unified preset commands
            save_unified_preset,
            load_unified_presets,
            get_unified_preset,
            delete_unified_preset,
            // Claude Code settings commands (read-only — see read-only guarantee, spec §22/§46)
            get_settings_by_scope,
            get_all_settings,
            get_mcp_servers,
            get_all_mcp_servers,
            get_claude_json_config,
            // File I/O commands for export/import
            write_text_file,
            save_screenshot,
            // File watcher commands
            start_file_watcher,
            stop_file_watcher,
            // Multi-provider commands
            detect_providers,
            scan_all_projects,
            load_provider_sessions,
            load_provider_sessions_page,
            load_provider_messages,
            load_provider_messages_paginated,
            get_provider_message_offset,
            search_all_providers,
            get_provider_tiers,
            // Grid's own normalized archive (archive_db)
            sync_grid_index,
            rebuild_grid_index,
            run_first_index,
            cancel_first_index,
            get_archive_db_status,
            delete_grid_local_data,
            // WSL commands
            detect_wsl_distros,
            is_wsl_available,
            // Antigravity token-monitor commands
            load_antigravity_state,
            get_antigravity_session,
            get_antigravity_project_summary,
            // Updater fallback
            force_quit_and_relaunch
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // macOS-only: Spotlight / Dock / Finder launches don't re-exec
            // argv, so `tauri-plugin-single-instance` cannot see them. The OS
            // instead delivers the target as an Apple Event that Tauri
            // surfaces as `RunEvent::Opened { urls }`. We convert the first
            // resolvable URL into a `SessionHint` and re-use the same
            // `cli-session-hint` event the single-instance callback emits so
            // the frontend has one unified listener.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                for url in urls {
                    if let Some(hint) = cli::parse_session_hint_from_url(url) {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                        let _ = app.emit("cli-session-hint", hint);
                        break;
                    }
                }
            }
            // Prevent unused-variable warnings on non-macOS builds.
            #[cfg(not(target_os = "macos"))]
            {
                let _ = app;
                let _ = event;
            }
        });
}

#[cfg(target_os = "linux")]
fn configure_linux_ime_environment() {
    // configure_linux_ime_environment runs during process startup before Tauri
    // spawns threads, so applying linux_ime_environment_updates with
    // std::env::set_var avoids the Rust 2024 environment mutation hazard.
    let gtk_im_module = std::env::var("GTK_IM_MODULE").ok();
    let xmodifiers = std::env::var("XMODIFIERS").ok();
    let ibus_address = std::env::var("IBUS_ADDRESS").ok();

    for (key, value) in linux_ime_environment_updates(
        gtk_im_module.as_deref(),
        xmodifiers.as_deref(),
        ibus_address.as_deref(),
    ) {
        std::env::set_var(key, value);
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_ime_environment() {}

// Pure helper used by the Linux IME setup above and exercised by unit tests;
// gated to where it is referenced so non-Linux release builds do not see it as
// dead code under `-D warnings`.
#[cfg(any(target_os = "linux", test))]
fn linux_ime_environment_updates(
    gtk_im_module: Option<&str>,
    xmodifiers: Option<&str>,
    ibus_address: Option<&str>,
) -> Vec<(&'static str, &'static str)> {
    let has_ibus_signal = [gtk_im_module, xmodifiers, ibus_address]
        .into_iter()
        .flatten()
        .any(|value| value.contains("ibus"));

    if !has_ibus_signal {
        return Vec::new();
    }

    let mut updates = Vec::new();

    if gtk_im_module.map_or(true, str::is_empty) {
        updates.push(("GTK_IM_MODULE", "ibus"));
    }

    if xmodifiers.map_or(true, str::is_empty) {
        updates.push(("XMODIFIERS", "@im=ibus"));
    }

    updates
}

#[cfg(test)]
mod ime_environment_tests {
    use super::linux_ime_environment_updates;

    #[test]
    fn linux_ime_environment_sets_missing_ibus_variables_when_ibus_is_available() {
        let updates = linux_ime_environment_updates(None, None, Some("unix:path=/tmp/ibus"));

        assert_eq!(
            updates,
            vec![("GTK_IM_MODULE", "ibus"), ("XMODIFIERS", "@im=ibus"),]
        );
    }

    #[test]
    fn linux_ime_environment_preserves_existing_values() {
        let updates =
            linux_ime_environment_updates(Some("custom-gtk"), Some("@im=custom"), Some("ibus"));

        assert!(updates.is_empty());
    }

    #[test]
    fn linux_ime_environment_uses_existing_ibus_values_as_signal() {
        let updates = linux_ime_environment_updates(Some("ibus"), None, None);

        assert_eq!(updates, vec![("XMODIFIERS", "@im=ibus")]);
    }

    #[test]
    fn linux_ime_environment_does_nothing_without_ibus_signal() {
        let updates = linux_ime_environment_updates(None, None, None);

        assert!(updates.is_empty());
    }
}

