//! Deterministic, transport-neutral diagnostic jobs.
//!
//! A job is a bounded semantic request.  This module deliberately contains no
//! transport handle, protocol payload builder, retry loop, or executor.  Raw
//! request/response data stays in the capture/audit layer and is referred to
//! here by an opaque [`EvidenceRef`].

use std::{fmt, str::FromStr};

use crate::ea189::Ea189DpfProbe;

/// Version of the stable job vocabulary and its deterministic plan shape.
pub const JOB_MODEL_VERSION: u16 = 1;

/// Stable public identifier for a diagnostic job.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobId {
    /// Read the bounded set of DTC information represented by this job.
    DtcScan,
    /// Read the closed, candidate-only EA189 DPF probe set.
    Ea189DpfProbe,
}

impl JobId {
    pub const DTC_SCAN: Self = Self::DtcScan;
    pub const EA189_DPF_PROBE: Self = Self::Ea189DpfProbe;

    pub const fn id(self) -> &'static str {
        match self {
            Self::DtcScan => "dtc.scan",
            Self::Ea189DpfProbe => "ea189.dpf.probe",
        }
    }

    pub const fn as_str(self) -> &'static str {
        self.id()
    }

    pub fn parse(value: &str) -> Result<Self, JobError> {
        match value {
            "dtc.scan" => Ok(Self::DtcScan),
            "ea189.dpf.probe" => Ok(Self::Ea189DpfProbe),
            value => Err(JobError::UnknownJobId(value.into())),
        }
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for JobId {
    type Err = JobError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for JobId {
    type Error = JobError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// Closed semantic vocabulary for currently supported jobs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobKind {
    DtcScan,
    Ea189DpfProbe,
}

impl JobKind {
    pub const fn job_id(self) -> JobId {
        match self {
            Self::DtcScan => JobId::DtcScan,
            Self::Ea189DpfProbe => JobId::Ea189DpfProbe,
        }
    }

    pub const fn id(self) -> &'static str {
        self.job_id().id()
    }

    pub const fn as_str(self) -> &'static str {
        self.id()
    }

    pub fn parse(value: &str) -> Result<Self, JobError> {
        Ok(JobId::parse(value)?.into())
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

impl From<JobKind> for JobId {
    fn from(kind: JobKind) -> Self {
        kind.job_id()
    }
}

impl From<JobId> for JobKind {
    fn from(id: JobId) -> Self {
        match id {
            JobId::DtcScan => Self::DtcScan,
            JobId::Ea189DpfProbe => Self::Ea189DpfProbe,
        }
    }
}

/// An explicitly known ECU role.  The role is metadata; it is not an address
/// range and cannot make a plan scan for ECUs.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EcuRole {
    Engine,
    Transmission,
    Gateway,
    Unknown,
    VendorSpecific(String),
}

impl EcuRole {
    pub fn vendor_specific(value: impl Into<String>) -> Result<Self, ScopeError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ScopeError::EmptyRole);
        }
        Ok(Self::VendorSpecific(value))
    }

    pub const fn as_str(&self) -> &str {
        match self {
            Self::Engine => "engine",
            Self::Transmission => "transmission",
            Self::Gateway => "gateway",
            Self::Unknown => "unknown",
            Self::VendorSpecific(value) => value.as_str(),
        }
    }
}

/// A non-empty opaque identifier for an evidenced request target.
///
/// This is intentionally not a CAN/UDS address type.  A caller can only put a
/// known target into a scope, never an address range or an implicit scan.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KnownTarget(String);

impl KnownTarget {
    pub fn new(value: impl Into<String>) -> Result<Self, ScopeError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ScopeError::EmptyTarget);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-empty opaque identifier for one already-known OBD responder.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KnownResponder(String);

impl KnownResponder {
    pub fn new(value: impl Into<String>) -> Result<Self, ScopeError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ScopeError::EmptyResponder);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A closed scope vocabulary.  Every address-bearing member is explicitly
/// named `Known`; there is no `all_ecus` or address-discovery variant.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticScope {
    KnownEcu { role: EcuRole, target: KnownTarget },
    KnownObdResponders(Vec<KnownResponder>),
    VehicleWide,
}

