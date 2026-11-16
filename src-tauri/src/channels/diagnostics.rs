use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    commands::diagnostics::snapshot,
    contract::{
        CommandError, SubscriptionReceipt, diagnostics::DiagnosticsEvent, stream::StreamKind,
    },
    state::AppState,
    windows::console,
};

pub(crate) const DIAGNOSTICS_STREAM_INTERVAL_MS: u32 = 500;
#[tauri::command]
pub(crate) fn subscribe_diagnostics(
    subscription_id: String,
    on_event: Channel<DiagnosticsEvent>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    console::validate_window(&window)?;
    state.diagnostics_revision()?;

    let mut last_revision = None;
    subscribe_polling_stream(
        subscription_id,
        StreamKind::Diagnostics,
        DIAGNOSTICS_STREAM_INTERVAL_MS,
        on_event,
        state.inner(),
        &window,
        move |state| {
            let Ok(revision) = state.diagnostics_revision() else {
                return PollingStreamOutput::Stop;
            };
            if last_revision == Some(revision) {
                return PollingStreamOutput::NoChange;
            }
            let Ok(next) = snapshot(state) else {
                return PollingStreamOutput::Stop;
            };
            last_revision = Some(revision);
            PollingStreamOutput::Event(DiagnosticsEvent::Snapshot(next))
        },
    )
}
#[tauri::command]
pub(crate) fn unsubscribe_diagnostics(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::Diagnostics,
        state.inner(),
        &window,
        console::validate_window,
    )
}
