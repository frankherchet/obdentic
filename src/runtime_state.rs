//! Stable, transport-neutral runtime state vocabulary.
//!
//! Phase and activity are deliberately independent.  Context facts describe
//! what is known about the current session without creating a combinatorial
//! lifecycle enum.  This module has no transport, clock, or I/O dependency.

use std::{collections::BTreeMap, fmt, str::FromStr};

/// Version of the serialized runtime-state contract.
pub const STATE_VERSION: u16 = 1;

/// Runtime lifecycle phase.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Phase {
    /// Process/runtime initialization; no vehicle is assumed to be known.
    #[default]
    Init,
    /// Bounded identity, topology, capability discovery, or cache validation.
    Discover,
    /// Initialized enough to accept an allowed operation in the current context.
    Ready,
    /// Runtime/session shutdown is in progress.
    Stopping,
    /// Runtime/session shutdown has completed.
    Stopped,
    /// A fatal runtime/session failure has stopped normal operation.
    Fault,
}

impl Phase {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Init => "phase/init",
            Self::Discover => "phase/discover",
            Self::Ready => "phase/ready",
            Self::Stopping => "phase/stopping",
            Self::Stopped => "phase/stopped",
            Self::Fault => "phase/fault",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "phase/init" => Ok(Self::Init),
            "phase/discover" => Ok(Self::Discover),
            "phase/ready" => Ok(Self::Ready),
            "phase/stopping" => Ok(Self::Stopping),
            "phase/stopped" => Ok(Self::Stopped),
            "phase/fault" => Ok(Self::Fault),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "phase",
                id: id.into(),
            }),
        }
    }
}

/// Current user/system intent, independent of lifecycle phase.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Activity {
    /// No user/system operation is currently active.
    #[default]
    Idle,
    /// One bounded, explicit semantic read requested by a consumer.
    Read,
    /// Long-lived scheduled observation; its individual reads remain observe activity.
    Observe,
    /// A bounded diagnostic job against the vehicle, not offline interpretation.
    Diagnose,
    /// Vocabulary only: this does not grant permission to mutate a vehicle.
    Write,
}

impl Activity {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Idle => "activity/idle",
            Self::Read => "activity/read",
            Self::Observe => "activity/observe",
            Self::Diagnose => "activity/diagnose",
            Self::Write => "activity/write",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "activity/idle" => Ok(Self::Idle),
            "activity/read" => Ok(Self::Read),
            "activity/observe" => Ok(Self::Observe),
            "activity/diagnose" => Ok(Self::Diagnose),
            "activity/write" => Ok(Self::Write),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "activity",
                id: id.into(),
            }),
        }
    }
}

/// Transport/session fact.  It is descriptive and does not contain a handle.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TransportState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Unhealthy,
}

impl TransportState {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Disconnected => "transport/disconnected",
            Self::Connecting => "transport/connecting",
            Self::Connected => "transport/connected",
            Self::Unhealthy => "transport/unhealthy",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "transport/disconnected" => Ok(Self::Disconnected),
            "transport/connecting" => Ok(Self::Connecting),
            "transport/connected" => Ok(Self::Connected),
            "transport/unhealthy" => Ok(Self::Unhealthy),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "transport",
                id: id.into(),
            }),
        }
    }
}

/// Privacy-safe vehicle fact.  Identifiers such as VINs are intentionally not
/// part of runtime state.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum VehicleState {
    #[default]
    Unknown,
    Identified,
}

impl VehicleState {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Unknown => "vehicle/unknown",
            Self::Identified => "vehicle/identified",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "vehicle/unknown" => Ok(Self::Unknown),
            "vehicle/identified" => Ok(Self::Identified),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "vehicle",
                id: id.into(),
            }),
        }
    }
}

/// Validation fact for the currently available ECU topology.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TopologyState {
    #[default]
    Unknown,
    Discovering,
    Validated,
    Stale,
}

impl TopologyState {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Unknown => "topology/unknown",
            Self::Discovering => "topology/discovering",
            Self::Validated => "topology/validated",
            Self::Stale => "topology/stale",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "topology/unknown" => Ok(Self::Unknown),
            "topology/discovering" => Ok(Self::Discovering),
            "topology/validated" => Ok(Self::Validated),
            "topology/stale" => Ok(Self::Stale),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "topology",
                id: id.into(),
            }),
        }
    }
}

/// Recording fact; it does not perform recording.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RecordingState {
    #[default]
    Inactive,
    Active,
}

impl RecordingState {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Inactive => "recording/inactive",
            Self::Active => "recording/active",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "recording/inactive" => Ok(Self::Inactive),
            "recording/active" => Ok(Self::Active),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "recording",
                id: id.into(),
            }),
        }
    }
}

/// Whether the current capability is read-only or explicitly mutation-capable.
/// Capability is not authorization: an `Activity::Write` remains only an
/// intent in this model.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SafetyCapability {
    #[default]
    ReadOnly,
    ExplicitMutation,
}

