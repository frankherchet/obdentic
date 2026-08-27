//! Read-only functional OBD responder discovery.
//!
//! The session actor owns the adapter and already performs the bounded Mode 01
//! support-page exchange.  This module validates and preserves the resulting
//! evidence without assigning ECU roles or probing addresses.

use crate::{
    ble::{SessionClient, SupportDiscovery},
    topology::{
        AddressingContext, Confidence, EcuNode, EcuTopology, ObservedResponder, Protocol,
        ProtocolContext, Provenance, ResponderIdentity,
    },
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum DiscoveryStatus {
    Observed,
    NotObserved,
    Unsupported,
    Unknown,
}

/// Capability status for one known Mode 01 semantic on one responder.
///
/// Unlike `DiscoveryStatus`, this vocabulary describes a PID result rather
/// than whether a continuation page was observed.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryError {
    RequestNotAllowlisted([u8; 2]),
    MalformedPayload { request: [u8; 2], payload: Vec<u8> },
    MalformedResponderMetadata,
    UnsupportedSemantic(String),
    InvalidProvenance(crate::topology::TopologyError),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestNotAllowlisted(request) => {
                write!(
                    formatter,
                    "functional discovery request is not allowlisted: {request:02X?}"
                )
            }
            Self::MalformedPayload { request, .. } => {
                write!(
                    formatter,
                    "malformed functional discovery payload for {request:02X?}"
                )
            }
            Self::MalformedResponderMetadata => {
                formatter.write_str("functional discovery responder metadata is empty")
            }
            Self::UnsupportedSemantic(semantic) => {
                write!(
                    formatter,
                    "functional discovery semantic is not allowlisted: {semantic}"
                )
            }
            Self::InvalidProvenance(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// A single response to one allowlisted functional support-page request.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct FunctionalPageObservation {
    request: [u8; 2],
    responder: ResponderIdentity,
    payload: Vec<u8>,
    provenance: Provenance,
}

impl FunctionalPageObservation {
    pub fn new(
        request: [u8; 2],
        responder: ResponderIdentity,
        payload: Vec<u8>,
        provenance: Provenance,
    ) -> Result<Self, DiscoveryError> {
        validate_request(request)?;
        if payload.len() != 6 || payload.get(..2) != Some([0x41, request[1]].as_slice()) {
            return Err(DiscoveryError::MalformedPayload { request, payload });
        }
        Ok(Self {
            request,
            responder,
            payload,
            provenance,
        })
    }

    /// Convert adapter metadata to an opaque responder identity.  A header
    /// such as `7E8` is not promoted to a CAN address here.
    pub fn from_responder_metadata(
        request: [u8; 2],
        metadata: Option<&str>,
        payload: Vec<u8>,
        context: ProtocolContext,
        provenance: Provenance,
    ) -> Result<Self, DiscoveryError> {
        let responder = match metadata {
            Some(value) if !value.trim().is_empty() => ResponderIdentity::opaque(context, value),
            Some(_) => return Err(DiscoveryError::MalformedResponderMetadata),
            None => ResponderIdentity::unknown(context),
        };
        Self::new(request, responder, payload, provenance)
    }

    pub fn request(&self) -> [u8; 2] {
        self.request
    }

    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn bitmap(&self) -> u32 {
        u32::from_be_bytes(
            self.payload[2..]
                .try_into()
                .expect("validated support page"),
        )
    }

    fn advertises_continuation(&self) -> bool {
        self.payload[5] & 1 != 0
    }
}

/// Per-responder Mode 01 capability evidence.
///
/// Pages remain separate and retain their original payload and provenance;
/// this type never merges two responders into a vehicle-wide bitmap.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct EcuCapability {
    responder: ResponderIdentity,
    mode01_pages: Vec<FunctionalPageObservation>,
    request_target: Option<crate::topology::RequestTargetEvidence>,
}

impl EcuCapability {
    fn new(responder: ResponderIdentity, mut mode01_pages: Vec<FunctionalPageObservation>) -> Self {
        mode01_pages.sort();
        Self {
            responder,
            mode01_pages,
            request_target: None,
        }
    }

    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn mode01_pages(&self) -> &[FunctionalPageObservation] {
        &self.mode01_pages
    }

    pub fn request_target(&self) -> Option<&crate::topology::RequestTargetEvidence> {
        self.request_target.as_ref()
    }

