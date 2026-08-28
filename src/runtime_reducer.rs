//! Pure, deterministic transitions for [`crate::runtime_state::RuntimeState`].
//!
//! The reducer owns no transport, session, clock, or persistence resources.
//! It only validates an event against the current value and returns the next
//! value.

use crate::runtime_state::{
    Activity, Phase, RecordingState, RuntimeContext, RuntimeState, SafetyCapability, SourceState,
    TopologyState, TransportState, VehicleState,
};

/// A typed fact update carried by a [`RuntimeEvent`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ContextUpdate {
    Transport(TransportState),
    Vehicle(VehicleState),
    Topology(TopologyState),
    Recording(RecordingState),
    Source(SourceState),
    Safety(SafetyCapability),
}

/// Alias for callers that describe context changes as facts.
pub type ContextFact = ContextUpdate;

/// Explicit lifecycle, activity, and context events understood by the reducer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeEvent {
    InitializationCompleted,
    InitializationFailed,
    DiscoveryStarted,
    DiscoveryCompleted,
    DiscoveryFailed,
    ReadStarted,
    ReadCompleted,
    ReadFailedRecoverable,
    ObservationStarted,
    ObservationStopped,
    /// A scheduler read is internal to observation and never changes activity.
    ObservationReadStarted,
    ObservationReadCompleted,
    ObservationReadFailedRecoverable,
    DiagnosticJobStarted,
    DiagnosticJobCompleted,
    ShutdownRequested,
    ShutdownCompleted,
    FatalRuntimeError,
    /// Reserved until a later safety design explicitly enables writes.
    WriteStarted,
    ContextUpdated(ContextUpdate),
    /// Compatibility spelling for code that calls a context change an event.
    ContextChanged(ContextUpdate),
    /// A typed convenience form for each context dimension.
    TransportChanged(TransportState),
    VehicleChanged(VehicleState),
    TopologyChanged(TopologyState),
    RecordingChanged(RecordingState),
    SourceChanged(SourceState),
    SafetyChanged(SafetyCapability),
}

/// Stable event categories used in transition errors.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeEventKind {
    InitializationCompleted,
    InitializationFailed,
    DiscoveryStarted,
    DiscoveryCompleted,
    DiscoveryFailed,
    ReadStarted,
    ReadCompleted,
    ReadFailedRecoverable,
    ObservationStarted,
    ObservationStopped,
    ObservationReadStarted,
    ObservationReadCompleted,
    ObservationReadFailedRecoverable,
    DiagnosticJobStarted,
    DiagnosticJobCompleted,
    ShutdownRequested,
    ShutdownCompleted,
    FatalRuntimeError,
    WriteStarted,
    ContextUpdated,
}

impl RuntimeEvent {
    /// Return the stable category used when reporting a rejected event.
    pub const fn kind(self) -> RuntimeEventKind {
        match self {
            Self::InitializationCompleted => RuntimeEventKind::InitializationCompleted,
            Self::InitializationFailed => RuntimeEventKind::InitializationFailed,
            Self::DiscoveryStarted => RuntimeEventKind::DiscoveryStarted,
            Self::DiscoveryCompleted => RuntimeEventKind::DiscoveryCompleted,
            Self::DiscoveryFailed => RuntimeEventKind::DiscoveryFailed,
            Self::ReadStarted => RuntimeEventKind::ReadStarted,
            Self::ReadCompleted => RuntimeEventKind::ReadCompleted,
            Self::ReadFailedRecoverable => RuntimeEventKind::ReadFailedRecoverable,
            Self::ObservationStarted => RuntimeEventKind::ObservationStarted,
            Self::ObservationStopped => RuntimeEventKind::ObservationStopped,
            Self::ObservationReadStarted => RuntimeEventKind::ObservationReadStarted,
            Self::ObservationReadCompleted => RuntimeEventKind::ObservationReadCompleted,
            Self::ObservationReadFailedRecoverable => {
                RuntimeEventKind::ObservationReadFailedRecoverable
            }
            Self::DiagnosticJobStarted => RuntimeEventKind::DiagnosticJobStarted,
            Self::DiagnosticJobCompleted => RuntimeEventKind::DiagnosticJobCompleted,
            Self::ShutdownRequested => RuntimeEventKind::ShutdownRequested,
            Self::ShutdownCompleted => RuntimeEventKind::ShutdownCompleted,
            Self::FatalRuntimeError => RuntimeEventKind::FatalRuntimeError,
            Self::WriteStarted => RuntimeEventKind::WriteStarted,
            Self::ContextUpdated(_)
            | Self::ContextChanged(_)
            | Self::TransportChanged(_)
            | Self::VehicleChanged(_)
            | Self::TopologyChanged(_)
            | Self::RecordingChanged(_)
            | Self::SourceChanged(_)
            | Self::SafetyChanged(_) => RuntimeEventKind::ContextUpdated,
        }
    }

