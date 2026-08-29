//! Read-only safety boundary for runtime activities and vehicle operations.
//!
//! This module deliberately has no transport dependency.  Callers first build
//! a typed request and then ask [`SafetyPolicy`] to turn it into an executable
//! [`Operation`].  Since `Operation` has only read-only variants, a rejected
//! request cannot be passed to a transport by accident.

use crate::{
    diagnostic_job::{DiagnosticScope, KnownTarget},
    ea189::{Ea189DpfProbe, Ea189DpfProbeError, Ea189DpfProbeRequest},
    prepare_read,
    runtime_state::Activity,
    ReadRequest,
};
use std::fmt;

/// Standards-based OBD-II DTC read selection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DtcReadKind {
    /// A bounded scan across the standard stored, pending and permanent DTC
    /// information categories.
    All,
    /// SAE J1979 Mode 03.
    Stored,
    /// SAE J1979 Mode 07.
    Pending,
    /// SAE J1979 Mode 0A.
    Permanent,
}

impl DtcReadKind {
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::All => "dtc.scan",
            Self::Stored => "dtc.read",
            Self::Pending => "dtc.pending",
            Self::Permanent => "dtc.permanent",
        }
    }

    pub const fn id(self) -> &'static str {
        self.identifier()
    }

    pub const fn as_str(self) -> &'static str {
        self.identifier()
    }

    pub fn from_identifier(identifier: &str) -> Option<Self> {
        match identifier {
            "dtc.scan" => Some(Self::All),
            "dtc.read" => Some(Self::Stored),
            "dtc.pending" => Some(Self::Pending),
            "dtc.permanent" => Some(Self::Permanent),
            _ => None,
        }
    }
}

impl fmt::Display for DtcReadKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.identifier())
    }
}

/// Operation classes used by capability and policy checks.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationKind {
    Write,
    SignalRead,
    DtcRead,
    Ea189DpfProbe,
    DtcClear,
    SecurityAccess,
    DiagnosticSessionControl,
    ActuatorTest,
    BasicSettings,
    Coding,
    Adaptation,
    EcuReset,
    ForcedRegeneration,
    RoutineControl,
    RawCanInjection,
    RawUdsInjection,
    RawElmInjection,
}

/// Explicitly typed requests accepted at the safety boundary.
///
/// The blocked variants are request vocabulary only.  They can never become
/// an [`Operation`], and no variant carries caller-provided protocol bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationRequest {
    ReadSignal(ReadRequest),
    ReadDtcs(DtcReadKind),
    Ea189DpfProbe(Ea189DpfProbeRequest),
    ClearDtcs,
    SecurityAccess,
    DiagnosticSessionControl,
    ActuatorTest,
    BasicSettings,
    Coding,
    Adaptation,
    EcuReset,
    ForcedRegeneration,
    RoutineControl,
    RawCanInjection,
    RawUdsInjection,
    RawElmInjection,
}

/// Alias for callers that name the input side a requested operation.
pub type RequestedOperation = OperationRequest;

impl OperationRequest {
    pub fn read_signal(identifier: &str) -> Result<Self, SafetyError> {
        prepare_read(identifier)
            .map(Self::ReadSignal)
            .map_err(|_| SafetyError::UnknownSignal(identifier.into()))
    }

    pub const fn read_signal_typed(request: ReadRequest) -> Self {
        Self::ReadSignal(request)
    }

    pub const fn read_dtcs(kind: DtcReadKind) -> Self {
        Self::ReadDtcs(kind)
    }

    pub const fn read_dtc(kind: DtcReadKind) -> Self {
        Self::ReadDtcs(kind)
    }

    pub fn ea189_dpf_probe(probe: Ea189DpfProbe, target: KnownTarget) -> Self {
        Self::Ea189DpfProbe(Ea189DpfProbeRequest::for_engine(probe, target))
    }

    pub fn ea189_dpf_probe_for_scope(
        probe: Ea189DpfProbe,
        scope: DiagnosticScope,
    ) -> Result<Self, Ea189DpfProbeError> {
        Ea189DpfProbeRequest::from_scope(probe, scope).map(Self::Ea189DpfProbe)
    }