impl DiagnosticScope {
    pub fn known_ecu(role: EcuRole, target: KnownTarget) -> Self {
        Self::KnownEcu { role, target }
    }

    pub fn known_obd_responders<I>(responders: I) -> Result<Self, ScopeError>
    where
        I: IntoIterator<Item = KnownResponder>,
    {
        let mut responders: Vec<_> = responders.into_iter().collect();
        if responders.is_empty() {
            return Err(ScopeError::EmptyResponderSet);
        }
        responders.sort();
        if responders.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ScopeError::DuplicateResponder);
        }
        Ok(Self::KnownObdResponders(responders))
    }

    pub const fn vehicle_wide() -> Self {
        Self::VehicleWide
    }

    pub fn targets(&self) -> Vec<JobTarget> {
        match self {
            Self::KnownEcu { role, target } => {
                vec![JobTarget::KnownEcu {
                    role: role.clone(),
                    target: target.clone(),
                }]
            }
            Self::KnownObdResponders(responders) => responders
                .iter()
                .cloned()
                .map(JobTarget::KnownObdResponder)
                .collect(),
            Self::VehicleWide => vec![JobTarget::Vehicle],
        }
    }

    pub fn responders(&self) -> &[KnownResponder] {
        match self {
            Self::KnownObdResponders(responders) => responders,
            Self::KnownEcu { .. } | Self::VehicleWide => &[],
        }
    }
}

/// A concrete target selected by a validated scope.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobTarget {
    KnownEcu { role: EcuRole, target: KnownTarget },
    KnownObdResponder(KnownResponder),
    Vehicle,
}

/// Safety classification attached to a job and plan.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SafetyClass {
    ReadOnly,
}

/// A semantic diagnostic job request.  Its private fields make the request
/// immutable after construction and leave no slot for raw protocol bytes.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiagnosticJob {
    id: JobId,
    kind: JobKind,
    scope: DiagnosticScope,
    safety: SafetyClass,
}

impl DiagnosticJob {
    pub fn new(kind: JobKind, scope: DiagnosticScope) -> Self {
        Self {
            id: kind.job_id(),
            kind,
            scope,
            safety: SafetyClass::ReadOnly,
        }
    }

    pub fn try_new(kind: JobKind, scope: DiagnosticScope) -> Result<Self, JobError> {
        if matches!(kind, JobKind::Ea189DpfProbe)
            && !matches!(
                &scope,
                DiagnosticScope::KnownEcu {
                    role: EcuRole::Engine,
                    ..
                }
            )
        {
            return Err(JobError::RequiresKnownEngineEcu);
        }
        Ok(Self::new(kind, scope))
    }

    pub fn from_id(id: &str, scope: DiagnosticScope) -> Result<Self, JobError> {
        Self::try_new(JobId::parse(id)?.into(), scope)
    }

    pub fn dtc_scan(scope: DiagnosticScope) -> Self {
        Self::new(JobKind::DtcScan, scope)
    }

    /// Build the bounded EA189 DPF probe against one already-known engine
    /// ECU. The constructor cannot create a vehicle-wide or functional scan.
    pub fn ea189_dpf_probe(target: KnownTarget) -> Self {
        Self::new(
            JobKind::Ea189DpfProbe,
            DiagnosticScope::known_ecu(EcuRole::Engine, target),
        )
    }

    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn kind(&self) -> JobKind {
        self.kind
    }

    pub fn scope(&self) -> &DiagnosticScope {
        &self.scope
    }

    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    pub fn plan(&self) -> JobPlan {
        JobPlan::for_job(self)
    }
}

/// The only step operation currently implemented by `dtc.scan`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobStepKind {
    ReadDtc,
    ReadEa189Dpf(Ea189DpfProbe),
}

/// One planned, ordered semantic read.  The sequence is zero-based and is
/// assigned by [`JobPlan`], not by a transport executor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JobStep {
    sequence: usize,
    kind: JobStepKind,
    target: JobTarget,
}

impl JobStep {
    fn new(sequence: usize, kind: JobStepKind, target: JobTarget) -> Self {
        Self {
            sequence,
            kind,
            target,
        }
    }

    pub const fn sequence(&self) -> usize {
        self.sequence
    }

    pub const fn kind(&self) -> JobStepKind {
        self.kind
    }

