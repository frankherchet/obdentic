from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if text.count(old) != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {text.count(old)}")
    return text.replace(old, new, 1)


path = Path("src/knowledge_db.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "pub const SUPPORTED_KNOWLEDGE_SCHEMA_VERSION: u32 = 1;",
    "pub const SUPPORTED_KNOWLEDGE_SCHEMA_VERSION: u32 = 2;",
    "schema version",
)

text = replace_once(
    text,
    """pub struct KnowledgeCatalog {
    pin: KnowledgePin,
    definitions: BTreeMap<String, KnowledgeDefinition>,
    sets: BTreeMap<String, KnowledgeSet>,
}""",
    """pub struct KnowledgeCatalog {
    pin: KnowledgePin,
    definitions: BTreeMap<String, KnowledgeDefinition>,
    semantic_definitions: BTreeMap<String, Vec<String>>,
    sets: BTreeMap<String, KnowledgeSet>,
}""",
    "catalog fields",
)

text = replace_once(
    text,
    """        let mut definitions = BTreeMap::new();
        let mut semantics = BTreeMap::<String, String>::new();
        let mut sets = BTreeMap::new();""",
    """        let mut definitions = BTreeMap::new();
        let mut semantic_definitions = BTreeMap::<String, Vec<String>>::new();
        let mut sets = BTreeMap::new();""",
    "catalog maps",
)

text = replace_once(
    text,
    """                if let Some(first_id) =
                    semantics.insert(definition.semantic().to_owned(), definition.id().to_owned())
                {
                    return Err(KnowledgeLoadError::DuplicateSemantic {
                        semantic: definition.semantic().to_owned(),
                        first_id,
                        second_id: definition.id().to_owned(),
                    });
                }
                definitions.insert(definition.id().to_owned(), definition);""",
    """                semantic_definitions
                    .entry(definition.semantic().to_owned())
                    .or_default()
                    .push(definition.id().to_owned());
                definitions.insert(definition.id().to_owned(), definition);""",
    "semantic insertion",
)

text = replace_once(
    text,
    """        let semantic_definitions: BTreeMap<&str, &KnowledgeDefinition> = definitions
            .values()
            .map(|definition| (definition.semantic(), definition))
            .collect();
        for set in sets.values() {
            for member in set.members() {
                if !semantic_definitions.contains_key(member.as_str()) {
                    return Err(KnowledgeLoadError::UnknownSetMember {
                        set: set.id().to_owned(),
                        semantic: member.clone(),
                    });
                }
            }
        }

        if let Some(set) = sets.get(STANDARD_UDS_ECU_IDENTIFICATION_SET) {
            for member in set.members() {
                let definition = semantic_definitions[member.as_str()];
                match definition.operation() {
                    KnowledgeReadOperation::UdsReadDataByIdentifier { did, .. }
                        if *did != VIN_DID => {}
                    KnowledgeReadOperation::UdsReadDataByIdentifier { .. } => {
                        return Err(KnowledgeLoadError::VinInEcuIdentificationSet)
                    }
                }
            }
        }

        Ok(Self {
            pin,
            definitions,
            sets,
        })""",
    """        for (semantic, definition_ids) in &semantic_definitions {
            let mut applicability_keys = BTreeMap::<KnowledgeApplicabilityKey, String>::new();
            for definition_id in definition_ids {
                let definition = &definitions[definition_id];
                let key = definition.applicability().key();
                if let Some(first_id) = applicability_keys.insert(key, definition_id.clone()) {
                    return Err(KnowledgeLoadError::DuplicateApplicability {
                        semantic: semantic.clone(),
                        first_id,
                        second_id: definition_id.clone(),
                    });
                }
            }
        }

        for set in sets.values() {
            for member in set.members() {
                if !semantic_definitions.contains_key(member.as_str()) {
                    return Err(KnowledgeLoadError::UnknownSetMember {
                        set: set.id().to_owned(),
                        semantic: member.clone(),
                    });
                }
            }
        }

        if let Some(set) = sets.get(STANDARD_UDS_ECU_IDENTIFICATION_SET) {
            for member in set.members() {
                for definition_id in &semantic_definitions[member.as_str()] {
                    let definition = &definitions[definition_id];
                    match definition.operation() {
                        KnowledgeReadOperation::UdsReadDataByIdentifier { did, .. }
                            if *did != VIN_DID => {}
                        KnowledgeReadOperation::UdsReadDataByIdentifier { .. } => {
                            return Err(KnowledgeLoadError::VinInEcuIdentificationSet)
                        }
                    }
                }
            }
        }

        Ok(Self {
            pin,
            definitions,
            semantic_definitions,
            sets,
        })""",
    "catalog validation",
)

