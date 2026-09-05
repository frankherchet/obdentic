//! Pinned, offline canonical Vehicle/ECU Knowledge loading.
//!
//! This module is intentionally transport-free. Canonical YAML can only be
//! translated into closed read-only Rust primitives; it can never carry raw
//! CAN/UDS/ELM command bytes into the diagnostic session.

use crate::protocol::ReadOperation;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

pub const SUPPORTED_KNOWLEDGE_SCHEMA_VERSION: u32 = 2;
pub const CANONICAL_KNOWLEDGE_REPOSITORY: &str = "frankherchet/obdentic-knowledge";
pub const STANDARD_UDS_ECU_IDENTIFICATION_SET: &str = "uds.standard.ecu_identification";
const VIN_DID: u16 = 0xF190;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgePin {
    repository: String,
    revision: String,
    schema_version: u32,
}

impl KnowledgePin {
    pub fn new(
        repository: impl Into<String>,
        revision: impl Into<String>,
        schema_version: u32,
    ) -> Result<Self, KnowledgeLoadError> {
        let repository = repository.into();
        let revision = revision.into();
        if repository.trim().is_empty() {
            return Err(KnowledgeLoadError::InvalidPin(
                "repository must not be empty".into(),
            ));
        }
        if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(KnowledgeLoadError::InvalidPin(
                "revision must be a 40-character hexadecimal Git commit".into(),
            ));
        }
        if schema_version != SUPPORTED_KNOWLEDGE_SCHEMA_VERSION {
            return Err(KnowledgeLoadError::UnsupportedSchemaVersion {
                path: PathBuf::from("knowledge.lock"),
                found: schema_version,
            });
        }
        Ok(Self {
            repository,
            revision: revision.to_ascii_lowercase(),
            schema_version,
        })
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, KnowledgeLoadError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|error| KnowledgeLoadError::Io {
            path: path.to_path_buf(),
            error: error.to_string(),
        })?;
        Self::parse(path, &text)
    }

    fn parse(path: &Path, text: &str) -> Result<Self, KnowledgeLoadError> {
        let mut values = BTreeMap::new();
        for (line_index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(KnowledgeLoadError::InvalidPin(format!(
                    "{}:{}: expected `key = value`",
                    path.display(),
                    line_index + 1
                )));
            };
            let key = key.trim();
            let value = value.trim();
            if !matches!(key, "repository" | "revision" | "schema_version") {
                return Err(KnowledgeLoadError::InvalidPin(format!(
                    "{}:{}: unknown key {key:?}",
                    path.display(),
                    line_index + 1
                )));
            }
            if values.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(KnowledgeLoadError::InvalidPin(format!(
                    "{}:{}: duplicate key {key:?}",
                    path.display(),
                    line_index + 1
                )));
            }
        }

        let repository = values
            .remove("repository")
            .ok_or_else(|| KnowledgeLoadError::InvalidPin("missing repository".into()))?;
        let revision = values
            .remove("revision")
            .ok_or_else(|| KnowledgeLoadError::InvalidPin("missing revision".into()))?;
        let schema_version = values
            .remove("schema_version")
            .ok_or_else(|| KnowledgeLoadError::InvalidPin("missing schema_version".into()))?
            .parse::<u32>()
            .map_err(|_| {
                KnowledgeLoadError::InvalidPin("schema_version must be an integer".into())
            })?;
        Self::new(repository, revision, schema_version)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KnowledgeCatalog {
    pin: KnowledgePin,
    definitions: BTreeMap<String, KnowledgeDefinition>,
    semantic_definitions: BTreeMap<String, Vec<String>>,
    sets: BTreeMap<String, KnowledgeSet>,
}

impl KnowledgeCatalog {
    /// Load the repository-pinned canonical Knowledge DB from an OBDentic
    /// checkout. No network or Git command is used at runtime.
    pub fn load_pinned(project_root: impl AsRef<Path>) -> Result<Self, KnowledgeLoadError> {
        let project_root = project_root.as_ref();
        let pin = KnowledgePin::load(project_root.join("knowledge.lock"))?;
        if pin.repository() != CANONICAL_KNOWLEDGE_REPOSITORY {
            return Err(KnowledgeLoadError::InvalidPin(format!(
                "unexpected canonical repository {:?}; expected {:?}",
                pin.repository(),
                CANONICAL_KNOWLEDGE_REPOSITORY
            )));
        }
        Self::load_from_directory(project_root.join("knowledge"), pin)
    }

