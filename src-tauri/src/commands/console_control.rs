use std::fs;

use nte_dps_tool::{core::live_capture::LiveCapturePhase, storage::paths::capture_log_dir};
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::{
    commands::desktop_window,
    contract::CommandError,
    file_dialog,
    state::{AppState, DesktopWindowKind, PresentationError},
    windows::{console, hud, main_dps},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConsoleControlAction {
    ToggleCapture,
    ResetSession,
    ToggleHud,
    TogglePassthrough,
    ToggleProcessing,
    TogglePin,
    OpenTeamDetails,
    OpenCaptureLogs,
}

#[tauri::command]
pub(crate) async fn execute_console_control(
    action: ConsoleControlAction,
    app: AppHandle,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    console::validate_window(&window)?;
    match action {
        ConsoleControlAction::ToggleCapture => toggle_capture(&state),
        ConsoleControlAction::ResetSession => {
            state.ensure_session_undo_runtime_available()?;
            let worker_state = state.inner().clone();
            let undo_token = tauri::async_runtime::spawn_blocking(move || {
                if worker_state.is_capture_or_replay_active()? {
                    return Err(CommandError::new(
                        "confirmation_required",
                        "Confirm resetting the active capture from the main window",
                    ));
                }
                Ok::<_, CommandError>(worker_state.reset_session_with_undo_action()?)
            })
            .await
            .map_err(|_| {
                CommandError::new("reset_failed", "Failed to reset the current session")
            })??;
            crate::windows::island::publish_reset_notice(&app, state.inner(), undo_token);
            Ok(())
        }
        ConsoleControlAction::ToggleHud => toggle_hud(&app, &state),
        ConsoleControlAction::TogglePassthrough => toggle_passthrough(&app, &state),
        ConsoleControlAction::ToggleProcessing => {
            let worker_state = state.inner().clone();
            tauri::async_runtime::spawn_blocking(move || {
                worker_state.set_main_processing_paused(!worker_state.main_processing_paused())
            })
            .await
            .map_err(|_| CommandError::from(PresentationError::StateUnavailable))??;
            Ok(())
        }
        ConsoleControlAction::TogglePin => toggle_pin(&state, &window),
        ConsoleControlAction::OpenTeamDetails => super::main_dps::open_team_details(&app, &state),
        ConsoleControlAction::OpenCaptureLogs => open_capture_logs().await,
    }
}

fn toggle_capture(state: &AppState) -> Result<(), CommandError> {
    match state.capture_phase() {
        LiveCapturePhase::Starting | LiveCapturePhase::Running => {
            state.request_capture_stop().map_err(CommandError::from)
        }
        LiveCapturePhase::Stopping => Ok(()),
        LiveCapturePhase::Idle | LiveCapturePhase::Stopped | LiveCapturePhase::Failed => state
            .request_capture_start(false)
            .map_err(CommandError::from),
    }
}

fn toggle_hud(app: &AppHandle, state: &AppState) -> Result<(), CommandError> {
    let window = app
        .get_webview_window(hud::HUD_WINDOW_LABEL)
        .ok_or_else(CommandError::window_operation_failed)?;
    if window
        .is_visible()
        .map_err(|_| CommandError::window_operation_failed())?
    {
        window
            .hide()
            .map_err(|_| CommandError::window_operation_failed())
    } else {
        hud::set_passthrough(&window, state, false)?;
        window
            .show()
            .and_then(|_| window.unminimize())
            .map_err(|_| CommandError::window_operation_failed())
    }
}

fn toggle_passthrough(app: &AppHandle, state: &AppState) -> Result<(), CommandError> {
    let hud_window = app.get_webview_window(hud::HUD_WINDOW_LABEL);
    if let Some(window) = hud_window.as_ref()
        && window
            .is_visible()
            .map_err(|_| CommandError::window_operation_failed())?
    {
        return hud::toggle_passthrough(window, state);
    }
    let main_window = app
        .get_webview_window(main_dps::MAIN_DPS_WINDOW_LABEL)
        .ok_or_else(CommandError::window_operation_failed)?;
    main_dps::toggle_passthrough(&main_window, state)
}

fn toggle_pin(state: &AppState, console_window: &WebviewWindow) -> Result<(), CommandError> {
    let enabled = !console_window
        .is_always_on_top()
        .map_err(|_| CommandError::window_operation_failed())?;
    desktop_window::set_window_always_on_top(
        console_window,
        state,
        DesktopWindowKind::Console,
        enabled,
    )
}

async fn open_capture_logs() -> Result<(), CommandError> {
    let path = capture_log_dir();
    let path = tauri::async_runtime::spawn_blocking(move || {
        fs::create_dir_all(&path).map_err(|error| {
            log::error!("create capture log directory failed: {error}");
            CommandError::window_operation_failed()
        })?;
        Ok::<_, CommandError>(path)
    })
    .await
    .map_err(|error| {
        log::error!("create capture log directory task failed: {error}");
        CommandError::window_operation_failed()
    })??;
    file_dialog::open_directory(path).await.map_err(|error| {
        log::error!("open capture log directory failed: {error}");
        CommandError::window_operation_failed()
    })
}
