//! Transport-free projection from private observed inventory into normalized ECU facts.
//!
//! The projection is deliberately conservative. Raw ECU-identification values remain
//! evidence unless an upstream, reviewed normalization rule has produced an explicit
//! normalized fact. In particular, opaque bytes are never guessed to be UTF-8, ASCII,
//! hexadecimal semantic identity, or another canonical fingerprint representation.

use crate::{
    ecu_identification::{IdentificationObservation, IdentificationResultStatus},
    effective_knowledge::ObservedEcuFacts,
    knowledge_db::{
        AsciiTrim, FingerprintField, KnowledgeCatalog, KnowledgeConfidence, KnowledgeDecoder,
    },
    topology::{Confidence, EcuRole, Provenance, ResponderIdentity},
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

/// Normalize one successful, pinned canonical ECU-identification observation.
///
/// This is deliberately an offline, fail-closed seam.  It accepts only the exact
/// canonical definition that produced the observation; private serial/date fields,
/// opaque bytes and numeric decoders remain unprojected evidence.
pub fn normalize_identification_observation(
    observation: &IdentificationObservation,
    catalog: &KnowledgeCatalog,
) -> Result<Option<NormalizedInventoryFact>, String> {
    let definition = catalog
        .definition(observation.definition_id())
        .ok_or_else(|| {
            format!(
                "ECU identification definition {:?} is absent from pinned canonical knowledge",
                observation.definition_id()
            )
        })?;
    if observation.knowledge_repository() != catalog.pin().repository()
        || observation.knowledge_revision() != catalog.pin().revision()
        || observation.definition_version() != definition.version()
        || observation.semantic() != definition.semantic()
        || observation.request() != definition.operation().request_bytes()
    {
        return Err(format!(
            "ECU identification observation does not match canonical definition {}@{}",
            definition.id(),
            definition.version()
        ));
    }
    if observation.status() != IdentificationResultStatus::Supported {
        return Ok(None);
    }

    let Ok(field) = FingerprintField::from_semantic(definition.semantic()) else {
        return Ok(None);
    };
    let KnowledgeDecoder::Ascii { trim } = definition.decoder() else {
        return Ok(None);
    };
    let value = observation
        .value()
        .ok_or_else(|| "supported ECU identification observation has no payload".to_string())?;
    let mut response = vec![
        definition.response().positive_service(),
        observation.request()[1],
        observation.request()[2],
    ];
    response.extend_from_slice(value);
    let value = normalize_ascii(definition.validate_response(&response)?, *trim)?;
    NormalizedInventoryFact::new(
        observation.expected_responder().clone(),
        field,
        value,
        Provenance::new(
            format!(
                "canonical-knowledge:{}@{}:{}@{}",
                catalog.pin().repository(),
                catalog.pin().revision(),
                definition.id(),
                definition.version()
            ),
            confidence(definition.provenance().confidence()),
        )
        .map_err(|error| error.to_string())?,
    )
    .map(Some)
}

const fn confidence(confidence: KnowledgeConfidence) -> Confidence {
    match confidence {
        KnowledgeConfidence::High => Confidence::High,
        KnowledgeConfidence::Medium => Confidence::Medium,
        KnowledgeConfidence::Low => Confidence::Low,
    }
}

fn normalize_ascii(payload: &[u8], trim: AsciiTrim) -> Result<String, String> {
    if !payload.is_ascii() {
        return Err("canonical ASCII decoder received non-ASCII ECU identification bytes".into());
    }
    let trim_byte = |byte: &u8| match trim {
        AsciiTrim::None => false,
        AsciiTrim::Space => *byte == b' ',
        AsciiTrim::Nul => *byte == 0,
        AsciiTrim::SpaceAndNul => matches!(*byte, b' ' | 0),
    };
    let end = payload
        .iter()
        .rposition(|byte| !trim_byte(byte))
        .map_or(0, |index| index + 1);
    let payload = &payload[..end];
    if payload.is_empty() {
        return Err("canonical ASCII decoder produced an empty ECU identification value".into());
    }
    if payload.iter().any(u8::is_ascii_control) {
        return Err("canonical ASCII decoder received control characters".into());
    }
    String::from_utf8(payload.to_vec())
        .map_err(|_| "canonical ASCII decoder received invalid UTF-8 bytes".into())
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

    const TEST_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    fn catalog(semantic: &str, id: &str, did: u16, decoder: &str) -> KnowledgeCatalog {
        let root = std::env::temp_dir().join(format!(
            "obdentic-inventory-facts-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let standards = root.join("standards");
        std::fs::create_dir_all(&standards).unwrap();
        std::fs::write(
            standards.join("fixture.yaml"),
            format!(
                r#"schema_version: 2
namespace: test.inventory
definitions:
  - id: {id}
    semantic: {semantic}
    version: 1
    applicability:
      kind: generic
      provenance:
        classification: VERIFIED
        confidence: high
        sources: [{{kind: standard, citation: fixture}}]
    operation: {{type: uds.read_data_by_identifier, identifier: "0x{did:04X}"}}
    response: {{positive_service: "0x62", identifier_echo: true}}
    decoder: {decoder}
    provenance:
      classification: VERIFIED
      confidence: high
      sources: [{{kind: standard, citation: fixture}}]
    hardware_validation: {{status: not_applicable}}
"#
            ),
        )
        .unwrap();
        let catalog = KnowledgeCatalog::load_from_directory(
            &root,
            crate::knowledge_db::KnowledgePin::new(
                crate::knowledge_db::CANONICAL_KNOWLEDGE_REPOSITORY,
                TEST_REVISION,
                2,
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        catalog
    }

    fn observation(
        catalog: &KnowledgeCatalog,
        semantic: &str,
        id: &str,
        did: u16,
        payload: Vec<u8>,
    ) -> IdentificationObservation {
        IdentificationObservation::new(
            target("7E0"),
            responder("7E8"),
            semantic,
            id,
            1,
            catalog.pin().repository(),
            catalog.pin().revision(),
            [0x22, (did >> 8) as u8, did as u8],
            IdentificationResultStatus::Supported,
            Vec::new(),
            None,
            Some(payload),
            Vec::new(),
        )
        .unwrap()
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
    fn canonical_ascii_decoder_is_the_only_path_from_observation_to_fact() {
        let catalog = catalog(
            "ecu.manufacturer_spare_part_number",
            "test.f187.part_number",
            0xf187,
            "{type: ascii, trim: space_and_nul}",
        );
        let observation = observation(
            &catalog,
            "ecu.manufacturer_spare_part_number",
            "test.f187.part_number",
            0xf187,
            b"1K0-ABC  \0".to_vec(),
        );

        let fact = normalize_identification_observation(&observation, &catalog)
            .unwrap()
            .unwrap();
        assert_eq!(
            fact.field(),
            FingerprintField::EcuManufacturerSparePartNumber
        );
        assert_eq!(fact.value(), "1K0-ABC");
        assert_eq!(observation.value(), Some(b"1K0-ABC  \0".as_slice()));
        assert_eq!(fact.provenance().confidence(), Confidence::High);
        assert_eq!(
            fact.provenance().source(),
            format!(
                "canonical-knowledge:{}@{}:test.f187.part_number@1",
                catalog.pin().repository(),
                catalog.pin().revision()
            )
        );
        let projected = project_observed_ecu_facts(
            &VehicleCacheSnapshot::new([], [], [mapping("7E8", None)]),
            [fact],
        )
        .unwrap();
        let effective = EffectiveVehicleKnowledge::resolve(
            &catalog,
            projected.into_iter().map(ProjectedEcuFacts::into_observed),
        )
        .unwrap();
        assert_eq!(
            effective
                .ecus()
                .next()
                .unwrap()
                .semantic("ecu.manufacturer_spare_part_number")
                .unwrap()
                .state(),
            SemanticResolutionState::ResolvedGeneric
        );
    }

    #[test]
    fn canonical_ascii_trim_and_validation_are_exact() {
        for (trim, payload, expected) in [
            ("none", b"A B".as_slice(), "A B"),
            ("space", b" A  ".as_slice(), " A"),
            ("nul", b"A\0".as_slice(), "A"),
            ("space_and_nul", b"A \0".as_slice(), "A"),
        ] {
            let catalog = catalog(
                "ecu.system_name",
                "test.f197.system_name",
                0xf197,
                &format!("{{type: ascii, trim: {trim}}}"),
            );
            let observation = observation(
                &catalog,
                "ecu.system_name",
                "test.f197.system_name",
                0xf197,
                payload.to_vec(),
            );
            assert_eq!(
                normalize_identification_observation(&observation, &catalog)
                    .unwrap()
                    .unwrap()
                    .value(),
                expected
            );
        }

        let catalog = catalog(
            "ecu.system_name",
            "test.f197.system_name",
            0xf197,
            "{type: ascii, trim: none}",
        );
        for payload in [vec![0x80], b"A\nB".to_vec(), Vec::new()] {
            let observation = observation(
                &catalog,
                "ecu.system_name",
                "test.f197.system_name",
                0xf197,
                payload,
            );
            assert!(normalize_identification_observation(&observation, &catalog).is_err());
        }
    }

    #[test]
    fn opaque_and_private_identification_semantics_remain_evidence_only() {
        for (semantic, id, did, decoder) in [
            (
                "ecu.manufacturer_software_version",
                "test.f189.software_version",
                0xf189,
                "{type: opaque_bytes}",
            ),
            (
                "ecu.serial_number",
                "test.f18c.serial_number",
                0xf18c,
                "{type: ascii, trim: none}",
            ),
        ] {
            let catalog = catalog(semantic, id, did, decoder);
            let observation = observation(&catalog, semantic, id, did, b"PRIVATE".to_vec());
            assert!(normalize_identification_observation(&observation, &catalog)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn canonical_binding_mismatches_fail_closed() {
        let catalog = catalog(
            "ecu.system_name",
            "test.f197.system_name",
            0xf197,
            "{type: ascii, trim: none}",
        );
        for (semantic, id, version, repository, revision, request) in [
            (
                "ecu.system_name",
                "missing.definition",
                1,
                catalog.pin().repository(),
                catalog.pin().revision(),
                [0x22, 0xf1, 0x97],
            ),
            (
                "other.semantic",
                "test.f197.system_name",
                1,
                catalog.pin().repository(),
                catalog.pin().revision(),
                [0x22, 0xf1, 0x97],
            ),
            (
                "ecu.system_name",
                "test.f197.system_name",
                2,
                catalog.pin().repository(),
                catalog.pin().revision(),
                [0x22, 0xf1, 0x97],
            ),
            (
                "ecu.system_name",
                "test.f197.system_name",
                1,
                "other/repository",
                catalog.pin().revision(),
                [0x22, 0xf1, 0x97],
            ),
            (
                "ecu.system_name",
                "test.f197.system_name",
                1,
                catalog.pin().repository(),
                "abcdefabcdefabcdefabcdefabcdefabcdefabcd",
                [0x22, 0xf1, 0x97],
            ),
            (
                "ecu.system_name",
                "test.f197.system_name",
                1,
                catalog.pin().repository(),
                catalog.pin().revision(),
                [0x22, 0xf1, 0x96],
            ),
        ] {
            let observation = IdentificationObservation::new(
                target("7E0"),
                responder("7E8"),
                semantic,
                id,
                version,
                repository,
                revision,
                request,
                IdentificationResultStatus::Supported,
                Vec::new(),
                None,
                Some(b"name".to_vec()),
                Vec::new(),
            )
            .unwrap();
            assert!(normalize_identification_observation(&observation, &catalog).is_err());
        }
    }

    #[test]
    fn non_supported_identification_statuses_never_project_facts() {
        let catalog = catalog(
            "ecu.system_name",
            "test.f197.system_name",
            0xf197,
            "{type: ascii, trim: none}",
        );
        for (status, nrc, errors) in [
            (
                IdentificationResultStatus::Unsupported,
                Some(0x31),
                Vec::new(),
            ),
            (
                IdentificationResultStatus::NegativeResponse,
                Some(0x22),
                Vec::new(),
            ),
            (
                IdentificationResultStatus::Unavailable,
                Some(0x22),
                Vec::new(),
            ),
            (IdentificationResultStatus::Malformed, None, Vec::new()),
            (
                IdentificationResultStatus::Timeout,
                None,
                vec!["timeout".into()],
            ),
            (
                IdentificationResultStatus::TransportError,
                None,
                vec!["transport".into()],
            ),
            (IdentificationResultStatus::NotProbed, None, Vec::new()),
        ] {
            let observation = IdentificationObservation::new(
                target("7E0"),
                responder("7E8"),
                "ecu.system_name",
                "test.f197.system_name",
                1,
                catalog.pin().repository(),
                catalog.pin().revision(),
                [0x22, 0xf1, 0x97],
                status,
                Vec::new(),
                nrc,
                None,
                errors,
            )
            .unwrap();
            assert!(normalize_identification_observation(&observation, &catalog)
                .unwrap()
                .is_none());
        }
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

        let catalog = catalog(
            "ecu.manufacturer_software_version",
            "test.f189.software_version",
            0xf189,
            "{type: opaque_bytes}",
        );
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
