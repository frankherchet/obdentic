//! Bounded standards-based ECU-identification planning.
//!
//! Canonical Knowledge selects the candidates. This module deliberately has
//! no transport access and exposes no constructor that accepts a caller DID.

use crate::{
    knowledge_db::{KnowledgeCatalog, STANDARD_UDS_ECU_IDENTIFICATION_SET},
    protocol::ReadOperation,
    topology::{RequestTarget, ResponderIdentity},
};

const VIN_DID: u16 = 0xF190;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationCandidate {
    semantic: String,
    definition_id: String,
    definition_version: u32,
    did: u16,
}

impl IdentificationCandidate {
    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }

    pub const fn definition_version(&self) -> u32 {
        self.definition_version
    }

    pub const fn did(&self) -> u16 {
        self.did
    }

    pub const fn request_bytes(&self) -> [u8; 3] {
        let [high, low] = self.did.to_be_bytes();
        [0x22, high, low]
    }

    pub(crate) const fn operation(&self) -> ReadOperation {
        ReadOperation::uds_read_data_by_identifier(self.did)
    }
}

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
                    return Err(
                        "negative ECU identification evidence requires NRC and no value".into(),
                    );
                }
            }
            IdentificationResultStatus::Malformed => {
                if self.value.is_some() {
                    return Err("malformed ECU identification evidence cannot carry a value".into());
                }
            }
            IdentificationResultStatus::Timeout | IdentificationResultStatus::TransportError => {
                if self.nrc.is_some() || self.value.is_some() || self.errors.is_empty() {
                    return Err(
                        "transport ECU identification evidence requires an error only".into(),
                    );
                }
            }
            IdentificationResultStatus::NotProbed => {
                if self.nrc.is_some() || self.value.is_some() || !self.responses.is_empty() {
                    return Err(
                        "not-probed ECU identification evidence cannot contain a response".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EcuIdentificationPlan {
    knowledge_repository: String,
    knowledge_revision: String,
    knowledge_schema_version: u32,
    set_id: String,
    set_version: u32,
    candidates: Vec<IdentificationCandidate>,
}

impl EcuIdentificationPlan {
    pub fn from_catalog(catalog: &KnowledgeCatalog) -> Result<Self, String> {
        let set = catalog
            .set(STANDARD_UDS_ECU_IDENTIFICATION_SET)
            .ok_or_else(|| {
                format!(
                    "canonical knowledge is missing set {STANDARD_UDS_ECU_IDENTIFICATION_SET:?}"
                )
            })?;
        let candidates = set
            .members()
            .iter()
            .map(|semantic| {
                let definition = catalog.semantic(semantic).ok_or_else(|| {
                    format!(
                        "knowledge set {:?} references unresolved semantic {semantic:?}",
                        set.id()
                    )
                })?;
                let did = definition.operation().did();
                if did == VIN_DID {
                    return Err(
                        "VIN/F190 must not enter bounded ECU identification discovery".into(),
                    );
                }
                let request = definition.operation().request_bytes();
                if request[0] != 0x22 || u16::from_be_bytes([request[1], request[2]]) != did {
                    return Err(format!(
                        "knowledge definition {:?} did not resolve to a typed UDS ReadDataByIdentifier request",
                        definition.id()
                    ));
                }
                Ok(IdentificationCandidate {
                    semantic: semantic.clone(),
                    definition_id: definition.id().into(),
                    definition_version: definition.version(),
                    did,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(Self {
            knowledge_repository: catalog.pin().repository().into(),
            knowledge_revision: catalog.pin().revision().into(),
            knowledge_schema_version: catalog.pin().schema_version(),
            set_id: set.id().into(),
            set_version: set.version(),
            candidates,
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

    pub fn set_id(&self) -> &str {
        &self.set_id
    }

    pub const fn set_version(&self) -> u32 {
        self.set_version
    }

    pub fn candidates(&self) -> &[IdentificationCandidate] {
        &self.candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> EcuIdentificationPlan {
        EcuIdentificationPlan::from_catalog(
            &KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap(),
        )
        .unwrap()
    }

    #[test]
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
    fn plan_is_exactly_the_canonical_set_in_declared_order() {
        let catalog = KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap();
        let set = catalog.set(STANDARD_UDS_ECU_IDENTIFICATION_SET).unwrap();
        let plan = EcuIdentificationPlan::from_catalog(&catalog).unwrap();

        assert_eq!(
            plan.candidates()
                .iter()
                .map(IdentificationCandidate::semantic)
                .collect::<Vec<_>>(),
            set.members().iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn candidates_are_only_typed_rdbi_and_never_vin() {
        for candidate in plan().candidates() {
            assert_ne!(candidate.did(), VIN_DID);
            assert_eq!(candidate.request_bytes()[0], 0x22);
            assert_eq!(
                u16::from_be_bytes([candidate.request_bytes()[1], candidate.request_bytes()[2]]),
                candidate.did()
            );
        }
    }

    #[test]
    fn f189_resolves_without_a_second_did_list() {
        let plan = plan();
        let candidate = plan
            .candidates()
            .iter()
            .find(|candidate| candidate.semantic() == "ecu.manufacturer_software_version")
            .unwrap();

        assert_eq!(
            candidate.definition_id(),
            "uds.f189.manufacturer_software_version"
        );
        assert_eq!(candidate.did(), 0xF189);
        assert_eq!(candidate.request_bytes(), [0x22, 0xF1, 0x89]);
    }

    #[test]
    fn plan_retains_exact_knowledge_provenance_and_is_deterministic() {
        let first = plan();
        let second = plan();
        assert_eq!(first, second);
        assert_eq!(
            first.knowledge_repository(),
            "frankherchet/obdentic-knowledge"
        );
        assert_eq!(
            first.knowledge_revision(),
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28"
        );
        assert_eq!(first.knowledge_schema_version(), 1);
        assert_eq!(first.set_id(), STANDARD_UDS_ECU_IDENTIFICATION_SET);
    }
}
