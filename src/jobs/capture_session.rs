//! The `capture` command's session: plan which signals to record, start the
//! scheduler against a live session, and run until the caller's stop
//! future resolves (Ctrl-C) or the scheduler stops unexpectedly.

use super::JobRuntime;
use crate::{
    audit::AuditState,
    ble::{SessionClient, SignalSupport, SignalSupportStatus},
    capture_events::{CaptureEvent, CaptureSubscription, SubscriptionFilterOutcome},
    capture_format::CaptureSink,
    runtime_reducer::RuntimeEvent,
    runtime_state::RecordingState,
    scheduler::{ObservationPlan, Subscription, TelemetryScheduler},
    telemetry::TelemetryState,
};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::sleep;

/// Which configured signals will actually be recorded, and the capture
/// evidence for the ones that were left out.
#[derive(Debug)]
pub struct CapturePlan {
    scheduled: Vec<Subscription>,
    subscriptions: Vec<CaptureSubscription>,
    total_configured: usize,
}

impl CapturePlan {
    /// The subset of configured signals the adapter advertises support for.
    /// Scheduling and routing use exactly this list.
    pub fn scheduled(&self) -> &[Subscription] {
        &self.scheduled
    }

    /// One [`CaptureSubscription`] per configured signal, recording why a
    /// signal was scheduled, omitted as unsupported, or omitted as unknown.
    pub fn subscriptions(&self) -> &[CaptureSubscription] {
        &self.subscriptions
    }

    /// How many signals the profile configured, before filtering.
    pub fn total_configured(&self) -> usize {
        self.total_configured
    }
}

/// Filters a profile's configured signals down to the ones the adapter
/// advertises support for, recording the decision for every signal either
/// way. Fails closed if nothing is left to schedule.
pub fn plan_capture(
    profile_name: &str,
    configured: Vec<Subscription>,
    advertised: &[SignalSupport],
) -> Result<CapturePlan, String> {
    let total_configured = configured.len();
    let mut scheduled = Vec::new();
    let mut subscriptions = Vec::new();
    for subscription in configured {
        let filter = match advertised
            .iter()
            .find(|signal| signal.semantic == subscription.semantic())
            .map(|signal| signal.status)
            .unwrap_or(SignalSupportStatus::Unknown)
        {
            SignalSupportStatus::Supported => SubscriptionFilterOutcome::Scheduled,
            SignalSupportStatus::Unsupported => SubscriptionFilterOutcome::Unsupported,
            SignalSupportStatus::Unknown => SubscriptionFilterOutcome::Unknown,
        };
        subscriptions.push(CaptureSubscription::new(
            subscription.semantic(),
            subscription.interval_us(),
            filter,
        ));
        if filter == SubscriptionFilterOutcome::Scheduled {
            scheduled.push(subscription);
        }
    }
    if scheduled.is_empty() {
        return Err(format!(
            "capture profile {profile_name} has no signals supported by the adapter"
        ));
    }
    Ok(CapturePlan {
        scheduled,
        subscriptions,
        total_configured,
    })
}

/// One running capture: a telemetry scheduler recording through an already
/// open [`CaptureSink`].
pub struct CaptureSession {
    scheduler: TelemetryScheduler,
    sink: CaptureSink,
}

impl CaptureSession {
    /// Starts the scheduler and begins recording. On failure, records the
    /// start failure as capture evidence, tears down the runtime state, and
    /// closes `sink` before returning the error — the caller has nothing
    /// left to clean up either way.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        profile_name: &str,
        plan: &CapturePlan,
        routing: Vec<ObservationPlan>,
        connect: impl Future<Output = Result<SessionClient, String>> + Send,
        telemetry: Arc<Mutex<TelemetryState>>,
        audit: Arc<Mutex<AuditState>>,
        rt: &mut JobRuntime<'_>,
        sink: CaptureSink,
    ) -> Result<Self, String> {
        rt.apply(RuntimeEvent::recording(RecordingState::Active))
            .await?;
        let scheduler = match TelemetryScheduler::start_with_runtime(
            connect,
            routing,
            telemetry,
            audit,
            Some(sink.sender().clone()),
            Some(profile_name.into()),
            Some(plan.subscriptions.clone()),
            rt.runtime_client(),
            None,
        )
        .await
        {
            Ok(scheduler) => scheduler,
            Err(error) => {
                record_capture_start_failure(sink.sender(), profile_name, &error).await?;
                let inactive = rt
                    .apply(RuntimeEvent::recording(RecordingState::Inactive))
                    .await;
                let shutdown = rt.finish().await;
                rt.release_capture();
                let recorded = sink.close().await;
                inactive?;
                shutdown?;
                recorded?;
                return Err(error);
            }
        };
        Ok(Self { scheduler, sink })
    }

    /// Runs until `stop` resolves (the caller's Ctrl-C wait) or the
    /// scheduler stops on its own, then tears everything down: scheduler,
    /// recording state, runtime shutdown, and the sink.
    pub async fn run_until(
        self,
        stop: impl Future<Output = Result<(), String>>,
        rt: &mut JobRuntime<'_>,
    ) -> Result<(), String> {
        let wait_result = tokio::select! {
            signal = stop => signal,
            () = wait_for_scheduler(&self.scheduler) => {
                Err("capture session stopped unexpectedly".into())
            }
        };
        let stopped = self.scheduler.stop().await;
        let inactive = rt
            .apply(RuntimeEvent::recording(RecordingState::Inactive))
            .await;
        let shutdown = rt.finish().await;
        rt.release_capture();
        let recorded = self.sink.close().await;
        stopped?;
        wait_result?;
        inactive?;
        shutdown?;
        recorded?;
        Ok(())
    }
}