text = replace_once(
    text,
    """    pub fn semantic(&self, semantic: &str) -> Option<&KnowledgeDefinition> {
        self.definitions
            .values()
            .find(|definition| definition.semantic() == semantic)
    }

    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &KnowledgeDefinition> {
        self.definitions.values()
    }""",
    """    /// Resolve a semantic only when canonical Knowledge has exactly one
    /// definition for it. Applicability-aware consumers should use
    /// `definitions_for_semantic` instead of silently picking a candidate.
    pub fn semantic(&self, semantic: &str) -> Option<&KnowledgeDefinition> {
        let ids = self.semantic_definitions.get(semantic)?;
        if ids.len() != 1 {
            return None;
        }
        self.definitions.get(&ids[0])
    }

    pub fn definitions_for_semantic(&self, semantic: &str) -> Vec<&KnowledgeDefinition> {
        self.semantic_definitions
            .get(semantic)
            .into_iter()
            .flatten()
            .filter_map(|id| self.definitions.get(id))
            .collect()
    }

    pub fn semantic_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.semantic_definitions.keys().map(String::as_str)
    }

    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &KnowledgeDefinition> {
        self.definitions.values()
    }""",
    "catalog semantic access",
)

text = replace_once(
    text,
    """pub struct KnowledgeDefinition {
    id: String,
    semantic: String,
    version: u32,
    description: Option<String>,
    operation: KnowledgeReadOperation,""",
    """pub struct KnowledgeDefinition {
    id: String,
    semantic: String,
    version: u32,
    description: Option<String>,
    applicability: KnowledgeApplicability,
    operation: KnowledgeReadOperation,""",
    "definition applicability field",
)

text = replace_once(
    text,
    """        let operation = KnowledgeReadOperation::from_raw(path, raw.operation)?;""",
    """        let applicability = KnowledgeApplicability::from_raw(path, raw.applicability)?;
        let operation = KnowledgeReadOperation::from_raw(path, raw.operation)?;""",
    "definition applicability parse",
)

text = replace_once(
    text,
    """            version: raw.version,
            description: raw.description,
            operation,""",
    """            version: raw.version,
            description: raw.description,
            applicability,
            operation,""",
    "definition applicability store",
)

text = replace_once(
    text,
    """    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn operation(&self) -> &KnowledgeReadOperation {""",
    """    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn applicability(&self) -> &KnowledgeApplicability {
        &self.applicability
    }

    pub fn operation(&self) -> &KnowledgeReadOperation {""",
    "definition applicability accessor",
)

