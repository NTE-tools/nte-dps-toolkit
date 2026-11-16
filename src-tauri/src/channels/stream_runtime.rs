use std::{sync::atomic::Ordering, time::Duration};

use serde::Serialize;
use tauri::{State, WebviewWindow, ipc::Channel};
use tokio::time::{MissedTickBehavior, interval};

use crate::{
    contract::{CommandError, SubscriptionReceipt, stream::StreamKind},
    state::{AppState, StreamRegistration, StreamRegistryError},
};

pub(crate) fn is_valid_subscription_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn validate_subscription_id(value: &str) -> Result<(), CommandError> {
    if is_valid_subscription_id(value) {
        Ok(())
    } else {
        Err(CommandError::invalid_subscription_id())
    }
}

impl From<StreamRegistryError> for CommandError {
    fn from(_: StreamRegistryError) -> Self {
        Self::stream_runtime_unavailable()
    }
}

// The receipt binds generation; the ordered Channel binds delivery sequence.
#[tauri::command]
pub(crate) fn acknowledge_stream_delivery(
    subscription_id: String,
    stream_kind: StreamKind,
    stream_generation: String,
    delivery_sequence: String,
    state: State<'_, AppState>,
    window: WebviewWindow,
) -> Result<(), CommandError> {
    validate_subscription_id(&subscription_id)?;
    let generation = parse_delivery_counter(&stream_generation)?;
    let sequence = parse_delivery_counter(&delivery_sequence)?;
    state
        .acknowledge_stream(
            window.label(),
            &stream_kind.stream_key(&subscription_id),
            generation,
            sequence,
        )
        .map(|_| ())
        .map_err(CommandError::from)
}

fn parse_delivery_counter(value: &str) -> Result<u64, CommandError> {
    if value.is_empty()
        || value.len() > 20
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CommandError::invalid_subscription_id());
    }
    value
        .parse()
        .map_err(|_| CommandError::invalid_subscription_id())
}

/// Result of one revision-aware projection pass.
pub(crate) enum PollingStreamOutput<T> {
    NoChange,
    Event(T),
    Stop,
}

pub(crate) struct StreamDeliveryEndpoint<T> {
    channel: Channel<T>,
}

impl<T> StreamDeliveryEndpoint<T> {
    pub(crate) fn new(channel: Channel<T>) -> Self {
        Self { channel }
    }
}

pub(crate) fn subscribe_polling_stream<T, Poll>(
    subscription_id: String,
    stream_kind: StreamKind,
    interval_ms: u32,
    on_event: Channel<T>,
    state: &AppState,
    window: &WebviewWindow,
    poll: Poll,
) -> Result<SubscriptionReceipt, CommandError>
where
    T: Serialize + Send + 'static,
    Poll: FnMut(&AppState) -> PollingStreamOutput<T> + Send + 'static,
{
    let stream_key = stream_kind.stream_key(&subscription_id);
    let state = state.clone();
    let registration = state.reserve_stream(window.label(), &stream_key)?;
    let stream_generation = registration.generation();
    spawn_polling_stream(
        StreamDeliveryEndpoint::new(on_event),
        state,
        registration,
        interval_ms,
        poll,
    )?;
    Ok(SubscriptionReceipt::new(
        subscription_id,
        stream_kind,
        stream_generation,
        interval_ms,
    ))
}

pub(crate) fn subscribe_blocking_polling_stream<T, Poll>(
    subscription_id: String,
    stream_kind: StreamKind,
    interval_ms: u32,
    on_event: Channel<T>,
    state: &AppState,
    window: &WebviewWindow,
    poll: Poll,
) -> Result<SubscriptionReceipt, CommandError>
where
    T: Serialize + Send + 'static,
    Poll: FnMut(&AppState, &std::sync::Arc<std::sync::atomic::AtomicBool>) -> PollingStreamOutput<T>
        + Send
        + 'static,
{
    let stream_key = stream_kind.stream_key(&subscription_id);
    let state = state.clone();
    let registration = state.reserve_stream(window.label(), &stream_key)?;
    let stream_generation = registration.generation();
    spawn_blocking_polling_stream(
        StreamDeliveryEndpoint::new(on_event),
        state,
        registration,
        interval_ms,
        poll,
    )?;
    Ok(SubscriptionReceipt::new(
        subscription_id,
        stream_kind,
        stream_generation,
        interval_ms,
    ))
}

pub(crate) fn unsubscribe_stream(
    subscription_id: &str,
    stream_kind: StreamKind,
    state: &AppState,
    window: &WebviewWindow,
    validate_window: impl FnOnce(&WebviewWindow) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    validate_subscription_id(subscription_id)?;
    validate_window(window)?;
    state.stop_stream(window.label(), &stream_kind.stream_key(subscription_id))?;
    Ok(())
}

