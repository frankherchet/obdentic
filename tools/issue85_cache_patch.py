from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement anchor, found {count}: {old[:120]!r}")
    file.write_text(text.replace(old, new, 1))


# Domain model: evidence/result vocabulary only. No transport path is added here.
replace_once(
    "src/ecu_identification.rs",
    "use crate::{\n    knowledge_db::{KnowledgeCatalog, STANDARD_UDS_ECU_IDENTIFICATION_SET},\n    protocol::ReadOperation,\n};",
    "use crate::{\n    knowledge_db::{KnowledgeCatalog, STANDARD_UDS_ECU_IDENTIFICATION_SET},\n    protocol::ReadOperation,\n    topology::{RequestTarget, ResponderIdentity},\n};",
)

model = r'''

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum IdentificationResultStatus {
    Supported,
    Unsupported,
    NegativeResponse,
    Unavailable,
    Malformed,
    Timeout,
    TransportError,
    NotProbed,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IdentificationResponseEvidence {
    responder: Option<ResponderIdentity>,
    payload: Vec<u8>,
}

impl IdentificationResponseEvidence {
    pub fn new(responder: Option<ResponderIdentity>, payload: Vec<u8>) -> Self {
        Self { responder, payload }
    }

    pub fn responder(&self) -> Option<&ResponderIdentity> {
        self.responder.as_ref()
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// Persistable evidence for one canonical ECU-identification candidate against
/// one already evidenced physical ECU target.  This value is interpretation
/// metadata plus normalized evidence; it is never a transport command source.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IdentificationObservation {
    target: RequestTarget,
    expected_responder: ResponderIdentity,
    semantic: String,
    definition_id: String,
    definition_version: u32,
    knowledge_repository: String,
    knowledge_revision: String,
    request: [u8; 3],
    status: IdentificationResultStatus,
    responses: Vec<IdentificationResponseEvidence>,
    nrc: Option<u8>,
    value: Option<Vec<u8>>,
    errors: Vec<String>,
}

impl IdentificationObservation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target: RequestTarget,
        expected_responder: ResponderIdentity,
        semantic: impl Into<String>,
        definition_id: impl Into<String>,
        definition_version: u32,
        knowledge_repository: impl Into<String>,
        knowledge_revision: impl Into<String>,
        request: [u8; 3],
        status: IdentificationResultStatus,
        responses: Vec<IdentificationResponseEvidence>,
        nrc: Option<u8>,
        value: Option<Vec<u8>>,
        errors: Vec<String>,
    ) -> Result<Self, String> {
        let observation = Self {
            target,
            expected_responder,
            semantic: semantic.into(),
            definition_id: definition_id.into(),
            definition_version,
            knowledge_repository: knowledge_repository.into(),
            knowledge_revision: knowledge_revision.into(),
            request,
            status,
            responses,
            nrc,
            value,
            errors,
        };
        observation.validate()?;
        Ok(observation)
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn expected_responder(&self) -> &ResponderIdentity {
        &self.expected_responder
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }

    pub const fn definition_version(&self) -> u32 {
        self.definition_version
    }

    pub fn knowledge_repository(&self) -> &str {
        &self.knowledge_repository
    }

    pub fn knowledge_revision(&self) -> &str {
        &self.knowledge_revision
    }

    pub const fn request(&self) -> [u8; 3] {
        self.request
    }

    pub const fn status(&self) -> IdentificationResultStatus {
        self.status
    }

    pub fn responses(&self) -> &[IdentificationResponseEvidence] {
        &self.responses
    }

    pub const fn nrc(&self) -> Option<u8> {
        self.nrc
    }

    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_deref()
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.semantic.trim().is_empty()
            || self.definition_id.trim().is_empty()
            || self.definition_version == 0
            || self.knowledge_repository.trim().is_empty()
            || self.knowledge_revision.trim().is_empty()
        {
            return Err("ECU identification evidence metadata is incomplete".into());
        }
        if self.request[0] != 0x22 {
            return Err("ECU identification evidence must use ReadDataByIdentifier".into());
        }
        if self.request == [0x22, 0xF1, 0x90] {
            return Err("VIN/F190 must not be persisted as ECU identification evidence".into());
        }
        if self.target.context() != self.expected_responder.context() {
            return Err("ECU identification target and responder contexts differ".into());
        }
        if self.target.address().is_none() {
            return Err("ECU identification evidence requires a concrete target".into());
        }
        match self.status {
            IdentificationResultStatus::Supported => {
                if self.value.is_none() || self.nrc.is_some() || !self.errors.is_empty() {
                    return Err("supported ECU identification evidence is inconsistent".into());
                }
            }
            IdentificationResultStatus::Unsupported
            | IdentificationResultStatus::NegativeResponse
            | IdentificationResultStatus::Unavailable => {
                if self.nrc.is_none() || self.value.is_some() {
                    return Err("negative ECU identification evidence requires NRC and no value".into());
                }
            }
            IdentificationResultStatus::Malformed => {
                if self.value.is_some() {
                    return Err("malformed ECU identification evidence cannot carry a value".into());
                }
            }
            IdentificationResultStatus::Timeout
            | IdentificationResultStatus::TransportError => {
                if self.nrc.is_some() || self.value.is_some() || self.errors.is_empty() {
                    return Err("transport ECU identification evidence requires an error only".into());
                }
            }
            IdentificationResultStatus::NotProbed => {
                if self.nrc.is_some() || self.value.is_some() || !self.responses.is_empty() {
                    return Err("not-probed ECU identification evidence cannot contain a response".into());
                }
            }
        }
        Ok(())
    }
}
'''
replace_once(
    "src/ecu_identification.rs",
    "\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct EcuIdentificationPlan {",
    model + "\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct EcuIdentificationPlan {",
)

