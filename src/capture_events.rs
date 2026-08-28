use crate::{
    diagnostic_job::{
        DiagnosticJob, JobPlan, JobResult, JobStatus, JobStep, SessionError, SkipReason, StepError,
        StepOutcome,
    },
    runtime_reducer::RuntimeEvent,
    runtime_state::RuntimeState,
    Transaction,
};

/// Relative capture time in integer microseconds.
pub type CaptureTimeUs = u64;

/// The monotonic offsets associated with one diagnostic read.
///
/// A duration is intentionally not stored: `finished_us - started_us` can be
/// derived by a consumer without creating another value that can disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadTiming {
    pub due_us: CaptureTimeUs,
    pub started_us: CaptureTimeUs,
    pub finished_us: CaptureTimeUs,
}

/// One responder/payload pair preserved for offline re-analysis. The
/// responder string is an adapter-level identity, not an asserted CAN ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponderEvidence {
    pub responder: Option<String>,
    pub payload: Vec<u8>,
}

impl ResponderEvidence {
    pub fn new(responder: Option<String>, payload: Vec<u8>) -> Result<Self, String> {
        if payload.is_empty() {
            return Err("responder evidence payload must not be empty".into());
        }
        Ok(Self { responder, payload })
    }
}

impl ReadTiming {
    pub const fn new(
        due_us: CaptureTimeUs,
        started_us: CaptureTimeUs,
        finished_us: CaptureTimeUs,
    ) -> Self {
        Self {
            due_us,
            started_us,
            finished_us,
        }
    }

    pub const fn is_monotonic(self) -> bool {
        self.due_us <= self.started_us && self.started_us <= self.finished_us
    }
}

/// The deterministic result of applying session support knowledge to a
/// requested capture subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionFilterOutcome {
    Scheduled,
    Unsupported,
    Unknown,
}

/// One requested subscription and its deterministic support-filter result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSubscription {
    semantic: String,
    requested_interval_us: CaptureTimeUs,
    filter: SubscriptionFilterOutcome,
}

impl CaptureSubscription {
    pub fn new(
        semantic: impl Into<String>,
        requested_interval_us: CaptureTimeUs,
        filter: SubscriptionFilterOutcome,
    ) -> Self {
        Self {
            semantic: semantic.into(),
            requested_interval_us,
            filter,
        }
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn requested_interval_us(&self) -> CaptureTimeUs {
        self.requested_interval_us
    }

    pub const fn filter(&self) -> SubscriptionFilterOutcome {
        self.filter
    }

    pub fn into_event(self) -> CaptureEvent {
        CaptureEvent::subscription_configured(
            self.semantic,
            self.requested_interval_us,
            self.filter,
        )
    }
}

/// Values are extensible beyond the numeric values currently exposed by the
/// vehicle catalog.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureValue {
    Number(f64),
    Boolean(bool),
    Enum(String),
    Text(String),
    Unavailable { reason: String },
}

/// Per-step status retained in diagnostic-job audit records.
///
/// `Skipped` is used for the deterministic suffix that follows cancellation;
/// a fatal session stops the remaining steps and records them as skipped only
/// when the executor emits their planned outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticJobStepStatus {
    Success,
    Recoverable,
    Fatal,
    Skipped,
}

impl DiagnosticJobStepStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Recoverable => "recoverable",
            Self::Fatal => "fatal",
            Self::Skipped => "skipped",
        }
    }
}

pub const MAX_DIAGNOSTIC_JOB_ID_LEN: usize = 64;
pub const MAX_DIAGNOSTIC_SOURCE_LEN: usize = 64;
pub const MAX_DIAGNOSTIC_ERROR_LEN: usize = 256;
pub const MAX_DIAGNOSTIC_MODE: u8 = 0x7f;

