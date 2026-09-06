//! Pure ECU-identification normalization driven only by pinned canonical Knowledge.
//!
//! Raw identification evidence stays authoritative. This module may project a supported
//! observation into a fingerprint fact only when the exact canonical definition that produced
//! the observation declares a deterministic decoder whose text representation is suitable for
//! the closed fingerprint vocabulary.

use crate::{
    ecu_identification::{IdentificationObservation, IdentificationResultStatus},
    inventory_facts::NormalizedInventoryFact,
    knowledge_db::{
        AsciiTrim, FingerprintField, KnowledgeCatalog, KnowledgeConfidence, KnowledgeDecoder,
    },
    topology::{Confidence, Provenance},
};

/// Normalize one persisted ECU-identification observation against the exact pinned Knowledge
/// catalog that declared it.
///
/// Binding is fail-closed and happens before status/decoder handling. A stale or foreign
/// observation is therefore never silently ignored merely because it would not produce a fact.
pub fn normalize_identification_observation(
    observation: &IdentificationObservation,
    catalog: &KnowledgeCatalog,
) -> Result<Option<NormalizedInventoryFact>, String> {
    bind_observation(observation, catalog)?;

    let Some(field) = fingerprint_field_for_identification_semantic(observation.semantic()) else {
        return Ok(None);
    };

    if observation.status() != IdentificationResultStatus::Supported {
        return Ok(None);
    }

    let definition = catalog
        .definition(observation.definition_id())
        .expect("binding verified canonical definition existence");
    let value = observation
        .value()
        .ok_or_else(|| "supported ECU identification observation lost its value".to_string())?;

    // Reconstruct the normalized positive UDS response so the canonical response contract is
    // re-applied offline before decoder execution. The persisted value itself remains untouched.
    let request = observation.request();
    let mut response = Vec::with_capacity(3 + value.len());
    response.extend_from_slice(&[0x62, request[1], request[2]]);
    response.extend_from_slice(value);
    let payload = definition.validate_response(&response)?;

    let normalized = match definition.decoder() {
        KnowledgeDecoder::OpaqueBytes => return Ok(None),
        KnowledgeDecoder::Ascii { trim } => decode_ascii(payload, *trim)?,
        // Numeric decoding has no canonical fingerprint-string representation in the current
        // contract. Do not invent formatting in the Core.
        KnowledgeDecoder::LinearInteger { .. } => return Ok(None),
    };

    let provenance = Provenance::new(
        format!(
            "knowledge:{}@{}:{}@{}",
            catalog.pin().repository(),
            catalog.pin().revision(),
            definition.id(),
            definition.version()
        ),
        topology_confidence(definition.provenance().confidence()),
    )
    .map_err(|error| error.to_string())?;

    NormalizedInventoryFact::new(
        observation.expected_responder().clone(),
        field,
        normalized,
        provenance,
    )
    .map(Some)
}

fn bind_observation(
    observation: &IdentificationObservation,
    catalog: &KnowledgeCatalog,
) -> Result<(), String> {
    if observation.knowledge_repository() != catalog.pin().repository() {
        return Err(format!(
            "ECU identification Knowledge repository mismatch: observation {:?}, catalog {:?}",
            observation.knowledge_repository(),
            catalog.pin().repository()
        ));
    }
    if observation.knowledge_revision() != catalog.pin().revision() {
        return Err(format!(
            "ECU identification Knowledge revision mismatch: observation {:?}, catalog {:?}",
            observation.knowledge_revision(),
            catalog.pin().revision()
        ));
    }

    let definition = catalog
        .definition(observation.definition_id())
        .ok_or_else(|| {
            format!(
                "ECU identification definition {:?} is absent from pinned Knowledge",
                observation.definition_id()
            )
        })?;
    if definition.version() != observation.definition_version() {
        return Err(format!(
            "ECU identification definition version mismatch for {:?}: observation {}, catalog {}",
            definition.id(),
            observation.definition_version(),
            definition.version()
        ));
    }
    if definition.semantic() != observation.semantic() {
        return Err(format!(
            "ECU identification semantic mismatch for {:?}: observation {:?}, catalog {:?}",
            definition.id(),
            observation.semantic(),
            definition.semantic()
        ));
    }
    if definition.operation().request_bytes() != observation.request() {
        return Err(format!(
            "ECU identification request mismatch for {:?}: observation {:02X?}, catalog {:02X?}",
            definition.id(),
            observation.request(),
            definition.operation().request_bytes()
        ));
    }
    Ok(())
}