/// Starts one lightweight async owner per subscription.
///
/// The registry owns cancellation and replacement. Tokio's skipped-tick policy
/// coalesces slow projections, while the Tauri Channel preserves event ordering.
/// A disconnected Channel or invalid projection ends the subscription and
/// removes its registry entry.
pub(crate) fn spawn_polling_stream<T, Poll>(
    delivery: StreamDeliveryEndpoint<T>,
    state: AppState,
    registration: StreamRegistration,
    interval_ms: u32,
    mut poll: Poll,
) -> Result<(), CommandError>
where
    T: Serialize + Send + 'static,
    Poll: FnMut(&AppState) -> PollingStreamOutput<T> + Send + 'static,
{
    match state.activate_stream(&registration) {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            registration.cancel();
            let _ = state.finish_stream(&registration);
            return Err(CommandError::stream_runtime_unavailable());
        }
    }

    let task_state = state.clone();
    let task_registration = registration.clone();
    tauri::async_runtime::spawn(async move {
        let stop = task_registration.stop_token();
        let active = task_registration.activation_token();
        let mut ticker = interval(Duration::from_millis(u64::from(interval_ms.max(1))));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        while !stop.load(Ordering::Acquire) {
            ticker.tick().await;
            if stop.load(Ordering::Acquire)
                || !active.load(Ordering::Acquire)
                || !task_registration.delivery_ready()
            {
                continue;
            }

            let event = match poll(&task_state) {
                PollingStreamOutput::NoChange => continue,
                PollingStreamOutput::Event(event) => event,
                PollingStreamOutput::Stop => break,
            };

            if stop.load(Ordering::Acquire)
                || !task_registration.begin_delivery()
                || delivery.channel.send(event).is_err()
            {
                break;
            }
        }

        task_registration.cancel();
        if task_state.finish_stream(&task_registration).is_err() {
            log::warn!("Stream registry cleanup reset after an interrupted update");
        }
    });

    Ok(())
}

/// Subscription-owned blocking work; cancellation is passed into finite IPC.
/// Runs its timer on Tokio and dispatches each active poll pass onto Tokio's
/// blocking thread pool so it neither stalls async workers nor pins a sleeping
/// OS thread while idle.
pub(crate) fn spawn_blocking_polling_stream<T, Poll>(
    delivery: StreamDeliveryEndpoint<T>,
    state: AppState,
    registration: StreamRegistration,
    interval_ms: u32,
    poll: Poll,
) -> Result<(), CommandError>
where
    T: Serialize + Send + 'static,
    Poll: FnMut(&AppState, &std::sync::Arc<std::sync::atomic::AtomicBool>) -> PollingStreamOutput<T>
        + Send
        + 'static,
{
    match state.activate_stream(&registration) {
        Ok(true) => {}
        _ => {
            registration.cancel();
            let _ = state.finish_stream(&registration);
            return Err(CommandError::stream_runtime_unavailable());
        }
    }
    let task_state = state.clone();
    let task_registration = registration.clone();
    tauri::async_runtime::spawn(async move {
        let stop = task_registration.stop_token();
        let active = task_registration.activation_token();
        let mut ticker = interval(Duration::from_millis(u64::from(interval_ms.max(1))));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut poll = Some(poll);

        while !stop.load(Ordering::Acquire) {
            ticker.tick().await;
            if stop.load(Ordering::Acquire)
                || !active.load(Ordering::Acquire)
                || !task_registration.delivery_ready()
            {
                continue;
            }
            let Some(mut poll_fn) = poll.take() else {
                break;
            };
            let poll_state = task_state.clone();
            let poll_stop = stop.clone();
            let Ok((poll_fn, output)) = tauri::async_runtime::spawn_blocking(move || {
                let output = poll_fn(&poll_state, &poll_stop);
                (poll_fn, output)
            })
            .await
            else {
                break;
            };
            poll = Some(poll_fn);
            match output {
                PollingStreamOutput::NoChange => {}
                PollingStreamOutput::Event(event) => {
                    if stop.load(Ordering::Acquire)
                        || !task_registration.begin_delivery()
                        || delivery.channel.send(event).is_err()
                    {
                        break;
                    }
                }
                PollingStreamOutput::Stop => break,
            }
        }
        task_registration.cancel();
        let _ = task_state.finish_stream(&task_registration);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_delivery_cancels_and_removes_both_polling_worker_kinds() {
        struct Dropped(std::sync::mpsc::Sender<()>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        for blocking in [false, true] {
            let state = AppState::default();
            let registration = state.reserve_stream("console", "timeline:failure").unwrap();
            let stop = registration.stop_token();
            let (sender, receiver) = std::sync::mpsc::channel();
            let dropped = Dropped(sender);
            let channel = Channel::new(move |_| {
                let _ = &dropped;
                Err(std::io::Error::other("fixture disconnect").into())
            });
            let delivery = StreamDeliveryEndpoint::new(channel);
            if blocking {
                spawn_blocking_polling_stream(delivery, state.clone(), registration, 1, |_, _| {
                    PollingStreamOutput::Event("snapshot")
                })
                .unwrap();
            } else {
                spawn_polling_stream(delivery, state.clone(), registration, 1, |_| {
                    PollingStreamOutput::Event("snapshot")
                })
                .unwrap();
            }
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("worker released channel");
            assert!(stop.load(Ordering::Acquire));
            assert_eq!(state.stream_registry_len().unwrap(), 0);
        }
    }

    #[test]
    fn delivery_counters_reject_noncanonical_and_overflowing_input() {
        for value in ["", "0", "01", "-1", "+1", "1.0", "18446744073709551616"] {
            assert!(parse_delivery_counter(value).is_err(), "{value}");
        }
        assert_eq!(parse_delivery_counter("1").unwrap(), 1);
        assert_eq!(
            parse_delivery_counter("18446744073709551615").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn subscription_id_policy_is_bounded_and_url_safe() {
        assert!(is_valid_subscription_id("main-window_01"));
        assert!(!is_valid_subscription_id(""));
        assert!(!is_valid_subscription_id("contains:colon"));
        assert!(!is_valid_subscription_id(&"a".repeat(65)));
    }
}