/// A closed vocabulary of passive observations from one capture session.
///
/// These values contain no transport handles and no operation that can issue
/// a diagnostic request. A writer can consume them in the order received.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    CaptureStarted {
        wallclock_ms: Option<u64>,
        profile: Option<String>,
    },
    SessionInitialized,
    SubscriptionConfigured {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
        filter: SubscriptionFilterOutcome,
    },
    SupportDiscovery {
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    },
    /// Preserves every normalized response before semantic selection. This
    /// event may accompany either a successful read or an explicit ambiguity.
    ResponsesObserved {
        semantic: String,
        request_payload: Vec<u8>,
        responses: Vec<ResponderEvidence>,
        selected_responder: Option<String>,
        selection_error: Option<String>,
    },
    ReadSucceeded {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
        due_us: CaptureTimeUs,
        started_us: CaptureTimeUs,
        finished_us: CaptureTimeUs,
        request_payload: Vec<u8>,
        response_payload: Vec<u8>,
        value: CaptureValue,
        unit: String,
        source: String,
        profile: String,
        decoder: String,
        provenance: String,
    },
    ReadFailed {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
        timing: Option<ReadTiming>,
        request_payload: Option<Vec<u8>>,
        error: String,
    },
    SlotsSkipped {
        semantic: String,
        count: u64,
        first_due_us: CaptureTimeUs,
        last_due_us: CaptureTimeUs,
    },
    SessionError {
        error: String,
    },
    /// An observational snapshot of one reducer transition. The recorder
    /// persists this value; it never applies `event` or derives `to` itself.
    RuntimeStateChanged {
        from: RuntimeState,
        to: RuntimeState,
        event: RuntimeEvent,
    },
    ShutdownRequested,
    SessionStopped {
        offset_us: CaptureTimeUs,
    },
    DiagnosticJobStarted {
        job_id: String,
        model_version: u16,
        step_count: u64,
    },
    DiagnosticJobStep {
        job_id: String,
        step_sequence: u64,
        mode: u8,
        source: Option<String>,
        status: DiagnosticJobStepStatus,
        error: Option<String>,
    },
    DiagnosticJobCompleted {
        job_id: String,
        status: JobStatus,
    },
    DiagnosticJobFailed {
        job_id: String,
        error: String,
    },
    DiagnosticJobCancelled {
        job_id: String,
    },
}

impl CaptureEvent {
    pub fn capture_started(wallclock_ms: Option<u64>, profile: Option<String>) -> Self {
        Self::CaptureStarted {
            wallclock_ms,
            profile,
        }
    }

    pub fn subscription_configured(
        semantic: impl Into<String>,
        requested_interval_us: CaptureTimeUs,
        filter: SubscriptionFilterOutcome,
    ) -> Self {
        Self::SubscriptionConfigured {
            semantic: semantic.into(),
            requested_interval_us,
            filter,
        }
    }

    pub fn support_discovery(request_payload: Vec<u8>, response_payload: Vec<u8>) -> Self {
        Self::support_discovery_with_responder(request_payload, None, response_payload)
    }