replace_once(
    "src/ecu_identification.rs",
    "    #[test]\n    fn plan_is_exactly_the_canonical_set_in_declared_order() {",
    r'''    #[test]
    fn result_statuses_are_not_collapsed() {
        let statuses = [
            IdentificationResultStatus::Supported,
            IdentificationResultStatus::Unsupported,
            IdentificationResultStatus::NegativeResponse,
            IdentificationResultStatus::Unavailable,
            IdentificationResultStatus::Malformed,
            IdentificationResultStatus::Timeout,
            IdentificationResultStatus::TransportError,
            IdentificationResultStatus::NotProbed,
        ];
        for (index, status) in statuses.iter().enumerate() {
            assert!(!statuses[..index].contains(status));
        }
    }

    #[test]
    fn observation_rejects_vin_and_inconsistent_timeout() {
        let context = crate::topology::ProtocolContext::new(
            crate::topology::Protocol::Obd2,
            crate::topology::AddressingContext::Physical,
        );
        let target = crate::topology::RequestTarget::concrete(
            context.clone(),
            crate::topology::RequestAddress::new("elm-header", "7E0"),
        );
        let responder = crate::topology::ResponderIdentity::address(context, "7E8");
        let base = |request, status, errors| {
            IdentificationObservation::new(
                target.clone(),
                responder.clone(),
                "ecu.manufacturer_software_version",
                "uds.f189.manufacturer_software_version",
                1,
                "frankherchet/obdentic-knowledge",
                "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
                request,
                status,
                Vec::new(),
                None,
                None,
                errors,
            )
        };
        assert!(base(
            [0x22, 0xF1, 0x90],
            IdentificationResultStatus::NotProbed,
            Vec::new()
        )
        .is_err());
        assert!(base(
            [0x22, 0xF1, 0x89],
            IdentificationResultStatus::Timeout,
            Vec::new()
        )
        .is_err());
        assert!(base(
            [0x22, 0xF1, 0x89],
            IdentificationResultStatus::Timeout,
            vec!["Carly command timed out".into()]
        )
        .is_ok());
    }

    #[test]
    fn plan_is_exactly_the_canonical_set_in_declared_order() {''',
)

