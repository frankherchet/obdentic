use crate::{
    audit::AuditState,
    ble::{start_session, SessionClient},
    evidence::EvidenceEvent,
    prepare_read,
    telemetry::TelemetryState,
    ReadRequest,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{sleep_until, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subscription {
    request: ReadRequest,
    interval: Duration,
    interval_ms: u64,
}

impl Subscription {
    pub fn new(semantic: &str, interval: Duration) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("subscription interval must be greater than zero".into());
        }
        Ok(Self {
            request: prepare_read(semantic)?,
            interval,
            interval_ms: duration_ms(interval)?,
        })
    }

    pub fn semantic(self) -> &'static str {
        self.request.metadata().semantic
    }
}

pub struct TelemetryScheduler {
    cancel: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}

impl TelemetryScheduler {
    pub async fn start(
        adapter_id: &str,
        subscriptions: Vec<Subscription>,
        telemetry: Arc<Mutex<TelemetryState>>,
        audit: Arc<Mutex<AuditState>>,
        recorder: Option<mpsc::Sender<EvidenceEvent>>,
    ) -> Result<Self, String> {
        if subscriptions.is_empty() {
            return Err("telemetry scheduler needs at least one subscription".into());
        }
        let session = start_session(adapter_id).await?;
        let session_start = Instant::now();
        let started = EvidenceEvent::SessionStart {
            unix_timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis(),
            subscriptions: subscriptions
                .iter()
                .map(|subscription| (subscription.semantic(), subscription.interval_ms))
                .collect(),
        };
        if let Err(error) = emit(&recorder, started) {
            let cleanup = session.shutdown().await;
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!("{error}; cleanup failed: {cleanup}")),
            };
        }
        let (cancel, cancelled) = oneshot::channel();
        let task = tokio::spawn(run(
            session,
            subscriptions,
            telemetry,
            audit,
            recorder,
            session_start,
            cancelled,
        ));
        Ok(Self { cancel, task })
    }

    pub async fn stop(self) -> Result<(), String> {
        let _ = self.cancel.send(());
        self.task
            .await
            .map_err(|error| format!("telemetry scheduler stopped unexpectedly: {error}"))?
    }
}

async fn run(
    session: SessionClient,
    subscriptions: Vec<Subscription>,
    telemetry: Arc<Mutex<TelemetryState>>,
    audit: Arc<Mutex<AuditState>>,
    recorder: Option<mpsc::Sender<EvidenceEvent>>,
    session_start: Instant,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<(), String> {
    let mut schedule = subscriptions
        .into_iter()
        .map(|subscription| (subscription, session_start))
        .collect::<Vec<_>>();
    let result = 'scheduler: loop {
        let next = schedule.iter().map(|(_, due)| *due).min().unwrap();
        tokio::select! {
            _ = &mut cancelled => break Ok(()),
            _ = sleep_until(next) => {
                let now = Instant::now();
                for (subscription, due) in &mut schedule {
                    if *due > now {
                        continue;
                    }
                    let scheduled_offset_ms = due.duration_since(session_start).as_millis();
                    let read_started_offset_ms = Instant::now().duration_since(session_start).as_millis();
                    let outcome = session.read(subscription.request).await;
                    let read_finished_offset_ms = Instant::now().duration_since(session_start).as_millis();
                    let read_duration_ms = read_finished_offset_ms.saturating_sub(read_started_offset_ms);
                    let requested_interval_ms = subscription.interval_ms;
                    match outcome {
                        Ok(transaction) => {
                            telemetry.lock().map_err(|_| "telemetry state lock poisoned")?.ingest(&transaction);
                            audit.lock().map_err(|_| "audit state lock poisoned")?.ingest(&transaction);
                            if let Err(error) = emit(&recorder, EvidenceEvent::Read {
                                semantic: subscription.semantic(),
                                requested_interval_ms,
                                scheduled_offset_ms,
                                read_started_offset_ms,
                                read_finished_offset_ms,
                                read_duration_ms,
                                transaction,
                            }) {
                                break 'scheduler Err(error);
                            }
                        }
                        Err(error) => {
                            if let Err(recorder_error) = emit(&recorder, EvidenceEvent::ReadError {
                                semantic: subscription.semantic(),
                                requested_interval_ms,
                                scheduled_offset_ms,
                                read_started_offset_ms,
                                read_finished_offset_ms,
                                read_duration_ms,
                                error: error.clone(),
                            }) {
                                break 'scheduler Err(recorder_error);
                            }
                            break 'scheduler Err(error);
                        }
                    }
                    while *due <= Instant::now() {
                        *due += subscription.interval;
                    }
                }
            }
        }
    };
    let stopped = emit(
        &recorder,
        EvidenceEvent::SessionStop {
            offset_ms: Instant::now().duration_since(session_start).as_millis(),
        },
    );
    let result = match (result, stopped) {
        (Ok(()), stopped) => stopped,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(stopped)) => Err(format!("{error}; evidence stop failed: {stopped}")),
    };
    let result = match (result, session.shutdown().await) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    };
    if let Err(error) = &result {
        audit
            .lock()
            .map_err(|_| "audit state lock poisoned")?
            .record_error(error);
    }
    result
}

fn duration_ms(duration: Duration) -> Result<u64, String> {
    duration
        .as_millis()
        .try_into()
        .map_err(|_| "subscription interval exceeds supported milliseconds".into())
}

fn emit(
    recorder: &Option<mpsc::Sender<EvidenceEvent>>,
    event: EvidenceEvent,
) -> Result<(), String> {
    let Some(recorder) = recorder else {
        return Ok(());
    };
    recorder.try_send(event).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => "evidence recorder is full".into(),
        mpsc::error::TrySendError::Closed(_) => "evidence recorder is closed".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_only_accept_known_read_only_signals_and_positive_intervals() {
        assert_eq!(
            Subscription::new("engine.rpm", Duration::from_millis(200))
                .unwrap()
                .semantic(),
            "engine.rpm"
        );
        assert!(Subscription::new("dtc.clear", Duration::from_secs(1)).is_err());
        assert!(Subscription::new("engine.rpm", Duration::ZERO).is_err());
    }

    #[test]
    fn evidence_emission_rejects_full_or_closed_recorders() {
        let (sender, _receiver) = mpsc::channel(1);
        let recorder = Some(sender);
        emit(&recorder, EvidenceEvent::SessionStop { offset_ms: 0 }).unwrap();
        assert_eq!(
            emit(&recorder, EvidenceEvent::SessionStop { offset_ms: 1 }),
            Err("evidence recorder is full".into())
        );

        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        assert_eq!(
            emit(&Some(sender), EvidenceEvent::SessionStop { offset_ms: 0 }),
            Err("evidence recorder is closed".into())
        );
    }
}