    pub fn support_discovery_with_responder(
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    ) -> Self {
        Self::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        }
    }

    pub fn responses_observed(
        semantic: impl Into<String>,
        request_payload: Vec<u8>,
        responses: Vec<ResponderEvidence>,
        selected_responder: Option<String>,
        selection_error: Option<String>,
    ) -> Result<Self, String> {
        if request_payload.is_empty() {
            return Err("observed response request payload must not be empty".into());
        }
        if responses.is_empty() {
            return Err("observed response list must not be empty".into());
        }
        if responses.iter().any(|response| response.payload.is_empty()) {
            return Err("observed response payload must not be empty".into());
        }
        Ok(Self::ResponsesObserved {
            semantic: semantic.into(),
            request_payload,
            responses,
            selected_responder,
            selection_error,
        })
    }

    /// Copy a completed read into an owned event without rebuilding its
    /// diagnostic request. The raw request and response bytes are preserved
    /// exactly as observed by the diagnostic layer.
    pub fn read_succeeded_from_transaction(
        transaction: &Transaction,
        requested_interval_us: CaptureTimeUs,
        timing: ReadTiming,
    ) -> Result<Self, String> {
        let metadata = crate::supported_signals()
            .iter()
            .find(|signal| signal.metadata().semantic == transaction.semantic())
            .map(|signal| signal.metadata())
            .ok_or_else(|| format!("unknown signal in transaction: {}", transaction.semantic()))?;

        Ok(Self::ReadSucceeded {
            semantic: transaction.semantic().into(),
            requested_interval_us,
            due_us: timing.due_us,
            started_us: timing.started_us,
            finished_us: timing.finished_us,
            request_payload: transaction.request().to_vec(),
            response_payload: transaction.response().to_vec(),
            value: CaptureValue::Number(transaction.value()),
            unit: transaction.unit().into(),
            source: transaction.source().into(),
            profile: transaction.profile().into(),
            decoder: metadata.decoder.into(),
            provenance: metadata.provenance.into(),
        })
    }

    /// Build an explicit failed-read event from an already prepared payload.
    /// Passing `None` represents a failure before a request was prepared.
    pub fn read_failed(
        semantic: impl Into<String>,
        requested_interval_us: CaptureTimeUs,
        timing: Option<ReadTiming>,
        request_payload: Option<Vec<u8>>,
        error: impl Into<String>,
    ) -> Self {
        Self::ReadFailed {
            semantic: semantic.into(),
            requested_interval_us,
            timing,
            request_payload,
            error: error.into(),
        }
    }

    pub fn slots_skipped(
        semantic: impl Into<String>,
        count: u64,
        first_due_us: CaptureTimeUs,
        last_due_us: CaptureTimeUs,
    ) -> Self {
        Self::SlotsSkipped {
            semantic: semantic.into(),
            count,
            first_due_us,
            last_due_us,
        }
    }

    pub fn session_error(error: impl Into<String>) -> Self {
        Self::SessionError {
            error: error.into(),
        }
    }

    /// Build an observational runtime-state transition for persistence.
    pub const fn runtime_state_changed(
        from: RuntimeState,
        to: RuntimeState,
        event: RuntimeEvent,
    ) -> Self {
        Self::RuntimeStateChanged { from, to, event }
    }

    /// Compatibility spelling for callers that call the observation a
    /// transition rather than a state change.
    pub const fn runtime_state_transition(
        from: RuntimeState,
        to: RuntimeState,
        event: RuntimeEvent,
    ) -> Self {
        Self::runtime_state_changed(from, to, event)
    }

    /// Record the immutable plan shape without retaining scope, transport, or
    /// any request payload.
    pub fn diagnostic_job_started(job: &DiagnosticJob) -> Self {
        Self::diagnostic_job_started_from_plan(&job.plan())
    }

    pub fn diagnostic_job_started_from_plan(plan: &JobPlan) -> Self {
        Self::DiagnosticJobStarted {
            job_id: plan.id().to_string(),
            model_version: plan.model_version(),
            step_count: plan.steps().len() as u64,
        }
    }

    /// Record one bounded semantic step outcome. No opaque evidence reference
    /// or raw request is copied into the capture event.
    pub fn diagnostic_job_step(
        job_id: impl Into<String>,
        step_sequence: u64,
        mode: u8,
        source: Option<String>,
        status: DiagnosticJobStepStatus,
        error: Option<String>,
    ) -> Result<Self, String> {
        let job_id = job_id.into();
        validate_diagnostic_job_id(&job_id)?;
        validate_diagnostic_mode(mode)?;
        validate_diagnostic_source(source.as_deref())?;
        validate_diagnostic_error(error.as_deref())?;
        Ok(Self::DiagnosticJobStep {
            job_id,
            step_sequence,
            mode,
            source,
            status,
            error,
        })
    }

    /// Convert a closed job-model outcome into privacy-safe capture metadata.
    /// Evidence references stay in the executor/audit layer and are not
    /// serialized here.
    pub fn diagnostic_job_step_outcome(
        job: &DiagnosticJob,
        step: &JobStep,
        mode: u8,
        source: Option<String>,
        outcome: &StepOutcome,
    ) -> Result<Self, String> {
        let (status, error) = match outcome {
            StepOutcome::Succeeded { .. } => (DiagnosticJobStepStatus::Success, None),
            StepOutcome::RecoverableError { error, .. } => (
                DiagnosticJobStepStatus::Recoverable,
                Some(step_error_name(error).into()),
            ),
            StepOutcome::NotRun {
                reason: SkipReason::SessionFailed,
            } => (
                DiagnosticJobStepStatus::Fatal,
                Some("session_failed".into()),
            ),
            StepOutcome::NotRun {
                reason: SkipReason::Cancelled,
            } => (DiagnosticJobStepStatus::Skipped, Some("cancelled".into())),
        };
        Self::diagnostic_job_step(
            job.id().to_string(),
            step.sequence() as u64,
            mode,
            source,
            status,
            error,
        )
    }

    pub fn diagnostic_job_completed(result: &JobResult) -> Result<Self, String> {
        match result.status() {
            JobStatus::Completed | JobStatus::CompletedWithErrors => {
                Ok(Self::DiagnosticJobCompleted {
                    job_id: result.id().to_string(),
                    status: result.status(),
                })
            }
            status => Err(format!("diagnostic job result is not completed: {status}")),
        }
    }

    pub fn diagnostic_job_terminal(result: &JobResult) -> Result<Self, String> {
        match result.status() {
            JobStatus::Completed | JobStatus::CompletedWithErrors => {
                Self::diagnostic_job_completed(result)
            }
            JobStatus::Failed => Ok(Self::DiagnosticJobFailed {
                job_id: result.id().to_string(),
                error: result
                    .session_error()
                    .map(session_error_name)
                    .unwrap_or("session_failed")
                    .into(),
            }),
            JobStatus::Cancelled => Ok(Self::DiagnosticJobCancelled {
                job_id: result.id().to_string(),
            }),
            status => Err(format!("diagnostic job result is not terminal: {status}")),
        }
    }

    pub fn diagnostic_job_failed(job_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self::DiagnosticJobFailed {
            job_id: job_id.into(),
            error: error.into(),
        }
    }

    pub fn diagnostic_job_cancelled(job_id: impl Into<String>) -> Self {
        Self::DiagnosticJobCancelled {
            job_id: job_id.into(),
        }
    }
}

