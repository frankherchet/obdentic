//! Transport-free projection from private observed inventory into normalized ECU facts.
//!
//! The projection is deliberately conservative. Raw ECU-identification values remain
//! evidence unless an upstream, reviewed normalization rule has produced an explicit
//! normalized fact. In particular, opaque bytes are never guessed to be UTF-8, ASCII,
//! hexadecimal semantic identity, or another canonical fingerprint representation.

use crate::{
    effective_knowledge::ObservedEcuFacts,
    knowledge_db::FingerprintField,
    topology::{EcuRole, Provenance, ResponderIdentity},
    vehicle_cache::VehicleCacheSnapshot,
};
use std::collections::{BTreeMap, BTreeSet};

/// One explicitly normalized private-inventory fact tied to an already-known responder.
///
/// This type is an input seam for reviewed byte-to-fact normalization. Constructing it does
/// not perform decoding and does not make a raw `IdentificationObservation` applicable by
/// itself.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NormalizedInventoryFact {
    responder: ResponderIdentity,
    field: FingerprintField,
    value: String,
    provenance: Provenance,
}

impl NormalizedInventoryFact {
    pub fn new(
        responder: ResponderIdentity,
        field: FingerprintField,
        value: impl Into<String>,
        provenance: Provenance,
    ) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty() {
            return Err(format!(
                "normalized private-inventory fact {} must not be empty",
                field.as_str()
            ));
        }
        Ok(Self {
            responder,
            field,
            value,
            provenance,
        })
    }

    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub const fn field(&self) -> FingerprintField {
        self.field
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedFactEvidence {
    value: String,
    provenance: Vec<Provenance>,
}

impl ProjectedFactEvidence {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn provenance(&self) -> &[Provenance] {
        &self.provenance
    }
}

/// Resolver-ready facts for one private observed responder.
///
/// `ecu_id` inside `ObservedEcuFacts` is a deterministic local projection identifier. It is
/// not a CAN address, VIN, canonical Knowledge key, or transport request target. The typed
/// responder remains available separately for audit and inventory correlation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedEcuFacts {
    responder: ResponderIdentity,
    observed: ObservedEcuFacts,
    evidence: BTreeMap<FingerprintField, ProjectedFactEvidence>,
}

impl ProjectedEcuFacts {
    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn observed(&self) -> &ObservedEcuFacts {
        &self.observed
    }

    pub fn evidence(&self, field: FingerprintField) -> Option<&ProjectedFactEvidence> {
        self.evidence.get(&field)
    }

    pub fn into_observed(self) -> ObservedEcuFacts {
        self.observed
    }
}