impl SafetyCapability {
    pub const fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "safety/read-only",
            Self::ExplicitMutation => "safety/explicit-mutation",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "safety/read-only" => Ok(Self::ReadOnly),
            "safety/explicit-mutation" => Ok(Self::ExplicitMutation),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "safety",
                id: id.into(),
            }),
        }
    }
}

/// Source fact for the current operation or data set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SourceState {
    Live,
    #[default]
    Offline,
}

impl SourceState {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Live => "source/live",
            Self::Offline => "source/offline",
        }
    }

    pub fn parse(id: &str) -> Result<Self, RuntimeStateError> {
        match id {
            "source/live" => Ok(Self::Live),
            "source/offline" => Ok(Self::Offline),
            _ => Err(RuntimeStateError::UnknownIdentifier {
                field: "source",
                id: id.into(),
            }),
        }
    }
}

/// Typed, non-volatile facts accompanying a runtime state.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeContext {
    transport: TransportState,
    vehicle: VehicleState,
    topology: TopologyState,
    recording: RecordingState,
    source: SourceState,
    safety: SafetyCapability,
}

impl RuntimeContext {
    pub const fn new(
        transport: TransportState,
        vehicle: VehicleState,
        topology: TopologyState,
        recording: RecordingState,
        source: SourceState,
        safety: SafetyCapability,
    ) -> Self {
        Self {
            transport,
            vehicle,
            topology,
            recording,
            source,
            safety,
        }
    }

    pub const fn transport(self) -> TransportState {
        self.transport
    }

    pub const fn vehicle(self) -> VehicleState {
        self.vehicle
    }

    pub const fn topology(self) -> TopologyState {
        self.topology
    }

    pub const fn recording(self) -> RecordingState {
        self.recording
    }

    pub const fn source(self) -> SourceState {
        self.source
    }

    pub const fn safety(self) -> SafetyCapability {
        self.safety
    }
}

/// The complete deterministic runtime-state value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeState {
    state_version: u16,
    phase: Phase,
    activity: Activity,
    context: RuntimeContext,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new(Phase::Init, Activity::Idle, RuntimeContext::default())
    }
}

impl RuntimeState {
    pub const fn new(phase: Phase, activity: Activity, context: RuntimeContext) -> Self {
        Self {
            state_version: STATE_VERSION,
            phase,
            activity,
            context,
        }
    }

    pub const fn state_version(self) -> u16 {
        self.state_version
    }

    pub const fn phase(self) -> Phase {
        self.phase
    }

    pub const fn activity(self) -> Activity {
        self.activity
    }

    pub const fn context(self) -> RuntimeContext {
        self.context
    }

    /// The orthogonal public identity, excluding volatile context facts.
    pub const fn identity(self) -> (Phase, Activity) {
        (self.phase, self.activity)
    }

    /// Serialize in the canonical field order.  All values are fixed IDs, so
    /// no escaping or locale/time-dependent formatting is involved.
    pub fn serialize(self) -> String {
        format!(
            "state_version={};phase={};activity={};transport={};vehicle={};topology={};recording={};source={};safety={}",
            self.state_version,
            self.phase.id(),
            self.activity.id(),
            self.context.transport.id(),
            self.context.vehicle.id(),
            self.context.topology.id(),
            self.context.recording.id(),
            self.context.source.id(),
            self.context.safety.id(),
        )
    }

    /// Parse the canonical state representation. Unknown fields and IDs are
    /// rejected so a newer contract cannot silently become an older state.
    pub fn parse(serialized: &str) -> Result<Self, RuntimeStateError> {
        let mut fields = BTreeMap::new();
        if serialized.is_empty() {
            return Err(RuntimeStateError::InvalidFormat);
        }
        for field in serialized.split(';') {
            let (name, value) = field
                .split_once('=')
                .ok_or(RuntimeStateError::InvalidFormat)?;
            if name.is_empty() || value.is_empty() || fields.insert(name, value).is_some() {
                return Err(RuntimeStateError::InvalidFormat);
            }
        }

        const REQUIRED: [&str; 9] = [
            "state_version",
            "phase",
            "activity",
            "transport",
            "vehicle",
            "topology",
            "recording",
            "source",
            "safety",
        ];
        if fields.len() != REQUIRED.len() {
            return Err(RuntimeStateError::InvalidFormat);
        }
        for name in REQUIRED {
            if !fields.contains_key(name) {
                return Err(RuntimeStateError::InvalidFormat);
            }
        }

        let version = fields
            .get("state_version")
            .ok_or(RuntimeStateError::InvalidFormat)?
            .parse()
            .map_err(|_| RuntimeStateError::InvalidVersion)?;
        if version != STATE_VERSION {
            return Err(RuntimeStateError::UnsupportedVersion(version));
        }

        Ok(Self {
            state_version: version,
            phase: Phase::parse(fields["phase"])?,
            activity: Activity::parse(fields["activity"])?,
            context: RuntimeContext::new(
                TransportState::parse(fields["transport"])?,
                VehicleState::parse(fields["vehicle"])?,
                TopologyState::parse(fields["topology"])?,
                RecordingState::parse(fields["recording"])?,
                SourceState::parse(fields["source"])?,
                SafetyCapability::parse(fields["safety"])?,
            ),
        })
    }
}