applicability_types = r'''
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KnowledgeApplicability {
    Generic {
        provenance: KnowledgeProvenance,
    },
    EcuFingerprint {
        predicates: Vec<FingerprintPredicate>,
        provenance: KnowledgeProvenance,
    },
}

impl KnowledgeApplicability {
    fn from_raw(path: &Path, raw: RawApplicability) -> Result<Self, KnowledgeLoadError> {
        let provenance = KnowledgeProvenance::from_raw(path, raw.provenance)?;
        match raw.kind.as_str() {
            "generic" => {
                if raw.predicates.is_some() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "generic applicability must not contain predicates".into(),
                    });
                }
                Ok(Self::Generic { provenance })
            }
            "ecu_fingerprint" => {
                let raw_predicates = raw.predicates.ok_or_else(|| {
                    KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "ecu_fingerprint applicability requires predicates".into(),
                    }
                })?;
                if raw_predicates.is_empty() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "ecu_fingerprint applicability requires at least one predicate"
                            .into(),
                    });
                }
                let predicates = raw_predicates
                    .into_iter()
                    .map(|predicate| FingerprintPredicate::from_raw(path, predicate))
                    .collect::<Result<Vec<_>, _>>()?;
                let unique_fields: BTreeSet<FingerprintField> =
                    predicates.iter().map(FingerprintPredicate::field).collect();
                if unique_fields.len() != predicates.len() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "ecu_fingerprint applicability repeats a predicate field".into(),
                    });
                }
                Ok(Self::EcuFingerprint {
                    predicates,
                    provenance,
                })
            }
            other => Err(KnowledgeLoadError::UnknownApplicability(other.into())),
        }
    }

    pub const fn is_generic(&self) -> bool {
        matches!(self, Self::Generic { .. })
    }

    pub fn predicates(&self) -> &[FingerprintPredicate] {
        match self {
            Self::Generic { .. } => &[],
            Self::EcuFingerprint { predicates, .. } => predicates,
        }
    }

    pub fn provenance(&self) -> &KnowledgeProvenance {
        match self {
            Self::Generic { provenance } | Self::EcuFingerprint { provenance, .. } => provenance,
        }
    }

    fn key(&self) -> KnowledgeApplicabilityKey {
        match self {
            Self::Generic { .. } => KnowledgeApplicabilityKey::Generic,
            Self::EcuFingerprint { predicates, .. } => {
                let mut pairs = predicates
                    .iter()
                    .map(|predicate| (predicate.field, predicate.equals.clone()))
                    .collect::<Vec<_>>();
                pairs.sort();
                KnowledgeApplicabilityKey::EcuFingerprint(pairs)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum KnowledgeApplicabilityKey {
    Generic,
    EcuFingerprint(Vec<(FingerprintField, String)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FingerprintPredicate {
    field: FingerprintField,
    equals: String,
}

impl FingerprintPredicate {
    fn from_raw(path: &Path, raw: RawFingerprintPredicate) -> Result<Self, KnowledgeLoadError> {
        if raw.equals.is_empty() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: "fingerprint predicate equality value must not be empty".into(),
            });
        }
        Ok(Self {
            field: FingerprintField::from_name(&raw.field)?,
            equals: raw.equals,
        })
    }

    pub const fn field(&self) -> FingerprintField {
        self.field
    }

    pub fn equals(&self) -> &str {
        &self.equals
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FingerprintField {
    VehicleManufacturer,
    EcuLogicalRole,
    EcuAddressingFamily,
    EcuBootSoftwareIdentification,
    EcuApplicationSoftwareIdentification,
    EcuManufacturerSparePartNumber,
    EcuManufacturerSoftwareNumber,
    EcuManufacturerSoftwareVersion,
    EcuSystemSupplierIdentifier,
    EcuManufacturerHardwareNumber,
    EcuSystemSupplierHardwareNumber,
    EcuSystemSupplierHardwareVersion,
    EcuSystemSupplierSoftwareNumber,
    EcuSystemSupplierSoftwareVersion,
    EcuSystemName,
}

impl FingerprintField {
    fn from_name(name: &str) -> Result<Self, KnowledgeLoadError> {
        match name {
            "vehicle.manufacturer" => Ok(Self::VehicleManufacturer),
            "ecu.logical_role" => Ok(Self::EcuLogicalRole),
            "ecu.addressing_family" => Ok(Self::EcuAddressingFamily),
            "ecu.boot_software_identification" => Ok(Self::EcuBootSoftwareIdentification),
            "ecu.application_software_identification" => {
                Ok(Self::EcuApplicationSoftwareIdentification)
            }
            "ecu.manufacturer_spare_part_number" => Ok(Self::EcuManufacturerSparePartNumber),
            "ecu.manufacturer_software_number" => Ok(Self::EcuManufacturerSoftwareNumber),
            "ecu.manufacturer_software_version" => Ok(Self::EcuManufacturerSoftwareVersion),
            "ecu.system_supplier_identifier" => Ok(Self::EcuSystemSupplierIdentifier),
            "ecu.manufacturer_hardware_number" => Ok(Self::EcuManufacturerHardwareNumber),
            "ecu.system_supplier_hardware_number" => Ok(Self::EcuSystemSupplierHardwareNumber),
            "ecu.system_supplier_hardware_version" => {
                Ok(Self::EcuSystemSupplierHardwareVersion)
            }
            "ecu.system_supplier_software_number" => Ok(Self::EcuSystemSupplierSoftwareNumber),
            "ecu.system_supplier_software_version" => {
                Ok(Self::EcuSystemSupplierSoftwareVersion)
            }
            "ecu.system_name" => Ok(Self::EcuSystemName),
            other => Err(KnowledgeLoadError::UnknownFingerprintField(other.into())),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VehicleManufacturer => "vehicle.manufacturer",
            Self::EcuLogicalRole => "ecu.logical_role",
            Self::EcuAddressingFamily => "ecu.addressing_family",
            Self::EcuBootSoftwareIdentification => "ecu.boot_software_identification",
            Self::EcuApplicationSoftwareIdentification => "ecu.application_software_identification",
            Self::EcuManufacturerSparePartNumber => "ecu.manufacturer_spare_part_number",
            Self::EcuManufacturerSoftwareNumber => "ecu.manufacturer_software_number",
            Self::EcuManufacturerSoftwareVersion => "ecu.manufacturer_software_version",
            Self::EcuSystemSupplierIdentifier => "ecu.system_supplier_identifier",
            Self::EcuManufacturerHardwareNumber => "ecu.manufacturer_hardware_number",
            Self::EcuSystemSupplierHardwareNumber => "ecu.system_supplier_hardware_number",
            Self::EcuSystemSupplierHardwareVersion => "ecu.system_supplier_hardware_version",
            Self::EcuSystemSupplierSoftwareNumber => "ecu.system_supplier_software_number",
            Self::EcuSystemSupplierSoftwareVersion => "ecu.system_supplier_software_version",
            Self::EcuSystemName => "ecu.system_name",
        }
    }
}

'''
text = replace_once(
    text,
    "#[derive(Clone, Debug, Eq, PartialEq)]\npub enum KnowledgeReadOperation {",
    applicability_types + "#[derive(Clone, Debug, Eq, PartialEq)]\npub enum KnowledgeReadOperation {",
    "applicability types",
)