    /// Return capability for a known catalog semantic without issuing a
    /// request or consulting a global bitmap.
    pub fn status(&self, semantic: &str) -> Result<CapabilityStatus, DiscoveryError> {
        let request = crate::prepare_read(semantic)
            .map_err(|_| DiscoveryError::UnsupportedSemantic(semantic.into()))?;
        Ok(self.pid_status(request.pid()))
    }

    /// Return capability for a catalog PID. Unknown PIDs remain unknown; this
    /// method is observational and cannot authorize an arbitrary probe.
    pub fn pid_status(&self, pid: u8) -> CapabilityStatus {
        if pid == 0 {
            return CapabilityStatus::Unknown;
        }
        let page = (pid.saturating_sub(1)) & !0x1f;
        let page_bits = self
            .mode01_pages
            .iter()
            .filter(|observation| observation.request[1] == page)
            .map(|observation| observation.bitmap() & (1 << (31 - ((pid - 1) & 0x1f))) != 0)
            .collect::<Vec<_>>();
        if !page_bits.is_empty() {
            return match page_bits.as_slice() {
                values if values.iter().all(|value| *value) => CapabilityStatus::Supported,
                values if values.iter().all(|value| !*value) => CapabilityStatus::Unsupported,
                _ => CapabilityStatus::Unknown,
            };
        }

        if page == 0 {
            return CapabilityStatus::Unknown;
        }
        let previous = page.saturating_sub(0x20);
        match self
            .mode01_pages
            .iter()
            .filter(|observation| observation.request[1] == previous)
            .map(FunctionalPageObservation::advertises_continuation)
            .collect::<Vec<_>>()
            .as_slice()
        {
            values if !values.is_empty() && values.iter().all(|value| !*value) => {
                CapabilityStatus::Unsupported
            }
            _ => CapabilityStatus::Unknown,
        }
    }

    pub fn provenances(&self) -> impl Iterator<Item = &Provenance> {
        self.mode01_pages
            .iter()
            .map(FunctionalPageObservation::provenance)
    }
}

/// Ordered observations and derived per-responder continuation state.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct FunctionalResponderDiscovery {
    observations: Vec<FunctionalPageObservation>,
}

impl FunctionalResponderDiscovery {
    pub fn new(observations: impl IntoIterator<Item = FunctionalPageObservation>) -> Self {
        let mut observations = observations.into_iter().collect::<Vec<_>>();
        observations.sort();
        Self { observations }
    }