    /// Load a pre-pinned knowledge directory. This seam is used by offline
    /// tests and replay tooling without requiring Git or network access.
    pub fn load_from_directory(
        knowledge_root: impl AsRef<Path>,
        pin: KnowledgePin,
    ) -> Result<Self, KnowledgeLoadError> {
        let knowledge_root = knowledge_root.as_ref();
        let files = canonical_yaml_files(knowledge_root)?;
        if files.is_empty() {
            return Err(KnowledgeLoadError::EmptyKnowledgeRepository(
                knowledge_root.to_path_buf(),
            ));
        }

        let mut definitions = BTreeMap::new();
        let mut semantic_definitions = BTreeMap::<String, Vec<String>>::new();
        let mut sets = BTreeMap::new();

        for relative_path in files {
            let path = knowledge_root.join(&relative_path);
            let text = fs::read_to_string(&path).map_err(|error| KnowledgeLoadError::Io {
                path: path.clone(),
                error: error.to_string(),
            })?;
            let document = parse_document(&relative_path, &text, pin.schema_version())?;

            for raw_definition in document.definitions {
                let definition = KnowledgeDefinition::from_raw(&relative_path, raw_definition)?;
                if definitions.contains_key(definition.id()) {
                    return Err(KnowledgeLoadError::DuplicateDefinitionId {
                        id: definition.id().to_owned(),
                        path: relative_path.clone(),
                    });
                }
                semantic_definitions
                    .entry(definition.semantic().to_owned())
                    .or_default()
                    .push(definition.id().to_owned());
                definitions.insert(definition.id().to_owned(), definition);
            }

            for raw_set in document.sets {
                let set = KnowledgeSet::from_raw(&relative_path, raw_set)?;
                let set_id = set.id().to_owned();
                if sets.insert(set_id.clone(), set).is_some() {
                    return Err(KnowledgeLoadError::DuplicateSetId {
                        id: set_id,
                        path: relative_path.clone(),
                    });
                }
            }
        }

        for (semantic, definition_ids) in &semantic_definitions {
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
        })
    }

    pub fn pin(&self) -> &KnowledgePin {
        &self.pin
    }

    pub fn definition(&self, id: &str) -> Option<&KnowledgeDefinition> {
        self.definitions.get(id)
    }

    /// Resolve a semantic only when canonical Knowledge has exactly one
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
    }

    pub fn set(&self, id: &str) -> Option<&KnowledgeSet> {
        self.sets.get(id)
    }

    pub fn sets(&self) -> impl ExactSizeIterator<Item = &KnowledgeSet> {
        self.sets.values()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeSet {
    id: String,
    version: u32,
    description: Option<String>,
    members: Vec<String>,
}

impl KnowledgeSet {
    fn from_raw(path: &Path, raw: RawSet) -> Result<Self, KnowledgeLoadError> {
        if raw.id.trim().is_empty() || raw.version == 0 || raw.members.is_empty() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: format!("invalid definition set {:?}", raw.id),
            });
        }
        let unique: BTreeSet<&str> = raw.members.iter().map(String::as_str).collect();
        if unique.len() != raw.members.len() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: format!("definition set {:?} contains duplicate members", raw.id),
            });
        }
        Ok(Self {
            id: raw.id,
            version: raw.version,
            description: raw.description,
            members: raw.members,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn members(&self) -> &[String] {
        &self.members
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KnowledgeDefinition {
    id: String,
    semantic: String,
    version: u32,
    description: Option<String>,
    applicability: KnowledgeApplicability,
    operation: KnowledgeReadOperation,
    response: KnowledgeResponse,
    decoder: KnowledgeDecoder,
    unit: Option<String>,
    plausible_range: Option<PlausibleRange>,
    provenance: KnowledgeProvenance,
    hardware_validation: HardwareValidation,
    notes: Option<String>,
}

impl KnowledgeDefinition {
    fn from_raw(path: &Path, raw: RawDefinition) -> Result<Self, KnowledgeLoadError> {
        if raw.id.trim().is_empty() || raw.semantic.trim().is_empty() || raw.version == 0 {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: "definition id/semantic must be non-empty and version must be positive"
                    .into(),
            });
        }
        let applicability = KnowledgeApplicability::from_raw(path, raw.applicability)?;
        let operation = KnowledgeReadOperation::from_raw(path, raw.operation)?;
        let response = KnowledgeResponse::from_raw(path, &operation, raw.response)?;
        let decoder = KnowledgeDecoder::from_raw(path, raw.decoder)?;
        let plausible_range = raw
            .plausible_range
            .map(PlausibleRange::from_raw)
            .transpose()?;
        let provenance = KnowledgeProvenance::from_raw(path, raw.provenance)?;
        let hardware_validation = HardwareValidation::from_raw(path, raw.hardware_validation)?;
        Ok(Self {
            id: raw.id,
            semantic: raw.semantic,
            version: raw.version,
            description: raw.description,
            applicability,
            operation,
            response,
            decoder,
            unit: raw.unit,
            plausible_range,
            provenance,
            hardware_validation,
            notes: raw.notes,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn version(&self) -> u32 {
        self.version
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn applicability(&self) -> &KnowledgeApplicability {
        &self.applicability
    }

    pub fn operation(&self) -> &KnowledgeReadOperation {
        &self.operation
    }

    pub fn response(&self) -> &KnowledgeResponse {
        &self.response
    }

    pub fn decoder(&self) -> &KnowledgeDecoder {
        &self.decoder
    }

    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    pub fn plausible_range(&self) -> Option<&PlausibleRange> {
        self.plausible_range.as_ref()
    }

    pub fn provenance(&self) -> &KnowledgeProvenance {
        &self.provenance
    }

    pub fn hardware_validation(&self) -> &HardwareValidation {
        &self.hardware_validation
    }

    pub fn notes(&self) -> Option<&str> {
        self.notes.as_deref()
    }

    /// Validate normalized UDS response evidence using the same closed core
    /// protocol primitive used elsewhere in OBDentic. No transport call occurs.
    pub fn validate_response<'a>(&self, response: &'a [u8]) -> Result<&'a [u8], String> {
        let payload = self
            .operation
            .read_operation()
            .validate_response(response, self.semantic())
            .map_err(|error| error.to_string())?;
        self.response.validate_payload(payload)?;
        Ok(payload)
    }
}

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
                let raw_predicates =
                    raw.predicates
                        .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: "ecu_fingerprint applicability requires predicates".into(),
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
            "ecu.system_supplier_hardware_version" => Ok(Self::EcuSystemSupplierHardwareVersion),
            "ecu.system_supplier_software_number" => Ok(Self::EcuSystemSupplierSoftwareNumber),
            "ecu.system_supplier_software_version" => Ok(Self::EcuSystemSupplierSoftwareVersion),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KnowledgeReadOperation {
    UdsReadDataByIdentifier { did: u16, ecu_role: Option<String> },
}

impl KnowledgeReadOperation {
    fn from_raw(path: &Path, raw: RawOperation) -> Result<Self, KnowledgeLoadError> {
        match raw.kind.as_str() {
            "uds.read_data_by_identifier" => {
                if raw.pid.is_some() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "UDS ReadDataByIdentifier must not contain a PID".into(),
                    });
                }
                let identifier =
                    raw.identifier
                        .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: "UDS ReadDataByIdentifier requires identifier".into(),
                        })?;
                Ok(Self::UdsReadDataByIdentifier {
                    did: parse_hex_u16(path, "identifier", &identifier)?,
                    ecu_role: raw.ecu_role,
                })
            }
            // The data schema reserves this closed primitive for the later
            // OBD-II migration. This core slice intentionally refuses it until
            // its complete response-length contract is modeled.
            "obd2.mode01.pid" => Err(KnowledgeLoadError::OperationNotSupportedByCore(raw.kind)),
            _ => Err(KnowledgeLoadError::UnknownOperation(raw.kind)),
        }
    }

    pub const fn did(&self) -> u16 {
        match self {
            Self::UdsReadDataByIdentifier { did, .. } => *did,
        }
    }

    pub fn ecu_role(&self) -> Option<&str> {
        match self {
            Self::UdsReadDataByIdentifier { ecu_role, .. } => ecu_role.as_deref(),
        }
    }

    pub const fn request_bytes(&self) -> [u8; 3] {
        self.read_operation().request_bytes()
    }

    const fn read_operation(&self) -> ReadOperation {
        match self {
            Self::UdsReadDataByIdentifier { did, .. } => {
                ReadOperation::uds_read_data_by_identifier(*did)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeResponse {
    positive_service: u8,
    identifier_echo: bool,
    minimum_payload_length: Option<usize>,
    exact_payload_length: Option<usize>,
}

impl KnowledgeResponse {
    fn from_raw(
        path: &Path,
        operation: &KnowledgeReadOperation,
        raw: RawResponse,
    ) -> Result<Self, KnowledgeLoadError> {
        let positive_service = parse_hex_u8(path, "positive_service", &raw.positive_service)?;
        match operation {
            KnowledgeReadOperation::UdsReadDataByIdentifier { .. }
                if positive_service != 0x62 || !raw.identifier_echo =>
            {
                return Err(KnowledgeLoadError::InvalidDefinition {
                    path: path.to_path_buf(),
                    reason: "UDS ReadDataByIdentifier requires positive service 0x62 and identifier_echo=true".into(),
                });
            }
            KnowledgeReadOperation::UdsReadDataByIdentifier { .. } => {}
        }
        if let (Some(minimum), Some(exact)) = (raw.minimum_payload_length, raw.exact_payload_length)
        {
            if minimum > exact {
                return Err(KnowledgeLoadError::InvalidDefinition {
                    path: path.to_path_buf(),
                    reason: "minimum_payload_length exceeds exact_payload_length".into(),
                });
            }
        }
        Ok(Self {
            positive_service,
            identifier_echo: raw.identifier_echo,
            minimum_payload_length: raw.minimum_payload_length,
            exact_payload_length: raw.exact_payload_length,
        })
    }

    pub const fn positive_service(&self) -> u8 {
        self.positive_service
    }

    pub const fn identifier_echo(&self) -> bool {
        self.identifier_echo
    }

    pub const fn minimum_payload_length(&self) -> Option<usize> {
        self.minimum_payload_length
    }

    pub const fn exact_payload_length(&self) -> Option<usize> {
        self.exact_payload_length
    }

    fn validate_payload(&self, payload: &[u8]) -> Result<(), String> {
        if let Some(exact) = self.exact_payload_length {
            if payload.len() != exact {
                return Err(format!(
                    "knowledge response payload length {} does not match exact length {exact}",
                    payload.len()
                ));
            }
        }
        if let Some(minimum) = self.minimum_payload_length {
            if payload.len() < minimum {
                return Err(format!(
                    "knowledge response payload length {} is below minimum {minimum}",
                    payload.len()
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum KnowledgeDecoder {
    OpaqueBytes,
    Ascii {
        trim: AsciiTrim,
    },
    LinearInteger {
        width_bytes: u8,
        endian: Endian,
        signed: bool,
        scale: f64,
        offset: f64,
    },
}

impl KnowledgeDecoder {
    fn from_raw(path: &Path, raw: RawDecoder) -> Result<Self, KnowledgeLoadError> {
        match raw.kind.as_str() {
            "opaque_bytes" => {
                ensure_decoder_fields_absent(
                    path,
                    &raw,
                    &["trim", "width_bytes", "endian", "signed", "scale", "offset"],
                )?;
                Ok(Self::OpaqueBytes)
            }
            "ascii" => {
                ensure_decoder_fields_absent(
                    path,
                    &raw,
                    &["width_bytes", "endian", "signed", "scale", "offset"],
                )?;
                let trim = match raw.trim.as_deref() {
                    Some("none") => AsciiTrim::None,
                    Some("space") => AsciiTrim::Space,
                    Some("nul") => AsciiTrim::Nul,
                    Some("space_and_nul") => AsciiTrim::SpaceAndNul,
                    Some(other) => {
                        return Err(KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: format!("unknown ASCII trim policy {other:?}"),
                        })
                    }
                    None => {
                        return Err(KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: "ASCII decoder requires trim".into(),
                        })
                    }
                };
                Ok(Self::Ascii { trim })
            }
            "linear_integer" => {
                if raw.trim.is_some() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer decoder must not contain trim".into(),
                    });
                }
                let width_bytes =
                    raw.width_bytes
                        .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: "linear_integer decoder requires width_bytes".into(),
                        })?;
                if !(1..=8).contains(&width_bytes) {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer width_bytes must be within 1..=8".into(),
                    });
                }
                let endian = match raw.endian.as_deref() {
                    Some("big") => Endian::Big,
                    Some("little") => Endian::Little,
                    Some(other) => {
                        return Err(KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: format!("unknown endian value {other:?}"),
                        })
                    }
                    None => {
                        return Err(KnowledgeLoadError::InvalidDefinition {
                            path: path.to_path_buf(),
                            reason: "linear_integer decoder requires endian".into(),
                        })
                    }
                };
                let signed = raw
                    .signed
                    .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer decoder requires signed".into(),
                    })?;
                let scale = raw
                    .scale
                    .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer decoder requires scale".into(),
                    })?;
                let offset = raw
                    .offset
                    .ok_or_else(|| KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer decoder requires offset".into(),
                    })?;
                if !scale.is_finite() || !offset.is_finite() {
                    return Err(KnowledgeLoadError::InvalidDefinition {
                        path: path.to_path_buf(),
                        reason: "linear_integer scale/offset must be finite".into(),
                    });
                }
                Ok(Self::LinearInteger {
                    width_bytes,
                    endian,
                    signed,
                    scale,
                    offset,
                })
            }
            _ => Err(KnowledgeLoadError::UnknownDecoder(raw.kind)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsciiTrim {
    None,
    Space,
    Nul,
    SpaceAndNul,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endian {
    Big,
    Little,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlausibleRange {
    minimum: f64,
    maximum: f64,
}

impl PlausibleRange {
    fn from_raw(raw: RawPlausibleRange) -> Result<Self, KnowledgeLoadError> {
        if !raw.minimum.is_finite() || !raw.maximum.is_finite() || raw.minimum > raw.maximum {
            return Err(KnowledgeLoadError::InvalidRange);
        }
        Ok(Self {
            minimum: raw.minimum,
            maximum: raw.maximum,
        })
    }

    pub const fn minimum(&self) -> f64 {
        self.minimum
    }

    pub const fn maximum(&self) -> f64 {
        self.maximum
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeProvenance {
    classification: ProvenanceClassification,
    confidence: KnowledgeConfidence,
    sources: Vec<KnowledgeSource>,
}

impl KnowledgeProvenance {
    fn from_raw(path: &Path, raw: RawProvenance) -> Result<Self, KnowledgeLoadError> {
        if raw.sources.is_empty() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: "provenance requires at least one source".into(),
            });
        }
        let classification = match raw.classification.as_str() {
            "VERIFIED" => ProvenanceClassification::Verified,
            "COMMUNITY" => ProvenanceClassification::Community,
            "INFERRED" => ProvenanceClassification::Inferred,
            "EXPERIMENTAL" => ProvenanceClassification::Experimental,
            other => return Err(KnowledgeLoadError::UnknownProvenance(other.into())),
        };
        let confidence = match raw.confidence.as_str() {
            "high" => KnowledgeConfidence::High,
            "medium" => KnowledgeConfidence::Medium,
            "low" => KnowledgeConfidence::Low,
            other => return Err(KnowledgeLoadError::UnknownConfidence(other.into())),
        };
        let sources = raw
            .sources
            .into_iter()
            .map(|source| KnowledgeSource::from_raw(path, source))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            classification,
            confidence,
            sources,
        })
    }

    pub const fn classification(&self) -> ProvenanceClassification {
        self.classification
    }

    pub const fn confidence(&self) -> KnowledgeConfidence {
        self.confidence
    }

    pub fn sources(&self) -> &[KnowledgeSource] {
        &self.sources
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvenanceClassification {
    Verified,
    Community,
    Inferred,
    Experimental,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnowledgeConfidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeSource {
    kind: KnowledgeSourceKind,
    citation: String,
    url: Option<String>,
    note: Option<String>,
}

impl KnowledgeSource {
    fn from_raw(path: &Path, raw: RawSource) -> Result<Self, KnowledgeLoadError> {
        if raw.citation.trim().is_empty() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: "source citation must not be empty".into(),
            });
        }
        let kind = match raw.kind.as_str() {
            "standard" => KnowledgeSourceKind::Standard,
            "manufacturer_document" => KnowledgeSourceKind::ManufacturerDocument,
            "community" => KnowledgeSourceKind::Community,
            "research" => KnowledgeSourceKind::Research,
            "hardware_evidence" => KnowledgeSourceKind::HardwareEvidence,
            other => return Err(KnowledgeLoadError::UnknownSourceKind(other.into())),
        };
        Ok(Self {
            kind,
            citation: raw.citation,
            url: raw.url,
            note: raw.note,
        })
    }

    pub const fn kind(&self) -> KnowledgeSourceKind {
        self.kind
    }

    pub fn citation(&self) -> &str {
        &self.citation
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnowledgeSourceKind {
    Standard,
    ManufacturerDocument,
    Community,
    Research,
    HardwareEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareValidation {
    status: HardwareValidationStatus,
    reference: Option<String>,
    note: Option<String>,
}

impl HardwareValidation {
    fn from_raw(path: &Path, raw: RawHardwareValidation) -> Result<Self, KnowledgeLoadError> {
        let status = match raw.status.as_str() {
            "not_validated" => HardwareValidationStatus::NotValidated,
            "validated" => HardwareValidationStatus::Validated,
            "not_applicable" => HardwareValidationStatus::NotApplicable,
            other => return Err(KnowledgeLoadError::UnknownHardwareValidation(other.into())),
        };
        if status == HardwareValidationStatus::Validated && raw.reference.is_none() {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: "validated hardware knowledge requires a reference".into(),
            });
        }
        Ok(Self {
            status,
            reference: raw.reference,
            note: raw.note,
        })
    }

    pub const fn status(&self) -> HardwareValidationStatus {
        self.status
    }

    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HardwareValidationStatus {
    NotValidated,
    Validated,
    NotApplicable,
}

#[derive(Debug, PartialEq)]
pub enum KnowledgeLoadError {
    Io {
        path: PathBuf,
        error: String,
    },
    Yaml {
        path: PathBuf,
        error: String,
    },
    InvalidPin(String),
    UnsupportedSchemaVersion {
        path: PathBuf,
        found: u32,
    },
    EmptyKnowledgeRepository(PathBuf),
    NonUtf8Path(PathBuf),
    SymlinkNotAllowed(PathBuf),
    DuplicateDefinitionId {
        id: String,
        path: PathBuf,
    },
    DuplicateSemantic {
        semantic: String,
        first_id: String,
        second_id: String,
    },
    DuplicateApplicability {
        semantic: String,
        first_id: String,
        second_id: String,
    },
    DuplicateSetId {
        id: String,
        path: PathBuf,
    },
    UnknownSetMember {
        set: String,
        semantic: String,
    },
    VinInEcuIdentificationSet,
    InvalidDefinition {
        path: PathBuf,
        reason: String,
    },
    OperationNotSupportedByCore(String),
    UnknownOperation(String),
    UnknownApplicability(String),
    UnknownFingerprintField(String),
    UnknownDecoder(String),
    UnknownProvenance(String),
    UnknownConfidence(String),
    UnknownSourceKind(String),
    UnknownHardwareValidation(String),
    InvalidRange,
}

impl fmt::Display for KnowledgeLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(formatter, "{}: {error}", path.display()),
            Self::Yaml { path, error } => write!(formatter, "{}: invalid knowledge YAML: {error}", path.display()),
            Self::InvalidPin(reason) => write!(formatter, "invalid knowledge pin: {reason}"),
            Self::UnsupportedSchemaVersion { path, found } => write!(
                formatter,
                "{}: unsupported knowledge schema version {found}; supported version is {SUPPORTED_KNOWLEDGE_SCHEMA_VERSION}",
                path.display()
            ),
            Self::EmptyKnowledgeRepository(path) => write!(formatter, "{}: no canonical knowledge YAML files found", path.display()),
            Self::NonUtf8Path(path) => write!(formatter, "knowledge path is not UTF-8: {}", path.display()),
            Self::SymlinkNotAllowed(path) => write!(formatter, "knowledge repository symlink is not allowed: {}", path.display()),
            Self::DuplicateDefinitionId { id, path } => write!(formatter, "{}: duplicate knowledge definition id {id:?}", path.display()),
            Self::DuplicateSemantic { semantic, first_id, second_id } => write!(formatter, "duplicate semantic {semantic:?}: {first_id:?} and {second_id:?}"),
            Self::DuplicateApplicability { semantic, first_id, second_id } => write!(formatter, "duplicate applicability for semantic {semantic:?}: {first_id:?} and {second_id:?}"),
            Self::DuplicateSetId { id, path } => write!(formatter, "{}: duplicate knowledge set id {id:?}", path.display()),
            Self::UnknownSetMember { set, semantic } => write!(formatter, "knowledge set {set:?} references unknown semantic {semantic:?}"),
            Self::VinInEcuIdentificationSet => formatter.write_str("standard ECU identification set must not include VIN/F190"),
            Self::InvalidDefinition { path, reason } => write!(formatter, "{}: invalid knowledge definition: {reason}", path.display()),
            Self::OperationNotSupportedByCore(operation) => write!(formatter, "knowledge operation {operation:?} is schema-known but not supported by this OBDentic core"),
            Self::UnknownOperation(operation) => write!(formatter, "unknown knowledge operation {operation:?}"),
            Self::UnknownApplicability(value) => write!(formatter, "unknown knowledge applicability kind {value:?}"),
            Self::UnknownFingerprintField(value) => write!(formatter, "unknown knowledge fingerprint field {value:?}"),
            Self::UnknownDecoder(decoder) => write!(formatter, "unknown knowledge decoder {decoder:?}"),
            Self::UnknownProvenance(value) => write!(formatter, "unknown knowledge provenance classification {value:?}"),
            Self::UnknownConfidence(value) => write!(formatter, "unknown knowledge confidence {value:?}"),
            Self::UnknownSourceKind(value) => write!(formatter, "unknown knowledge source kind {value:?}"),
            Self::UnknownHardwareValidation(value) => write!(formatter, "unknown hardware-validation status {value:?}"),
            Self::InvalidRange => formatter.write_str("knowledge plausible range must be finite and minimum <= maximum"),
        }
    }
}

