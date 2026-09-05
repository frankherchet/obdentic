use obdentic::knowledge_db::{
    KnowledgeApplicability, KnowledgeCatalog, KnowledgeLoadError, KnowledgePin,
    CANONICAL_KNOWLEDGE_REPOSITORY, STANDARD_UDS_ECU_IDENTIFICATION_SET,
};
use std::{fs, path::PathBuf, time::SystemTime};

const PINNED_REVISION: &str = "b356ff5afb850017ec546945f41d739071c74d76";

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "obdentic-knowledge-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn pinned_submodule_catalog_loads_without_git_or_network() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let catalog = KnowledgeCatalog::load_pinned(&root).unwrap();

    assert_eq!(catalog.pin().repository(), CANONICAL_KNOWLEDGE_REPOSITORY);
    assert_eq!(catalog.pin().schema_version(), 2);
    assert_eq!(catalog.pin().revision(), PINNED_REVISION);

    let set = catalog
        .set(STANDARD_UDS_ECU_IDENTIFICATION_SET)
        .expect("pinned Knowledge DB must provide the bounded ECU identification set");
    assert!(set
        .members()
        .contains(&"ecu.manufacturer_software_version".to_string()));
    assert!(!set.members().contains(&"vehicle.vin".to_string()));

    let definition = catalog
        .semantic("ecu.manufacturer_software_version")
        .expect("F189 semantic must resolve uniquely from canonical Knowledge");
    assert!(matches!(
        definition.applicability(),
        KnowledgeApplicability::Generic { .. }
    ));
    assert_eq!(definition.operation().request_bytes(), [0x22, 0xF1, 0x89]);
    assert_eq!(
        definition
            .validate_response(&[0x62, 0xF1, 0x89, b'9', b'9', b'7', b'7'])
            .unwrap(),
        b"9977"
    );
}

fn generic_applicability() -> &'static str {
    r#"    applicability:
      kind: generic
      provenance:
        classification: VERIFIED
        confidence: high
        sources:
          - kind: standard
            citation: synthetic standard fixture
"#
}

#[test]
fn f190_in_standard_ecu_identification_set_is_rejected() {
    let root = temp_dir("f190");
    let standards = root.join("standards/uds");
    fs::create_dir_all(&standards).unwrap();
    fs::write(
        standards.join("vin.yaml"),
        format!(
            r#"schema_version: 2
namespace: test.uds
sets:
  - id: uds.standard.ecu_identification
    version: 1
    members: [vehicle.vin]
definitions:
  - id: test.f190.vin
    semantic: vehicle.vin
    version: 1
{}    operation:
      type: uds.read_data_by_identifier
      identifier: "0xF190"
    response:
      positive_service: "0x62"
      identifier_echo: true
    decoder:
      type: opaque_bytes
    provenance:
      classification: VERIFIED
      confidence: high
      sources:
        - kind: standard
          citation: ISO 14229-1
    hardware_validation:
      status: not_applicable
"#,
            generic_applicability()
        ),
    )
    .unwrap();

    let pin = KnowledgePin::new(
        CANONICAL_KNOWLEDGE_REPOSITORY,
        "0123456789abcdef0123456789abcdef01234567",
        2,
    )
    .unwrap();
    let result = KnowledgeCatalog::load_from_directory(&root, pin);
    assert_eq!(result, Err(KnowledgeLoadError::VinInEcuIdentificationSet));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn raw_request_field_fails_before_any_runnable_operation_exists() {
    let root = temp_dir("raw-request");
    let standards = root.join("standards/uds");
    fs::create_dir_all(&standards).unwrap();
    fs::write(
        standards.join("unsafe.yaml"),
        format!(
            r#"schema_version: 2
namespace: test.unsafe
definitions:
  - id: test.unsafe
    semantic: test.unsafe
    version: 1
{}    operation:
      type: uds.read_data_by_identifier
      identifier: "0xF189"
      raw_request: "27 01"
    response:
      positive_service: "0x62"
      identifier_echo: true
    decoder:
      type: opaque_bytes
    provenance:
      classification: EXPERIMENTAL
      confidence: low
      sources:
        - kind: research
          citation: synthetic rejection fixture
    hardware_validation:
      status: not_validated
"#,
            generic_applicability()
        ),
    )
    .unwrap();

    let pin = KnowledgePin::new(
        CANONICAL_KNOWLEDGE_REPOSITORY,
        "0123456789abcdef0123456789abcdef01234567",
        2,
    )
    .unwrap();
    assert!(matches!(
        KnowledgeCatalog::load_from_directory(&root, pin),
        Err(KnowledgeLoadError::Yaml { .. })
    ));

    fs::remove_dir_all(root).unwrap();
}