    pub const fn kind(&self) -> OperationKind {
        match self {
            Self::ReadSignal(_) => OperationKind::SignalRead,
            Self::ReadDtcs(_) => OperationKind::DtcRead,
            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,
            Self::ClearDtcs => OperationKind::DtcClear,
            Self::SecurityAccess => OperationKind::SecurityAccess,
            Self::DiagnosticSessionControl => OperationKind::DiagnosticSessionControl,
            Self::ActuatorTest => OperationKind::ActuatorTest,
            Self::BasicSettings => OperationKind::BasicSettings,
            Self::Coding => OperationKind::Coding,
            Self::Adaptation => OperationKind::Adaptation,
            Self::EcuReset => OperationKind::EcuReset,
            Self::ForcedRegeneration => OperationKind::ForcedRegeneration,
            Self::RoutineControl => OperationKind::RoutineControl,
            Self::RawCanInjection => OperationKind::RawCanInjection,
            Self::RawUdsInjection => OperationKind::RawUdsInjection,
            Self::RawElmInjection => OperationKind::RawElmInjection,
        }
    }
}

/// The only operations that can leave the safety layer for transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    ReadSignal(ReadRequest),
    ReadDtcs(DtcReadKind),
    Ea189DpfProbe(Ea189DpfProbeRequest),
}

impl Operation {
    pub const fn kind(&self) -> OperationKind {
        match self {
            Self::ReadSignal(_) => OperationKind::SignalRead,
            Self::ReadDtcs(_) => OperationKind::DtcRead,
            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,
        }
    }

    pub const fn signal(request: ReadRequest) -> Self {
        Self::ReadSignal(request)
    }

    pub const fn dtcs(kind: DtcReadKind) -> Self {
        Self::ReadDtcs(kind)
    }

    pub const fn dtc(kind: DtcReadKind) -> Self {
        Self::ReadDtcs(kind)
    }

    pub fn ea189_dpf_probe(request: Ea189DpfProbeRequest) -> Self {
        Self::Ea189DpfProbe(request)
    }
}

/// Current product capability.  It is intentionally closed to read-only.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Capability {
    #[default]
    ReadOnly,
}

impl Capability {
    pub const fn read_only() -> Self {
        Self::ReadOnly
    }

    pub const fn identifier(self) -> &'static str {
        match self {
            Self::ReadOnly => "capability/read-only",
        }
    }

    pub const fn supports(self, operation: OperationKind) -> bool {
        match self {
            Self::ReadOnly => matches!(
                operation,
                OperationKind::SignalRead | OperationKind::DtcRead | OperationKind::Ea189DpfProbe
            ),
        }
    }

    pub const fn write_available(self) -> bool {
        false
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.identifier())
    }
}

/// The reason a request was rejected, retaining a typed operation class.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SafetyError {
    UnknownSignal(String),
    ActivityNotAllowed {
        activity: Activity,
        operation: OperationKind,
    },
    CapabilityUnavailable(OperationKind),
    OperationBlocked(OperationKind),
    WriteUnavailable,
}

impl fmt::Display for SafetyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSignal(signal) => write!(formatter, "unknown semantic signal: {signal}"),
            Self::ActivityNotAllowed {
                activity,
                operation,
            } => write!(formatter, "{operation:?} is not allowed for {activity:?}"),
            Self::CapabilityUnavailable(operation) => {
                write!(formatter, "current capability does not allow {operation:?}")
            }
            Self::OperationBlocked(operation) => {
                write!(
                    formatter,
                    "operation is blocked by the read-only safety policy: {operation:?}"
                )
            }
            Self::WriteUnavailable => formatter.write_str("activity/write is unavailable"),
        }
    }
}

impl std::error::Error for SafetyError {}

/// Pure policy translating typed requests into executable read-only operations.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SafetyPolicy {
    capability: Capability,
}

impl SafetyPolicy {
    pub const fn new(capability: Capability) -> Self {
        Self { capability }
    }

