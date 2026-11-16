use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    contract::{CommandError, SubscriptionReceipt, TechnicalEvent, stream::StreamKind},
    state::{AppState, TECHNICAL_STREAM_INTERVAL_MS},
    windows::hud,
};

#[tauri::command]
pub(crate) fn subscribe_technical_state(
    subscription_id: String,
    on_event: Channel<TechnicalEvent>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    hud::validate_window(&window)?;
    state.snapshot()?;

    let mut last_revision = None;
    subscribe_polling_stream(
        subscription_id,
        StreamKind::Technical,
        TECHNICAL_STREAM_INTERVAL_MS,
        on_event,
        state.inner(),
        &window,
        move |state| {
            let revision = state.stream_revision();
            if !should_emit_snapshot(last_revision, revision) {
                return PollingStreamOutput::NoChange;
            }
            let Ok(next) = state.snapshot() else {
                return PollingStreamOutput::Stop;
            };
            last_revision = Some(revision);
            PollingStreamOutput::Event(TechnicalEvent::Snapshot(next))
        },
    )
}

#[tauri::command]
pub(crate) fn unsubscribe_technical_state(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::Technical,
        state.inner(),
        &window,
        hud::validate_window,
    )
}

fn should_emit_snapshot(
    last_revision: Option<crate::state::StreamRevision>,
    current_revision: crate::state::StreamRevision,
) -> bool {
    last_revision != Some(current_revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_revision_skips_idle_snapshot_work() {
        let state = AppState::default();
        let revision = state.stream_revision();

        assert!(should_emit_snapshot(None, revision));
        assert!(!should_emit_snapshot(Some(revision), revision));

        state.set_passthrough(true);
        assert!(should_emit_snapshot(
            Some(revision),
            state.stream_revision()
        ));
    }
}