pub(crate) fn validate_diagnostic_job_id(value: &str) -> Result<(), String> {
    validate_diagnostic_text("job id", value, MAX_DIAGNOSTIC_JOB_ID_LEN)
}

pub(crate) fn validate_diagnostic_source(value: Option<&str>) -> Result<(), String> {
    value.map_or(Ok(()), |value| {
        validate_diagnostic_text("source", value, MAX_DIAGNOSTIC_SOURCE_LEN)
    })
}

pub(crate) fn validate_diagnostic_error(value: Option<&str>) -> Result<(), String> {
    value.map_or(Ok(()), |value| {
        validate_diagnostic_text("error", value, MAX_DIAGNOSTIC_ERROR_LEN)
    })
}

fn validate_diagnostic_text(label: &str, value: &str, max_len: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("diagnostic {label} must not be empty"));
    }
    if value.len() > max_len {
        return Err(format!("diagnostic {label} exceeds {max_len} bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "diagnostic {label} must not contain control characters"
        ));
    }
    Ok(())
}

pub(crate) fn validate_diagnostic_mode(mode: u8) -> Result<(), String> {
    (1..=MAX_DIAGNOSTIC_MODE)
        .contains(&mode)
        .then_some(())
        .ok_or_else(|| {
            format!("diagnostic mode must be between 0x01 and 0x{MAX_DIAGNOSTIC_MODE:02X}")
        })
}

fn step_error_name(error: &StepError) -> &'static str {
    match error {
        StepError::Unsupported => "unsupported",
        StepError::NegativeResponse => "negative_response",
        StepError::Timeout => "timeout",
        StepError::MalformedEvidence => "malformed_evidence",
        StepError::Other(_) => "other",
    }
}

