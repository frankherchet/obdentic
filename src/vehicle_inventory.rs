//! Storage-independent view of private, locally observed vehicle/ECU evidence.
//!
//! `VehicleInventory` is deliberately not canonical Vehicle Knowledge. It is a
//! projection of what this installation observed about one concrete vehicle.
//! It has no transport access and cannot promote observations into the pinned
//! canonical Knowledge repository.

use std::collections::BTreeMap;

use crate::{
    ecu_identification::IdentificationObservation,
    topology::{ObservationWindow, Provenance, RequestTarget, ResponderIdentity, RoleAssignment},
    vehicle_cache::{TargetMappingSnapshot, VehicleCache},
};

/// Private current-observation projection for one concrete vehicle.
///
/// The local vehicle ID is the privacy-safe key already owned by the cache. A
/// raw VIN is intentionally not part of this type. Historical cache records
/// remain persisted by `VehicleCache`; this first #87 slice exposes only their
/// count so downstream resolvers cannot accidentally depend on storage text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VehicleInventory {
    local_vehicle_id: String,
    first_seen_ms: u64,
    last_seen_ms: u64,
    ecus: Vec<EcuInstance>,
    unassigned_targets: Vec<ObservedTargetMapping>,
    historical_record_count: usize,
}

impl VehicleInventory {
    pub fn from_cache(cache: &VehicleCache) -> Self {
        let mut builders = BTreeMap::<ResponderIdentity, EcuInstanceBuilder>::new();

        for observation in cache.snapshot().topology() {
            builders
                .entry(observation.responder().clone())
                .or_insert_with(|| EcuInstanceBuilder::new(observation.responder().clone()))
                .responder_evidence
                .push(ObservedResponderEvidence::new(
                    observation.payload().map(<[u8]>::to_vec),
                    observation.observation(),
                    observation.provenance().clone(),
                ));
        }

        for capability in cache.snapshot().ecu_capabilities() {
            let builder = builders
                .entry(capability.responder().clone())
                .or_insert_with(|| EcuInstanceBuilder::new(capability.responder().clone()));
            builder
                .capabilities
                .extend(capability.pages().iter().map(|page| {
                    ObservedCapabilityPage::new(
                        page.request(),
                        page.payload().to_vec(),
                        page.provenance().clone(),
                    )
                }));
        }

        let mut unassigned_targets = Vec::new();
        for mapping in cache.snapshot().target_mappings() {
            let projected = ObservedTargetMapping::from_snapshot(mapping);
            match mapping.responder() {
                Some(responder) => builders
                    .entry(responder.clone())
                    .or_insert_with(|| EcuInstanceBuilder::new(responder.clone()))
                    .targets
                    .push(projected),
                None => unassigned_targets.push(projected),
            }
        }

        for observation in cache.snapshot().ecu_identification() {
            builders
                .entry(observation.expected_responder().clone())
                .or_insert_with(|| {
                    EcuInstanceBuilder::new(observation.expected_responder().clone())
                })
                .identification
                .push(observation.clone());
        }

        let mut ecus = builders
            .into_values()
            .map(EcuInstanceBuilder::finish)
            .collect::<Vec<_>>();
        ecus.sort();
        unassigned_targets.sort();

        Self {
            local_vehicle_id: cache.local_key().to_owned(),
            first_seen_ms: cache.first_seen_ms(),
            last_seen_ms: cache.last_seen_ms(),
            ecus,
            unassigned_targets,
            historical_record_count: cache.history().len(),
        }
    }

    pub fn local_vehicle_id(&self) -> &str {
        &self.local_vehicle_id
    }

    pub const fn first_seen_ms(&self) -> u64 {
        self.first_seen_ms
    }

    pub const fn last_seen_ms(&self) -> u64 {
        self.last_seen_ms
    }

    pub fn ecus(&self) -> &[EcuInstance] {
        &self.ecus
    }

    pub fn unassigned_targets(&self) -> &[ObservedTargetMapping] {
        &self.unassigned_targets
    }

    pub const fn historical_record_count(&self) -> usize {
        self.historical_record_count
    }
}

/// One currently observed ECU grouped by responder evidence.
///
/// `responder` is an observation key, not yet the final persistent stable
/// `EcuInstanceId`. A later #87 persistence slice will introduce that identity
/// explicitly instead of pretending a bus/header identity is permanent.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EcuInstance {
    responder: ResponderIdentity,
    responder_evidence: Vec<ObservedResponderEvidence>,
    capabilities: Vec<ObservedCapabilityPage>,
    targets: Vec<ObservedTargetMapping>,
    identification: Vec<IdentificationObservation>,
}

