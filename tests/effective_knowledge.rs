use obdentic::{
    effective_knowledge::{
        ApplicabilityMatch, EffectiveVehicleKnowledge, ObservedEcuFacts, SemanticResolutionState,
    },
    knowledge_db::{
        FingerprintField, KnowledgeCatalog, KnowledgePin, CANONICAL_KNOWLEDGE_REPOSITORY,
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

const FIXTURE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
static TEMP_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = TEMP_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "obdentic-effective-knowledge-{}-{nonce}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(path.join("manufacturers/test")).unwrap();
    path
}

fn provenance() -> &'static str {
    r#"      provenance:
        classification: EXPERIMENTAL
        confidence: high
        sources:
          - kind: research
            citation: synthetic applicability fixture
"#
}

fn definition(id: &str, semantic: &str, did: &str, predicates: &[(&str, &str)]) -> String {
    let applicability = if predicates.is_empty() {
        format!("    applicability:\n      kind: generic\n{}", provenance())
    } else {
        let predicate_yaml = predicates
            .iter()
            .map(|(field, value)| {
                format!("        - field: {field}\n          equals: {value:?}\n")
            })
            .collect::<String>();
        format!(
            "    applicability:\n      kind: ecu_fingerprint\n      predicates:\n{predicate_yaml}{}",
            provenance()
        )
    };
    format!(
        r#"  - id: {id}
    semantic: {semantic}
    version: 1
{applicability}    operation:
      type: uds.read_data_by_identifier
      identifier: "{did}"
    response:
      positive_service: "0x62"
      identifier_echo: true
    decoder:
      type: opaque_bytes
    provenance:
      classification: EXPERIMENTAL
      confidence: medium
      sources:
        - kind: research
          citation: synthetic definition fixture
    hardware_validation:
      status: not_validated
"#
    )
}

fn catalog() -> (PathBuf, KnowledgeCatalog) {
    let root = temp_dir();
    let mut yaml = String::from("schema_version: 2\nnamespace: test.effective\ndefinitions:\n");
    yaml.push_str(&definition(
        "test.signal.generic",
        "test.signal",
        "0x1234",
        &[],
    ));
    yaml.push_str(&definition(
        "test.signal.software",
        "test.signal",
        "0x1235",
        &[("ecu.manufacturer_software_version", "9980")],
    ));
    yaml.push_str(&definition(
        "test.signal.variant",
        "test.signal",
        "0x1236",
        &[
            ("ecu.manufacturer_software_version", "9980"),
            ("ecu.manufacturer_hardware_number", "03L907309"),
        ],
    ));
    yaml.push_str(&definition(
        "test.ambiguous.software",
        "test.ambiguous",
        "0x1240",
        &[("ecu.manufacturer_software_version", "9980")],
    ));
    yaml.push_str(&definition(
        "test.ambiguous.hardware",
        "test.ambiguous",
        "0x1241",
        &[("ecu.manufacturer_hardware_number", "03L907309")],
    ));
    yaml.push_str(&definition(
        "test.nomatch.specific",
        "test.nomatch",
        "0x1250",
        &[("ecu.manufacturer_software_version", "9980")],
    ));
    fs::write(root.join("manufacturers/test/fixture.yaml"), yaml).unwrap();
    let pin = KnowledgePin::new(CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION, 2).unwrap();
    let catalog = KnowledgeCatalog::load_from_directory(&root, pin).unwrap();
    (root, catalog)
}

fn ecu(id: &str, software: Option<&str>, hardware: Option<&str>) -> ObservedEcuFacts {
    let mut facts = ObservedEcuFacts::new(id).unwrap();
    if let Some(software) = software {
        facts
            .insert(FingerprintField::EcuManufacturerSoftwareVersion, software)
            .unwrap();
    }
    if let Some(hardware) = hardware {
        facts
            .insert(FingerprintField::EcuManufacturerHardwareNumber, hardware)
            .unwrap();
    }
    facts
}

