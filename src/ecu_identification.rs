//! Bounded standards-based ECU-identification planning.
//!
//! Canonical Knowledge selects the candidates. This module deliberately has
//! no transport access and exposes no constructor that accepts a caller DID.

use crate::{
    knowledge_db::{KnowledgeCatalog, STANDARD_UDS_ECU_IDENTIFICATION_SET},
    protocol::ReadOperation,
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