text = replace_once(
    text,
    """    DuplicateSemantic {
        semantic: String,
        first_id: String,
        second_id: String,
    },
    DuplicateSetId {""",
    """    DuplicateSemantic {
        semantic: String,
        first_id: String,
        second_id: String,
    },
    DuplicateApplicability {
        semantic: String,
        first_id: String,
        second_id: String,
    },
    DuplicateSetId {""",
    "duplicate applicability error",
)

text = replace_once(
    text,
    """    UnknownOperation(String),
    UnknownDecoder(String),""",
    """    UnknownOperation(String),
    UnknownApplicability(String),
    UnknownFingerprintField(String),
    UnknownDecoder(String),""",
    "applicability errors",
)

text = replace_once(
    text,
    """            Self::DuplicateSemantic { semantic, first_id, second_id } => write!(formatter, "duplicate semantic {semantic:?}: {first_id:?} and {second_id:?}"),
            Self::DuplicateSetId { id, path } =>""",
    """            Self::DuplicateSemantic { semantic, first_id, second_id } => write!(formatter, "duplicate semantic {semantic:?}: {first_id:?} and {second_id:?}"),
            Self::DuplicateApplicability { semantic, first_id, second_id } => write!(formatter, "duplicate applicability for semantic {semantic:?}: {first_id:?} and {second_id:?}"),
            Self::DuplicateSetId { id, path } =>""",
    "display duplicate applicability",
)

text = replace_once(
    text,
    """            Self::UnknownOperation(operation) => write!(formatter, "unknown knowledge operation {operation:?}"),
            Self::UnknownDecoder(decoder) =>""",
    """            Self::UnknownOperation(operation) => write!(formatter, "unknown knowledge operation {operation:?}"),
            Self::UnknownApplicability(value) => write!(formatter, "unknown knowledge applicability kind {value:?}"),
            Self::UnknownFingerprintField(value) => write!(formatter, "unknown knowledge fingerprint field {value:?}"),
            Self::UnknownDecoder(decoder) =>""",
    "display applicability errors",
)

text = replace_once(
    text,
    """    description: Option<String>,
    operation: RawOperation,""",
    """    description: Option<String>,
    applicability: RawApplicability,
    operation: RawOperation,""",
    "raw definition applicability",
)

raw_types = r'''
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApplicability {
    #[serde(rename = "kind")]
    kind: String,
    predicates: Option<Vec<RawFingerprintPredicate>>,
    provenance: RawProvenance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFingerprintPredicate {
    field: String,
    equals: String,
}

'''
text = replace_once(
    text,
    "#[derive(Debug, Deserialize)]\n#[serde(deny_unknown_fields)]\nstruct RawOperation {",
    raw_types + "#[derive(Debug, Deserialize)]\n#[serde(deny_unknown_fields)]\nstruct RawOperation {",
    "raw applicability types",
)