# Cache schema v4: append per-ECU identification evidence without changing
# the validation signature used to decide whether a vehicle cache is stale.
replace_once(
    "src/vehicle_cache.rs",
    "use crate::{\n    functional_discovery::EcuCapability,",
    "use crate::{\n    ecu_identification::{\n        IdentificationObservation, IdentificationResponseEvidence, IdentificationResultStatus,\n    },\n    functional_discovery::EcuCapability,",
)
replace_once(
    "src/vehicle_cache.rs",
    'const HEADER: &str = "OBDENTIC-VEHICLE-CACHE\\t3";\nconst V2_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\\t2";',
    'const HEADER: &str = "OBDENTIC-VEHICLE-CACHE\\t4";\nconst V3_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\\t3";\nconst V2_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\\t2";',
)
replace_once(
    "src/vehicle_cache.rs",
    "    target_mappings: Vec<TargetMappingSnapshot>,\n}",
    "    target_mappings: Vec<TargetMappingSnapshot>,\n    ecu_identification: Vec<IdentificationObservation>,\n}",
)
replace_once(
    "src/vehicle_cache.rs",
    r'''    pub fn new(
        topology: impl IntoIterator<Item = TopologyObservation>,
        ecu_capabilities: impl IntoIterator<Item = EcuCapabilitySnapshot>,
        target_mappings: impl IntoIterator<Item = TargetMappingSnapshot>,
    ) -> Self {
        let mut snapshot = Self {
            topology: topology.into_iter().collect(),
            ecu_capabilities: ecu_capabilities.into_iter().collect(),
            target_mappings: target_mappings.into_iter().collect(),
        };
        snapshot.topology.sort();
        snapshot.ecu_capabilities.sort();
        snapshot.target_mappings.sort();
        snapshot
    }''',
    r'''    pub fn new(
        topology: impl IntoIterator<Item = TopologyObservation>,
        ecu_capabilities: impl IntoIterator<Item = EcuCapabilitySnapshot>,
        target_mappings: impl IntoIterator<Item = TargetMappingSnapshot>,
    ) -> Self {
        Self::with_ecu_identification(topology, ecu_capabilities, target_mappings, [])
    }

    pub fn with_ecu_identification(
        topology: impl IntoIterator<Item = TopologyObservation>,
        ecu_capabilities: impl IntoIterator<Item = EcuCapabilitySnapshot>,
        target_mappings: impl IntoIterator<Item = TargetMappingSnapshot>,
        ecu_identification: impl IntoIterator<Item = IdentificationObservation>,
    ) -> Self {
        let mut snapshot = Self {
            topology: topology.into_iter().collect(),
            ecu_capabilities: ecu_capabilities.into_iter().collect(),
            target_mappings: target_mappings.into_iter().collect(),
            ecu_identification: ecu_identification.into_iter().collect(),
        };
        snapshot.topology.sort();
        snapshot.ecu_capabilities.sort();
        snapshot.target_mappings.sort();
        snapshot.ecu_identification.sort();
        snapshot
    }''',
)
replace_once(
    "src/vehicle_cache.rs",
    "    pub fn target_mappings(&self) -> &[TargetMappingSnapshot] {\n        &self.target_mappings\n    }\n\n    pub fn validation_signature(&self) -> ValidationSignature {",
    "    pub fn target_mappings(&self) -> &[TargetMappingSnapshot] {\n        &self.target_mappings\n    }\n\n    pub fn ecu_identification(&self) -> &[IdentificationObservation] {\n        &self.ecu_identification\n    }\n\n    pub fn validation_signature(&self) -> ValidationSignature {",
)
replace_once(
    "src/vehicle_cache.rs",
    "            for mapping in &cache.snapshot.target_mappings {\n                file.write_all(b\"\\ntarget_mapping\\t\")?;\n                file.write_all(encode_target_mapping(mapping).as_bytes())?;\n            }\n            for line in &cache.history {",
    "            for mapping in &cache.snapshot.target_mappings {\n                file.write_all(b\"\\ntarget_mapping\\t\")?;\n                file.write_all(encode_target_mapping(mapping).as_bytes())?;\n            }\n            for observation in &cache.snapshot.ecu_identification {\n                file.write_all(b\"\\necu_identification\\t\")?;\n                file.write_all(encode_identification_observation(observation).as_bytes())?;\n            }\n            for line in &cache.history {",
)

