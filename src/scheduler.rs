use crate::{
    audit::AuditState,
    ble::{is_session_unhealthy, start_session, SessionClient},
    capture_events::{
        CaptureEvent, CaptureSubscription, CaptureTimeUs, ReadTiming, SubscriptionFilterOutcome,
    },
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
    interval_us: CaptureTimeUs,
}

impl Subscription {
    pub fn new(semantic: &str, interval: Duration) -> Result<Self, String> {
        if interval.is_zero() {
            return Err("subscription interval must be greater than zero".into());
        }
        Ok(Self {
            request: prepare_read(semantic)?,
            interval,
            interval_us: duration_us(interval)?,
        })
    }

    pub fn semantic(self) -> &'static str {
        self.request.metadata().semantic
    }

    pub fn interval(self) -> Duration {
        self.interval
    }

    pub const fn interval_us(self) -> CaptureTimeUs {
        self.interval_us
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
        recorder: Option<mpsc::Sender<CaptureEvent>>,
        capture_profile: Option<String>,
        capture_subscriptions: Option<Vec<CaptureSubscription>>,
    ) -> Result<Self, String> {
        if subscriptions.is_empty() {
            return Err("telemetry scheduler needs at least one subscription".into());
        }
        let session = start_session(adapter_id).await?;
        let session_start = Instant::now();
        let started = CaptureEvent::capture_started(
            Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_millis()
                    .try_into()
                    .map_err(|_| "wall clock timestamp exceeds supported range")?,
            ),
            capture_profile,
        );
        let configured = capture_subscriptions.unwrap_or_else(|| {
            subscriptions
                .iter()
                .map(|subscription| {
                    CaptureSubscription::new(
                        subscription.semantic(),
                        subscription.interval_us,
                        SubscriptionFilterOutcome::Scheduled,
                    )
                })
                .collect()
        });
        let discovery = match session.support_discovery().await {
            Ok(discovery) => discovery,
            Err(error) => {
                let cleanup = session.shutdown().await;
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!("{error}; cleanup failed: {cleanup}")),
                };
            }
        };
        let mut events = std::iter::once(started)
            .chain(std::iter::once(CaptureEvent::SessionInitialized))
            .chain(configured.into_iter().map(CaptureSubscription::into_event))
            .chain(discovery.into_iter().map(|page| {
                CaptureEvent::support_discovery(page.request.into(), page.response.into())
            }));
        if let Err(error) = events.try_for_each(|event| emit(&recorder, event)) {
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

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

async fn run(
    session: SessionClient,
    subscriptions: Vec<Subscription>,
    telemetry: Arc<Mutex<TelemetryState>>,
    audit: Arc<Mutex<AuditState>>,
    recorder: Option<mpsc::Sender<CaptureEvent>>,
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
            _ = &mut cancelled => {
                if let Err(error) = emit(&recorder, CaptureEvent::ShutdownRequested) {
                    break Err(error);
                }
                break Ok(())
            },
            _ = sleep_until(next) => {
                let now = Instant::now();
                for (subscription, due) in &mut schedule {
                    if *due > now {
                        continue;
                    }
                    let due_us = offset_us(session_start, *due);
                    let read_started = Instant::now();
                    let started_us = offset_us(session_start, read_started);
                    let outcome = session.read(subscription.request).await;
                    let read_finished = Instant::now();
                    let finished_us = offset_us(session_start, read_finished);
                    let timing = ReadTiming::new(due_us, started_us, finished_us);
                    match outcome {
                        Ok(transaction) => {
                            telemetry.lock().map_err(|_| "telemetry state lock poisoned")?.ingest(&transaction);
                            audit.lock().map_err(|_| "audit state lock poisoned")?.ingest(&transaction);
                            let event = CaptureEvent::read_succeeded_from_transaction(
                                &transaction,
                                subscription.interval_us,
                                timing,
                            )?;
                            if let Err(error) = emit(&recorder, event) {
                                break 'scheduler Err(error);
                            }
                        }
                        Err(error) => {
                            if is_session_unhealthy(&error) {
                                let fatal = error.clone();
                                if let Err(record_error) = record_fatal_read_failure(
                                    &audit,
                                    &recorder,
                                    subscription.semantic(),
                                    subscription.interval_us(),
                                    timing,
                                    subscription.request.bytes().into(),
                                    &error,
                                ) {
                                    break 'scheduler Err(record_error);
                                }
                                break 'scheduler Err(fatal);
                            }
                            if let Err(error) = record_read_failure(
                                &audit,
                                &recorder,
                                subscription.semantic(),
                                subscription.interval_us,
                                timing,
                                subscription.request.bytes().into(),
                                &error,
                            ) {
                                break 'scheduler Err(error);
                            }
                        }
                    }
                    if let Some((skipped, first_skipped, last_skipped)) =
                        advance_due(due, subscription.interval, Instant::now())
                    {
                        if let Err(error) = emit(
                            &recorder,
                            CaptureEvent::slots_skipped(
                                subscription.semantic(),
                                skipped,
                                offset_us(session_start, first_skipped),
                                offset_us(session_start, last_skipped),
                            ),
                        ) {
                            break 'scheduler Err(error);
                        }
                    }
                }
            }
        }
    };
    let stopped = emit(
        &recorder,
        CaptureEvent::SessionStopped {
            offset_us: offset_us(session_start, Instant::now()),
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

fn duration_us(duration: Duration) -> Result<CaptureTimeUs, String> {
    duration
        .as_micros()
        .try_into()
        .map_err(|_| "subscription interval exceeds supported microseconds".into())
}

fn offset_us(origin: Instant, at: Instant) -> CaptureTimeUs {
    at.saturating_duration_since(origin)
        .as_micros()
        .try_into()
        .unwrap_or(CaptureTimeUs::MAX)
}

fn advance_due(
    due: &mut Instant,
    interval: Duration,
    now: Instant,
) -> Option<(u64, Instant, Instant)> {
    *due += interval;
    let first_skipped = *due;
    let mut skipped = 0_u64;
    while *due <= now {
        skipped += 1;
        *due += interval;
    }
    (skipped > 0).then_some((skipped, first_skipped, *due - interval))
}

fn record_read_failure(
    audit: &Arc<Mutex<AuditState>>,
    recorder: &Option<mpsc::Sender<CaptureEvent>>,
    semantic: &'static str,
    interval_us: CaptureTimeUs,
    timing: ReadTiming,
    request_payload: Vec<u8>,
    error: &str,
) -> Result<(), String> {
    emit(
        recorder,
        CaptureEvent::read_failed(
            semantic,
            interval_us,
            Some(timing),
            Some(request_payload),
            error,
        ),
    )?;
    audit
        .lock()
        .map_err(|_| "audit state lock poisoned".to_string())?
        .record_error(error);
    Ok(())
}

fn record_fatal_read_failure(
    audit: &Arc<Mutex<AuditState>>,
    recorder: &Option<mpsc::Sender<CaptureEvent>>,
    semantic: &'static str,
    interval_us: CaptureTimeUs,
    timing: ReadTiming,
    request_payload: Vec<u8>,
    error: &str,
) -> Result<(), String> {
    record_read_failure(
        audit,
        recorder,
        semantic,
        interval_us,
        timing,
        request_payload,
        error,
    )?;
    emit(recorder, CaptureEvent::session_error(error))
}

fn emit(recorder: &Option<mpsc::Sender<CaptureEvent>>, event: CaptureEvent) -> Result<(), String> {
    let Some(recorder) = recorder else {
        return Ok(());
    };
    recorder.try_send(event).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => "capture recorder is full".into(),
        mpsc::error::TrySendError::Closed(_) => "capture recorder is closed".into(),
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
        emit(&recorder, CaptureEvent::SessionStopped { offset_us: 0 }).unwrap();
        assert_eq!(
            emit(&recorder, CaptureEvent::SessionStopped { offset_us: 1 }),
            Err("capture recorder is full".into())
        );

        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        assert_eq!(
            emit(&Some(sender), CaptureEvent::SessionStopped { offset_us: 0 }),
            Err("capture recorder is closed".into())
        );
    }

    #[test]
    fn advancing_due_skips_missed_slots_without_backfill() {
        let origin = Instant::now();
        let interval = Duration::from_millis(100);
        let mut due = origin;

        assert_eq!(
            advance_due(&mut due, interval, origin + Duration::from_millis(350)),
            Some((
                3,
                origin + Duration::from_millis(100),
                origin + Duration::from_millis(300),
            ))
        );
        assert_eq!(due, origin + Duration::from_millis(400));
    }

    #[test]
    fn read_failures_are_emitted_and_audited_without_a_session_error() {
        let (sender, mut receiver) = mpsc::channel(2);
        let audit = Arc::new(Mutex::new(AuditState::new(2).unwrap()));
        let subscription = Subscription::new("engine.rpm", Duration::from_millis(250)).unwrap();
        let timing = ReadTiming::new(1, 2, 3);

        record_read_failure(
            &audit,
            &Some(sender),
            subscription.semantic(),
            subscription.interval_us(),
            timing,
            subscription.request.bytes().into(),
            "conflicting 010C responses",
        )
        .unwrap();

        assert_eq!(
            receiver.try_recv().unwrap(),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: Some(timing),
                request_payload: Some(vec![0x01, 0x0c]),
                error: "conflicting 010C responses".into(),
            }
        );
        let entries = audit.lock().unwrap().snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].semantic, "scheduler.error");
        assert!(entries[0].source.contains("conflicting 010C responses"));
    }

    #[test]
    fn fatal_read_failure_keeps_read_evidence_and_emits_session_error() {
        let (sender, mut receiver) = mpsc::channel(2);
        let audit = Arc::new(Mutex::new(AuditState::new(2).unwrap()));
        let subscription = Subscription::new("engine.rpm", Duration::from_millis(250)).unwrap();
        let timing = ReadTiming::new(1, 2, 3);
        let error = "diagnostic session became unresponsive after repeated transport failures: Carly write timed out: 010C";

        record_fatal_read_failure(
            &audit,
            &Some(sender),
            subscription.semantic(),
            subscription.interval_us(),
            timing,
            subscription.request.bytes().into(),
            error,
        )
        .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap(),
            CaptureEvent::ReadFailed { error: value, .. } if value == error
        ));
        assert_eq!(
            receiver.try_recv().unwrap(),
            CaptureEvent::SessionError {
                error: error.into()
            }
        );
    }
}