marker = "#[cfg(test)]\nmod tests {"
prefix, tests = text.split(marker, 1)
tests = tests.replace(
    "KnowledgePin::new(CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION, 1).unwrap()",
    "KnowledgePin::new(CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION, 2).unwrap()",
)
tests = tests.replace("schema_version: 1", "schema_version: 2")
tests = tests.replace("schema_version = 1", "schema_version = 2")
tests = tests.replace(
    """    operation:
      type: uds.read_data_by_identifier""",
    """    applicability:
      kind: generic
      provenance:
        classification: VERIFIED
        confidence: high
        sources:
          - kind: standard
            citation: ISO 14229-1
    operation:
      type: uds.read_data_by_identifier""",
    1,
)
tests = tests.replace("Path::new(\"unsafe.yaml\"), &text, 1", "Path::new(\"unsafe.yaml\"), &text, 2")
tests = tests.replace("Path::new(\"obd2.yaml\"), &text, 1", "Path::new(\"obd2.yaml\"), &text, 2")
tests = tests.replace("Path::new(\"uds.yaml\"), &valid_document(\"\"), 1", "Path::new(\"uds.yaml\"), &valid_document(\"\"), 2")
tests = tests.replace(
    """    fn unsupported_schema_version_is_rejected_before_definition_conversion() {
        let text = valid_document("").replace("schema_version: 2", "schema_version: 2");
        assert!(matches!(
            parse_document(Path::new("future.yaml"), &text, 1),
            Err(KnowledgeLoadError::UnsupportedSchemaVersion { found: 2, .. })
        ));
    }""",
    """    fn unsupported_schema_version_is_rejected_before_definition_conversion() {
        let text = valid_document("").replace("schema_version: 2", "schema_version: 3");
        assert!(matches!(
            parse_document(Path::new("future.yaml"), &text, 2),
            Err(KnowledgeLoadError::UnsupportedSchemaVersion { found: 3, .. })
        ));
    }""",
)
tests = tests.replace(
    """            &valid_document("").replace("0xF189", "0xF190"),
            1,""",
    """            &valid_document("").replace("0xF189", "0xF190"),
            2,""",
)
text = prefix + marker + tests
path.write_text(text, encoding="utf-8")

Path("knowledge.lock").write_text(
    "repository = frankherchet/obdentic-knowledge\n"
    "revision = b356ff5afb850017ec546945f41d739071c74d76\n"
    "schema_version = 2\n",
    encoding="utf-8",
)