impl std::error::Error for KnowledgeLoadError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    schema_version: u32,
    namespace: String,
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    sets: Vec<RawSet>,
    definitions: Vec<RawDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    id: String,
    version: u32,
    description: Option<String>,
    members: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefinition {
    id: String,
    semantic: String,
    version: u32,
    description: Option<String>,
    applicability: RawApplicability,
    operation: RawOperation,
    response: RawResponse,
    decoder: RawDecoder,
    unit: Option<String>,
    plausible_range: Option<RawPlausibleRange>,
    provenance: RawProvenance,
    hardware_validation: RawHardwareValidation,
    notes: Option<String>,
}

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOperation {
    #[serde(rename = "type")]
    kind: String,
    identifier: Option<String>,
    pid: Option<String>,
    ecu_role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResponse {
    positive_service: String,
    identifier_echo: bool,
    minimum_payload_length: Option<usize>,
    exact_payload_length: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecoder {
    #[serde(rename = "type")]
    kind: String,
    trim: Option<String>,
    width_bytes: Option<u8>,
    endian: Option<String>,
    signed: Option<bool>,
    scale: Option<f64>,
    offset: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlausibleRange {
    minimum: f64,
    maximum: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvenance {
    classification: String,
    confidence: String,
    sources: Vec<RawSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    kind: String,
    citation: String,
    url: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHardwareValidation {
    status: String,
    reference: Option<String>,
    note: Option<String>,
}

fn parse_document(
    path: &Path,
    text: &str,
    pinned_schema_version: u32,
) -> Result<RawDocument, KnowledgeLoadError> {
    let document: RawDocument =
        serde_yaml_ng::from_str(text).map_err(|error| KnowledgeLoadError::Yaml {
            path: path.to_path_buf(),
            error: error.to_string(),
        })?;
    if document.schema_version != SUPPORTED_KNOWLEDGE_SCHEMA_VERSION
        || document.schema_version != pinned_schema_version
    {
        return Err(KnowledgeLoadError::UnsupportedSchemaVersion {
            path: path.to_path_buf(),
            found: document.schema_version,
        });
    }
    if document.namespace.trim().is_empty() || document.definitions.is_empty() {
        return Err(KnowledgeLoadError::InvalidDefinition {
            path: path.to_path_buf(),
            reason: "namespace must be non-empty and definitions must not be empty".into(),
        });
    }
    Ok(document)
}

fn canonical_yaml_files(root: &Path) -> Result<Vec<PathBuf>, KnowledgeLoadError> {
    let mut files = Vec::<(String, PathBuf)>::new();
    for canonical_root in ["standards", "manufacturers", "semantic"] {
        let path = root.join(canonical_root);
        if path.exists() {
            collect_yaml_files(root, &path, &mut files)?;
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn collect_yaml_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), KnowledgeLoadError> {
    let entries = fs::read_dir(directory).map_err(|error| KnowledgeLoadError::Io {
        path: directory.to_path_buf(),
        error: error.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| KnowledgeLoadError::Io {
            path: directory.to_path_buf(),
            error: error.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| KnowledgeLoadError::Io {
            path: path.clone(),
            error: error.to_string(),
        })?;
        if file_type.is_symlink() {
            return Err(KnowledgeLoadError::SymlinkNotAllowed(path));
        }
        if file_type.is_dir() {
            collect_yaml_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let is_yaml = matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yaml" | "yml")
        );
        if !is_yaml {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("canonical knowledge traversal remains under root")
            .to_path_buf();
        let sort_key = relative
            .to_str()
            .ok_or_else(|| KnowledgeLoadError::NonUtf8Path(relative.clone()))?
            .replace('\\', "/");
        files.push((sort_key, relative));
    }
    Ok(())
}

fn parse_hex_u16(path: &Path, field: &str, value: &str) -> Result<u16, KnowledgeLoadError> {
    if value.len() != 6 || !value.starts_with("0x") {
        return Err(KnowledgeLoadError::InvalidDefinition {
            path: path.to_path_buf(),
            reason: format!("{field} must use exact 0xFFFF form"),
        });
    }
    u16::from_str_radix(&value[2..], 16).map_err(|_| KnowledgeLoadError::InvalidDefinition {
        path: path.to_path_buf(),
        reason: format!("invalid hexadecimal {field} {value:?}"),
    })
}

fn parse_hex_u8(path: &Path, field: &str, value: &str) -> Result<u8, KnowledgeLoadError> {
    if value.len() != 4 || !value.starts_with("0x") {
        return Err(KnowledgeLoadError::InvalidDefinition {
            path: path.to_path_buf(),
            reason: format!("{field} must use exact 0xFF form"),
        });
    }
    u8::from_str_radix(&value[2..], 16).map_err(|_| KnowledgeLoadError::InvalidDefinition {
        path: path.to_path_buf(),
        reason: format!("invalid hexadecimal {field} {value:?}"),
    })
}

fn ensure_decoder_fields_absent(
    path: &Path,
    raw: &RawDecoder,
    fields: &[&str],
) -> Result<(), KnowledgeLoadError> {
    for field in fields {
        let present = match *field {
            "trim" => raw.trim.is_some(),
            "width_bytes" => raw.width_bytes.is_some(),
            "endian" => raw.endian.is_some(),
            "signed" => raw.signed.is_some(),
            "scale" => raw.scale.is_some(),
            "offset" => raw.offset.is_some(),
            _ => false,
        };
        if present {
            return Err(KnowledgeLoadError::InvalidDefinition {
                path: path.to_path_buf(),
                reason: format!("decoder {:?} must not contain {field}", raw.kind),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    fn pin() -> KnowledgePin {
        KnowledgePin::new(CANONICAL_KNOWLEDGE_REPOSITORY, FIXTURE_REVISION, 2).unwrap()
    }

    fn valid_document(extra: &str) -> String {
        format!(
            r#"schema_version: 2
namespace: test.uds
sets:
  - id: uds.standard.ecu_identification
    version: 1
    members: [ecu.software_version]
definitions:
  - id: test.f189.software_version
    semantic: ecu.software_version
    version: 1
    applicability:
      kind: generic
      provenance:
        classification: VERIFIED
        confidence: high
        sources:
          - kind: standard
            citation: ISO 14229-1
    operation:
      type: uds.read_data_by_identifier
      identifier: "0xF189"
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
{extra}"#
        )
    }

    #[test]
    fn pin_parser_is_strict_and_preserves_revision() {
        let parsed = KnowledgePin::parse(
            Path::new("knowledge.lock"),
            &format!(
                "repository = {CANONICAL_KNOWLEDGE_REPOSITORY}\nrevision = {FIXTURE_REVISION}\nschema_version = 2\n"
            ),
        )
        .unwrap();
        assert_eq!(parsed, pin());
        assert!(KnowledgePin::parse(
            Path::new("knowledge.lock"),
            &format!(
                "repository = {CANONICAL_KNOWLEDGE_REPOSITORY}\nrevision = {FIXTURE_REVISION}\nschema_version = 2\nraw_command = 22114F\n"
            ),
        )
        .is_err());
    }

    #[test]
    fn yaml_unknown_fields_fail_closed() {
        let text = valid_document("    raw_request: \"27 01\"\n");
        let error = parse_document(Path::new("unsafe.yaml"), &text, 2).unwrap_err();
        assert!(matches!(error, KnowledgeLoadError::Yaml { .. }));
    }

    #[test]
    fn schema_known_but_unimplemented_operation_fails_closed() {
        let text = valid_document("").replace(
            "type: uds.read_data_by_identifier\n      identifier: \"0xF189\"",
            "type: obd2.mode01.pid\n      pid: \"0x0C\"",
        );
        let raw = parse_document(Path::new("obd2.yaml"), &text, 2).unwrap();
        let error = KnowledgeDefinition::from_raw(
            Path::new("obd2.yaml"),
            raw.definitions.into_iter().next().unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            KnowledgeLoadError::OperationNotSupportedByCore("obd2.mode01.pid".into())
        );
    }

    #[test]
    fn typed_uds_operation_uses_core_protocol_validation() {
        let raw = parse_document(Path::new("uds.yaml"), &valid_document(""), 2).unwrap();
        let definition = KnowledgeDefinition::from_raw(
            Path::new("uds.yaml"),
            raw.definitions.into_iter().next().unwrap(),
        )
        .unwrap();
        assert_eq!(definition.operation().request_bytes(), [0x22, 0xF1, 0x89]);
        assert_eq!(
            definition
                .validate_response(&[0x62, 0xF1, 0x89, b'1', b'2', b'3'])
                .unwrap(),
            b"123"
        );
        assert!(definition
            .validate_response(&[0x62, 0xF1, 0x88, b'1'])
            .is_err());
    }

    #[test]
    fn unsupported_schema_version_is_rejected_before_definition_conversion() {
        let text = valid_document("").replace("schema_version: 2", "schema_version: 3");
        assert!(matches!(
            parse_document(Path::new("future.yaml"), &text, 2),
            Err(KnowledgeLoadError::UnsupportedSchemaVersion { found: 3, .. })
        ));
    }

    #[test]
    fn f190_cannot_be_accepted_as_standard_ecu_identification() {
        let raw = parse_document(
            Path::new("vin.yaml"),
            &valid_document("").replace("0xF189", "0xF190"),
            2,
        )
        .unwrap();
        let definition = KnowledgeDefinition::from_raw(
            Path::new("vin.yaml"),
            raw.definitions.into_iter().next().unwrap(),
        )
        .unwrap();
        assert_eq!(definition.operation().did(), VIN_DID);
    }
}