encode = r'''

fn encode_identification_observation(observation: &IdentificationObservation) -> String {
    let mut fields = encode_target(observation.target());
    fields.extend(encode_responder_fields(observation.expected_responder()));
    fields.extend([
        observation.semantic().into(),
        observation.definition_id().into(),
        observation.definition_version().to_string(),
        observation.knowledge_repository().into(),
        observation.knowledge_revision().into(),
        hex(&observation.request()),
        encode_identification_status(observation.status()).into(),
        if observation.nrc().is_some() { "1" } else { "0" }.into(),
    ]);
    if let Some(nrc) = observation.nrc() {
        fields.push(nrc.to_string());
    }
    fields.push(if observation.value().is_some() { "1" } else { "0" }.into());
    if let Some(value) = observation.value() {
        fields.push(hex(value));
    }
    fields.push(observation.responses().len().to_string());
    for response in observation.responses() {
        fields.push(if response.responder().is_some() { "1" } else { "0" }.into());
        if let Some(responder) = response.responder() {
            fields.extend(encode_responder_fields(responder));
        }
        fields.push(hex(response.payload()));
    }
    fields.push(observation.errors().len().to_string());
    fields.extend(observation.errors().iter().cloned());
    encode_fields(&fields)
}

fn encode_identification_status(status: IdentificationResultStatus) -> &'static str {
    match status {
        IdentificationResultStatus::Supported => "supported",
        IdentificationResultStatus::Unsupported => "unsupported",
        IdentificationResultStatus::NegativeResponse => "negative_response",
        IdentificationResultStatus::Unavailable => "unavailable",
        IdentificationResultStatus::Malformed => "malformed",
        IdentificationResultStatus::Timeout => "timeout",
        IdentificationResultStatus::TransportError => "transport_error",
        IdentificationResultStatus::NotProbed => "not_probed",
    }
}
'''
replace_once(
    "src/vehicle_cache.rs",
    "\nfn encode_role_assignment(role: &RoleAssignment) -> Vec<String> {",
    encode + "\nfn encode_role_assignment(role: &RoleAssignment) -> Vec<String> {",
)

replace_once(
    "src/vehicle_cache.rs",
    r'''    match lines.next() {
        Some(HEADER) => parse_v3(lines, requested_key),
        Some(V2_HEADER) => parse_v2(lines, requested_key),''',
    r'''    match lines.next() {
        Some(HEADER) => parse_v4(lines, requested_key),
        Some(V3_HEADER) => parse_v3(lines, requested_key),
        Some(V2_HEADER) => parse_v2(lines, requested_key),''',
)
replace_once(
    "src/vehicle_cache.rs",
    r'''fn parse_v2<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, false)
}

fn parse_v3<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, true)
}

fn parse_versioned<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
    has_role: bool,
) -> Result<VehicleCache, String> {''',
    r'''fn parse_v2<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, false, false)
}

fn parse_v3<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, true, false)
}

fn parse_v4<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, true, true)
}

fn parse_versioned<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
    has_role: bool,
    has_identification: bool,
) -> Result<VehicleCache, String> {''',
)
replace_once(
    "src/vehicle_cache.rs",
    "    let mut target_mappings = Vec::new();\n    let mut history = Vec::new();",
    "    let mut target_mappings = Vec::new();\n    let mut ecu_identification = Vec::new();\n    let mut history = Vec::new();",
)
replace_once(
    "src/vehicle_cache.rs",
    r'''            "target_mapping" => {
                target_mappings.push(parse_target_mapping_with_role(&fields, has_role)?);
            }
            "history" => history.push(one_field(&fields, "history")?.to_owned()),''',
    r'''            "target_mapping" => {
                target_mappings.push(parse_target_mapping_with_role(&fields, has_role)?);
            }
            "ecu_identification" if has_identification => {
                ecu_identification.push(parse_identification_observation(&fields)?);
            }
            "history" => history.push(one_field(&fields, "history")?.to_owned()),''',
)
replace_once(
    "src/vehicle_cache.rs",
    "        VehicleCacheSnapshot::new(topology, capabilities.into_values(), target_mappings),",
    "        VehicleCacheSnapshot::with_ecu_identification(\n            topology,\n            capabilities.into_values(),\n            target_mappings,\n            ecu_identification,\n        ),",
)