    pub const fn dpf_probe(&self) -> Option<Ea189DpfProbe> {
        match self.kind {
            JobStepKind::ReadEa189Dpf(probe) => Some(probe),
            JobStepKind::ReadDtc => None,
        }
    }

    pub fn target(&self) -> &JobTarget {
        &self.target
    }
}

/// A fully expanded, deterministic and bounded execution plan.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JobPlan {
    model_version: u16,
    id: JobId,
    kind: JobKind,
    scope: DiagnosticScope,
    safety: SafetyClass,
    steps: Vec<JobStep>,
}

impl JobPlan {
    pub fn for_job(job: &DiagnosticJob) -> Self {
        let steps = match job.kind {
            JobKind::DtcScan => job
                .scope
                .targets()
                .into_iter()
                .enumerate()
                .map(|(sequence, target)| JobStep::new(sequence, JobStepKind::ReadDtc, target))
                .collect(),
            JobKind::Ea189DpfProbe => match &job.scope {
                DiagnosticScope::KnownEcu {
                    role: EcuRole::Engine,
                    target,
                } => Ea189DpfProbe::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, probe)| {
                        JobStep::new(
                            sequence,
                            JobStepKind::ReadEa189Dpf(probe),
                            JobTarget::KnownEcu {
                                role: EcuRole::Engine,
                                target: target.clone(),
                            },
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            },
        };
        Self {
            model_version: JOB_MODEL_VERSION,
            id: job.id,
            kind: job.kind,
            scope: job.scope.clone(),
            safety: job.safety,
            steps,
        }
    }

    pub const fn model_version(&self) -> u16 {
        self.model_version
    }

    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn kind(&self) -> JobKind {
        self.kind
    }

    pub fn scope(&self) -> &DiagnosticScope {
        &self.scope
    }

    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    pub fn steps(&self) -> &[JobStep] {
        &self.steps
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Opaque link to capture/audit data.  It carries no raw protocol payload and
/// therefore cannot be used to construct a mutating request.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EvidenceRef(String);

impl EvidenceRef {
    pub fn new(value: impl Into<String>) -> Result<Self, EvidenceError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(EvidenceError::EmptyReference);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Evidence retained for one successful or recoverably failed step.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StepEvidence {
    reference: EvidenceRef,
    responder: Option<KnownResponder>,
}

impl StepEvidence {
    pub fn new(reference: EvidenceRef, responder: Option<KnownResponder>) -> Self {
        Self {
            reference,
            responder,
        }
    }

    pub fn reference(&self) -> &EvidenceRef {
        &self.reference
    }

    pub fn responder(&self) -> Option<&KnownResponder> {
        self.responder.as_ref()
    }
}

/// A bounded, recoverable failure belonging to one planned step.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StepError {
    Unsupported,
    NegativeResponse,
    Timeout,
    MalformedEvidence,
    Other(String),
}

impl StepError {
    pub fn other(value: impl Into<String>) -> Self {
        Self::Other(value.into())
    }
}

/// A fatal session-level failure.  It stops all remaining steps.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SessionError {
    Disconnected,
    Transport,
    Fault,
    Other(String),
}

impl SessionError {
    pub fn other(value: impl Into<String>) -> Self {
        Self::Other(value.into())
    }
}

/// Why a planned step did not run after a terminal job condition.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SkipReason {
    SessionFailed,
    Cancelled,
}

/// Outcome of one planned step.  Evidence is kept alongside recoverable
/// errors so a negative/invalid read cannot discard earlier observations.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StepOutcome {
    Succeeded {
        evidence: Vec<StepEvidence>,
    },
    RecoverableError {
        error: StepError,
        evidence: Vec<StepEvidence>,
    },
    NotRun {
        reason: SkipReason,
    },
}

impl StepOutcome {
    pub fn succeeded(evidence: impl IntoIterator<Item = StepEvidence>) -> Self {
        Self::Succeeded {
            evidence: evidence.into_iter().collect(),
        }
    }

    pub fn recoverable(error: StepError, evidence: impl IntoIterator<Item = StepEvidence>) -> Self {
        Self::RecoverableError {
            error,
            evidence: evidence.into_iter().collect(),
        }
    }

    pub const fn not_run(reason: SkipReason) -> Self {
        Self::NotRun { reason }
    }