Path("src/effective_knowledge.rs").write_text(r'''//! Pure Effective Vehicle Knowledge resolution.
//!
//! This module performs no adapter, session, transport or network I/O. It
//! combines already-normalized observed ECU identity facts with pinned
//! canonical Knowledge and preserves ambiguity instead of guessing.

use crate::knowledge_db::{
    FingerprintField, HardwareValidation, KnowledgeApplicability, KnowledgeCatalog,
    KnowledgeDefinition, KnowledgeProvenance,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedEcuFacts {
    ecu_id: String,
    facts: BTreeMap<FingerprintField, String>,
}

impl ObservedEcuFacts {
    pub fn new(ecu_id: impl Into<String>) -> Result<Self, String> {
        let ecu_id = ecu_id.into();
        if ecu_id.trim().is_empty() {
            return Err("observed ECU identity requires a non-empty local ECU id".into());
        }
        Ok(Self {
            ecu_id,
            facts: BTreeMap::new(),
        })
    }

    pub fn insert(
        &mut self,
        field: FingerprintField,
        value: impl Into<String>,
    ) -> Result<(), String> {
        let value = value.into();
        if value.is_empty() {
            return Err(format!(
                "normalized ECU fingerprint fact {} must not be empty",
                field.as_str()
            ));
        }
        self.facts.insert(field, value);
        Ok(())
    }

    pub fn with_fact(
        mut self,
        field: FingerprintField,
        value: impl Into<String>,
    ) -> Result<Self, String> {
        self.insert(field, value)?;
        Ok(self)
    }

    pub fn ecu_id(&self) -> &str {
        &self.ecu_id
    }

    pub fn fact(&self, field: FingerprintField) -> Option<&str> {
        self.facts.get(&field).map(String::as_str)
    }

    pub fn facts(&self) -> &BTreeMap<FingerprintField, String> {
        &self.facts
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicabilityMatch {
    Generic,
    ExactMatch,
    PartialCandidate,
    NoMatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticResolutionState {
    ResolvedGeneric,
    ResolvedSpecific,
    InsufficientIdentity,
    Ambiguous,
    NoMatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateResolution {
    definition_id: String,
    definition_version: u32,
    applicability_match: ApplicabilityMatch,
    specificity: usize,
    applicability_provenance: KnowledgeProvenance,
    definition_provenance: KnowledgeProvenance,
    hardware_validation: HardwareValidation,
}

impl CandidateResolution {
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }

    pub const fn definition_version(&self) -> u32 {
        self.definition_version
    }

    pub const fn applicability_match(&self) -> ApplicabilityMatch {
        self.applicability_match
    }

    pub const fn specificity(&self) -> usize {
        self.specificity
    }

    pub fn applicability_provenance(&self) -> &KnowledgeProvenance {
        &self.applicability_provenance
    }

    pub fn definition_provenance(&self) -> &KnowledgeProvenance {
        &self.definition_provenance
    }

    pub fn hardware_validation(&self) -> &HardwareValidation {
        &self.hardware_validation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticResolution {
    semantic: String,
    state: SemanticResolutionState,
    selected_definition_id: Option<String>,
    candidates: Vec<CandidateResolution>,
}

impl SemanticResolution {
    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn state(&self) -> SemanticResolutionState {
        self.state
    }

    pub fn selected_definition_id(&self) -> Option<&str> {
        self.selected_definition_id.as_deref()
    }

    pub fn selected_definition<'a>(
        &self,
        catalog: &'a KnowledgeCatalog,
    ) -> Option<&'a KnowledgeDefinition> {
        self.selected_definition_id()
            .and_then(|id| catalog.definition(id))
    }

    pub fn candidates(&self) -> &[CandidateResolution] {
        &self.candidates
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveEcuKnowledge {
    ecu_id: String,
    resolutions: BTreeMap<String, SemanticResolution>,
}

impl EffectiveEcuKnowledge {
    pub fn ecu_id(&self) -> &str {
        &self.ecu_id
    }

    pub fn semantic(&self, semantic: &str) -> Option<&SemanticResolution> {
        self.resolutions.get(semantic)
    }

    pub fn semantics(&self) -> impl ExactSizeIterator<Item = &SemanticResolution> {
        self.resolutions.values()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveVehicleKnowledge {
    knowledge_repository: String,
    knowledge_revision: String,
    knowledge_schema_version: u32,
    ecus: BTreeMap<String, EffectiveEcuKnowledge>,
}

impl EffectiveVehicleKnowledge {
    pub fn resolve(
        catalog: &KnowledgeCatalog,
        ecus: impl IntoIterator<Item = ObservedEcuFacts>,
    ) -> Result<Self, String> {
        let mut resolved_ecus = BTreeMap::new();
        for ecu in ecus {
            if resolved_ecus.contains_key(ecu.ecu_id()) {
                return Err(format!("duplicate observed ECU id {:?}", ecu.ecu_id()));
            }
            let mut resolutions = BTreeMap::new();
            for semantic in catalog.semantic_ids() {
                let definitions = catalog.definitions_for_semantic(semantic);
                resolutions.insert(
                    semantic.to_owned(),
                    resolve_semantic(semantic, definitions, &ecu),
                );
            }
            resolved_ecus.insert(
                ecu.ecu_id().to_owned(),
                EffectiveEcuKnowledge {
                    ecu_id: ecu.ecu_id().to_owned(),
                    resolutions,
                },
            );
        }
        Ok(Self {
            knowledge_repository: catalog.pin().repository().to_owned(),
            knowledge_revision: catalog.pin().revision().to_owned(),
            knowledge_schema_version: catalog.pin().schema_version(),
            ecus: resolved_ecus,
        })
    }

    pub fn knowledge_repository(&self) -> &str {
        &self.knowledge_repository
    }

    pub fn knowledge_revision(&self) -> &str {
        &self.knowledge_revision
    }

    pub const fn knowledge_schema_version(&self) -> u32 {
        self.knowledge_schema_version
    }

    pub fn ecu(&self, ecu_id: &str) -> Option<&EffectiveEcuKnowledge> {
        self.ecus.get(ecu_id)
    }

    pub fn ecus(&self) -> impl ExactSizeIterator<Item = &EffectiveEcuKnowledge> {
        self.ecus.values()
    }
}

fn resolve_semantic(
    semantic: &str,
    mut definitions: Vec<&KnowledgeDefinition>,
    ecu: &ObservedEcuFacts,
) -> SemanticResolution {
    definitions.sort_by_key(|definition| definition.id());
    let candidates = definitions
        .into_iter()
        .map(|definition| evaluate_candidate(definition, ecu))
        .collect::<Vec<_>>();

    let exact = candidates
        .iter()
        .filter(|candidate| candidate.applicability_match == ApplicabilityMatch::ExactMatch)
        .collect::<Vec<_>>();

    let (state, selected_definition_id) = if !exact.is_empty() {
        let maximum = exact
            .iter()
            .map(|candidate| candidate.specificity)
            .max()
            .expect("non-empty exact candidates have maximum specificity");
        let finalists = exact
            .into_iter()
            .filter(|candidate| candidate.specificity == maximum)
            .collect::<Vec<_>>();
        if finalists.len() == 1 {
            (
                SemanticResolutionState::ResolvedSpecific,
                Some(finalists[0].definition_id.clone()),
            )
        } else {
            (SemanticResolutionState::Ambiguous, None)
        }
    } else if candidates.iter().any(|candidate| {
        candidate.applicability_match == ApplicabilityMatch::PartialCandidate
    }) {
        (SemanticResolutionState::InsufficientIdentity, None)
    } else {
        let generic = candidates
            .iter()
            .filter(|candidate| candidate.applicability_match == ApplicabilityMatch::Generic)
            .collect::<Vec<_>>();
        match generic.as_slice() {
            [candidate] => (
                SemanticResolutionState::ResolvedGeneric,
                Some(candidate.definition_id.clone()),
            ),
            [] => (SemanticResolutionState::NoMatch, None),
            _ => (SemanticResolutionState::Ambiguous, None),
        }
    };

    SemanticResolution {
        semantic: semantic.to_owned(),
        state,
        selected_definition_id,
        candidates,
    }
}

fn evaluate_candidate(
    definition: &KnowledgeDefinition,
    ecu: &ObservedEcuFacts,
) -> CandidateResolution {
    let (applicability_match, specificity) = match definition.applicability() {
        KnowledgeApplicability::Generic { .. } => (ApplicabilityMatch::Generic, 0),
        KnowledgeApplicability::EcuFingerprint { predicates, .. } => {
            let mut missing = false;
            let mut conflict = false;
            for predicate in predicates {
                match ecu.fact(predicate.field()) {
                    Some(value) if value == predicate.equals() => {}
                    Some(_) => conflict = true,
                    None => missing = true,
                }
            }
            let applicability_match = if conflict {
                ApplicabilityMatch::NoMatch
            } else if missing {
                ApplicabilityMatch::PartialCandidate
            } else {
                ApplicabilityMatch::ExactMatch
            };
            (applicability_match, predicates.len())
        }
    };

    CandidateResolution {
        definition_id: definition.id().to_owned(),
        definition_version: definition.version(),
        applicability_match,
        specificity,
        applicability_provenance: definition.applicability().provenance().clone(),
        definition_provenance: definition.provenance().clone(),
        hardware_validation: definition.hardware_validation().clone(),
    }
}
''', encoding="utf-8")