parse_model = r'''

fn parse_identification_observation(fields: &[String]) -> Result<IdentificationObservation, String> {
    let mut index = 0;
    let target = parse_target(fields, &mut index)?;
    let expected_responder = parse_responder(fields, &mut index)?;
    let semantic = field(fields, &mut index, "identification semantic")?.to_owned();
    let definition_id = field(fields, &mut index, "identification definition")?.to_owned();
    let definition_version = field(fields, &mut index, "identification definition version")?
        .parse::<u32>()
        .map_err(|error| format!("invalid identification definition version: {error}"))?;
    let knowledge_repository =
        field(fields, &mut index, "identification knowledge repository")?.to_owned();
    let knowledge_revision =
        field(fields, &mut index, "identification knowledge revision")?.to_owned();
    let request_bytes = parse_bytes(field(fields, &mut index, "identification request")?)?;
    let request: [u8; 3] = request_bytes
        .try_into()
        .map_err(|_| "ECU identification request must contain exactly three bytes".to_string())?;
    let status = parse_identification_status(field(fields, &mut index, "identification status")?)?;
    let nrc = if parse_optional_flag(fields, &mut index, "identification NRC")? {
        Some(
            field(fields, &mut index, "identification NRC value")?
                .parse::<u8>()
                .map_err(|error| format!("invalid identification NRC: {error}"))?,
        )
    } else {
        None
    };
    let value = if parse_optional_flag(fields, &mut index, "identification value")? {
        Some(parse_bytes(field(fields, &mut index, "identification value bytes")?)?)
    } else {
        None
    };
    let response_count = field(fields, &mut index, "identification response count")?
        .parse::<usize>()
        .map_err(|error| format!("invalid identification response count: {error}"))?;
    let mut responses = Vec::with_capacity(response_count);
    for _ in 0..response_count {
        let responder = if parse_optional_flag(fields, &mut index, "identification responder")? {
            Some(parse_responder(fields, &mut index)?)
        } else {
            None
        };
        let payload = parse_bytes(field(fields, &mut index, "identification response payload")?)?;
        responses.push(IdentificationResponseEvidence::new(responder, payload));
    }
    let error_count = field(fields, &mut index, "identification error count")?
        .parse::<usize>()
        .map_err(|error| format!("invalid identification error count: {error}"))?;
    let mut errors = Vec::with_capacity(error_count);
    for _ in 0..error_count {
        errors.push(field(fields, &mut index, "identification error")?.to_owned());
    }
    finish_fields(fields, index)?;
    IdentificationObservation::new(
        target,
        expected_responder,
        semantic,
        definition_id,
        definition_version,
        knowledge_repository,
        knowledge_revision,
        request,
        status,
        responses,
        nrc,
        value,
        errors,
    )
}

fn parse_identification_status(value: &str) -> Result<IdentificationResultStatus, String> {
    match value {
        "supported" => Ok(IdentificationResultStatus::Supported),
        "unsupported" => Ok(IdentificationResultStatus::Unsupported),
        "negative_response" => Ok(IdentificationResultStatus::NegativeResponse),
        "unavailable" => Ok(IdentificationResultStatus::Unavailable),
        "malformed" => Ok(IdentificationResultStatus::Malformed),
        "timeout" => Ok(IdentificationResultStatus::Timeout),
        "transport_error" => Ok(IdentificationResultStatus::TransportError),
        "not_probed" => Ok(IdentificationResultStatus::NotProbed),
        _ => Err("vehicle cache contains an invalid ECU identification status".into()),
    }
}
'''
replace_once(
    "src/vehicle_cache.rs",
    "\nfn parse_role_assignment(fields: &[String], index: &mut usize) -> Result<RoleAssignment, String> {",
    parse_model + "\nfn parse_role_assignment(fields: &[String], index: &mut usize) -> Result<RoleAssignment, String> {",
)
replace_once(
    "src/vehicle_cache.rs",
    "    for mapping in &snapshot.target_mappings {\n        if let Some(role) = mapping.role() {",
    "    for observation in &snapshot.ecu_identification {\n        observation.validate()?;\n        validate_context(observation.target().context())?;\n        if let Some(address) = observation.target().address() {\n            validate_text(\"identification target namespace\", address.namespace())?;\n            validate_text(\"identification target value\", address.value())?;\n        }\n        validate_responder(observation.expected_responder())?;\n        validate_text(\"identification semantic\", observation.semantic())?;\n        validate_text(\"identification definition\", observation.definition_id())?;\n        validate_text(\"identification knowledge repository\", observation.knowledge_repository())?;\n        validate_text(\"identification knowledge revision\", observation.knowledge_revision())?;\n        if observation.value().is_some_and(|value| value.len() > 4096) {\n            return Err(\"vehicle cache ECU identification value is too large\".into());\n        }\n        for response in observation.responses() {\n            if let Some(responder) = response.responder() {\n                validate_responder(responder)?;\n            }\n            if response.payload().len() > 4096 {\n                return Err(\"vehicle cache ECU identification response is too large\".into());\n            }\n        }\n        for error in observation.errors() {\n            validate_text(\"identification error\", error)?;\n        }\n    }\n    for mapping in &snapshot.target_mappings {\n        if let Some(role) = mapping.role() {",
)

