//! Pitlane desktop: the Tauri app around the board. Repositories, lanes and cards go through
//! `pit-core` directly, every git call off the UI thread; runs, sessions, the merge queue and
//! quotas go through the daemon (`pitd`), which the app holds while it is open. Each module holds
//! the commands of one part of the app.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use pit_core::Repo;
use tauri::Manager;

mod accounts;
mod cards;
mod code_view;
mod daemon;
mod dictate;
mod env;
mod files;
mod holes;
mod lanes;
mod menu;
mod os;
mod preview;
mod runs;
mod settings;
mod term;
mod term_cmds;
mod vscode;

/// How long quitting waits for the daemon to end its sessions.
const QUIT_WAIT: Duration = Duration::from_secs(12);

type CmdResult<T> = Result<T, String>;

pub(crate) async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

fn open(path: &str) -> Result<Repo> {
    Repo::open_default(Path::new(path))
}

pub fn run() {
    env::extend_path();
    daemon::hold_daemon();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(runs::Following::default())
        .manage(term::Terms::default())
        .manage(dictate::Dictation::default())
        .manage(vscode::Vscode::default())
        .manage(code_view::CodeView::default())
        .menu(menu::menu)
        .setup(|app| {
            menu::tray(app.handle())?;
            Ok(())
        })
        .on_menu_event(|app, e| {
            if e.id().as_ref() == "close-window"
                && let Some(w) = app.get_webview_window("main") {
                    let _ = w.close();
                }
        })
        // Closing the window only hides it: Pitlane stays in the Dock and the menu bar, agents
        // keep going, and the Dock icon or the tray brings it back. Quit (⌘Q, the Dock's or the
        // tray's) is what ends it.
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e
                && w.label() == "main" && !daemon::QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = w.hide();
                }
        })
        .invoke_handler(tauri::generate_handler![
            runs::perf,
            runs::perf_set,
            runs::perf_hold,
            runs::presence,
            runs::presence_set,
            dictate::dictate_start,
            dictate::dictate_stop,
            dictate::dictate_cancel,
            settings::settings,
            settings::prefs_set,
            settings::remove_repo,
            settings::flow_get,
            settings::flow_set,
            os::reveal,
            settings::pitlane_home,
            settings::add_repo,
            settings::select_repo,
            lanes::yard,
            lanes::new_lane,
            lanes::ship_plan,
            lanes::ship,
            lanes::clean_plan,
            lanes::clean,
            lanes::undo_clean,
            lanes::trash_list,
            cards::cards_list,
            cards::card_add,
            cards::card_update,
            cards::card_answer,
            cards::card_reanswer,
            cards::card_unblock,
            cards::card_move,
            cards::card_talk,
            cards::card_talk_stop,
            cards::project_repos,
            cards::project_repo_remove,
            cards::project_context,
            cards::project_context_set,
            settings::capacity,
            cards::card_attach,
            cards::card_detach,
            cards::card_image,
            cards::run_shot,
            cards::card_reorder,
            cards::card_delete,
            cards::cards_forget,
            cards::card_restore,
            cards::card_note,
            cards::cards_analyze,
            runs::runs_list,
            runs::run_start,
            runs::run_resume,
            runs::run_say,
            runs::run_tune,
            runs::run_stop,
            runs::runs_stop_all,
            runs::runs_stop_analyses,
            runs::run_all_finish,
            runs::report_create,
            runs::reports_list,
            runs::report_digest,
            runs::report_delete,
            runs::report_restore,
            runs::report_suggest_card,
            os::print_page,
            runs::board_assist,
            runs::run_decide,
            runs::run_continue,
            runs::run_verification,
            lanes::lane_diff,
            runs::card_review,
            runs::queue_list,
            lanes::lane_drop_hunk,
            lanes::plan,
            lanes::files_list,
            term_cmds::term_open,
            term_cmds::term_resume,
            code_view::vscode_theme,
            code_view::vscode_prewarm,
            code_view::vscode_places,
            code_view::vscode_show,
            code_view::vscode_hide,
            code_view::vscode_holes,
            code_view::vscode_open,
            preview::preview_show,
            preview::preview_hide,
            preview::preview_reload,
            preview::preview_close,
            os::open_external,
            term_cmds::term_write,
            term_cmds::term_resize,
            term_cmds::term_close,
            term_cmds::term_list,
            term_cmds::term_buffer,
            runs::queue_add,
            runs::queue_paused,
            daemon::daemon_mismatch,
            runs::queue_resume,
            runs::queue_remove,
            accounts::accounts_list,
            accounts::account_use,
            runs::run_timeline,
            runs::run_follow,
            runs::sessions_list,
            accounts::quota
        ])
        .build(tauri::generate_context!())
        .expect("Pitlane failed to start")
        .run(|app, event| {
            // A click on the Dock icon brings the hidden window back.
            if let tauri::RunEvent::Reopen { .. } = event {
                menu::show_main(app);
            }
            if let tauri::RunEvent::Exit = event {
                // Nothing outlives the app: the daemon stops every agent and ends.
                daemon::QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
                // The daemon needs at most ~9 s to end its sessions; a daemon that hangs past
                // that must not keep the app from quitting.
                if let Some(mut c) = daemon::running().and_then(|c| c.with_timeout(Some(QUIT_WAIT)).ok()) {
                    let _ = c.shutdown();
                }
                app.state::<term::Terms>().close_all();
                app.state::<vscode::Vscode>().stop();
                // The Whisper model's Metal buffers must go before the C++ teardown at exit, or
                // ggml aborts and macOS shows "Pitlane quit unexpectedly". If a transcription
                // still holds it, leave without running that teardown at all.
                if !app.state::<dictate::Dictation>().release() {
                    unsafe { libc::_exit(0) };
                }
            }
        });
}