lib = Path("src/lib.rs")
lib_text = lib.read_text(encoding="utf-8")
lib_text = replace_once(
    lib_text,
    "pub mod ecu_identification_discovery;\n",
    "pub mod ecu_identification_discovery;\npub mod effective_knowledge;\n",
    "lib module",
)
lib.write_text(lib_text, encoding="utf-8")

Path("tests/knowledge_db.rs").write_text(r'''use obdentic::knowledge_db::{
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
''', encoding="utf-8")

Path("tests/effective_knowledge.rs").write_text(r'''use obdentic::{
    effective_knowledge::{
        ApplicabilityMatch, EffectiveVehicleKnowledge, ObservedEcuFacts,
        SemanticResolutionState,
    },
    knowledge_db::{
        FingerprintField, KnowledgeCatalog, KnowledgePin, CANONICAL_KNOWLEDGE_REPOSITORY,
    },
};
use std::{fs, path::PathBuf, time::SystemTime};

const FIXTURE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

fn temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "obdentic-effective-knowledge-{}-{nonce}",
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

fn definition(
    id: &str,
    semantic: &str,
    did: &str,
    predicates: &[(&str, &str)],
) -> String {
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
    yaml.push_str(&definition("test.signal.generic", "test.signal", "0x1234", &[]));
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
    let pin = KnowledgePin::new(
        CANONICAL_KNOWLEDGE_REPOSITORY,
        FIXTURE_REVISION,
        2,
    )
    .unwrap();
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
    let resolution = effective.ecu("engine").unwrap().semantic("test.signal").unwrap();
    assert_eq!(resolution.state(), SemanticResolutionState::ResolvedSpecific);
    assert_eq!(resolution.selected_definition_id(), Some("test.signal.variant"));
    assert_eq!(resolution.candidates().len(), 3);
    assert!(resolution.candidates().iter().any(|candidate| {
        candidate.definition_id() == "test.signal.generic"
            && candidate.applicability_match() == ApplicabilityMatch::Generic
    }));
    assert_eq!(
        resolution.selected_definition(&catalog).unwrap().operation().request_bytes(),
        [0x22, 0x12, 0x36]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_specific_identity_blocks_generic_fallback() {
    let (root, catalog) = catalog();
    let effective = EffectiveVehicleKnowledge::resolve(
        &catalog,
        [ecu("engine", Some("9980"), None)],
    )
    .unwrap();
    let resolution = effective.ecu("engine").unwrap().semantic("test.signal").unwrap();
    assert_eq!(
        resolution.state(),
        SemanticResolutionState::ResolvedSpecific,
        "an exact less-specific candidate still resolves; partial only blocks generic when no exact specific exists"
    );
    assert_eq!(resolution.selected_definition_id(), Some("test.signal.software"));

    let no_exact = EffectiveVehicleKnowledge::resolve(&catalog, [ecu("engine-2", None, None)])
        .unwrap();
    let resolution = no_exact
        .ecu("engine-2")
        .unwrap()
        .semantic("test.signal")
        .unwrap();
    assert_eq!(resolution.state(), SemanticResolutionState::InsufficientIdentity);
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
''', encoding="utf-8")

