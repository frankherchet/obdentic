//! Transport-neutral ECU topology and provenance facts.
//!
//! This module only describes evidence.  It deliberately contains no
//! transport handle and no operation that can issue a diagnostic request.

use std::{borrow::Borrow, fmt};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Confidence {
    Unknown,
    Low,
    Medium,
    High,
    Verified,
}

/// The source and confidence attached to one fact.  A source is required so
/// a later merge cannot turn independent evidence into an untraceable fact.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Provenance {
    source: String,
    confidence: Confidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyError {
    EmptyProvenanceSource,
    ObservationWindowReversed,
    ConfiguredControllerHasNoIdentity,
}

impl Provenance {
    pub fn new(source: impl Into<String>, confidence: Confidence) -> Result<Self, TopologyError> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(TopologyError::EmptyProvenanceSource);
        }
        Ok(Self { source, confidence })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn confidence(&self) -> Confidence {
        self.confidence
    }
}

impl fmt::Display for TopologyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyProvenanceSource => "topology provenance source must not be empty",
            Self::ObservationWindowReversed => "topology observation window ends before it starts",
            Self::ConfiguredControllerHasNoIdentity => {
                "configured controller must have an identity or logical address"
            }
        })
    }
}

impl std::error::Error for TopologyError {}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Protocol {
    Obd2,
    Uds,
    Can,
    Doip,
    Unknown,
    VendorSpecific(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum AddressingContext {
    Functional,
    Physical,
    Unknown,
    VendorSpecific(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ProtocolContext {
    protocol: Protocol,
    addressing: AddressingContext,
}

impl ProtocolContext {
    pub fn new(protocol: Protocol, addressing: AddressingContext) -> Self {
        Self {
            protocol,
            addressing,
        }
    }

    pub fn protocol(&self) -> &Protocol {
        &self.protocol
    }

    pub fn addressing(&self) -> &AddressingContext {
        &self.addressing
    }
}

/// Identity reported by a responder.  Address-looking text stays opaque: no
/// responder header is silently promoted to a concrete CAN identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum ResponderIdentity {
    Address {
        context: ProtocolContext,
        value: String,
    },
    Opaque {
        context: ProtocolContext,
        value: String,
    },
    Unknown {
        context: ProtocolContext,
    },
}

impl ResponderIdentity {
    pub fn address(context: ProtocolContext, value: impl Into<String>) -> Self {
        Self::Address {
            context,
            value: value.into(),
        }
    }

    pub fn opaque(context: ProtocolContext, value: impl Into<String>) -> Self {
        Self::Opaque {
            context,
            value: value.into(),
        }
    }

    pub fn unknown(context: ProtocolContext) -> Self {
        Self::Unknown { context }
    }

    pub fn context(&self) -> &ProtocolContext {
        match self {
            Self::Address { context, .. }
            | Self::Opaque { context, .. }
            | Self::Unknown { context } => context,
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Address { value, .. } | Self::Opaque { value, .. } => Some(value),
            Self::Unknown { .. } => None,
        }
    }
}

/// A concrete target/address used by a read-only request, kept distinct from
/// both responder identities and manufacturer logical addresses.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RequestAddress {
    namespace: String,
    value: String,
}

impl RequestAddress {
    pub fn new(namespace: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            value: value.into(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RequestTarget {
    context: ProtocolContext,
    address: Option<RequestAddress>,
}

impl RequestTarget {
    /// A functional target has no concrete address claim.
    pub fn functional(context: ProtocolContext) -> Self {
        Self {
            context,
            address: None,
        }
    }

    pub fn concrete(context: ProtocolContext, address: RequestAddress) -> Self {
        Self {
            context,
            address: Some(address),
        }
    }

    pub fn context(&self) -> &ProtocolContext {
        &self.context
    }

    pub fn address(&self) -> Option<&RequestAddress> {
        self.address.as_ref()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RequestTargetEvidence {
    target: RequestTarget,
    provenance: Provenance,
}

impl RequestTargetEvidence {
    pub fn new(target: RequestTarget, provenance: Provenance) -> Self {
        Self { target, provenance }
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ConfiguredIdentity {
    authority: String,
    identifier: String,
}

impl ConfiguredIdentity {
    pub fn new(authority: impl Into<String>, identifier: impl Into<String>) -> Self {
        Self {
            authority: authority.into(),
            identifier: identifier.into(),
        }
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }
}

/// A logical address reported by a manufacturer topology source.  This is
/// intentionally not convertible to `RequestAddress` without new evidence.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct LogicalAddress {
    authority: String,
    value: String,
}

impl LogicalAddress {
    pub fn new(authority: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            authority: authority.into(),
            value: value.into(),
        }
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ConfiguredController {
    identity: Option<ConfiguredIdentity>,
    logical_address: Option<LogicalAddress>,
    provenance: Provenance,
}

impl ConfiguredController {
    pub fn new(
        identity: Option<ConfiguredIdentity>,
        logical_address: Option<LogicalAddress>,
        provenance: Provenance,
    ) -> Result<Self, TopologyError> {
        if identity.is_none() && logical_address.is_none() {
            return Err(TopologyError::ConfiguredControllerHasNoIdentity);
        }
        Ok(Self {
            identity,
            logical_address,
            provenance,
        })
    }

    pub fn identity(&self) -> Option<&ConfiguredIdentity> {
        self.identity.as_ref()
    }

    pub fn logical_address(&self) -> Option<&LogicalAddress> {
        self.logical_address.as_ref()
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ObservationWindow {
    first_observed_ms: u64,
    last_observed_ms: u64,
}

impl ObservationWindow {
    pub fn new(first_observed_ms: u64, last_observed_ms: u64) -> Result<Self, TopologyError> {
        if last_observed_ms < first_observed_ms {
            return Err(TopologyError::ObservationWindowReversed);
        }
        Ok(Self {
            first_observed_ms,
            last_observed_ms,
        })
    }

    pub const fn first_observed_ms(self) -> u64 {
        self.first_observed_ms
    }

    pub const fn last_observed_ms(self) -> u64 {
        self.last_observed_ms
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ObservedResponder {
    identity: ResponderIdentity,
    payload: Option<Vec<u8>>,
    observation: Option<ObservationWindow>,
    provenance: Provenance,
}

impl ObservedResponder {
    pub fn new(identity: ResponderIdentity, provenance: Provenance) -> Self {
        Self {
            identity,
            payload: None,
            observation: None,
            provenance,
        }
    }

    pub fn with_payload(mut self, payload: Vec<u8>) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_observation(mut self, observation: ObservationWindow) -> Self {
        self.observation = Some(observation);
        self
    }

    pub fn observed_at(self, timestamp_ms: u64) -> Self {
        self.with_observation(ObservationWindow {
            first_observed_ms: timestamp_ms,
            last_observed_ms: timestamp_ms,
        })
    }

    pub fn identity(&self) -> &ResponderIdentity {
        &self.identity
    }

    pub fn payload(&self) -> Option<&[u8]> {
        self.payload.as_deref()
    }

    pub fn observation(&self) -> Option<ObservationWindow> {
        self.observation
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum ReachabilityState {
    Observed,
    NotObserved,
    Unknown,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct ReachabilityEvidence {
    state: ReachabilityState,
    provenance: Provenance,
}

impl ReachabilityEvidence {
    pub fn new(state: ReachabilityState, provenance: Provenance) -> Self {
        Self { state, provenance }
    }

    pub const fn state(&self) -> ReachabilityState {
        self.state
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum EcuRole {
    Engine,
    Transmission,
    Gateway,
    Unknown,
    VendorSpecific(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct RoleAssignment {
    role: EcuRole,
    provenance: Provenance,
}

impl RoleAssignment {
    pub fn new(role: EcuRole, provenance: Provenance) -> Self {
        Self { role, provenance }
    }

    pub fn role(&self) -> &EcuRole {
        &self.role
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// A node is a container for facts that share a protocol context.  Optional
/// facts remain optional; adding a configured controller never adds a
/// responder, reachability, target, or role.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct EcuNode {
    context: ProtocolContext,
    provenance: Provenance,
    configured: Option<ConfiguredController>,
    responders: Vec<ObservedResponder>,
    reachability: Option<ReachabilityEvidence>,
    request_target: Option<RequestTargetEvidence>,
    role: Option<RoleAssignment>,
}

impl EcuNode {
    pub fn new(context: ProtocolContext, provenance: Provenance) -> Self {
        Self {
            context,
            provenance,
            configured: None,
            responders: Vec::new(),
            reachability: None,
            request_target: None,
            role: None,
        }
    }

    pub fn with_configured_controller(mut self, configured: ConfiguredController) -> Self {
        self.configured = Some(configured);
        self
    }

    pub fn with_observed_responder(mut self, responder: ObservedResponder) -> Self {
        self.responders.push(responder);
        self.responders.sort();
        self
    }

    pub fn with_reachability(mut self, reachability: ReachabilityEvidence) -> Self {
        self.reachability = Some(reachability);
        self
    }

    pub fn with_request_target(mut self, target: RequestTargetEvidence) -> Self {
        self.request_target = Some(target);
        self
    }

    pub fn with_role(mut self, role: RoleAssignment) -> Self {
        self.role = Some(role);
        self
    }

    pub fn context(&self) -> &ProtocolContext {
        &self.context
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn configured_controller(&self) -> Option<&ConfiguredController> {
        self.configured.as_ref()
    }

    pub fn observed_responders(&self) -> &[ObservedResponder] {
        &self.responders
    }

    pub fn reachability(&self) -> Option<&ReachabilityEvidence> {
        self.reachability.as_ref()
    }

    pub fn request_target(&self) -> Option<&RequestTargetEvidence> {
        self.request_target.as_ref()
    }

    pub fn role(&self) -> Option<&RoleAssignment> {
        self.role.as_ref()
    }

    pub fn first_observed_ms(&self) -> Option<u64> {
        self.responders
            .iter()
            .filter_map(|responder| {
                responder
                    .observation()
                    .map(ObservationWindow::first_observed_ms)
            })
            .min()
    }

    pub fn last_observed_ms(&self) -> Option<u64> {
        self.responders
            .iter()
            .filter_map(|responder| {
                responder
                    .observation()
                    .map(ObservationWindow::last_observed_ms)
            })
            .max()
    }
}

/// A deterministic, serialization-ready collection of independent topology
/// facts.  Merge concatenates and sorts; it does not infer links or collapse
/// provenance from separate sources.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct EcuTopology {
    nodes: Vec<EcuNode>,
}

impl EcuTopology {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_nodes(nodes: impl IntoIterator<Item = EcuNode>) -> Self {
        let mut topology = Self {
            nodes: nodes.into_iter().collect(),
        };
        topology.nodes.sort();
        topology
    }

    pub fn push(&mut self, node: EcuNode) {
        self.nodes.push(node);
        self.nodes.sort();
    }

    pub fn merge<T>(mut self, other: T) -> Self
    where
        T: Borrow<Self>,
    {
        self.nodes.extend(other.borrow().nodes.iter().cloned());
        self.nodes.sort();
        self
    }

    pub fn nodes(&self) -> &[EcuNode] {
        &self.nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional)
    }

    fn responder(value: &str) -> ResponderIdentity {
        ResponderIdentity::address(context(), value)
    }

    #[test]
    fn unknown_responder_has_no_role() {
        let node = EcuNode::new(context(), provenance("capture")).with_observed_responder(
            ObservedResponder::new(ResponderIdentity::unknown(context()), provenance("capture")),
        );

        assert!(node.role().is_none());
        assert_eq!(node.observed_responders().len(), 1);
    }

    #[test]
    fn configured_controller_can_exist_without_responder_or_reachability() {
        let configured = ConfiguredController::new(
            Some(ConfiguredIdentity::new("gateway-list", "engine-controller")),
            Some(LogicalAddress::new("vw", "0x01")),
            provenance("gateway installation list"),
        )
        .unwrap();
        let node = EcuNode::new(context(), provenance("gateway installation list"))
            .with_configured_controller(configured);

        assert!(node.configured_controller().is_some());
        assert!(node.observed_responders().is_empty());
        assert!(node.reachability().is_none());
    }

    #[test]
    fn observed_responder_can_exist_without_installation_entry() {
        let node = EcuNode::new(context(), provenance("read capture")).with_observed_responder(
            ObservedResponder::new(responder("7E9"), provenance("read capture")),
        );

        assert!(node.configured_controller().is_none());
        assert_eq!(node.observed_responders().len(), 1);
        assert!(node.reachability().is_none());
    }

    #[test]
    fn merging_preserves_independent_facts_without_mapping_them() {
        let configured =
            EcuTopology::from_nodes([EcuNode::new(context(), provenance("installation list"))
                .with_configured_controller(
                    ConfiguredController::new(
                        Some(ConfiguredIdentity::new("gateway-list", "ecu-1")),
                        None,
                        provenance("installation list"),
                    )
                    .unwrap(),
                )]);
        let observed =
            EcuTopology::from_nodes([EcuNode::new(context(), provenance("read capture"))
                .with_observed_responder(ObservedResponder::new(
                    responder("7E8"),
                    provenance("read capture"),
                ))]);

        let merged = configured.merge(&observed);
        assert_eq!(merged.nodes().len(), 2);
        assert!(merged
            .nodes()
            .iter()
            .all(|node| node.reachability().is_none()));
        assert_eq!(
            merged
                .nodes()
                .iter()
                .filter(|node| node.configured_controller().is_some())
                .count(),
            1
        );
    }

    #[test]
    fn role_requires_explicit_provenance() {
        let node = EcuNode::new(context(), provenance("vehicle knowledge")).with_role(
            RoleAssignment::new(EcuRole::Engine, provenance("vehicle knowledge")),
        );

        assert_eq!(node.role().unwrap().role(), &EcuRole::Engine);
        assert_eq!(
            node.role().unwrap().provenance().source(),
            "vehicle knowledge"
        );
    }

    #[test]
    fn distinct_responders_stay_distinct_with_equal_payloads() {
        let first = ObservedResponder::new(responder("7E8"), provenance("capture"))
            .with_payload(vec![0x41, 0x0c, 0, 0]);
        let second = ObservedResponder::new(responder("7E9"), provenance("capture"))
            .with_payload(vec![0x41, 0x0c, 0, 0]);
        let node = EcuNode::new(context(), provenance("capture"))
            .with_observed_responder(first)
            .with_observed_responder(second);

        assert_eq!(node.observed_responders().len(), 2);
        assert_ne!(
            node.observed_responders()[0].identity(),
            node.observed_responders()[1].identity()
        );
    }

    #[test]
    fn request_target_and_responder_identity_are_separate_types() {
        let target = RequestTargetEvidence::new(
            RequestTarget::concrete(
                ProtocolContext::new(Protocol::Uds, AddressingContext::Physical),
                RequestAddress::new("opaque-address-space", "target-1"),
            ),
            provenance("target mapping"),
        );
        let node = EcuNode::new(context(), provenance("capture")).with_request_target(target);

        assert!(node.request_target().unwrap().target().address().is_some());
        assert!(node.observed_responders().is_empty());
    }

    #[test]
    fn logical_address_is_not_a_request_target() {
        let logical = LogicalAddress::new("manufacturer", "0x01");
        let controller =
            ConfiguredController::new(None, Some(logical), provenance("gateway")).unwrap();
        let node =
            EcuNode::new(context(), provenance("gateway")).with_configured_controller(controller);

        assert!(node.request_target().is_none());
        assert_eq!(
            node.configured_controller()
                .unwrap()
                .logical_address()
                .unwrap()
                .value(),
            "0x01"
        );
    }

    #[test]
    fn observations_and_topologies_have_deterministic_ordering() {
        let first = EcuNode::new(context(), provenance("capture")).with_observed_responder(
            ObservedResponder::new(responder("7E9"), provenance("capture")),
        );
        let second = EcuNode::new(context(), provenance("capture")).with_observed_responder(
            ObservedResponder::new(responder("7E8"), provenance("capture")),
        );
        let left = EcuTopology::from_nodes([first.clone(), second.clone()]);
        let right = EcuTopology::from_nodes([second, first]);

        assert_eq!(left, right);
        assert_eq!(left.clone().merge(right.clone()), right.merge(left));
    }

    #[test]
    fn observation_window_is_monotonic_and_exposed() {
        assert_eq!(
            ObservationWindow::new(20, 10),
            Err(TopologyError::ObservationWindowReversed)
        );
        let node = EcuNode::new(context(), provenance("capture")).with_observed_responder(
            ObservedResponder::new(responder("7E8"), provenance("capture"))
                .with_observation(ObservationWindow::new(10, 20).unwrap()),
        );
        assert_eq!(node.first_observed_ms(), Some(10));
        assert_eq!(node.last_observed_ms(), Some(20));
    }
}