    pub fn evidence(&self) -> &[StepEvidence] {
        match self {
            Self::Succeeded { evidence } | Self::RecoverableError { evidence, .. } => evidence,
            Self::NotRun { .. } => &[],
        }
    }
}

/// Terminal condition supplied by the executor when it turns observations
/// into an immutable result.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Termination {
    Completed,
    SessionFailed(SessionError),
    Cancelled,
}

/// Stable status of a diagnostic job.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobStatus {
    Planned,
    Running,
    Completed,
    CompletedWithErrors,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Immutable result of executing every step prefix represented by a plan.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JobResult {
    model_version: u16,
    id: JobId,
    kind: JobKind,
    status: JobStatus,
    steps: Vec<JobStepResult>,
    session_error: Option<SessionError>,
}

impl JobResult {
    pub fn from_outcomes(
        plan: &JobPlan,
        outcomes: impl IntoIterator<Item = StepOutcome>,
        termination: Termination,
    ) -> Result<Self, ResultError> {
        let outcomes: Vec<_> = outcomes.into_iter().collect();
        if outcomes.len() != plan.steps.len() {
            return Err(ResultError::StepCountMismatch {
                expected: plan.steps.len(),
                actual: outcomes.len(),
            });
        }

        let first_not_run = outcomes
            .iter()
            .position(|outcome| matches!(outcome, StepOutcome::NotRun { .. }));
        match &termination {
            Termination::Completed if first_not_run.is_some() => {
                return Err(ResultError::IncompleteCompletedJob);
            }
            Termination::SessionFailed(_) => {
                validate_not_run_suffix(&outcomes, SkipReason::SessionFailed)?;
            }
            Termination::Cancelled => {
                validate_not_run_suffix(&outcomes, SkipReason::Cancelled)?;
            }
            Termination::Completed => {}
        }

        let status = match termination {
            Termination::Completed => {
                if outcomes
                    .iter()
                    .any(|outcome| matches!(outcome, StepOutcome::RecoverableError { .. }))
                {
                    JobStatus::CompletedWithErrors
                } else {
                    JobStatus::Completed
                }
            }
            Termination::SessionFailed(error) => {
                return Ok(Self {
                    model_version: plan.model_version,
                    id: plan.id,
                    kind: plan.kind,
                    status: JobStatus::Failed,
                    steps: plan
                        .steps
                        .iter()
                        .cloned()
                        .zip(outcomes)
                        .map(|(step, outcome)| JobStepResult { step, outcome })
                        .collect(),
                    session_error: Some(error),
                });
            }
            Termination::Cancelled => JobStatus::Cancelled,
        };

        Ok(Self {
            model_version: plan.model_version,
            id: plan.id,
            kind: plan.kind,
            status,
            steps: plan
                .steps
                .iter()
                .cloned()
                .zip(outcomes)
                .map(|(step, outcome)| JobStepResult { step, outcome })
                .collect(),
            session_error: None,
        })
    }

    pub const fn model_version(&self) -> u16 {
        self.model_version
    }

    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn kind(&self) -> JobKind {
        self.kind
    }

    pub const fn status(&self) -> JobStatus {
        self.status
    }

    pub fn steps(&self) -> &[JobStepResult] {
        &self.steps
    }

    pub fn session_error(&self) -> Option<&SessionError> {
        self.session_error.as_ref()
    }
}

fn validate_not_run_suffix(
    outcomes: &[StepOutcome],
    reason: SkipReason,
) -> Result<(), ResultError> {
    let Some(first_not_run) = outcomes
        .iter()
        .position(|outcome| matches!(outcome, StepOutcome::NotRun { .. }))
    else {
        return Err(ResultError::TerminalWithoutSkippedSteps);
    };
    if outcomes[..first_not_run]
        .iter()
        .any(|outcome| matches!(outcome, StepOutcome::NotRun { .. }))
        || outcomes[first_not_run..]
            .iter()
            .any(|outcome| *outcome != StepOutcome::NotRun { reason })
    {
        return Err(ResultError::InvalidSkippedSteps);
    }
    Ok(())
}

/// Result paired with its planned step, retaining stable ordering.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JobStepResult {
    step: JobStep,
    outcome: StepOutcome,
}