Path("docs/effective-knowledge.md").write_text(r'''# Effective Vehicle Knowledge

Effective Vehicle Knowledge is a pure composition layer between private observed ECU identity facts and pinned canonical Knowledge.

```text
raw responder evidence
  -> protocol normalization
  -> private observed ECU facts
              +
       pinned canonical Knowledge
              -> applicability resolution
              -> effective semantic catalog
```

The resolver performs no adapter, session, transport, Git or network I/O.

## Normalized observed facts

The resolver accepts `ObservedEcuFacts`, keyed by a local ECU-instance identifier and the closed `FingerprintField` vocabulary from Knowledge schema v2. VIN is deliberately absent from that vocabulary.

Values are already-normalized strings and are compared by exact equality. This module does not guess encodings for opaque F18x/F19x payload bytes. Converting raw ECU-identification evidence into normalized facts belongs upstream in the private observed-inventory/Vehicle Knowledge boundary and must itself be justified by deterministic evidence.

In particular, the resolver never performs ASCII guessing, case folding, trimming heuristics, regex/range matching, fuzzy similarity, ML classification or decoded-value plausibility scoring.

## Resolution states

For every ECU instance and semantic, all canonical candidate definitions remain visible. Each candidate is classified as:

- `Generic`
- `ExactMatch`
- `PartialCandidate`
- `NoMatch`

The semantic result is one of:

- `ResolvedSpecific`
- `ResolvedGeneric`
- `InsufficientIdentity`
- `Ambiguous`
- `NoMatch`

Specific exact matches outrank generic knowledge. More exact predicates mean greater specificity. A tie at greatest specificity remains ambiguous.

If there is no exact specific match but at least one specific candidate is only partial because identity evidence is missing, generic fallback is blocked and the result is `InsufficientIdentity`. Generic knowledge is selected only when every specific candidate is proven not to match.

This is deliberately conservative: missing identity evidence never becomes a reason to guess a decoder.

## Provenance

Every candidate result retains:

- definition ID and version
- applicability match and specificity
- applicability provenance/confidence
- definition provenance/confidence
- hardware-validation state

The effective catalog also records the pinned Knowledge repository, revision and schema version. A later capture-provenance slice can persist these identities without changing the matching result.

## Safety boundary

Applicability does not create executable operations. A selected definition remains one of the already typed, read-only `KnowledgeReadOperation` values loaded by `knowledge_db`.

Any later live consumer still follows:

```text
effective definition
  -> closed typed read operation
  -> SubscriptionPolicy where applicable
  -> SafetyPolicy
  -> single-owner DiagnosticSession
```

No resolver API accepts or generates raw CAN/UDS/ELM commands, arbitrary PIDs/DIDs, session control, SecurityAccess, coding/adaptation, actuator operations, DTC clear or writes.

## Current integration boundary

This slice deliberately stops before #87/#88 integration:

- #87 owns the full private observed-inventory/cache model and the deterministic conversion from preserved ECU evidence to normalized identity facts.
- #88 will consume effective semantic availability from this resolver rather than resolving protocol details inside profiles.

Keeping those integrations separate makes the evidence -> facts -> effective Knowledge direction explicit and testable.
''', encoding="utf-8")