impl fmt::Display for RuntimeState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.serialize())
    }
}

impl FromStr for RuntimeState {
    type Err = RuntimeStateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Errors returned when a serialized runtime state is not understood safely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeStateError {
    InvalidFormat,
    InvalidVersion,
    UnsupportedVersion(u16),
    UnknownIdentifier { field: &'static str, id: String },
}

impl fmt::Display for RuntimeStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => formatter.write_str("invalid runtime-state format"),
            Self::InvalidVersion => formatter.write_str("invalid runtime-state version"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported runtime-state version {version}")
            }
            Self::UnknownIdentifier { field, id } => {
                write!(formatter, "unknown runtime-state {field} identifier {id}")
            }
        }
    }
}

impl std::error::Error for RuntimeStateError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> RuntimeContext {
        RuntimeContext::new(
            TransportState::Connected,
            VehicleState::Identified,
            TopologyState::Validated,
            RecordingState::Active,
            SourceState::Live,
            SafetyCapability::ReadOnly,
        )
    }

    #[test]
    fn public_ids_are_stable_strings() {
        assert_eq!(Phase::Init.id(), "phase/init");
        assert_eq!(Phase::Discover.id(), "phase/discover");
        assert_eq!(Phase::Ready.id(), "phase/ready");
        assert_eq!(Phase::Stopping.id(), "phase/stopping");
        assert_eq!(Phase::Stopped.id(), "phase/stopped");
        assert_eq!(Phase::Fault.id(), "phase/fault");
        assert_eq!(Activity::Idle.id(), "activity/idle");
        assert_eq!(Activity::Read.id(), "activity/read");
        assert_eq!(Activity::Observe.id(), "activity/observe");
        assert_eq!(Activity::Diagnose.id(), "activity/diagnose");
        assert_eq!(Activity::Write.id(), "activity/write");
    }

    #[test]
    fn serialization_is_versioned_deterministic_and_round_trips() {
        let state = RuntimeState::new(Phase::Ready, Activity::Observe, context());
        let serialized = state.serialize();

        assert_eq!(state.state_version(), STATE_VERSION);
        assert_eq!(
            serialized,
            "state_version=1;phase=phase/ready;activity=activity/observe;transport=transport/connected;vehicle=vehicle/identified;topology=topology/validated;recording=recording/active;source=source/live;safety=safety/read-only"
        );
        assert_eq!(RuntimeState::parse(&serialized), Ok(state));
        assert_eq!(serialized, state.to_string());
    }

    #[test]
    fn default_state_uses_current_version() {
        let state = RuntimeState::default();

        assert_eq!(state.state_version(), STATE_VERSION);
        assert_eq!(state.identity(), (Phase::Init, Activity::Idle));
    }

    #[test]
    fn unknown_future_ids_and_versions_fail_closed() {
        let unknown_id = RuntimeState::new(Phase::Ready, Activity::Read, context())
            .serialize()
            .replace("activity/read", "activity/future");
        assert!(matches!(
            RuntimeState::parse(&unknown_id),
            Err(RuntimeStateError::UnknownIdentifier {
                field: "activity",
                ..
            })
        ));

        let unknown_version = RuntimeState::new(Phase::Ready, Activity::Read, context())
            .serialize()
            .replace("state_version=1", "state_version=2");
        assert_eq!(
            RuntimeState::parse(&unknown_version),
            Err(RuntimeStateError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn context_does_not_change_orthogonal_identity() {
        let first = RuntimeState::new(Phase::Ready, Activity::Read, RuntimeContext::default());
        let second = RuntimeState::new(Phase::Ready, Activity::Read, context());

        assert_eq!(first.identity(), second.identity());
        assert_ne!(first, second);
    }

    #[test]
    fn write_is_representable_without_permission() {
        let state = RuntimeState::new(Phase::Ready, Activity::Write, RuntimeContext::default());

        assert_eq!(state.activity(), Activity::Write);
        assert_eq!(state.context().safety(), SafetyCapability::ReadOnly);
        assert_eq!(RuntimeState::parse(&state.serialize()), Ok(state));
    }

    #[test]
    fn observe_is_the_scheduler_activity() {
        let state = RuntimeState::new(Phase::Ready, Activity::Observe, RuntimeContext::default());

        assert_eq!(state.activity(), Activity::Observe);
        assert_ne!(state.activity(), Activity::Read);
    }
}
