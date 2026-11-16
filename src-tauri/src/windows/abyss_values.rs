use std::time::Duration;

use tauri::{LogicalPosition, LogicalSize, WebviewWindow, WindowEvent};

use crate::{
    contract::CommandError,
    state::AppState,
    windows::window_position::{self, DebouncedEvent, WindowPersistenceRuntime},
};

pub(crate) const ABYSS_VALUES_WINDOW_LABEL: &str = "abyss-values";
const GEOMETRY_SAVE_DEBOUNCE: Duration = Duration::from_millis(180);

pub(crate) fn validate_window(window: &WebviewWindow) -> Result<(), CommandError> {
    if window.label() == ABYSS_VALUES_WINDOW_LABEL {
        Ok(())
    } else {
        Err(CommandError::invalid_window())
    }
}

pub(crate) fn show(window: &WebviewWindow, state: &AppState) -> Result<(), CommandError> {
    let (size, position) = state.abyss_window_geometry();
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
            log::error!("restore Abyss Values window position failed: {error}");
            CommandError::window_operation_failed()
        })?;
    } else {
        window.center().map_err(window_error)?;
    }
    window
        .set_title(&nte_dps_tool::storage::i18n::t("Abyss monster values"))
        .map_err(window_error)?;
    window.show().map_err(window_error)?;
    window.unminimize().map_err(window_error)?;
    window.set_focus().map_err(window_error)
}

pub(crate) fn initialize(
    window: &WebviewWindow,
    state: &AppState,
    persistence: &WindowPersistenceRuntime,
) -> Result<(), String> {
    let persistence_state = state.clone();
    let sender = persistence.spawn_debounced(
        "tauri-abyss-values-geometry-save",
        GEOMETRY_SAVE_DEBOUNCE,
        move |(size, position): ([f32; 2], [f32; 2])| {
            if let Err(error) = persistence_state.set_abyss_window_geometry(size, position) {
                log::error!("persist Abyss Values window geometry failed: {error}");
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
                log::error!("hide abyss values window after native close failed: {error}");
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
    log::error!("open abyss values window failed: {error}");
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