# Existing format expectations move to v4; v4 is no longer unsupported.
replace_once(
    "src/vehicle_cache.rs",
    '"OBDENTIC-VEHICLE-CACHE\\t3\\nlocal_key\\tlocal-key\\nfirst_seen_ms\\t1\\nlast_seen_ms\\t2\\nhistory\\tevidence\\n"',
    '"OBDENTIC-VEHICLE-CACHE\\t4\\nlocal_key\\tlocal-key\\nfirst_seen_ms\\t1\\nlast_seen_ms\\t2\\nhistory\\tevidence\\n"',
)
replace_once(
    "src/vehicle_cache.rs",
    '"OBDENTIC-VEHICLE-CACHE\\t4\\nlocal_key\\tlocal-key\\nfirst_seen_ms\\t1\\nlast_seen_ms\\t1\\n",',
    '"OBDENTIC-VEHICLE-CACHE\\t5\\nlocal_key\\tlocal-key\\nfirst_seen_ms\\t1\\nlast_seen_ms\\t1\\n",',
)

cache_tests = r'''

    #[test]
    fn round_trips_per_ecu_identification_without_affecting_validation_signature() {
        let root = root("ecu-identification");
        let store = CacheStore::new(&root);
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let target = RequestTarget::concrete(
            context.clone(),
            RequestAddress::new("elm-header", "7E0"),
        );
        let responder = ResponderIdentity::address(context, "7E8");
        let supported = IdentificationObservation::new(
            target.clone(),
            responder.clone(),
            "ecu.manufacturer_software_version",
            "uds.f189.manufacturer_software_version",
            1,
            "frankherchet/obdentic-knowledge",
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
            [0x22, 0xF1, 0x89],
            IdentificationResultStatus::Supported,
            vec![IdentificationResponseEvidence::new(
                Some(responder.clone()),
                vec![0x62, 0xF1, 0x89, 0x31, 0x2E],
            )],
            None,
            Some(vec![0x31, 0x2E]),
            Vec::new(),
        )
        .unwrap();
        let timeout = IdentificationObservation::new(
            target,
            responder,
            "ecu.boot_software_identification",
            "uds.f180.boot_software_identification",
            1,
            "frankherchet/obdentic-knowledge",
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
            [0x22, 0xF1, 0x80],
            IdentificationResultStatus::Timeout,
            Vec::new(),
            None,
            None,
            vec!["Carly command timed out".into()],
        )
        .unwrap();
        let snapshot = VehicleCacheSnapshot::with_ecu_identification([], [], [], [supported, timeout]);
        let signature = snapshot.validation_signature();
        assert!(signature.topology().is_empty());
        assert!(signature.ecu_capabilities().is_empty());
        assert!(signature.target_mappings().is_empty());

        store
            .save(&VehicleCache::with_snapshot(
                "local-key",
                1,
                2,
                snapshot.clone(),
                Vec::new(),
            ))
            .unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert_eq!(loaded.snapshot(), &snapshot);
        assert_eq!(
            loaded
                .snapshot()
                .ecu_identification()
                .iter()
                .map(IdentificationObservation::status)
                .collect::<Vec<_>>(),
            vec![
                IdentificationResultStatus::Timeout,
                IdentificationResultStatus::Supported,
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_v3_cache_without_ecu_identification() {
        let root = root("v3");
        let store = CacheStore::new(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("6c6f63616c2d6b6579.tsv"),
            "OBDENTIC-VEHICLE-CACHE\t3\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t1\n",
        )
        .unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert!(loaded.snapshot().ecu_identification().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_vin_did_in_ecu_identification_cache() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let observation = IdentificationObservation::new(
            RequestTarget::concrete(
                context.clone(),
                RequestAddress::new("elm-header", "7E0"),
            ),
            ResponderIdentity::address(context, "7E8"),
            "ecu.vin",
            "uds.f190.vin",
            1,
            "frankherchet/obdentic-knowledge",
            "revision",
            [0x22, 0xF1, 0x90],
            IdentificationResultStatus::NotProbed,
            Vec::new(),
            None,
            None,
            Vec::new(),
        );
        assert!(observation.is_err());
    }
'''
replace_once(
    "src/vehicle_cache.rs",
    "\n    #[test]\n    fn loads_legacy_textual_evidence_as_history_only() {",
    cache_tests + "\n    #[test]\n    fn loads_legacy_textual_evidence_as_history_only() {",
)