    pub const fn read_only() -> Self {
        Self::new(Capability::ReadOnly)
    }

    pub const fn capability(self) -> Capability {
        self.capability
    }

    /// Authorize a concrete request independently of runtime activity.
    pub fn authorize(self, request: OperationRequest) -> Result<Operation, SafetyError> {
        match request {
            OperationRequest::ReadSignal(signal) => self.authorize_read(signal),
            OperationRequest::ReadDtcs(kind) => self.authorize_operation(Operation::ReadDtcs(kind)),
            OperationRequest::Ea189DpfProbe(probe) => {
                self.authorize_operation(Operation::Ea189DpfProbe(probe))
            }
            OperationRequest::ClearDtcs => {
                Err(SafetyError::OperationBlocked(OperationKind::DtcClear))
            }
            OperationRequest::SecurityAccess => {
                Err(SafetyError::OperationBlocked(OperationKind::SecurityAccess))
            }
            OperationRequest::DiagnosticSessionControl => Err(SafetyError::OperationBlocked(
                OperationKind::DiagnosticSessionControl,
            )),
            OperationRequest::ActuatorTest => {
                Err(SafetyError::OperationBlocked(OperationKind::ActuatorTest))
            }
            OperationRequest::BasicSettings => {
                Err(SafetyError::OperationBlocked(OperationKind::BasicSettings))
            }
            OperationRequest::Coding => Err(SafetyError::OperationBlocked(OperationKind::Coding)),
            OperationRequest::Adaptation => {
                Err(SafetyError::OperationBlocked(OperationKind::Adaptation))
            }
            OperationRequest::EcuReset => {
                Err(SafetyError::OperationBlocked(OperationKind::EcuReset))
            }
            OperationRequest::ForcedRegeneration => Err(SafetyError::OperationBlocked(
                OperationKind::ForcedRegeneration,
            )),
            OperationRequest::RoutineControl => {
                Err(SafetyError::OperationBlocked(OperationKind::RoutineControl))
            }
            OperationRequest::RawCanInjection => Err(SafetyError::OperationBlocked(
                OperationKind::RawCanInjection,
            )),
            OperationRequest::RawUdsInjection => Err(SafetyError::OperationBlocked(
                OperationKind::RawUdsInjection,
            )),
            OperationRequest::RawElmInjection => Err(SafetyError::OperationBlocked(
                OperationKind::RawElmInjection,
            )),
        }
    }

    /// Authorize a request while enforcing the activity/operation distinction.
    /// In particular, `diagnose` does not make ordinary signal reads or
    /// protocol services safe, and `write` never reaches capability dispatch.
    pub fn authorize_activity(
        self,
        activity: Activity,
        request: OperationRequest,
    ) -> Result<Operation, SafetyError> {
        if matches!(activity, Activity::Write) {
            return Err(SafetyError::WriteUnavailable);
        }

        let operation = request.kind();
        // Classify and reject forbidden requests before considering activity;
        // `diagnose` must not hide a mutating request behind its job label.
        let executable = self.authorize(request)?;
        let activity_allows = match activity {
            Activity::Read | Activity::Observe => {
                matches!(operation, OperationKind::SignalRead)
            }
            Activity::Diagnose => {
                matches!(
                    operation,
                    OperationKind::DtcRead | OperationKind::Ea189DpfProbe
                )
            }
            Activity::Idle | Activity::Write => false,
        };
        if !activity_allows {
            return Err(SafetyError::ActivityNotAllowed {
                activity,
                operation,
            });
        }
        Ok(executable)
    }

    fn authorize_read(self, request: ReadRequest) -> Result<Operation, SafetyError> {
        self.authorize_operation(Operation::signal(request))
    }

    fn authorize_operation(self, operation: Operation) -> Result<Operation, SafetyError> {
        if self.capability.supports(operation.kind()) {
            Ok(operation)
        } else {
            Err(SafetyError::CapabilityUnavailable(operation.kind()))
        }
    }
}