impl JobStepResult {
    pub fn step(&self) -> &JobStep {
        &self.step
    }

    pub fn outcome(&self) -> &StepOutcome {
        &self.outcome
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum JobError {
    UnknownJobId(String),
    RequiresKnownEngineEcu,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScopeError {
    EmptyRole,
    EmptyTarget,
    EmptyResponder,
    EmptyResponderSet,
    DuplicateResponder,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EvidenceError {
    EmptyReference,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResultError {
    StepCountMismatch { expected: usize, actual: usize },
    IncompleteCompletedJob,
    TerminalWithoutSkippedSteps,
    InvalidSkippedSteps,
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownJobId(value) => {
                write!(formatter, "unsupported diagnostic job id: {value}")
            }
            Self::RequiresKnownEngineEcu => {
                formatter.write_str("EA189 DPF probe requires a known engine ECU scope")
            }
        }
    }
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyRole => "ECU role must not be empty",
            Self::EmptyTarget => "known target must not be empty",
            Self::EmptyResponder => "known responder must not be empty",
            Self::EmptyResponderSet => "known responder scope must not be empty",
            Self::DuplicateResponder => "known responder scope must not contain duplicates",
        })
    }
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("evidence reference must not be empty")
    }
}

impl fmt::Display for ResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StepCountMismatch { expected, actual } => {
                write!(
                    formatter,
                    "job result has {actual} steps; expected {expected}"
                )
            }
            Self::IncompleteCompletedJob => {
                formatter.write_str("completed job cannot contain skipped steps")
            }
            Self::TerminalWithoutSkippedSteps => {
                formatter.write_str("terminal job result must identify skipped steps")
            }
            Self::InvalidSkippedSteps => {
                formatter.write_str("skipped steps must form one matching suffix")
            }
        }
    }
}