/// Project the existing private cache snapshot into the exact normalized fact input used by
/// `EffectiveVehicleKnowledge`.
///
/// The snapshot itself contributes only facts whose normalization is already explicit in its
/// typed domain model. Today that means a known standard logical role. Raw standard UDS ECU
/// identification payloads are deliberately not decoded here; reviewed normalizers can supply
/// their results through `explicit_facts` while the original raw observation remains in the
/// cache independently.
pub fn project_observed_ecu_facts(
    snapshot: &VehicleCacheSnapshot,
    explicit_facts: impl IntoIterator<Item = NormalizedInventoryFact>,
) -> Result<Vec<ProjectedEcuFacts>, String> {
    let mut responders = BTreeSet::new();
    responders.extend(
        snapshot
            .topology()
            .iter()
            .map(|observation| observation.responder().clone()),
    );
    responders.extend(
        snapshot
            .ecu_capabilities()
            .iter()
            .map(|capability| capability.responder().clone()),
    );
    responders.extend(
        snapshot
            .target_mappings()
            .iter()
            .filter_map(|mapping| mapping.responder().cloned()),
    );
    responders.extend(
        snapshot
            .ecu_identification()
            .iter()
            .map(|observation| observation.expected_responder().clone()),
    );

    let mut explicit_facts = explicit_facts.into_iter().collect::<Vec<_>>();
    explicit_facts.sort();
    for fact in &explicit_facts {
        if !responders.contains(fact.responder()) {
            return Err(format!(
                "normalized fact {} refers to a responder absent from private observed inventory",
                fact.field().as_str()
            ));
        }
    }

    let mut facts_by_responder = responders
        .iter()
        .cloned()
        .map(|responder| (responder, BTreeMap::new()))
        .collect::<BTreeMap<_, BTreeMap<FingerprintField, FactAccumulator>>>();

    for mapping in snapshot.target_mappings() {
        let (Some(responder), Some(role_assignment)) = (mapping.responder(), mapping.role()) else {
            continue;
        };
        let Some(value) = normalized_standard_role(role_assignment.role()) else {
            continue;
        };
        merge_fact(
            facts_by_responder
                .get_mut(responder)
                .expect("target-mapping responder was added to projection inventory"),
            FingerprintField::EcuLogicalRole,
            value,
            role_assignment.provenance().clone(),
        )?;
    }

    for fact in explicit_facts {
        merge_fact(
            facts_by_responder
                .get_mut(fact.responder())
                .expect("explicit fact responder was validated against projection inventory"),
            fact.field(),
            fact.value().to_owned(),
            fact.provenance().clone(),
        )?;
    }

    facts_by_responder
        .into_iter()
        .enumerate()
        .map(|(index, (responder, facts))| {
            let mut observed = ObservedEcuFacts::new(format!("inventory-ecu-{index:04}"))?;
            let mut evidence = BTreeMap::new();
            for (field, fact) in facts {
                observed.insert(field, fact.value.clone())?;
                evidence.insert(
                    field,
                    ProjectedFactEvidence {
                        value: fact.value,
                        provenance: fact.provenance.into_iter().collect(),
                    },
                );
            }
            Ok(ProjectedEcuFacts {
                responder,
                observed,
                evidence,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FactAccumulator {
    value: String,
    provenance: BTreeSet<Provenance>,
}

fn merge_fact(
    facts: &mut BTreeMap<FingerprintField, FactAccumulator>,
    field: FingerprintField,
    value: String,
    provenance: Provenance,
) -> Result<(), String> {
    match facts.get_mut(&field) {
        Some(existing) if existing.value == value => {
            existing.provenance.insert(provenance);
            Ok(())
        }
        Some(existing) => Err(format!(
            "conflicting normalized private-inventory values for {}: {:?} versus {:?}",
            field.as_str(),
            existing.value,
            value
        )),
        None => {
            facts.insert(
                field,
                FactAccumulator {
                    value,
                    provenance: BTreeSet::from([provenance]),
                },
            );
            Ok(())
        }
    }
}

fn normalized_standard_role(role: &EcuRole) -> Option<String> {
    match role {
        EcuRole::Engine => Some("engine".into()),
        EcuRole::Transmission => Some("transmission".into()),
        EcuRole::Gateway => Some("gateway".into()),
        EcuRole::Unknown | EcuRole::VendorSpecific(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ecu_identification::{IdentificationObservation, IdentificationResultStatus},
        effective_knowledge::{EffectiveVehicleKnowledge, SemanticResolutionState},
        knowledge_db::KnowledgeCatalog,
        topology::{
            AddressingContext, Confidence, Protocol, ProtocolContext, RequestAddress,
            RequestTarget, RoleAssignment,
        },
        vehicle_cache::TargetMappingSnapshot,
    };

    fn context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical)
    }

    fn responder(value: &str) -> ResponderIdentity {
        ResponderIdentity::address(context(), value)
    }

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn target(value: &str) -> RequestTarget {
        RequestTarget::concrete(context(), RequestAddress::new("elm-header", value))
    }

    fn mapping(value: &str, role: Option<EcuRole>) -> TargetMappingSnapshot {
        TargetMappingSnapshot::new(
            role.map(|role| RoleAssignment::new(role, provenance("explicit-role"))),
            Some(responder(value)),
            target(if value == "7E8" { "7E0" } else { "7E1" }),
            provenance("target-evidence"),
        )
    }

    #[test]
    fn explicit_known_roles_project_per_responder_without_address_inference() {
        let snapshot = VehicleCacheSnapshot::new(
            [],
            [],
            [
                mapping("7E8", Some(EcuRole::Engine)),
                mapping("7E9", Some(EcuRole::Transmission)),
            ],
        );
        let projected = project_observed_ecu_facts(&snapshot, []).unwrap();
        assert_eq!(projected.len(), 2);
        assert_ne!(
            projected[0].observed().ecu_id(),
            projected[1].observed().ecu_id()
        );

        let engine = projected
            .iter()
            .find(|ecu| ecu.responder() == &responder("7E8"))
            .unwrap();
        let transmission = projected
            .iter()
            .find(|ecu| ecu.responder() == &responder("7E9"))
            .unwrap();
        assert_eq!(
            engine.observed().fact(FingerprintField::EcuLogicalRole),
            Some("engine")
        );
        assert_eq!(
            transmission
                .observed()
                .fact(FingerprintField::EcuLogicalRole),
            Some("transmission")
        );
        assert_eq!(
            engine
                .evidence(FingerprintField::EcuLogicalRole)
                .unwrap()
                .provenance()[0]
                .source(),
            "explicit-role"
        );
    }

    #[test]
    fn responder_address_alone_never_creates_a_role() {
        let snapshot = VehicleCacheSnapshot::new([], [], [mapping("7E8", None)]);
        let projected = project_observed_ecu_facts(&snapshot, []).unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(
            projected[0]
                .observed()
                .fact(FingerprintField::EcuLogicalRole),
            None
        );
    }

    #[test]
    fn unknown_and_vendor_roles_remain_unresolved_without_a_normalization_contract() {
        for role in [EcuRole::Unknown, EcuRole::VendorSpecific("custom".into())] {
            let snapshot = VehicleCacheSnapshot::new([], [], [mapping("7E8", Some(role))]);
            let projected = project_observed_ecu_facts(&snapshot, []).unwrap();
            assert_eq!(
                projected[0]
                    .observed()
                    .fact(FingerprintField::EcuLogicalRole),
                None
            );
        }
    }

    #[test]
    fn raw_identification_outcomes_never_become_normalized_text_facts() {
        let cases = [
            (
                IdentificationResultStatus::Supported,
                None,
                Some(vec![b'9', b'9', b'8', b'0']),
                Vec::new(),
            ),
            (
                IdentificationResultStatus::Unsupported,
                Some(0x31),
                None,
                Vec::new(),
            ),
            (
                IdentificationResultStatus::NegativeResponse,
                Some(0x22),
                None,
                Vec::new(),
            ),
            (
                IdentificationResultStatus::Unavailable,
                Some(0x22),
                None,
                Vec::new(),
            ),
            (
                IdentificationResultStatus::Malformed,
                None,
                None,
                vec!["malformed".into()],
            ),
            (
                IdentificationResultStatus::Timeout,
                None,
                None,
                vec!["timeout".into()],
            ),
            (
                IdentificationResultStatus::TransportError,
                None,
                None,
                vec!["transport".into()],
            ),
            (
                IdentificationResultStatus::NotProbed,
                None,
                None,
                Vec::new(),
            ),
        ];

        for (status, nrc, value, errors) in cases {
            let observation = IdentificationObservation::new(
                target("7E0"),
                responder("7E8"),
                "ecu.manufacturer_software_version",
                "uds.f189.manufacturer_software_version",
                1,
                "frankherchet/obdentic-knowledge",
                "b356ff5afb850017ec546945f41d739071c74d76",
                [0x22, 0xF1, 0x89],
                status,
                Vec::new(),
                nrc,
                value,
                errors,
            )
            .unwrap();
            let snapshot = VehicleCacheSnapshot::with_ecu_identification([], [], [], [observation]);
            let projected = project_observed_ecu_facts(&snapshot, []).unwrap();
            assert_eq!(projected.len(), 1);
            assert!(projected[0].observed().facts().is_empty());
        }
    }

    #[test]
    fn explicit_normalized_facts_merge_equal_provenance_and_reject_conflicts() {
        let snapshot = VehicleCacheSnapshot::new([], [], [mapping("7E8", None)]);
        let field = FingerprintField::EcuManufacturerSoftwareVersion;
        let first =
            NormalizedInventoryFact::new(responder("7E8"), field, "9980", provenance("decoder-a"))
                .unwrap();
        let second =
            NormalizedInventoryFact::new(responder("7E8"), field, "9980", provenance("decoder-b"))
                .unwrap();
        let projected = project_observed_ecu_facts(&snapshot, [second.clone(), first]).unwrap();
        assert_eq!(projected[0].observed().fact(field), Some("9980"));
        assert_eq!(projected[0].evidence(field).unwrap().provenance().len(), 2);

        let conflict =
            NormalizedInventoryFact::new(responder("7E8"), field, "9981", provenance("decoder-c"))
                .unwrap();
        assert!(project_observed_ecu_facts(&snapshot, [second, conflict]).is_err());
    }

    #[test]
    fn explicit_fact_must_belong_to_existing_private_inventory_responder() {
        let snapshot = VehicleCacheSnapshot::new([], [], [mapping("7E8", None)]);
        let fact = NormalizedInventoryFact::new(
            responder("7E9"),
            FingerprintField::EcuSystemName,
            "synthetic-system",
            provenance("fixture"),
        )
        .unwrap();
        assert!(project_observed_ecu_facts(&snapshot, [fact]).is_err());
    }

    #[test]
    fn projection_is_order_independent_and_feeds_effective_knowledge_directly() {
        let snapshot = VehicleCacheSnapshot::new([], [], [mapping("7E8", Some(EcuRole::Engine))]);
        let field = FingerprintField::EcuManufacturerSoftwareVersion;
        let a =
            NormalizedInventoryFact::new(responder("7E8"), field, "9980", provenance("a")).unwrap();
        let b =
            NormalizedInventoryFact::new(responder("7E8"), field, "9980", provenance("b")).unwrap();
        let first = project_observed_ecu_facts(&snapshot, [a.clone(), b.clone()]).unwrap();
        let second = project_observed_ecu_facts(&snapshot, [b, a]).unwrap();
        assert_eq!(first, second);

        let catalog = KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap();
        let effective = EffectiveVehicleKnowledge::resolve(
            &catalog,
            first.into_iter().map(ProjectedEcuFacts::into_observed),
        )
        .unwrap();
        let ecu = effective.ecus().next().unwrap();
        assert_eq!(
            ecu.semantic("ecu.manufacturer_software_version")
                .unwrap()
                .state(),
            SemanticResolutionState::ResolvedGeneric
        );
    }
}