    /// Construct a context event without exposing a second event hierarchy.
    pub const fn context(update: ContextUpdate) -> Self {
        Self::ContextUpdated(update)
    }

    pub const fn transport(state: TransportState) -> Self {
        Self::ContextUpdated(ContextUpdate::Transport(state))
    }

    pub const fn vehicle(state: VehicleState) -> Self {
        Self::ContextUpdated(ContextUpdate::Vehicle(state))
    }

    pub const fn topology(state: TopologyState) -> Self {
        Self::ContextUpdated(ContextUpdate::Topology(state))
    }

    pub const fn recording(state: RecordingState) -> Self {
        Self::ContextUpdated(ContextUpdate::Recording(state))
    }

    pub const fn source(state: SourceState) -> Self {
        Self::ContextUpdated(ContextUpdate::Source(state))
    }

    pub const fn safety(state: SafetyCapability) -> Self {
        Self::ContextUpdated(ContextUpdate::Safety(state))
    }
}

/// Why an explicit event was rejected.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransitionError {
    /// The event is not legal for the current phase/activity pair.
    InvalidOrder {
        phase: Phase,
        activity: Activity,
        event: RuntimeEventKind,
    },
    /// The runtime is read-only; no reducer event can enter `activity/write`.
    WriteUnavailable { phase: Phase, activity: Activity },
}

impl TransitionError {
    pub const fn phase(self) -> Phase {
        match self {
            Self::InvalidOrder { phase, .. } | Self::WriteUnavailable { phase, .. } => phase,
        }
    }

    pub const fn activity(self) -> Activity {
        match self {
            Self::InvalidOrder { activity, .. } | Self::WriteUnavailable { activity, .. } => {
                activity
            }
        }
    }

    pub const fn event(self) -> Option<RuntimeEventKind> {
        match self {
            Self::InvalidOrder { event, .. } => Some(event),
            Self::WriteUnavailable { .. } => None,
        }
    }
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOrder {
                phase,
                activity,
                event,
            } => write!(
                formatter,
                "event {event:?} is invalid in {}.{}",
                phase.id(),
                activity.id()
            ),
            Self::WriteUnavailable { phase, activity } => write!(
                formatter,
                "write activity is unavailable in {}.{}",
                phase.id(),
                activity.id()
            ),
        }
    }
}

impl std::error::Error for TransitionError {}

