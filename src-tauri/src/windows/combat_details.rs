use std::time::Duration;

use tauri::{Emitter, LogicalPosition, LogicalSize, WebviewWindow, WindowEvent};

use crate::{
    contract::CommandError,
    state::{AppState, MainDpsDetailKind},
    windows::window_position::{self, DebouncedEvent, WindowPersistenceRuntime},
};

pub(crate) const CHARACTER_DETAILS_WINDOW_LABEL: &str = "character-details";
pub(crate) const TEAM_DETAILS_WINDOW_LABEL: &str = "team-details";
pub(crate) const COMBAT_DETAILS_CHANGED_EVENT: &str = "main-dps-detail-requested";
const GEOMETRY_SAVE_DEBOUNCE: Duration = Duration::from_millis(180);

pub(crate) fn window_kind(window: &WebviewWindow) -> Result<MainDpsDetailKind, CommandError> {
    match window.label() {
        CHARACTER_DETAILS_WINDOW_LABEL => Ok(MainDpsDetailKind::Character),
        TEAM_DETAILS_WINDOW_LABEL => Ok(MainDpsDetailKind::Team),
        _ => Err(CommandError::invalid_window()),
    }
}

pub(crate) fn validate_window(window: &WebviewWindow) -> Result<(), CommandError> {
    window_kind(window).map(|_| ())
}

pub(crate) fn show(
    window: &WebviewWindow,
    state: &AppState,
    kind: MainDpsDetailKind,
) -> Result<(), CommandError> {
    let (size, position) = state.main_dps_detail_window_geometry(kind);
    if let Some([width, height]) = size
        && width.is_finite()
        && height.is_finite()
    {
        window
            .set_size(LogicalSize::new(f64::from(width), f64::from(height)))
            .map_err(window_error)?;
    }
    if let Some([x, y]) = position
        && x.is_finite()
        && y.is_finite()
    {
        window
            .set_position(LogicalPosition::new(f64::from(x), f64::from(y)))
            .map_err(window_error)?;
        window_position::ensure_window_reachable(window).map_err(|error| {
            log::error!("restore combat details window position failed: {error}");
            CommandError::window_operation_failed()
        })?;
    } else {
        window.center().map_err(window_error)?;
    }
    window.show().map_err(window_error)?;
    window.unminimize().map_err(window_error)?;
    window.set_focus().map_err(window_error)?;
    window
        .emit(COMBAT_DETAILS_CHANGED_EVENT, ())
        .map_err(window_error)
}

pub(crate) fn initialize(
    window: &WebviewWindow,
    state: &AppState,
    kind: MainDpsDetailKind,
    persistence: &WindowPersistenceRuntime,
) -> Result<(), String> {
    let webview = window.as_ref().clone();
    if let Err(error) = webview.set_auto_resize(true) {
        log::warn!("enable native combat details WebView auto-resize failed: {error}");
    }
    let persistence_state = state.clone();
    let thread_name = match kind {
        MainDpsDetailKind::Character => "tauri-character-detail-geometry-save",
        MainDpsDetailKind::Team => "tauri-team-detail-geometry-save",
    };
    let sender = persistence.spawn_debounced(
        thread_name,
        GEOMETRY_SAVE_DEBOUNCE,
        move |(size, position): ([f32; 2], [f32; 2])| {
            if let Err(error) =
                persistence_state.set_main_dps_detail_window_geometry(size, position, kind)
            {
                log::error!("persist combat detail window geometry failed: {error}");
            }
        },
    )?;

    let close_window = window.clone();
    let event_window = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            if !should_persist_geometry(
                event_window.is_minimized().unwrap_or(false),
                event_window.is_maximized().unwrap_or(false),
            ) {
                return;
            }
            let Ok(scale) = event_window.scale_factor() else {
                return;
            };
            let Ok(position) = event_window.outer_position() else {
                return;
            };
            let Ok(size) = event_window.inner_size() else {
                return;
            };
            let position = position.to_logical::<f32>(scale);
            let size = size.to_logical::<f32>(scale);
            let _ = sender.send(DebouncedEvent::Changed((
                [size.width, size.height],
                [position.x, position.y],
            )));
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Err(error) = close_window.hide() {
                log::error!("hide combat details window failed: {error}");
            }
        }
        WindowEvent::Destroyed => {
            let _ = sender.send(DebouncedEvent::Shutdown);
        }
        _ => {}
    });
    Ok(())
}

fn should_persist_geometry(minimized: bool, maximized: bool) -> bool {
    !minimized && !maximized
}

fn window_error(error: tauri::Error) -> CommandError {
    log::error!("combat details window operation failed: {error}");
    CommandError::window_operation_failed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_minimized_or_maximized_geometry_is_not_persisted() {
        assert!(should_persist_geometry(false, false));
        assert!(!should_persist_geometry(true, false));
        assert!(!should_persist_geometry(false, true));
    }
}
