use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    commands::packets::snapshot_since,
    contract::{CommandError, SubscriptionReceipt, packets::PacketsEvent, stream::StreamKind},
    state::AppState,
    windows::console,
};

pub(crate) const PACKETS_STREAM_INTERVAL_MS: u32 = 100;
#[tauri::command]
pub(crate) fn subscribe_packets(
    subscription_id: String,
    on_event: Channel<PacketsEvent>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    console::validate_window(&window)?;
    state.packet_stream_revision()?;

    let mut last_revision = None;
    subscribe_polling_stream(
        subscription_id,
        StreamKind::Packets,
        PACKETS_STREAM_INTERVAL_MS,
        on_event,
        state.inner(),
        &window,
        move |state| {
            let Ok(current_revision) = state.packet_stream_revision() else {
                return PollingStreamOutput::Stop;
            };
            if last_revision == Some(current_revision) {
                return PollingStreamOutput::NoChange;
            }
            let Ok((revision, replace, snapshot)) = snapshot_since(state, last_revision) else {
                return PollingStreamOutput::Stop;
            };
            let event = if replace {
                PacketsEvent::Snapshot(snapshot)
            } else {
                PacketsEvent::Append(snapshot)
            };
            last_revision = Some(revision);
            PollingStreamOutput::Event(event)
        },
    )
}
#[tauri::command]
pub(crate) fn unsubscribe_packets(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::Packets,
        state.inner(),
        &window,
        console::validate_window,
    )
}
