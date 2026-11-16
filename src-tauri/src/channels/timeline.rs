use nte_dps_tool::core::timeline::TimelineScope;
use tauri::{State, WebviewWindow, ipc::Channel};

use crate::{
    channels::stream_runtime::{
        PollingStreamOutput, subscribe_polling_stream, unsubscribe_stream, validate_subscription_id,
    },
    commands::timeline::snapshot,
    contract::{CommandError, SubscriptionReceipt, stream::StreamKind, timeline::TimelineEvent},
    state::AppState,
    windows::console,
};

pub(crate) const TIMELINE_STREAM_INTERVAL_MS: u32 = 100;
#[tauri::command]
pub(crate) fn subscribe_timeline(
    subscription_id: String,
    scope: TimelineScope,
    on_event: Channel<TimelineEvent>,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<SubscriptionReceipt, CommandError> {
    validate_subscription_id(&subscription_id)?;
    console::validate_window(&window)?;
    state.main_dps_stream_revision()?;

    let mut last_revision = None;
    subscribe_polling_stream(
        subscription_id,
        StreamKind::Timeline,
        TIMELINE_STREAM_INTERVAL_MS,
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
            PollingStreamOutput::Event(TimelineEvent::Snapshot(next))
        },
    )
}

#[tauri::command]
pub(crate) fn unsubscribe_timeline(
    subscription_id: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    unsubscribe_stream(
        &subscription_id,
        StreamKind::Timeline,
        state.inner(),
        &window,
        console::validate_window,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nte_dps_tool::core::timeline::{
        TimelineBucketProjection, TimelineCharacterProjection, TimelineProjection,
        TimelineRoleProjection, TimelineScope, TimelineSegmentProjection,
    };

    use crate::contract::{
        stream::tests::MAX_STREAM_DELIVERY_BYTES,
        timeline::{
            MAX_TIMELINE_BUCKETS, MAX_TIMELINE_CHARACTERS, MAX_TIMELINE_ROLES_PER_BUCKET,
            TimelineSnapshot,
        },
    };
    use nte_dps_tool::storage::config::TimelineDpsViewMode;

    #[test]
    fn maximum_timeline_contract_fits_the_shared_stream_byte_budget() {
        let roles = (0..MAX_TIMELINE_ROLES_PER_BUCKET)
            .map(|index| TimelineRoleProjection {
                character_id: index as u32,
                dps: f64::MAX,
            })
            .collect::<Vec<_>>();
        let mut projection = TimelineProjection {
            bucket_seconds: 0.2,
            effective_bucket_seconds: 0.2,
            bucket_seconds_min: 0.2,
            bucket_seconds_max: 60.0,
            bucket_seconds_step: 0.2,
            duration: 2_000.0,
            total_damage: f64::MAX,
            omitted_role_damage: f64::MAX,
            omitted_role_hits: u64::MAX,
            peak_dps: f64::MAX,
            time_stop_duration: 0.0,
            ..TimelineProjection::default()
        };
        projection.characters = (0..MAX_TIMELINE_CHARACTERS)
            .map(|index| TimelineCharacterProjection {
                id: index as u32,
                name: "W".repeat(128),
                color: "#FFFFFF".to_owned(),
                total_damage: f64::MAX,
            })
            .collect();
        projection.buckets = (0..MAX_TIMELINE_BUCKETS)
            .map(|index| TimelineBucketProjection {
                start: index as f64 * 0.2,
                end: (index + 1) as f64 * 0.2,
                team_dps: f64::MAX,
                damage: f64::MAX,
                hits: u64::MAX,
                cumulative_damage: f64::MAX,
                roles: roles.clone(),
            })
            .collect();
        projection.segments = (0..MAX_TIMELINE_BUCKETS)
            .map(|index| TimelineSegmentProjection {
                start: index as f64 * 0.2,
                end: (index + 1) as f64 * 0.2,
                dps: f64::MAX,
            })
            .collect();

        let snapshot = TimelineSnapshot::from_projection(
            &projection,
            u64::MAX,
            TimelineScope::Whole,
            TimelineDpsViewMode::Characters,
        );
        let bytes = serde_json::to_vec(&TimelineEvent::Snapshot(snapshot))
            .expect("maximal legal Timeline delivery stays serializable");
        assert!(bytes.len() <= MAX_STREAM_DELIVERY_BYTES);
        assert!(
            bytes.len() <= 12 * 1024 * 1024,
            "the item cap should retain at least 4 MiB of contract headroom"
        );
    }
}