impl EcuInstance {
    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn responder_evidence(&self) -> &[ObservedResponderEvidence] {
        &self.responder_evidence
    }

    pub fn capabilities(&self) -> &[ObservedCapabilityPage] {
        &self.capabilities
    }

    pub fn targets(&self) -> &[ObservedTargetMapping] {
        &self.targets
    }

    pub fn identification(&self) -> &[IdentificationObservation] {
        &self.identification
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservedResponderEvidence {
    payload: Option<Vec<u8>>,
    observation: Option<ObservationWindow>,
    provenance: Provenance,
}

impl ObservedResponderEvidence {
    pub fn new(
        payload: Option<Vec<u8>>,
        observation: Option<ObservationWindow>,
        provenance: Provenance,
    ) -> Self {
        Self {
            payload,
            observation,
            provenance,
        }
    }

    pub fn payload(&self) -> Option<&[u8]> {
        self.payload.as_deref()
    }

    pub const fn observation(&self) -> Option<ObservationWindow> {
        self.observation
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservedCapabilityPage {
    request: [u8; 2],
    payload: Vec<u8>,
    provenance: Provenance,
}

impl ObservedCapabilityPage {
    pub fn new(request: [u8; 2], payload: Vec<u8>, provenance: Provenance) -> Self {
        Self {
            request,
            payload,
            provenance,
        }
    }

    pub const fn request(&self) -> [u8; 2] {
        self.request
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservedTargetMapping {
    role: Option<RoleAssignment>,
    target: RequestTarget,
    provenance: Provenance,
}

impl ObservedTargetMapping {
    fn from_snapshot(snapshot: &TargetMappingSnapshot) -> Self {
        Self {
            role: snapshot.role().cloned(),
            target: snapshot.target().clone(),
            provenance: snapshot.provenance().clone(),
        }
    }

    pub fn role(&self) -> Option<&RoleAssignment> {
        self.role.as_ref()
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

struct EcuInstanceBuilder {
    responder: ResponderIdentity,
    responder_evidence: Vec<ObservedResponderEvidence>,
    capabilities: Vec<ObservedCapabilityPage>,
    targets: Vec<ObservedTargetMapping>,
    identification: Vec<IdentificationObservation>,
}

impl EcuInstanceBuilder {
    fn new(responder: ResponderIdentity) -> Self {
        Self {
            responder,
            responder_evidence: Vec::new(),
            capabilities: Vec::new(),
            targets: Vec::new(),
            identification: Vec::new(),
        }
    }

    fn finish(mut self) -> EcuInstance {
        self.responder_evidence.sort();
        self.capabilities.sort();
        self.targets.sort();
        self.identification.sort();
        EcuInstance {
            responder: self.responder,
            responder_evidence: self.responder_evidence,
            capabilities: self.capabilities,
            targets: self.targets,
            identification: self.identification,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ecu_identification::{
            IdentificationObservation, IdentificationResponseEvidence, IdentificationResultStatus,
        },
        topology::{AddressingContext, Confidence, Protocol, ProtocolContext, RequestAddress},
        vehicle_cache::{
            CapabilityPageSnapshot, EcuCapabilitySnapshot, TopologyObservation,
            VehicleCacheSnapshot,
        },
    };

    fn context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical)
    }

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::Verified).unwrap()
    }

    fn responder(value: &str) -> ResponderIdentity {
        ResponderIdentity::address(context(), value)
    }

    fn target(value: &str) -> RequestTarget {
        RequestTarget::concrete(context(), RequestAddress::new("elm-header", value))
    }

    fn supported_identification(
        responder_value: &str,
        target_value: &str,
        did: u16,
        semantic: &str,
        value: u8,
    ) -> IdentificationObservation {
        let [high, low] = did.to_be_bytes();
        let response = vec![0x62, high, low, value];
        IdentificationObservation::new(
            target(target_value),
            responder(responder_value),
            semantic,
            format!("test.{did:04x}"),
            1,
            "frankherchet/obdentic-knowledge",
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
            [0x22, high, low],
            IdentificationResultStatus::Supported,
            vec![IdentificationResponseEvidence::new(
                Some(responder(responder_value)),
                response,
            )],
            None,
            Some(vec![value]),
            Vec::new(),
        )
        .unwrap()
    }

    fn sample_cache(reverse: bool) -> VehicleCache {
        let ecu_a = responder("7E8");
        let ecu_b = responder("7E9");
        let mut topology = vec![
            TopologyObservation::new(
                context(),
                ecu_a.clone(),
                Some(vec![0x41, 0x00, 0x01, 0x02, 0x03, 0x04]),
                None,
                provenance("functional discovery A"),
            ),
            TopologyObservation::new(
                context(),
                ecu_b.clone(),
                Some(vec![0x41, 0x00, 0x05, 0x06, 0x07, 0x08]),
                None,
                provenance("functional discovery B"),
            ),
        ];
        let mut capabilities = vec![
            EcuCapabilitySnapshot::new(
                ecu_a.clone(),
                [CapabilityPageSnapshot::new(
                    [0x01, 0x00],
                    vec![0x41, 0x00, 0x01, 0x02, 0x03, 0x04],
                    provenance("capability A"),
                )],
            ),
            EcuCapabilitySnapshot::new(
                ecu_b.clone(),
                [CapabilityPageSnapshot::new(
                    [0x01, 0x00],
                    vec![0x41, 0x00, 0x05, 0x06, 0x07, 0x08],
                    provenance("capability B"),
                )],
            ),
        ];
        let mut mappings = vec![
            TargetMappingSnapshot::new(None, Some(ecu_a), target("7E0"), provenance("target A")),
            TargetMappingSnapshot::new(None, Some(ecu_b), target("7E1"), provenance("target B")),
        ];
        let mut identifications = vec![
            supported_identification(
                "7E8",
                "7E0",
                0xF188,
                "ecu.manufacturer_software_number",
                b'A',
            ),
            supported_identification(
                "7E9",
                "7E1",
                0xF189,
                "ecu.manufacturer_software_version",
                b'B',
            ),
        ];
        if reverse {
            topology.reverse();
            capabilities.reverse();
            mappings.reverse();
            identifications.reverse();
        }
        VehicleCache::with_snapshot(
            "vehicle-local-0001",
            100,
            200,
            VehicleCacheSnapshot::with_ecu_identification(
                topology,
                capabilities,
                mappings,
                identifications,
            ),
            vec!["private historical evidence WVWZZZ1KZ6W000001".into()],
        )
    }

    #[test]
    fn projects_vehicle_to_multiple_independent_ecu_instances() {
        let inventory = VehicleInventory::from_cache(&sample_cache(false));

        assert_eq!(inventory.local_vehicle_id(), "vehicle-local-0001");
        assert_eq!(inventory.first_seen_ms(), 100);
        assert_eq!(inventory.last_seen_ms(), 200);
        assert_eq!(inventory.ecus().len(), 2);
        assert_eq!(inventory.ecus()[0].responder().value(), Some("7E8"));
        assert_eq!(inventory.ecus()[1].responder().value(), Some("7E9"));
        assert_eq!(inventory.ecus()[0].capabilities().len(), 1);
        assert_eq!(inventory.ecus()[1].capabilities().len(), 1);
        assert_eq!(inventory.ecus()[0].targets().len(), 1);
        assert_eq!(inventory.ecus()[1].targets().len(), 1);
        assert_eq!(inventory.ecus()[0].identification().len(), 1);
        assert_eq!(inventory.ecus()[1].identification().len(), 1);
        assert_ne!(
            inventory.ecus()[0].identification()[0].semantic(),
            inventory.ecus()[1].identification()[0].semantic()
        );
    }

    #[test]
    fn projection_is_deterministic_and_does_not_expose_private_history_text() {
        let normal = VehicleInventory::from_cache(&sample_cache(false));
        let reversed = VehicleInventory::from_cache(&sample_cache(true));

        assert_eq!(normal, reversed);
        assert_eq!(normal.historical_record_count(), 1);
        assert!(!format!("{normal:?}").contains("WVWZZZ1KZ6W000001"));
    }

    #[test]
    fn responderless_targets_remain_explicitly_unassigned() {
        let snapshot = VehicleCacheSnapshot::new(
            [],
            [],
            [TargetMappingSnapshot::new(
                None,
                None,
                target("7E2"),
                provenance("configured target without responder"),
            )],
        );
        let cache = VehicleCache::with_snapshot("vehicle-local-0002", 1, 2, snapshot, Vec::new());

        let inventory = VehicleInventory::from_cache(&cache);

        assert!(inventory.ecus().is_empty());
        assert_eq!(inventory.unassigned_targets().len(), 1);
        assert_eq!(
            inventory.unassigned_targets()[0]
                .target()
                .address()
                .unwrap()
                .value(),
            "7E2"
        );
    }
}
