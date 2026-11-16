use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    contract::{
        CommandError, SubscriptionReceipt,
        main_dps_detail::{MAIN_DPS_DETAIL_PAGE_LIMIT, MainDpsDetailSnapshot},
        stream::StreamKind,
    },
    state::AppState,
    windows::combat_details,
};

pub(crate) const MAIN_DPS_DETAIL_STREAM_INTERVAL_MS: u32 = 250;
#[tauri::command]
pub(crate) fn subscribe_main_dps_detail(
    subscription_id: String,
    on_event: Channel<MainDpsDetailSnapshot>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    combat_details::validate_window(&window)?;
    let kind = combat_details::window_kind(&window)?;
    state.main_dps_stream_revision()?;
    let mut last_revision = None;
    let poll_window = window.clone();
    subscribe_polling_stream(
        subscription_id,
        StreamKind::MainDpsDetail,
        MAIN_DPS_DETAIL_STREAM_INTERVAL_MS,
        on_event,
        state.inner(),
        &window,
        move |state| {
            let Ok(revision) = state.main_dps_stream_revision() else {
                return PollingStreamOutput::Stop;
            };
            let visible = match poll_window.is_visible() {
                Ok(visible) => visible,
                Err(_) => return PollingStreamOutput::Stop,
            };
            if !visible || last_revision == Some(revision) {
                return PollingStreamOutput::NoChange;
            }
            let Ok(next) =
                MainDpsDetailSnapshot::from_state(state, kind, 0, MAIN_DPS_DETAIL_PAGE_LIMIT)
            else {
                return PollingStreamOutput::Stop;
            };
            last_revision = Some(revision);
            PollingStreamOutput::Event(next)
        },
    )
}
#[tauri::command]
pub(crate) fn unsubscribe_main_dps_detail(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::MainDpsDetail,
        state.inner(),
        &window,
        combat_details::validate_window,
    )
}