    /// Use the page data already collected by `SessionClient`.  The current
    /// session API exposes normalized payloads without responder metadata, so
    /// those observations remain explicitly unknown rather than guessed.
    pub fn from_support_discovery(pages: &[SupportDiscovery]) -> Result<Self, DiscoveryError> {
        let context = functional_context();
        let provenance = Provenance::new("Mode 01 functional support discovery", Confidence::High)
            .map_err(DiscoveryError::InvalidProvenance)?;
        pages
            .iter()
            .map(|page| {
                FunctionalPageObservation::new(
                    page.request,
                    ResponderIdentity::unknown(context.clone()),
                    page.response.to_vec(),
                    provenance.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self::new)
    }

    pub async fn from_session(session: &SessionClient) -> Result<Self, String> {
        let pages = session.support_discovery().await?;
        Self::from_support_discovery(&pages).map_err(|error| error.to_string())
    }

    pub fn observations(&self) -> &[FunctionalPageObservation] {
        &self.observations
    }

    pub fn responders(&self) -> Vec<ResponderIdentity> {
        self.observations
            .iter()
            .map(|observation| observation.responder.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Return independent, deterministically ordered capability evidence for
    /// every responder seen in the normalized support-page observations.
    pub fn capabilities(&self) -> Vec<EcuCapability> {
        self.responders()
            .into_iter()
            .map(|responder| {
                let pages = self
                    .observations
                    .iter()
                    .filter(|observation| observation.responder == responder)
                    .cloned()
                    .collect();
                EcuCapability::new(responder, pages)
            })
            .collect()
    }

    pub fn capability(
        &self,
        responder: &ResponderIdentity,
        semantic: &str,
    ) -> Result<CapabilityStatus, DiscoveryError> {
        if let Some(capability) = self
            .capabilities()
            .into_iter()
            .find(|capability| capability.responder() == responder)
        {
            return capability.status(semantic);
        }
        crate::prepare_read(semantic)
            .map(|_| CapabilityStatus::Unknown)
            .map_err(|_| DiscoveryError::UnsupportedSemantic(semantic.into()))
    }

    /// Return the independent status of one responder on one known page.
    pub fn status(
        &self,
        responder: &ResponderIdentity,
        page: u8,
    ) -> Result<DiscoveryStatus, DiscoveryError> {
        let request = [0x01, page];
        validate_request(request)?;
        if self.observations.iter().any(|observation| {
            observation.request == request && observation.responder == *responder
        }) {
            return Ok(DiscoveryStatus::Observed);
        }
        if page == 0 {
            return Ok(DiscoveryStatus::Unknown);
        }

        let previous = page.saturating_sub(0x20);
        let previous_request = [0x01, previous];
        let continuation = self
            .observations
            .iter()
            .filter(|observation| {
                observation.request == previous_request && observation.responder == *responder
            })
            .map(|observation| observation.payload[5] & 1 != 0)
            .collect::<Vec<_>>();
        Ok(match continuation.as_slice() {
            [] => DiscoveryStatus::Unknown,
            values if values.iter().all(|value| *value) => DiscoveryStatus::NotObserved,
            values if values.iter().all(|value| !*value) => DiscoveryStatus::Unsupported,
            _ => DiscoveryStatus::Unknown,
        })
    }

    /// Return only continuation pages whose preceding page explicitly
    /// advertised them for a responder.  The request remains functional and
    /// broadcast; no responder address is fabricated for the next request.
    pub fn next_requests(&self) -> Vec<[u8; 2]> {
        let mut requests = BTreeSet::new();
        for responder in self.responders() {
            for page in known_pages().into_iter().filter(|page| *page != 0) {
                if self.status(&responder, page) == Ok(DiscoveryStatus::NotObserved) {
                    requests.insert([0x01, page]);
                }
            }
        }
        requests.into_iter().collect()
    }

    /// Convert each preserved response pair into an independent topology
    /// observation.  No node receives a logical ECU role.
    pub fn topology(&self) -> EcuTopology {
        let context = functional_context();
        EcuTopology::from_nodes(self.observations.iter().map(|observation| {
            EcuNode::new(context.clone(), observation.provenance.clone()).with_observed_responder(
                ObservedResponder::new(
                    observation.responder.clone(),
                    observation.provenance.clone(),
                )
                .with_payload(observation.payload.clone()),
            )
        }))
    }
}

/// Discover from the support pages already owned and queried by one session
/// actor.  This call itself cannot probe an address or issue a write.
pub async fn discover_functional_responders(
    session: &SessionClient,
) -> Result<FunctionalResponderDiscovery, String> {
    FunctionalResponderDiscovery::from_session(session).await
}

pub fn known_functional_requests() -> Vec<[u8; 2]> {
    known_pages().into_iter().map(|page| [0x01, page]).collect()
}

fn functional_context() -> ProtocolContext {
    ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional)
}

fn known_pages() -> Vec<u8> {
    let mut pages = BTreeSet::from([0]);
    for signal in crate::supported_signals() {
        pages.insert(signal.request().pid() & !0x1f);
    }
    pages.into_iter().collect()
}

fn validate_request(request: [u8; 2]) -> Result<(), DiscoveryError> {
    if request[0] != 0x01 || !known_pages().contains(&request[1]) {
        return Err(DiscoveryError::RequestNotAllowlisted(request));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::Confidence;

    fn provenance() -> Provenance {
        Provenance::new("fake transport", Confidence::High).unwrap()
    }

    fn context() -> ProtocolContext {
        functional_context()
    }

    fn responder(name: &str) -> ResponderIdentity {
        ResponderIdentity::opaque(context(), name)
    }

    fn page(page: u8, responder_name: &str, continuation: bool) -> FunctionalPageObservation {
        FunctionalPageObservation::new(
            [0x01, page],
            responder(responder_name),
            [0x41, page, 0, 0, 0, u8::from(continuation)].into(),
            provenance(),
        )
        .unwrap()
    }

    fn bitmap_page(page: u8, responder_name: &str, bitmap: u32) -> FunctionalPageObservation {
        FunctionalPageObservation::new(
            [0x01, page],
            responder(responder_name),
            [[0x41, page].as_slice(), bitmap.to_be_bytes().as_slice()].concat(),
            provenance(),
        )
        .unwrap()
    }

    fn pid_bit(pid: u8) -> u32 {
        1 << (31 - ((pid - 1) & 0x1f))
    }

    #[test]
    fn discovers_one_responder_on_0100_without_a_role() {
        let discovery = FunctionalResponderDiscovery::new([page(0, "7E8", false)]);
        let responder = responder("7E8");

        assert_eq!(
            discovery.status(&responder, 0),
            Ok(DiscoveryStatus::Observed)
        );
        assert_eq!(discovery.topology().nodes().len(), 1);
        assert!(discovery.topology().nodes()[0].role().is_none());
    }

    #[test]
    fn preserves_two_responders_with_different_bitmaps() {
        let discovery =
            FunctionalResponderDiscovery::new([page(0, "7E8", true), page(0, "7E9", false)]);

        assert_eq!(discovery.responders().len(), 2);
        assert_eq!(discovery.next_requests(), vec![[0x01, 0x20]]);
    }

    #[test]
    fn continuation_is_evaluated_independently_per_responder() {
        let discovery = FunctionalResponderDiscovery::new([
            page(0, "7E8", true),
            page(0, "7E9", false),
            page(0x20, "7E8", true),
        ]);
        assert_eq!(
            discovery.status(&responder("7E8"), 0x20),
            Ok(DiscoveryStatus::Observed)
        );
        assert_eq!(
            discovery.status(&responder("7E9"), 0x20),
            Ok(DiscoveryStatus::Unsupported)
        );
        assert_eq!(discovery.next_requests(), vec![[0x01, 0x40]]);
    }

    #[test]
    fn missing_later_responder_is_not_observed_not_transport_failure() {
        let discovery = FunctionalResponderDiscovery::new([page(0, "7E8", true)]);

        assert_eq!(
            discovery.status(&responder("7E8"), 0x20),
            Ok(DiscoveryStatus::NotObserved)
        );
    }

    #[test]
    fn duplicate_same_responder_payload_is_preserved_without_conflict() {
        let observation = page(0, "7E8", false);
        let discovery = FunctionalResponderDiscovery::new([observation.clone(), observation]);

        assert_eq!(discovery.observations().len(), 2);
        assert_eq!(
            discovery.status(&responder("7E8"), 0),
            Ok(DiscoveryStatus::Observed)
        );
    }

    #[test]
    fn malformed_responder_metadata_is_an_explicit_error() {
        assert_eq!(
            FunctionalPageObservation::from_responder_metadata(
                [0x01, 0],
                Some("  "),
                vec![0x41, 0, 0, 0, 0, 0],
                context(),
                provenance(),
            ),
            Err(DiscoveryError::MalformedResponderMetadata)
        );
    }

    #[test]
    fn rejects_requests_outside_the_known_functional_allowlist() {
        assert_eq!(
            FunctionalPageObservation::new(
                [0x01, 0x01],
                responder("7E8"),
                vec![0x41, 0x01, 0, 0, 0, 0],
                provenance(),
            ),
            Err(DiscoveryError::RequestNotAllowlisted([0x01, 0x01]))
        );
        assert!(known_functional_requests()
            .iter()
            .all(|request| request[0] == 0x01));
    }

    #[test]
    fn rejects_malformed_page_payload() {
        assert!(matches!(
            FunctionalPageObservation::new(
                [0x01, 0],
                responder("7E8"),
                vec![0x41, 0],
                provenance(),
            ),
            Err(DiscoveryError::MalformedPayload { .. })
        ));
    }

    #[test]
    fn unknown_and_unqueried_pages_stay_unknown() {
        let discovery = FunctionalResponderDiscovery::new([page(0, "7E8", false)]);

        assert_eq!(
            discovery.status(&responder("7E8"), 0x20),
            Ok(DiscoveryStatus::Unsupported)
        );
        assert_eq!(
            discovery.status(&responder("7E8"), 0x40),
            Ok(DiscoveryStatus::Unknown)
        );
    }

    #[test]
    fn arrival_order_does_not_change_serializable_observations() {
        let first = page(0, "7E8", false);
        let second = page(0x20, "7E9", false);
        assert_eq!(
            FunctionalResponderDiscovery::new([first.clone(), second.clone()]),
            FunctionalResponderDiscovery::new([second, first])
        );
    }

    #[test]
    fn support_discovery_without_identity_remains_unknown() {
        let discovery = FunctionalResponderDiscovery::from_support_discovery(&[SupportDiscovery {
            request: [0x01, 0],
            response: [0x41, 0, 0, 0, 0, 0],
        }])
        .unwrap();
        assert!(discovery.responders()[0].value().is_none());
    }

    #[test]
    fn keeps_mode01_capabilities_independent_per_responder() {
        let discovery = FunctionalResponderDiscovery::new([
            bitmap_page(0, "7E8", pid_bit(0x0c)),
            bitmap_page(0, "7E9", pid_bit(0x0d)),
        ]);
        let capabilities = discovery.capabilities();

        assert_eq!(capabilities.len(), 2);
        assert_eq!(capabilities[0].responder().value(), Some("7E8"));
        assert_eq!(capabilities[1].responder().value(), Some("7E9"));
        assert_eq!(
            capabilities[0].status("engine.rpm"),
            Ok(CapabilityStatus::Supported)
        );
        assert_eq!(
            capabilities[0].status("vehicle.speed"),
            Ok(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            capabilities[1].status("engine.rpm"),
            Ok(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            capabilities[1].status("vehicle.speed"),
            Ok(CapabilityStatus::Supported)
        );
        assert_eq!(
            discovery.capability(&responder("7E8"), "engine.rpm"),
            Ok(CapabilityStatus::Supported)
        );
    }

    #[test]
    fn distinguishes_unknown_from_an_unadvertised_or_observed_pid() {
        let discovery = FunctionalResponderDiscovery::new([bitmap_page(0, "7E8", pid_bit(0x0c))]);
        let capability = &discovery.capabilities()[0];

        assert_eq!(
            capability.status("engine.rpm"),
            Ok(CapabilityStatus::Supported)
        );
        assert_eq!(
            capability.status("vehicle.speed"),
            Ok(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            capability.status("engine.control_module_voltage"),
            Ok(CapabilityStatus::Unknown)
        );
        assert_eq!(
            capability.status("future.signal"),
            Err(DiscoveryError::UnsupportedSemantic("future.signal".into()))
        );
    }

    #[test]
    fn continuation_capabilities_stay_scoped_to_the_responder() {
        let discovery = FunctionalResponderDiscovery::new([
            page(0, "7E8", true),
            page(0, "7E9", false),
            bitmap_page(0x20, "7E8", pid_bit(0x2c)),
        ]);

        assert_eq!(
            discovery.capability(&responder("7E8"), "engine.egr.commanded"),
            Ok(CapabilityStatus::Supported)
        );
        assert_eq!(
            discovery.capability(&responder("7E9"), "engine.egr.commanded"),
            Ok(CapabilityStatus::Unsupported)
        );
        assert_eq!(
            discovery.capabilities()[0].status("engine.egr.commanded"),
            Ok(CapabilityStatus::Supported)
        );
        assert_eq!(
            discovery.capabilities()[1].status("engine.egr.commanded"),
            Ok(CapabilityStatus::Unsupported)
        );
    }

    #[test]
    fn capability_order_and_page_evidence_are_deterministic() {
        let first = bitmap_page(0, "7E9", pid_bit(0x0d));
        let second = bitmap_page(0x20, "7E9", pid_bit(0x21));
        let left = FunctionalResponderDiscovery::new([first.clone(), second.clone()]);
        let right = FunctionalResponderDiscovery::new([second, first]);

        assert_eq!(left.capabilities(), right.capabilities());
        assert_eq!(left.capabilities()[0].mode01_pages().len(), 2);
        assert_eq!(
            left.capabilities()[0].mode01_pages()[0].request(),
            [0x01, 0]
        );
        assert_eq!(
            left.capabilities()[0]
                .provenances()
                .map(Provenance::source)
                .collect::<Vec<_>>(),
            ["fake transport", "fake transport"]
        );
    }

    #[test]
    fn conflicting_same_page_evidence_is_unknown() {
        let discovery = FunctionalResponderDiscovery::new([
            bitmap_page(0, "7E8", pid_bit(0x0c)),
            bitmap_page(0, "7E8", 0),
        ]);

        assert_eq!(
            discovery.capabilities()[0].status("engine.rpm"),
            Ok(CapabilityStatus::Unknown)
        );
    }
}