/// Apply one event without I/O, time, randomness, or mutation.
pub fn transition(
    state: &RuntimeState,
    event: RuntimeEvent,
) -> Result<RuntimeState, TransitionError> {
    let phase = state.phase();
    let activity = state.activity();
    let invalid = || {
        Err(TransitionError::InvalidOrder {
            phase,
            activity,
            event: event.kind(),
        })
    };

    match event {
        RuntimeEvent::ContextUpdated(update) | RuntimeEvent::ContextChanged(update) => {
            apply_context(state, update, invalid)
        }
        RuntimeEvent::TransportChanged(value) => {
            apply_context(state, ContextUpdate::Transport(value), invalid)
        }
        RuntimeEvent::VehicleChanged(value) => {
            apply_context(state, ContextUpdate::Vehicle(value), invalid)
        }
        RuntimeEvent::TopologyChanged(value) => {
            apply_context(state, ContextUpdate::Topology(value), invalid)
        }
        RuntimeEvent::RecordingChanged(value) => {
            apply_context(state, ContextUpdate::Recording(value), invalid)
        }
        RuntimeEvent::SourceChanged(value) => {
            apply_context(state, ContextUpdate::Source(value), invalid)
        }
        RuntimeEvent::SafetyChanged(value) => {
            apply_context(state, ContextUpdate::Safety(value), invalid)
        }
        RuntimeEvent::InitializationCompleted => {
            if phase == Phase::Init && activity == Activity::Idle {
                Ok(state_at(state, Phase::Ready, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::InitializationFailed => {
            if phase == Phase::Init && activity == Activity::Idle {
                Ok(state_at(state, Phase::Fault, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::DiscoveryStarted => {
            if phase == Phase::Ready && activity == Activity::Idle {
                Ok(state_at(state, Phase::Discover, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::DiscoveryCompleted | RuntimeEvent::DiscoveryFailed => {
            if phase == Phase::Discover && activity == Activity::Idle {
                Ok(state_at(state, Phase::Ready, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ReadStarted => {
            if phase == Phase::Ready && activity == Activity::Idle {
                Ok(state_at(state, Phase::Ready, Activity::Read))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ReadCompleted | RuntimeEvent::ReadFailedRecoverable => {
            if phase == Phase::Ready && activity == Activity::Read {
                Ok(state_at(state, Phase::Ready, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ObservationStarted => {
            if phase == Phase::Ready && activity == Activity::Idle {
                Ok(state_at(state, Phase::Ready, Activity::Observe))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ObservationStopped => {
            if phase == Phase::Ready && activity == Activity::Observe {
                Ok(state_at(state, Phase::Ready, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ObservationReadStarted
        | RuntimeEvent::ObservationReadCompleted
        | RuntimeEvent::ObservationReadFailedRecoverable => {
            if phase == Phase::Ready && activity == Activity::Observe {
                Ok(*state)
            } else {
                invalid()
            }
        }
        RuntimeEvent::DiagnosticJobStarted => {
            if phase == Phase::Ready && activity == Activity::Idle {
                Ok(state_at(state, Phase::Ready, Activity::Diagnose))
            } else {
                invalid()
            }
        }
        RuntimeEvent::DiagnosticJobCompleted => {
            if phase == Phase::Ready && activity == Activity::Diagnose {
                Ok(state_at(state, Phase::Ready, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ShutdownRequested => {
            if matches!(
                phase,
                Phase::Init | Phase::Discover | Phase::Ready | Phase::Fault
            ) && activity == Activity::Idle
            {
                Ok(state_at(state, Phase::Stopping, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::ShutdownCompleted => {
            if phase == Phase::Stopping && activity == Activity::Idle {
                Ok(state_at(state, Phase::Stopped, Activity::Idle))
            } else {
                invalid()
            }
        }
        RuntimeEvent::FatalRuntimeError => {
            if phase == Phase::Stopped {
                invalid()
            } else {
                Ok(state_at(state, Phase::Fault, Activity::Idle))
            }
        }
        RuntimeEvent::WriteStarted => Err(TransitionError::WriteUnavailable { phase, activity }),
    }
}

fn state_at(state: &RuntimeState, phase: Phase, activity: Activity) -> RuntimeState {
    RuntimeState::new(phase, activity, state.context())
}

fn apply_context(
    state: &RuntimeState,
    update: ContextUpdate,
    invalid: impl FnOnce() -> Result<RuntimeState, TransitionError>,
) -> Result<RuntimeState, TransitionError> {
    if state.phase() == Phase::Stopped {
        return invalid();
    }

    let current = state.context();
    let context = match update {
        ContextUpdate::Transport(value) => RuntimeContext::new(
            value,
            current.vehicle(),
            current.topology(),
            current.recording(),
            current.source(),
            current.safety(),
        ),
        ContextUpdate::Vehicle(value) => RuntimeContext::new(
            current.transport(),
            value,
            current.topology(),
            current.recording(),
            current.source(),
            current.safety(),
        ),
        ContextUpdate::Topology(value) => RuntimeContext::new(
            current.transport(),
            current.vehicle(),
            value,
            current.recording(),
            current.source(),
            current.safety(),
        ),
        ContextUpdate::Recording(value) => RuntimeContext::new(
            current.transport(),
            current.vehicle(),
            current.topology(),
            value,
            current.source(),
            current.safety(),
        ),
        ContextUpdate::Source(value) => RuntimeContext::new(
            current.transport(),
            current.vehicle(),
            current.topology(),
            current.recording(),
            value,
            current.safety(),
        ),
        ContextUpdate::Safety(value) => RuntimeContext::new(
            current.transport(),
            current.vehicle(),
            current.topology(),
            current.recording(),
            current.source(),
            value,
        ),
    };
    Ok(RuntimeState::new(state.phase(), state.activity(), context))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay(events: &[RuntimeEvent]) -> Vec<RuntimeState> {
        let mut state = RuntimeState::default();
        let mut states = vec![state];
        for &event in events {
            state = transition(&state, event).expect("legal replay event");
            states.push(state);
        }
        states
    }

    #[test]
    fn replay_is_deterministic() {
        let events = [
            RuntimeEvent::InitializationCompleted,
            RuntimeEvent::DiscoveryStarted,
            RuntimeEvent::topology(TopologyState::Discovering),
            RuntimeEvent::DiscoveryCompleted,
            RuntimeEvent::ReadStarted,
            RuntimeEvent::ReadCompleted,
            RuntimeEvent::ObservationStarted,
            RuntimeEvent::ObservationReadStarted,
            RuntimeEvent::ObservationReadCompleted,
            RuntimeEvent::ObservationStopped,
            RuntimeEvent::DiagnosticJobStarted,
            RuntimeEvent::DiagnosticJobCompleted,
        ];

        assert_eq!(replay(&events), replay(&events));
        assert_eq!(
            replay(&events).last().unwrap().identity(),
            (Phase::Ready, Activity::Idle)
        );
    }

    #[test]
    fn invalid_order_does_not_change_state() {
        let state = RuntimeState::default();

        assert!(matches!(
            transition(&state, RuntimeEvent::ReadCompleted),
            Err(TransitionError::InvalidOrder {
                phase: Phase::Init,
                activity: Activity::Idle,
                event: RuntimeEventKind::ReadCompleted,
            })
        ));
        assert_eq!(state, RuntimeState::default());
    }

    #[test]
    fn observation_internal_reads_do_not_flap_activity() {
        let ready = transition(
            &transition(
                &RuntimeState::default(),
                RuntimeEvent::InitializationCompleted,
            )
            .unwrap(),
            RuntimeEvent::ObservationStarted,
        )
        .unwrap();

        let after_start = transition(&ready, RuntimeEvent::ObservationReadStarted).unwrap();
        let after_complete =
            transition(&after_start, RuntimeEvent::ObservationReadCompleted).unwrap();
        let after_failure = transition(
            &after_complete,
            RuntimeEvent::ObservationReadFailedRecoverable,
        )
        .unwrap();
        assert_eq!(after_failure, ready);
        assert_eq!(after_failure.activity(), Activity::Observe);
    }

    #[test]
    fn recoverable_read_failure_is_not_fault_but_fatal_is() {
        let ready = transition(
            &RuntimeState::default(),
            RuntimeEvent::InitializationCompleted,
        )
        .unwrap();
        let reading = transition(&ready, RuntimeEvent::ReadStarted).unwrap();
        let recovered = transition(&reading, RuntimeEvent::ReadFailedRecoverable).unwrap();
        assert_eq!(recovered.identity(), (Phase::Ready, Activity::Idle));

        let fault = transition(&reading, RuntimeEvent::FatalRuntimeError).unwrap();
        assert_eq!(fault.identity(), (Phase::Fault, Activity::Idle));
    }

    #[test]
    fn shutdown_from_ready_and_fault_is_stopping_then_stopped() {
        let ready = transition(
            &RuntimeState::default(),
            RuntimeEvent::InitializationCompleted,
        )
        .unwrap();
        let ready_stopped = transition(
            &transition(&ready, RuntimeEvent::ShutdownRequested).unwrap(),
            RuntimeEvent::ShutdownCompleted,
        )
        .unwrap();
        assert_eq!(ready_stopped.identity(), (Phase::Stopped, Activity::Idle));

        let fault = transition(&RuntimeState::default(), RuntimeEvent::FatalRuntimeError).unwrap();
        let fault_stopped = transition(
            &transition(&fault, RuntimeEvent::ShutdownRequested).unwrap(),
            RuntimeEvent::ShutdownCompleted,
        )
        .unwrap();
        assert_eq!(fault_stopped.identity(), (Phase::Stopped, Activity::Idle));
    }

    #[test]
    fn context_updates_are_typed_and_deterministic() {
        let state = RuntimeState::default();
        let updated = transition(
            &transition(
                &transition(&state, RuntimeEvent::transport(TransportState::Connected)).unwrap(),
                RuntimeEvent::vehicle(VehicleState::Identified),
            )
            .unwrap(),
            RuntimeEvent::topology(TopologyState::Validated),
        )
        .unwrap();

        assert_eq!(updated.phase(), Phase::Init);
        assert_eq!(updated.activity(), Activity::Idle);
        assert_eq!(updated.context().transport(), TransportState::Connected);
        assert_eq!(updated.context().vehicle(), VehicleState::Identified);
        assert_eq!(updated.context().topology(), TopologyState::Validated);
        assert_eq!(
            transition(&state, RuntimeEvent::transport(TransportState::Connected)),
            transition(&state, RuntimeEvent::transport(TransportState::Connected))
        );
    }

    #[test]
    fn write_cannot_be_entered_even_when_capability_is_described() {
        let state = transition(
            &RuntimeState::default(),
            RuntimeEvent::safety(SafetyCapability::ExplicitMutation),
        )
        .unwrap();
        assert!(matches!(
            transition(&state, RuntimeEvent::WriteStarted),
            Err(TransitionError::WriteUnavailable { .. })
        ));
    }
}