fn fingerprint_field_for_identification_semantic(semantic: &str) -> Option<FingerprintField> {
    match semantic {
        "ecu.boot_software_identification" => Some(FingerprintField::EcuBootSoftwareIdentification),
        "ecu.application_software_identification" => {
            Some(FingerprintField::EcuApplicationSoftwareIdentification)
        }
        "ecu.manufacturer_spare_part_number" => {
            Some(FingerprintField::EcuManufacturerSparePartNumber)
        }
        "ecu.manufacturer_software_number" => {
            Some(FingerprintField::EcuManufacturerSoftwareNumber)
        }
        "ecu.manufacturer_software_version" => {
            Some(FingerprintField::EcuManufacturerSoftwareVersion)
        }
        "ecu.system_supplier_identifier" => Some(FingerprintField::EcuSystemSupplierIdentifier),
        "ecu.manufacturer_hardware_number" => {
            Some(FingerprintField::EcuManufacturerHardwareNumber)
        }
        "ecu.system_supplier_hardware_number" => {
            Some(FingerprintField::EcuSystemSupplierHardwareNumber)
        }
        "ecu.system_supplier_hardware_version" => {
            Some(FingerprintField::EcuSystemSupplierHardwareVersion)
        }
        "ecu.system_supplier_software_number" => {
            Some(FingerprintField::EcuSystemSupplierSoftwareNumber)
        }
        "ecu.system_supplier_software_version" => {
            Some(FingerprintField::EcuSystemSupplierSoftwareVersion)
        }
        "ecu.system_name" => Some(FingerprintField::EcuSystemName),
        // Manufacturing date and serial number are private observed evidence but intentionally
        // not canonical applicability fingerprint fields. Vehicle/manufacturer/role/addressing
        // facts originate from other typed evidence, not an identification payload.
        _ => None,
    }
}

fn decode_ascii(payload: &[u8], trim: AsciiTrim) -> Result<String, String> {
    let end = match trim {
        AsciiTrim::None => payload.len(),
        AsciiTrim::Space => trailing_start(payload, |byte| byte == 0x20),
        AsciiTrim::Nul => trailing_start(payload, |byte| byte == 0x00),
        AsciiTrim::SpaceAndNul => {
            trailing_start(payload, |byte| matches!(byte, 0x00 | 0x20))
        }
    };
    let text = &payload[..end];
    if text.is_empty() {
        return Err("canonical ASCII normalization produced an empty identity value".into());
    }
    if text.iter().any(|byte| *byte > 0x7f) {
        return Err("canonical ASCII normalization encountered a non-ASCII byte".into());
    }
    if text
        .iter()
        .any(|byte| *byte <= 0x1f || *byte == 0x7f)
    {
        return Err("canonical ASCII normalization encountered an untrimmed control byte".into());
    }
    // The 7-bit check above guarantees valid UTF-8 without introducing a fallback or lossy path.
    String::from_utf8(text.to_vec())
        .map_err(|_| "canonical ASCII normalization produced invalid UTF-8".to_string())
}

fn trailing_start(payload: &[u8], allowed: impl Fn(u8) -> bool) -> usize {
    payload
        .iter()
        .rposition(|byte| !allowed(*byte))
        .map_or(0, |index| index + 1)
}

