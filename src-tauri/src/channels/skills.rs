use nte_dps_tool::core::timeline::TimelineScope;
use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    commands::skills::snapshot,
    contract::{CommandError, SubscriptionReceipt, skills::SkillsEvent, stream::StreamKind},
    state::AppState,
    windows::console,
};

pub(crate) const SKILLS_STREAM_INTERVAL_MS: u32 = 100;
#[tauri::command]
pub(crate) fn subscribe_skills(
    subscription_id: String,
    scope: TimelineScope,
    on_event: Channel<SkillsEvent>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    console::validate_window(&window)?;
    state.main_dps_stream_revision()?;

    let mut last_revision = None;
    subscribe_polling_stream(
        subscription_id,
        StreamKind::Skills,
        SKILLS_STREAM_INTERVAL_MS,
        on_event,
        state.inner(),
        &window,
        move |state| {
            let Ok(revision) = state.main_dps_stream_revision() else {
                return PollingStreamOutput::Stop;
            };
            if last_revision == Some(revision) {
                return PollingStreamOutput::NoChange;
            }
            let Ok(next) = snapshot(state, scope) else {
                return PollingStreamOutput::Stop;
            };
            last_revision = Some(revision);
            PollingStreamOutput::Event(SkillsEvent::Snapshot(next))
        },
    )
}
#[tauri::command]
pub(crate) fn unsubscribe_skills(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::Skills,
        state.inner(),
        &window,
        console::validate_window,
    )
}