async fn wait_for_scheduler(scheduler: &TelemetryScheduler) {
    while !scheduler.is_finished() {
        sleep(Duration::from_millis(100)).await;
    }
}

/// Records a scheduler-start failure as capture evidence: the capture
/// started, its knowledge context, the session error, and an immediate
/// stop. Shared by every job that opens a recording before it knows the
/// session will connect.
pub async fn record_capture_start_failure(
    sender: &crate::capture_format::CaptureSender,
    profile_name: &str,
    error: &str,
) -> Result<(), String> {
    let wallclock_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "wall clock timestamp exceeds supported range".to_string())?;
    let context = crate::capture_events::CaptureKnowledgeContext::current()?;
    for event in [
        CaptureEvent::capture_started(Some(wallclock_ms), Some(profile_name.into())),
        CaptureEvent::knowledge_context(context),
        CaptureEvent::session_error(error),
        CaptureEvent::SessionStopped { offset_us: 0 },
    ] {
        sender
            .send(event)
            .await
            .map_err(|_| "capture recorder is closed".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_only_advertised_signals_and_records_all_decisions() {
        let configured = crate::capture::profile("engine-baseline")
            .unwrap()
            .subscriptions()
            .unwrap();
        let plan = plan_capture(
            "engine-baseline",
            configured,
            &[
                SignalSupport {
                    semantic: "engine.rpm",
                    status: SignalSupportStatus::Supported,
                },
                SignalSupport {
                    semantic: "engine.maf",
                    status: SignalSupportStatus::Unsupported,
                },
            ],
        )
        .unwrap();

        assert_eq!(plan.scheduled().len(), 1);
        assert_eq!(plan.scheduled()[0].semantic(), "engine.rpm");
        assert_eq!(
            plan.scheduled()[0].interval(),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(plan.subscriptions().len(), 13);
        assert_eq!(plan.total_configured(), 13);
        assert_eq!(
            plan.subscriptions()[0].filter(),
            SubscriptionFilterOutcome::Scheduled
        );
        assert_eq!(
            plan.subscriptions()[1].filter(),
            SubscriptionFilterOutcome::Unsupported
        );
        assert_eq!(
            plan.subscriptions()[2].filter(),
            SubscriptionFilterOutcome::Unknown
        );
    }

    #[test]
    fn rejects_a_profile_with_nothing_the_adapter_supports() {
        let configured = crate::capture::profile("engine-baseline")
            .unwrap()
            .subscriptions()
            .unwrap();
        let error = plan_capture("engine-baseline", configured, &[]).unwrap_err();
        assert_eq!(
            error,
            "capture profile engine-baseline has no signals supported by the adapter"
        );
    }

    #[tokio::test]
    async fn startup_failure_is_preserved_in_the_capture_event_stream() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        record_capture_start_failure(&sender, "engine-baseline", "Carly setup timed out")
            .await
            .unwrap();

        assert!(matches!(
            receiver.recv().await,
            Some(CaptureEvent::CaptureStarted {
                profile: Some(profile),
                ..
            }) if profile == "engine-baseline"
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(CaptureEvent::KnowledgeContext { .. })
        ));
        assert_eq!(
            receiver.recv().await,
            Some(CaptureEvent::session_error("Carly setup timed out"))
        );
        assert_eq!(
            receiver.recv().await,
            Some(CaptureEvent::SessionStopped { offset_us: 0 })
        );
    }

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-capture-session-{label}-{}-{nonce}.jsonl",
            std::process::id()
        ))
    }

    async fn ready_runtime_state() -> (
        crate::runtime_actor::RuntimeClient,
        tokio::task::JoinHandle<()>,
        crate::runtime_state::RuntimeState,
    ) {
        let (runtime, task) = crate::runtime_actor::start();
        let mut state = crate::runtime_state::RuntimeState::default();
        crate::scheduler::apply_runtime_event(&runtime, &mut state, None, RuntimeEvent::InitializationCompleted)
            .await
            .unwrap();
        (runtime, task, state)
    }

    fn engine_rpm_plan() -> CapturePlan {
        let configured = crate::capture::profile("engine-baseline")
            .unwrap()
            .subscriptions()
            .unwrap();
        plan_capture(
            "engine-baseline",
            configured,
            &[SignalSupport {
                semantic: "engine.rpm",
                status: SignalSupportStatus::Supported,
            }],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_successful_session_records_start_then_stop_in_order() {
        let (runtime, task, mut state) = ready_runtime_state().await;
        let path = temp_path("success");
        let sink = CaptureSink::open(&path).unwrap();
        let mut rt = JobRuntime::new(&runtime, &mut state, Some(sink.sender().clone()));
        let plan = engine_rpm_plan();
        let routing = plan
            .scheduled()
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<ObservationPlan>>();
        let connect =
            crate::ble::start_scripted_session(crate::test_support::ScriptedExchange::new(Vec::<String>::new()), false);

        let session = CaptureSession::start(
            "engine-baseline",
            &plan,
            routing,
            connect,
            Arc::new(Mutex::new(TelemetryState::new(4).unwrap())),
            Arc::new(Mutex::new(AuditState::new(4).unwrap())),
            &mut rt,
            sink,
        )
        .await
        .unwrap();

        session
            .run_until(std::future::ready(Ok(())), &mut rt)
            .await
            .unwrap();

        let events = crate::capture_format::read(&path).unwrap().events;
        let started = position(&events, |event| {
            matches!(event, CaptureEvent::CaptureStarted { .. })
        });
        let knowledge = position(&events, |event| {
            matches!(event, CaptureEvent::KnowledgeContext { .. })
        });
        let initialized = position(&events, |event| {
            matches!(event, CaptureEvent::SessionInitialized)
        });
        let subscribed = position(&events, |event| {
            matches!(event, CaptureEvent::SubscriptionConfigured { .. })
        });
        assert!(started < knowledge, "{events:?}");
        assert!(knowledge < initialized, "{events:?}");
        assert!(initialized < subscribed, "{events:?}");
        std::fs::remove_file(path).unwrap();
        drop(runtime);
        task.abort();
    }

    fn position(
        events: &[CaptureEvent],
        matches: impl Fn(&CaptureEvent) -> bool,
    ) -> usize {
        events
            .iter()
            .position(matches)
            .unwrap_or_else(|| panic!("expected a matching event in {events:?}"))
    }

    #[tokio::test]
    async fn a_failed_connect_still_records_the_failure_and_closes_the_sink() {
        let (runtime, task, mut state) = ready_runtime_state().await;
        let path = temp_path("failure");
        let sink = CaptureSink::open(&path).unwrap();
        let mut rt = JobRuntime::new(&runtime, &mut state, Some(sink.sender().clone()));
        let plan = engine_rpm_plan();
        let routing = plan
            .scheduled()
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<ObservationPlan>>();
        let connect = std::future::ready(Err("adapter unavailable".to_string()));

        let error = match CaptureSession::start(
            "engine-baseline",
            &plan,
            routing,
            connect,
            Arc::new(Mutex::new(TelemetryState::new(4).unwrap())),
            Arc::new(Mutex::new(AuditState::new(4).unwrap())),
            &mut rt,
            sink,
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("expected the failed connect future to prevent the session starting"),
        };
        assert_eq!(error, "adapter unavailable");

        let events = crate::capture_format::read(&path).unwrap().events;
        let started = position(&events, |event| {
            matches!(event, CaptureEvent::CaptureStarted { .. })
        });
        let knowledge = position(&events, |event| {
            matches!(event, CaptureEvent::KnowledgeContext { .. })
        });
        let session_error = position(&events, |event| {
            matches!(event, CaptureEvent::SessionError { .. })
        });
        let stopped = position(&events, |event| {
            matches!(event, CaptureEvent::SessionStopped { .. })
        });
        assert!(started < knowledge, "{events:?}");
        assert!(knowledge < session_error, "{events:?}");
        assert!(session_error < stopped, "{events:?}");
        std::fs::remove_file(path).unwrap();
        drop(runtime);
        task.abort();
    }
}