fn session_error_name(error: &SessionError) -> &'static str {
    match error {
        SessionError::Disconnected => "disconnected",
        SessionError::Transport => "transport",
        SessionError::Fault => "fault",
        SessionError::Other(_) => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_job::{
        DiagnosticScope, EvidenceRef, JobResult, KnownResponder, StepEvidence, Termination,
    };
    use crate::prepare_read;

    #[test]
    fn maps_transaction_without_changing_diagnostic_payloads_or_timing() {
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("user", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let event = CaptureEvent::read_succeeded_from_transaction(
            &transaction,
            4_294_967_297,
            ReadTiming::new(4_000_000_001, 4_000_000_123, 4_000_001_987),
        )
        .unwrap();

        assert_eq!(
            event,
            CaptureEvent::ReadSucceeded {
                semantic: "engine.rpm".into(),
                requested_interval_us: 4_294_967_297,
                due_us: 4_000_000_001,
                started_us: 4_000_000_123,
                finished_us: 4_000_001_987,
                request_payload: vec![0x01, 0x0c],
                response_payload: vec![0x41, 0x0c, 0x1a, 0xf8],
                value: CaptureValue::Number(1726.0),
                unit: "rpm".into(),
                source: "user".into(),
                profile: "obd2-v1".into(),
                decoder: "((A * 256) + B) / 4".into(),
                provenance: "SAE J1979 Mode 01 PID 0C".into(),
            }
        );
    }

    #[test]
    fn failed_reads_are_explicit_and_keep_known_timing_and_request() {
        let timing = ReadTiming::new(11, 17, 23);
        assert_eq!(
            CaptureEvent::read_failed(
                "engine.rpm",
                250_000,
                Some(timing),
                Some(vec![0x01, 0x0c]),
                "timeout",
            ),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: Some(timing),
                request_payload: Some(vec![0x01, 0x0c]),
                error: "timeout".into(),
            }
        );
        assert_eq!(
            CaptureEvent::read_failed("engine.rpm", 250_000, None, None, "not prepared"),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: None,
                request_payload: None,
                error: "not prepared".into(),
            }
        );
    }

    #[test]
    fn timing_is_integer_microseconds_and_monotonic() {
        let timing = ReadTiming::new(u64::MAX - 2, u64::MAX - 1, u64::MAX);
        assert!(timing.is_monotonic());
        assert_eq!(timing.due_us, u64::MAX - 2);
        assert_eq!(timing.started_us, u64::MAX - 1);
        assert_eq!(timing.finished_us, u64::MAX);
    }

    #[test]
    fn event_model_only_maps_existing_read_requests() {
        let read = prepare_read("engine.rpm").unwrap();
        assert_eq!(read.bytes(), [0x01, 0x0c]);
        assert!(prepare_read("dtc.clear").is_err());
    }

    #[test]
    fn subscription_configuration_keeps_support_filter_outcome() {
        for outcome in [
            SubscriptionFilterOutcome::Scheduled,
            SubscriptionFilterOutcome::Unsupported,
            SubscriptionFilterOutcome::Unknown,
        ] {
            assert_eq!(
                CaptureEvent::subscription_configured("engine.rpm", 250_000, outcome),
                CaptureEvent::SubscriptionConfigured {
                    semantic: "engine.rpm".into(),
                    requested_interval_us: 250_000,
                    filter: outcome,
                }
            );
        }
    }

    #[test]
    fn support_discovery_preserves_optional_responder() {
        assert_eq!(
            CaptureEvent::support_discovery_with_responder(
                vec![0x01, 0x00],
                Some("7E8".into()),
                vec![0x41, 0x00, 0x80, 0x00, 0x00, 0x01],
            ),
            CaptureEvent::SupportDiscovery {
                request_payload: vec![0x01, 0x00],
                responder: Some("7E8".into()),
                response_payload: vec![0x41, 0x00, 0x80, 0x00, 0x00, 0x01],
            }
        );
        assert_eq!(
            CaptureEvent::support_discovery(vec![0x01, 0x00], vec![0x41, 0x00]),
            CaptureEvent::SupportDiscovery {
                request_payload: vec![0x01, 0x00],
                responder: None,
                response_payload: vec![0x41, 0x00],
            }
        );
    }

    #[test]
    fn responses_observed_rejects_empty_request_or_evidence() {
        let response = ResponderEvidence::new(Some("7E8".into()), vec![0x41, 0x0c]).unwrap();
        assert!(CaptureEvent::responses_observed(
            "engine.rpm",
            Vec::new(),
            vec![response.clone()],
            None,
            None,
        )
        .is_err());
        assert!(CaptureEvent::responses_observed(
            "engine.rpm",
            vec![0x01, 0x0c],
            Vec::new(),
            None,
            None,
        )
        .is_err());
        assert!(CaptureEvent::responses_observed(
            "engine.rpm",
            vec![0x01, 0x0c],
            vec![ResponderEvidence {
                responder: Some("7E8".into()),
                payload: Vec::new(),
            }],
            None,
            None,
        )
        .is_err());
        assert!(ResponderEvidence::new(Some("7E8".into()), Vec::new()).is_err());
    }

    #[test]
    fn runtime_state_change_is_typed_observational_data_and_has_no_vin() {
        let from = RuntimeState::default();
        let to = RuntimeState::new(
            crate::runtime_state::Phase::Ready,
            crate::runtime_state::Activity::Idle,
            crate::runtime_state::RuntimeContext::default(),
        );
        let event =
            CaptureEvent::runtime_state_changed(from, to, RuntimeEvent::InitializationCompleted);

        assert_eq!(
            event,
            CaptureEvent::RuntimeStateChanged {
                from,
                to,
                event: RuntimeEvent::InitializationCompleted,
            }
        );
        assert!(!from.serialize().contains("VIN"));
        assert!(!to.serialize().contains("VIN"));
    }

    #[test]
    fn diagnostic_job_events_are_privacy_safe_and_map_closed_outcomes() {
        let scope =
            DiagnosticScope::known_obd_responders([KnownResponder::new("7E8").unwrap()]).unwrap();
        let job = DiagnosticJob::dtc_scan(scope);
        assert_eq!(
            CaptureEvent::diagnostic_job_started(&job),
            CaptureEvent::DiagnosticJobStarted {
                job_id: "dtc.scan".into(),
                model_version: 1,
                step_count: 1,
            }
        );
        let plan = job.plan();
        let step = &plan.steps()[0];
        let evidence = StepEvidence::new(EvidenceRef::new("opaque-ref").unwrap(), None);
        let event = CaptureEvent::diagnostic_job_step_outcome(
            &job,
            step,
            0x03,
            Some("7E8".into()),
            &StepOutcome::succeeded([evidence]),
        )
        .unwrap();
        assert_eq!(
            event,
            CaptureEvent::DiagnosticJobStep {
                job_id: "dtc.scan".into(),
                step_sequence: 0,
                mode: 0x03,
                source: Some("7E8".into()),
                status: DiagnosticJobStepStatus::Success,
                error: None,
            }
        );
        assert!(!format!("{event:?}").contains("opaque-ref"));
    }

    #[test]
    fn diagnostic_job_step_validation_keeps_identifiers_bounded() {
        assert!(CaptureEvent::diagnostic_job_step(
            "dtc.scan",
            0,
            0,
            None,
            DiagnosticJobStepStatus::Success,
            None,
        )
        .is_err());
        assert!(CaptureEvent::diagnostic_job_step(
            "dtc.scan",
            0,
            3,
            Some("source\nleak".into()),
            DiagnosticJobStepStatus::Success,
            None,
        )
        .is_err());
        assert!(CaptureEvent::diagnostic_job_step(
            "dtc.scan",
            0,
            3,
            Some("x".repeat(MAX_DIAGNOSTIC_SOURCE_LEN + 1)),
            DiagnosticJobStepStatus::Success,
            None,
        )
        .is_err());
    }

    #[test]
    fn diagnostic_job_terminal_event_follows_immutable_result_status() {
        let job = DiagnosticJob::dtc_scan(DiagnosticScope::vehicle_wide());
        let result = JobResult::from_outcomes(
            &job.plan(),
            [StepOutcome::not_run(SkipReason::Cancelled)],
            Termination::Cancelled,
        )
        .unwrap();
        assert_eq!(
            CaptureEvent::diagnostic_job_terminal(&result).unwrap(),
            CaptureEvent::DiagnosticJobCancelled {
                job_id: "dtc.scan".into(),
            }
        );
    }
}