#[test]
fn exact_more_specific_definition_beats_generic_and_preserves_candidates() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [ecu("engine", Some("9980"), Some("03L907309"))],
    )
    .unwrap();
    let resolution = effective
        .ecu("engine")
        .unwrap()
        .semantic("test.signal")
        .unwrap();
    assert_eq!(
        resolution.state(),
        SemanticResolutionState::ResolvedSpecific
    );
    assert_eq!(
        resolution.selected_definition_id(),
        Some("test.signal.variant")
    );
    assert_eq!(resolution.candidates().len(), 3);
    assert!(resolution.candidates().iter().any(|candidate| {
        candidate.definition_id() == "test.signal.generic"
            && candidate.applicability_match() == ApplicabilityMatch::Generic
    }));
    assert_eq!(
        resolution
            .selected_definition(&catalog)
            .unwrap()
            .operation()
            .request_bytes(),
        [0x22, 0x12, 0x36]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_specific_identity_blocks_generic_fallback() {
    let (root, catalog) = catalog();
    let effective =
        EffectiveVehicleKnowledge::resolve(&catalog, [ecu("engine", Some("9980"), None)]).unwrap();
    let resolution = effective
        .ecu("engine")
        .unwrap()
        .semantic("test.signal")
        .unwrap();
    assert_eq!(
        resolution.state(),
        SemanticResolutionState::ResolvedSpecific,
        "an exact less-specific candidate still resolves; partial only blocks generic when no exact specific exists"
    );
    assert_eq!(
        resolution.selected_definition_id(),
        Some("test.signal.software")
    );

    let no_exact =
        EffectiveVehicleKnowledge::resolve(&catalog, [ecu("engine-2", None, None)]).unwrap();
    let resolution = no_exact
        .ecu("engine-2")
        .unwrap()
        .semantic("test.signal")
        .unwrap();
    assert_eq!(
        resolution.state(),
        SemanticResolutionState::InsufficientIdentity
    );
    assert_eq!(resolution.selected_definition_id(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn proven_nonmatch_allows_generic_and_no_generic_yields_no_match() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [ecu("engine", Some("9978"), Some("different"))],
    )
    .unwrap();
    let ecu = effective.ecu("engine").unwrap();
    assert_eq!(
        ecu.semantic("test.signal").unwrap().state(),
        SemanticResolutionState::ResolvedGeneric
    );
    assert_eq!(
        ecu.semantic("test.nomatch").unwrap().state(),
        SemanticResolutionState::NoMatch
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn equal_specificity_exact_candidates_remain_ambiguous() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [ecu("engine", Some("9980"), Some("03L907309"))],
    )
    .unwrap();
    let resolution = effective
        .ecu("engine")
        .unwrap()
        .semantic("test.ambiguous")
        .unwrap();
    assert_eq!(resolution.state(), SemanticResolutionState::Ambiguous);
    assert_eq!(resolution.selected_definition_id(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn two_ecus_can_resolve_differently_and_input_order_does_not_matter() {
    let (root, catalog) = catalog();
    let first = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [
            ecu("ecu-b", Some("9978"), Some("different")),
            ecu("ecu-a", Some("9980"), Some("03L907309")),
        ],
    )
    .unwrap();
    let second = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [
            ecu("ecu-a", Some("9980"), Some("03L907309")),
            ecu("ecu-b", Some("9978"), Some("different")),
        ],
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first
            .ecu("ecu-a")
            .unwrap()
            .semantic("test.signal")
            .unwrap()
            .selected_definition_id(),
        Some("test.signal.variant")
    );
    assert_eq!(
        first
            .ecu("ecu-b")
            .unwrap()
            .semantic("test.signal")
            .unwrap()
            .selected_definition_id(),
        Some("test.signal.generic")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_fingerprint_resolves_identically_without_any_vin_input() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [
            ecu("vehicle-a/engine", Some("9980"), Some("03L907309")),
            ecu("vehicle-b/engine", Some("9980"), Some("03L907309")),
        ],
    )
    .unwrap();
    let selected = |id| {
        effective
            .ecu(id)
            .unwrap()
            .semantic("test.signal")
            .unwrap()
            .selected_definition_id()
            .unwrap()
    };
    assert_eq!(selected("vehicle-a/engine"), selected("vehicle-b/engine"));
    assert_eq!(effective.knowledge_schema_version(), 2);
    assert_eq!(effective.knowledge_revision(), FIXTURE_REVISION);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn candidate_provenance_and_versions_remain_inspectable() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [ecu("engine", Some("9980"), Some("03L907309"))],
    )
    .unwrap();
    let candidate = effective
        .ecu("engine")
        .unwrap()
        .semantic("test.signal")
        .unwrap()
        .candidates()
        .iter()
        .find(|candidate| candidate.definition_id() == "test.signal.variant")
        .unwrap();
    assert_eq!(candidate.definition_version(), 1);
    assert_eq!(candidate.specificity(), 2);
    assert_eq!(candidate.applicability_provenance().sources().len(), 1);
    assert_eq!(candidate.definition_provenance().sources().len(), 1);
    fs::remove_dir_all(root).unwrap();
}