impl std::error::Error for JobError {}
impl std::error::Error for ScopeError {}
impl std::error::Error for EvidenceError {}
impl std::error::Error for ResultError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn responders(values: &[&str]) -> Vec<KnownResponder> {
        values
            .iter()
            .map(|value| KnownResponder::new(*value).unwrap())
            .collect()
    }

    fn evidence(value: &str) -> StepEvidence {
        StepEvidence::new(EvidenceRef::new(value).unwrap(), None)
    }

    #[test]
    fn only_supported_job_id_has_stable_serialization() {
        assert_eq!(JobId::DtcScan.to_string(), "dtc.scan");
        assert_eq!(JobId::parse("dtc.scan"), Ok(JobId::DtcScan));
        assert_eq!(JobId::Ea189DpfProbe.to_string(), "ea189.dpf.probe");
        assert_eq!(JobId::parse("ea189.dpf.probe"), Ok(JobId::Ea189DpfProbe));
        assert_eq!(
            JobId::parse("dtc.clear"),
            Err(JobError::UnknownJobId("dtc.clear".into()))
        );
    }

    #[test]
    fn ea189_dpf_probe_plan_is_bounded_to_the_known_engine() {
        let target = KnownTarget::new("validated-engine").unwrap();
        let plan = DiagnosticJob::ea189_dpf_probe(target).plan();
        assert_eq!(plan.id(), JobId::Ea189DpfProbe);
        assert_eq!(plan.steps().len(), 7);
        for (sequence, step) in plan.steps().iter().enumerate() {
            assert_eq!(step.sequence(), sequence);
            assert!(matches!(step.kind(), JobStepKind::ReadEa189Dpf(_)));
            assert!(matches!(
                step.target(),
                JobTarget::KnownEcu {
                    role: EcuRole::Engine,
                    target: KnownTarget(value),
                } if value == "validated-engine"
            ));
        }
    }

    #[test]
    fn ea189_dpf_probe_job_rejects_non_engine_scopes() {
        assert_eq!(
            DiagnosticJob::try_new(JobKind::Ea189DpfProbe, DiagnosticScope::vehicle_wide(),),
            Err(JobError::RequiresKnownEngineEcu)
        );
    }

    #[test]
    fn responder_scope_is_sorted_and_not_a_scan() {
        let scope = DiagnosticScope::known_obd_responders(responders(&["ecu-b", "ecu-a"])).unwrap();
        let plan = DiagnosticJob::dtc_scan(scope).plan();
        assert_eq!(plan.steps().len(), 2);
        assert_eq!(plan.steps()[0].sequence(), 0);
        assert_eq!(plan.steps()[1].sequence(), 1);
        assert!(matches!(
            plan.steps()[0].target(),
            JobTarget::KnownObdResponder(KnownResponder(value)) if value == "ecu-a"
        ));
        assert!(matches!(
            plan.steps()[1].target(),
            JobTarget::KnownObdResponder(KnownResponder(value)) if value == "ecu-b"
        ));
    }

    #[test]
    fn dtc_scan_plan_has_multiple_reads_without_read_activity() {
        let scope = DiagnosticScope::known_obd_responders(responders(&["one", "two"])).unwrap();
        let job = DiagnosticJob::dtc_scan(scope);
        let plan = job.plan();
        assert_eq!(job.kind(), JobKind::DtcScan);
        assert!(plan
            .steps()
            .iter()
            .all(|step| step.kind() == JobStepKind::ReadDtc));
    }

    #[test]
    fn unknown_job_is_rejected_before_a_plan_exists() {
        let scope = DiagnosticScope::vehicle_wide();
        assert!(DiagnosticJob::from_id("readiness.scan", scope).is_err());
    }

    #[test]
    fn unknown_ecu_scope_has_no_address_scan_shape() {
        let target = KnownTarget::new("evidenced-engine").unwrap();
        let plan =
            DiagnosticJob::dtc_scan(DiagnosticScope::known_ecu(EcuRole::Unknown, target)).plan();
        assert_eq!(plan.steps().len(), 1);
        assert!(matches!(
            plan.steps()[0].target(),
            JobTarget::KnownEcu { .. }
        ));
    }

    #[test]
    fn recoverable_error_keeps_step_evidence() {
        let plan = DiagnosticJob::dtc_scan(
            DiagnosticScope::known_obd_responders(responders(&["one", "two"])).unwrap(),
        )
        .plan();
        let result = JobResult::from_outcomes(
            &plan,
            [
                StepOutcome::recoverable(StepError::Timeout, [evidence("first")]),
                StepOutcome::succeeded([evidence("second")]),
            ],
            Termination::Completed,
        )
        .unwrap();
        assert_eq!(result.status(), JobStatus::CompletedWithErrors);
        assert_eq!(
            result.steps()[0].outcome().evidence()[0]
                .reference()
                .as_str(),
            "first"
        );
        assert_eq!(
            result.steps()[1].outcome().evidence()[0]
                .reference()
                .as_str(),
            "second"
        );
    }

    #[test]
    fn fatal_session_error_stops_remaining_steps() {
        let plan = DiagnosticJob::dtc_scan(
            DiagnosticScope::known_obd_responders(responders(&["one", "two"])).unwrap(),
        )
        .plan();
        let result = JobResult::from_outcomes(
            &plan,
            [
                StepOutcome::succeeded([evidence("first")]),
                StepOutcome::not_run(SkipReason::SessionFailed),
            ],
            Termination::SessionFailed(SessionError::Disconnected),
        )
        .unwrap();
        assert_eq!(result.status(), JobStatus::Failed);
        assert_eq!(result.session_error(), Some(&SessionError::Disconnected));
        assert_eq!(
            result.steps()[1].outcome(),
            &StepOutcome::NotRun {
                reason: SkipReason::SessionFailed
            }
        );
    }

    #[test]
    fn cancellation_is_bounded_and_deterministic() {
        let plan = DiagnosticJob::dtc_scan(DiagnosticScope::vehicle_wide()).plan();
        let result = JobResult::from_outcomes(
            &plan,
            [StepOutcome::not_run(SkipReason::Cancelled)],
            Termination::Cancelled,
        )
        .unwrap();
        assert_eq!(result.status(), JobStatus::Cancelled);
        assert_eq!(result.steps().len(), 1);
    }

    #[test]
    fn model_has_no_arbitrary_payload_step() {
        let plan = DiagnosticJob::dtc_scan(DiagnosticScope::vehicle_wide()).plan();
        assert_eq!(plan.steps()[0].kind(), JobStepKind::ReadDtc);
        assert_eq!(plan.safety(), SafetyClass::ReadOnly);
    }
}