const fn topology_confidence(confidence: KnowledgeConfidence) -> Confidence {
    match confidence {
        KnowledgeConfidence::High => Confidence::High,
        KnowledgeConfidence::Medium => Confidence::Medium,
        KnowledgeConfidence::Low => Confidence::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        effective_knowledge::{EffectiveVehicleKnowledge, SemanticResolutionState},
        knowledge_db::{KnowledgePin, CANONICAL_KNOWLEDGE_REPOSITORY},
        topology::{
            AddressingContext, Protocol, ProtocolContext, RequestAddress, RequestTarget,
        },
        vehicle_cache::{TargetMappingSnapshot, VehicleCacheSnapshot},
    };
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::SystemTime,
    };

    const FIXTURE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical)
    }

    fn responder() -> crate::topology::ResponderIdentity {
        crate::topology::ResponderIdentity::address(context(), "7E8")
    }

    fn target() -> RequestTarget {
        RequestTarget::concrete(context(), RequestAddress::new("elm-header", "7E0"))
    }

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "obdentic-identification-normalization-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("manufacturers/test")).unwrap();
        root
    }

    fn ascii_catalog(semantic: &str, id: &str, trim: &str) -> (PathBuf, KnowledgeCatalog) {
        let root = temp_dir();
        let yaml = format!(
            r#"schema_version: 2
namespace: test.normalization
definitions:
  - id: {id}
    semantic: {semantic}
    version: 1
    applicability:
      kind: generic
      provenance:
        classification: EXPERIMENTAL
        confidence: high
        sources:
          - kind: research
            citation: synthetic normalization fixture
    operation:
      type: uds.read_data_by_identifier
      identifier: "0xF189"
    response:
      positive_service: "0x62"
      identifier_echo: true
    decoder:
      type: ascii
      trim: {trim}
    provenance:
      classification: EXPERIMENTAL
      confidence: medium
      sources:
        - kind: research
          citation: synthetic ASCII decoder fixture
    hardware_validation:
      status: not_validated
"#
        );
        fs::write(root.join("manufacturers/test/fixture.yaml"), yaml).unwrap();
        let pin = KnowledgePin::new(CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION, 2).unwrap();
        let catalog = KnowledgeCatalog::load_from_directory(&root, pin).unwrap();
        (root, catalog)
    }

    fn observation(
        catalog: &KnowledgeCatalog,
        definition_id: &str,
        semantic: &str,
        status: IdentificationResultStatus,
        value: Option<Vec<u8>>,
    ) -> IdentificationObservation {
        let (nrc, errors) = match status {
            IdentificationResultStatus::Supported => (None, Vec::new()),
            IdentificationResultStatus::Unsupported
            | IdentificationResultStatus::NegativeResponse => (Some(0x31), Vec::new()),
            IdentificationResultStatus::Unavailable => (Some(0x22), Vec::new()),
            IdentificationResultStatus::Malformed => (None, vec!["malformed".into()]),
            IdentificationResultStatus::Timeout => (None, vec!["timeout".into()]),
            IdentificationResultStatus::TransportError => (None, vec!["transport".into()]),
            IdentificationResultStatus::NotProbed => (None, vec!["not probed".into()]),
        };
        IdentificationObservation::new(
            target(),
            responder(),
            semantic,
            definition_id,
            1,
            catalog.pin().repository(),
            catalog.pin().revision(),
            [0x22, 0xF1, 0x89],
            status,
            Vec::new(),
            nrc,
            value,
            errors,
        )
        .unwrap()
    }

    #[test]
    fn current_pinned_f189_opaque_bytes_never_become_a_fingerprint() {
        let catalog = KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap();
        let definition = catalog
            .semantic("ecu.manufacturer_software_version")
            .unwrap();
        assert!(matches!(definition.decoder(), KnowledgeDecoder::OpaqueBytes));
        let observation = observation(
            &catalog,
            definition.id(),
            definition.semantic(),
            IdentificationResultStatus::Supported,
            Some(b"9980".to_vec()),
        );
        assert_eq!(
            normalize_identification_observation(&observation, &catalog).unwrap(),
            None
        );
    }

    #[test]
    fn canonical_ascii_trim_policies_are_trailing_only_and_exact() {
        let cases: [(&str, &[u8], &str); 4] = [
            ("none", b" ABC", " ABC"),
            ("space", b" ABC  ", " ABC"),
            ("nul", b"ABC\0\0", "ABC"),
            ("space_and_nul", b"ABC \0 ", "ABC"),
        ];
        for (trim, bytes, expected) in cases {
            let (root, catalog) = ascii_catalog(
                "ecu.manufacturer_software_version",
                "test.ascii",
                trim,
            );
            let observation = observation(
                &catalog,
                "test.ascii",
                "ecu.manufacturer_software_version",
                IdentificationResultStatus::Supported,
                Some(bytes.to_vec()),
            );
            let fact = normalize_identification_observation(&observation, &catalog)
                .unwrap()
                .unwrap();
            assert_eq!(fact.value(), expected);
            assert_eq!(
                fact.field(),
                FingerprintField::EcuManufacturerSoftwareVersion
            );
            assert_eq!(
                fact.provenance().source(),
                format!(
                    "knowledge:{}@{}:test.ascii@1",
                    CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION
                )
            );
            assert_eq!(fact.provenance().confidence(), Confidence::Medium);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn ascii_rejects_non_ascii_controls_and_empty_after_authorized_trim() {
        for (trim, bytes) in [
            ("none", vec![b'A', 0x80]),
            ("nul", vec![0x00, b'A', 0x00]),
            ("space", vec![b'A', 0x00, b' ']),
            ("space_and_nul", vec![b' ', 0x00, b' ']),
        ] {
            let (root, catalog) = ascii_catalog(
                "ecu.manufacturer_software_version",
                "test.ascii",
                trim,
            );
            let observation = observation(
                &catalog,
                "test.ascii",
                "ecu.manufacturer_software_version",
                IdentificationResultStatus::Supported,
                Some(bytes),
            );
            assert!(normalize_identification_observation(&observation, &catalog).is_err());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn non_supported_statuses_never_create_identity_facts() {
        let (root, catalog) = ascii_catalog(
            "ecu.manufacturer_software_version",
            "test.ascii",
            "none",
        );
        for status in [
            IdentificationResultStatus::Unsupported,
            IdentificationResultStatus::NegativeResponse,
            IdentificationResultStatus::Unavailable,
            IdentificationResultStatus::Malformed,
            IdentificationResultStatus::Timeout,
            IdentificationResultStatus::TransportError,
            IdentificationResultStatus::NotProbed,
        ] {
            let observation = observation(
                &catalog,
                "test.ascii",
                "ecu.manufacturer_software_version",
                status,
                None,
            );
            assert_eq!(
                normalize_identification_observation(&observation, &catalog).unwrap(),
                None
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binding_mismatches_fail_before_decoder_execution() {
        let (root, catalog) = ascii_catalog(
            "ecu.manufacturer_software_version",
            "test.ascii",
            "none",
        );
        let exact = observation(
            &catalog,
            "test.ascii",
            "ecu.manufacturer_software_version",
            IdentificationResultStatus::Supported,
            Some(b"9980".to_vec()),
        );
        assert!(normalize_identification_observation(&exact, &catalog)
            .unwrap()
            .is_some());

        let mismatches = [
            IdentificationObservation::new(
                target(), responder(), "ecu.manufacturer_software_version", "test.ascii", 1,
                "other/repository", catalog.pin().revision(), [0x22, 0xF1, 0x89],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
            IdentificationObservation::new(
                target(), responder(), "ecu.manufacturer_software_version", "test.ascii", 1,
                catalog.pin().repository(), "1111111111111111111111111111111111111111", [0x22, 0xF1, 0x89],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
            IdentificationObservation::new(
                target(), responder(), "ecu.manufacturer_software_version", "missing.definition", 1,
                catalog.pin().repository(), catalog.pin().revision(), [0x22, 0xF1, 0x89],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
            IdentificationObservation::new(
                target(), responder(), "ecu.manufacturer_software_version", "test.ascii", 2,
                catalog.pin().repository(), catalog.pin().revision(), [0x22, 0xF1, 0x89],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
            IdentificationObservation::new(
                target(), responder(), "ecu.system_name", "test.ascii", 1,
                catalog.pin().repository(), catalog.pin().revision(), [0x22, 0xF1, 0x89],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
            IdentificationObservation::new(
                target(), responder(), "ecu.manufacturer_software_version", "test.ascii", 1,
                catalog.pin().repository(), catalog.pin().revision(), [0x22, 0xF1, 0x88],
                IdentificationResultStatus::Supported, Vec::new(), None, Some(b"9980".to_vec()), Vec::new(),
            ).unwrap(),
        ];
        for mismatch in &mismatches {
            assert!(normalize_identification_observation(mismatch, &catalog).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn serial_and_manufacturing_date_ascii_remain_non_fingerprint_evidence() {
        for semantic in ["ecu.serial_number", "ecu.manufacturing_date"] {
            let (root, catalog) = ascii_catalog(semantic, "test.ascii", "none");
            let observation = observation(
                &catalog,
                "test.ascii",
                semantic,
                IdentificationResultStatus::Supported,
                Some(b"PRIVATE".to_vec()),
            );
            assert_eq!(
                normalize_identification_observation(&observation, &catalog).unwrap(),
                None
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn normalized_fact_feeds_inventory_projection_and_effective_knowledge_without_mutation() {
        let (root, catalog) = ascii_catalog(
            "ecu.manufacturer_software_version",
            "test.ascii",
            "space",
        );
        let observation = observation(
            &catalog,
            "test.ascii",
            "ecu.manufacturer_software_version",
            IdentificationResultStatus::Supported,
            Some(b"9980  ".to_vec()),
        );
        let raw_before = observation.value().unwrap().to_vec();
        let fact = normalize_identification_observation(&observation, &catalog)
            .unwrap()
            .unwrap();
        assert_eq!(observation.value().unwrap(), raw_before.as_slice());

        let mapping = TargetMappingSnapshot::new(
            None,
            Some(responder()),
            target(),
            Provenance::new("known target", Confidence::High).unwrap(),
        );
        let snapshot = VehicleCacheSnapshot::new([], [], [mapping]);
        let projected = crate::inventory_facts::project_observed_ecu_facts(&snapshot, [fact]).unwrap();
        let effective = EffectiveVehicleKnowledge::resolve(
            &catalog,
            projected.into_iter().map(|ecu| ecu.into_observed()),
        )
        .unwrap();
        let ecu = effective.ecus().next().unwrap();
        assert_eq!(
            ecu.semantic("ecu.manufacturer_software_version")
                .unwrap()
                .state(),
            SemanticResolutionState::ResolvedGeneric
        );
        fs::remove_dir_all(root).unwrap();
    }
}