impl Default for SafetyPolicy {
    fn default() -> Self {
        Self::read_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_write_is_serializable_vocabulary_but_not_executable() {
        assert!(!Capability::ReadOnly.write_available());
        assert!(!Capability::ReadOnly.supports(OperationKind::Write));
        assert_eq!(
            SafetyPolicy::default().authorize_activity(
                Activity::Write,
                OperationRequest::read_signal("engine.rpm").unwrap()
            ),
            Err(SafetyError::WriteUnavailable)
        );
    }

    #[test]
    fn explicit_signal_and_standard_dtc_reads_are_allowed() {
        let policy = SafetyPolicy::default();
        assert_eq!(
            policy.authorize_activity(
                Activity::Read,
                OperationRequest::read_signal("engine.rpm").unwrap()
            ),
            Ok(Operation::ReadSignal(prepare_read("engine.rpm").unwrap()))
        );
        assert_eq!(
            policy.authorize_activity(
                Activity::Diagnose,
                OperationRequest::ReadDtcs(DtcReadKind::Stored)
            ),
            Ok(Operation::ReadDtcs(DtcReadKind::Stored))
        );
    }

    #[test]
    fn ea189_dpf_probe_is_read_only_and_engine_targeted() {
        let target = crate::diagnostic_job::KnownTarget::new("validated-engine").unwrap();
        let request = OperationRequest::ea189_dpf_probe(Ea189DpfProbe::SootMassMeasured, target);
        assert_eq!(request.kind(), OperationKind::Ea189DpfProbe);
        let operation = SafetyPolicy::default()
            .authorize_activity(Activity::Diagnose, request)
            .unwrap();
        assert!(matches!(
            operation,
            Operation::Ea189DpfProbe(ref request)
                if request.probe() == Ea189DpfProbe::SootMassMeasured
                    && matches!(
                        request.scope(),
                        DiagnosticScope::KnownEcu {
                            role: crate::diagnostic_job::EcuRole::Engine,
                            ..
                        }
                    )
        ));
    }

    #[test]
    fn generic_signal_path_cannot_construct_an_ea189_dpf_did() {
        assert!(OperationRequest::read_signal("dpf.soot_mass_measured").is_err());
        let target = crate::diagnostic_job::KnownTarget::new("validated-engine").unwrap();
        let request =
            OperationRequest::ea189_dpf_probe(Ea189DpfProbe::DifferentialPressure, target);
        assert!(SafetyPolicy::default()
            .authorize_activity(Activity::Read, request)
            .is_err());
    }

    #[test]
    fn blocked_requests_never_produce_an_operation() {
        let blocked = [
            OperationRequest::ClearDtcs,
            OperationRequest::SecurityAccess,
            OperationRequest::DiagnosticSessionControl,
            OperationRequest::ActuatorTest,
            OperationRequest::BasicSettings,
            OperationRequest::Coding,
            OperationRequest::Adaptation,
            OperationRequest::EcuReset,
            OperationRequest::ForcedRegeneration,
            OperationRequest::RoutineControl,
            OperationRequest::RawCanInjection,
            OperationRequest::RawUdsInjection,
            OperationRequest::RawElmInjection,
        ];
        for request in blocked {
            assert!(SafetyPolicy::default().authorize(request).is_err());
        }
    }

    #[test]
    fn diagnose_is_not_a_protocol_safety_bypass() {
        let policy = SafetyPolicy::default();
        assert!(policy
            .authorize_activity(
                Activity::Diagnose,
                OperationRequest::read_signal("engine.rpm").unwrap()
            )
            .is_err());
        assert!(policy
            .authorize_activity(
                Activity::Diagnose,
                OperationRequest::DiagnosticSessionControl
            )
            .is_err());
        assert_eq!(
            policy.authorize_activity(Activity::Diagnose, OperationRequest::ClearDtcs),
            Err(SafetyError::OperationBlocked(OperationKind::DtcClear))
        );
    }

    #[test]
    fn unknown_signal_fails_closed() {
        assert_eq!(
            OperationRequest::read_signal("engine.unknown"),
            Err(SafetyError::UnknownSignal("engine.unknown".into()))
        );
    }
}
