from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement anchor, found {count}: {old[:80]!r}")
    file.write_text(text.replace(old, new, 1))


module = r'''//! Bounded standards-based ECU-identification planning.
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

        assert_eq!(candidate.definition_id(), "uds.f189.manufacturer_software_version");
        assert_eq!(candidate.did(), 0xF189);
        assert_eq!(candidate.request_bytes(), [0x22, 0xF1, 0x89]);
    }

    #[test]
    fn plan_retains_exact_knowledge_provenance_and_is_deterministic() {
        let first = plan();
        let second = plan();
        assert_eq!(first, second);
        assert_eq!(first.knowledge_repository(), "frankherchet/obdentic-knowledge");
        assert_eq!(
            first.knowledge_revision(),
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28"
        );
        assert_eq!(first.knowledge_schema_version(), 1);
        assert_eq!(first.set_id(), STANDARD_UDS_ECU_IDENTIFICATION_SET);
    }
}
'''
Path("src/ecu_identification.rs").write_text(module)

replace_once(
    "src/lib.rs",
    "pub mod dtc;\npub mod ea189;\npub(crate) mod elm;",
    "pub mod dtc;\npub mod ea189;\npub mod ecu_identification;\npub(crate) mod elm;",
)

Path("src/bin/obdentic-ecu-identification-plan.rs").write_text(r'''use obdentic::{
    ecu_identification::EcuIdentificationPlan, hex, knowledge_db::KnowledgeCatalog,
};
use std::{env, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let project_root = args
        .next()
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into());
    if args.next().is_some() {
        return Err("usage: obdentic-ecu-identification-plan [project-root]".into());
    }

    let catalog = KnowledgeCatalog::load_pinned(Path::new(&project_root))
        .map_err(|error| error.to_string())?;
    let plan = EcuIdentificationPlan::from_catalog(&catalog)?;

    println!("bounded ECU identification dry-run");
    println!("transport\tdisabled");
    println!("knowledge_repository\t{}", plan.knowledge_repository());
    println!("knowledge_revision\t{}", plan.knowledge_revision());
    println!("knowledge_schema\t{}", plan.knowledge_schema_version());
    println!("set\t{}@{}", plan.set_id(), plan.set_version());
    for candidate in plan.candidates() {
        println!(
            "candidate\t{}\t{}@{}\tDID {:04X}\t{}",
            candidate.semantic(),
            candidate.definition_id(),
            candidate.definition_version(),
            candidate.did(),
            hex(&candidate.request_bytes())
        );
    }
    Ok(())
}
''')

# SafetyPolicy: the canonical bounded candidate is a first-class read-only operation.
replace_once(
    "src/safety.rs",
    "    diagnostic_job::{DiagnosticScope, KnownTarget},\n    ea189::{Ea189DpfProbe, Ea189DpfProbeError, Ea189DpfProbeRequest},",
    "    diagnostic_job::{DiagnosticScope, KnownTarget},\n    ea189::{Ea189DpfProbe, Ea189DpfProbeError, Ea189DpfProbeRequest},\n    ecu_identification::IdentificationCandidate,",
)
replace_once(
    "src/safety.rs",
    "    Ea189DpfProbe,\n    DtcClear,",
    "    Ea189DpfProbe,\n    EcuIdentification,\n    DtcClear,",
)
replace_once(
    "src/safety.rs",
    "    Ea189DpfProbe(Ea189DpfProbeRequest),\n    ClearDtcs,",
    "    Ea189DpfProbe(Ea189DpfProbeRequest),\n    EcuIdentification(IdentificationCandidate),\n    ClearDtcs,",
)
replace_once(
    "src/safety.rs",
    "            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,\n            Self::ClearDtcs => OperationKind::DtcClear,",
    "            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,\n            Self::EcuIdentification(_) => OperationKind::EcuIdentification,\n            Self::ClearDtcs => OperationKind::DtcClear,",
)
replace_once(
    "src/safety.rs",
    "    Ea189DpfProbe(Ea189DpfProbeRequest),\n}\n\nimpl Operation {",
    "    Ea189DpfProbe(Ea189DpfProbeRequest),\n    EcuIdentification(IdentificationCandidate),\n}\n\nimpl Operation {",
)
replace_once(
    "src/safety.rs",
    "            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,\n        }",
    "            Self::Ea189DpfProbe(_) => OperationKind::Ea189DpfProbe,\n            Self::EcuIdentification(_) => OperationKind::EcuIdentification,\n        }",
)
replace_once(
    "src/safety.rs",
    "    pub fn ea189_dpf_probe(request: Ea189DpfProbeRequest) -> Self {\n        Self::Ea189DpfProbe(request)\n    }",
    "    pub fn ea189_dpf_probe(request: Ea189DpfProbeRequest) -> Self {\n        Self::Ea189DpfProbe(request)\n    }\n\n    pub fn ecu_identification(candidate: IdentificationCandidate) -> Self {\n        Self::EcuIdentification(candidate)\n    }",
)
replace_once(
    "src/safety.rs",
    "                OperationKind::SignalRead | OperationKind::DtcRead | OperationKind::Ea189DpfProbe\n            ),",
    "                OperationKind::SignalRead\n                    | OperationKind::DtcRead\n                    | OperationKind::Ea189DpfProbe\n                    | OperationKind::EcuIdentification\n            ),",
)
replace_once(
    "src/safety.rs",
    "            OperationRequest::Ea189DpfProbe(probe) => {\n                self.authorize_operation(Operation::Ea189DpfProbe(probe))\n            }",
    "            OperationRequest::Ea189DpfProbe(probe) => {\n                self.authorize_operation(Operation::Ea189DpfProbe(probe))\n            }\n            OperationRequest::EcuIdentification(candidate) => {\n                self.authorize_operation(Operation::EcuIdentification(candidate))\n            }",
)
replace_once(
    "src/safety.rs",
    "                    OperationKind::DtcRead | OperationKind::Ea189DpfProbe\n                )",
    "                    OperationKind::DtcRead\n                        | OperationKind::Ea189DpfProbe\n                        | OperationKind::EcuIdentification\n                )",
)
replace_once(
    "src/safety.rs",
    "    #[test]\n    fn generic_signal_path_cannot_construct_an_ea189_dpf_did() {",
    "    #[test]\n    fn bounded_ecu_identification_is_diagnose_only_and_read_only() {\n        let catalog = crate::knowledge_db::KnowledgeCatalog::load_pinned(env!(\"CARGO_MANIFEST_DIR\")).unwrap();\n        let plan = crate::ecu_identification::EcuIdentificationPlan::from_catalog(&catalog).unwrap();\n        let candidate = plan.candidates()[0].clone();\n        let request = OperationRequest::EcuIdentification(candidate.clone());\n        assert_eq!(request.kind(), OperationKind::EcuIdentification);\n        assert_eq!(\n            SafetyPolicy::default()\n                .authorize_activity(Activity::Diagnose, request)\n                .unwrap(),\n            Operation::EcuIdentification(candidate)\n        );\n        assert!(SafetyPolicy::default()\n            .authorize_activity(\n                Activity::Read,\n                OperationRequest::EcuIdentification(plan.candidates()[0].clone())\n            )\n            .is_err());\n    }\n\n    #[test]\n    fn generic_signal_path_cannot_construct_an_ea189_dpf_did() {",
)

# ELM: add a closed target request whose DID can only come from IdentificationCandidate.
replace_once(
    "src/elm.rs",
    "pub struct TargetedDpfProbeRequest {\n    operation: crate::protocol::ReadOperation,\n    target: RequestTarget,\n    expected_responder: ResponderIdentity,\n}\n\nimpl TargetedDpfProbeRequest {",
    "pub struct TargetedDpfProbeRequest {\n    operation: crate::protocol::ReadOperation,\n    target: RequestTarget,\n    expected_responder: ResponderIdentity,\n}\n\n#[derive(Clone, Debug, PartialEq, Eq)]\npub struct TargetedEcuIdentificationRequest {\n    candidate: crate::ecu_identification::IdentificationCandidate,\n    operation: crate::protocol::ReadOperation,\n    target: RequestTarget,\n    expected_responder: ResponderIdentity,\n}\n\nimpl TargetedDpfProbeRequest {",
)
replace_once(
    "src/elm.rs",
    "impl TargetedReadRequest {\n    pub fn new(",
    "impl TargetedEcuIdentificationRequest {\n    pub fn from_evidence(\n        candidate: &crate::ecu_identification::IdentificationCandidate,\n        target: &crate::topology::RequestTargetEvidence,\n        expected_responder: &crate::topology::ResponderIdentity,\n    ) -> Result<Self, String> {\n        if expected_responder.context() != target.target().context() {\n            return Err(\"ECU identification target and responder contexts differ\".into());\n        }\n        let responder = expected_responder.value().ok_or_else(|| {\n            \"ECU identification requires an evidenced expected responder\".to_string()\n        })?;\n        Self::new(\n            candidate.clone(),\n            target.target().clone(),\n            ResponderIdentity::ElmHeader(responder.to_owned()),\n        )\n    }\n\n    fn new(\n        candidate: crate::ecu_identification::IdentificationCandidate,\n        target: RequestTarget,\n        expected_responder: ResponderIdentity,\n    ) -> Result<Self, String> {\n        validate_request_target(&target)?;\n        validate_elm_header(&expected_responder, \"expected responder\")?;\n        let operation = candidate.operation();\n        if operation.request_bytes() != candidate.request_bytes() {\n            return Err(\"ECU identification candidate did not resolve deterministically\".into());\n        }\n        Ok(Self {\n            candidate,\n            operation,\n            target,\n            expected_responder: ResponderIdentity::ElmHeader(\n                expected_responder.as_str().to_ascii_uppercase(),\n            ),\n        })\n    }\n\n    pub fn candidate(&self) -> &crate::ecu_identification::IdentificationCandidate {\n        &self.candidate\n    }\n\n    pub fn did(&self) -> u16 {\n        self.operation.did()\n    }\n\n    pub fn request_bytes(&self) -> [u8; 3] {\n        self.operation.request_bytes()\n    }\n\n    pub fn target(&self) -> &RequestTarget {\n        &self.target\n    }\n\n    pub fn expected_responder(&self) -> &ResponderIdentity {\n        &self.expected_responder\n    }\n}\n\nimpl TargetedReadRequest {\n    pub fn new(",
)
replace_once(
    "src/elm.rs",
    "pub(crate) struct DpfProbeReadEvidence {\n    pub(crate) responses: DiagnosticResponses,\n    pub(crate) observations: Vec<ResponseObservation>,\n}\n",
    "pub(crate) struct DpfProbeReadEvidence {\n    pub(crate) responses: DiagnosticResponses,\n    pub(crate) observations: Vec<ResponseObservation>,\n}\n\n#[derive(Debug)]\npub(crate) struct EcuIdentificationReadEvidence {\n    pub(crate) responses: DiagnosticResponses,\n    pub(crate) observations: Vec<ResponseObservation>,\n}\n",
)
replace_once(
    "src/elm.rs",
    "    pub(crate) async fn read_dpf_probe_with_evidence(\n        &mut self,\n        request: &TargetedDpfProbeRequest,\n    ) -> Result<DpfProbeReadEvidence, ReadEvidenceError> {",
    "    pub(crate) async fn read_dpf_probe_with_evidence(\n        &mut self,\n        request: &TargetedDpfProbeRequest,\n    ) -> Result<DpfProbeReadEvidence, ReadEvidenceError> {",
)
# Insert the new method after the existing DPF method by anchoring its closing block.
replace_once(
    "src/elm.rs",
    "        Ok(DpfProbeReadEvidence {\n            observations: vec![responses.observation(None)],\n            responses,\n        })\n    }\n}\n\npub(crate) async fn read_elm_with_evidence<E>(",
    "        Ok(DpfProbeReadEvidence {\n            observations: vec![responses.observation(None)],\n            responses,\n        })\n    }\n\n    pub(crate) async fn read_ecu_identification_with_evidence(\n        &mut self,\n        request: &TargetedEcuIdentificationRequest,\n    ) -> Result<EcuIdentificationReadEvidence, ReadEvidenceError> {\n        if let Err(error) = configure_target(\n            &mut self.exchange,\n            request.target(),\n            request.expected_responder(),\n        )\n        .await\n        {\n            let restore = restore_functional(&mut self.exchange).await;\n            return Err(ReadEvidenceError {\n                error: combine_setup_errors(error, restore),\n                observations: Vec::new(),\n            });\n        }\n\n        let read = match read_elm_ecu_identification_responses(&mut self.exchange, request).await {\n            Ok(responses) => {\n                let selection_error = targeted_payload(&responses, request.expected_responder()).err();\n                Ok(EcuIdentificationReadEvidence {\n                    observations: vec![responses.observation(selection_error)],\n                    responses,\n                })\n            }\n            Err(error) => Err(ReadEvidenceError {\n                error,\n                observations: Vec::new(),\n            }),\n        };\n        let restore = restore_functional(&mut self.exchange).await;\n        match (read, restore) {\n            (Ok(read), Ok(())) => Ok(read),\n            (Err(error), Ok(())) => Err(error),\n            (Ok(read), Err(error)) => Err(ReadEvidenceError {\n                error: format!(\n                    \"ECU identification read succeeded; restoring functional addressing failed: {error}\"\n                ),\n                observations: read.observations,\n            }),\n            (Err(mut error), Err(restore)) => {\n                error.error = format!(\n                    \"{}; restoring functional addressing failed: {restore}\",\n                    error.error\n                );\n                Err(error)\n            }\n        }\n    }\n}\n\npub(crate) async fn read_elm_with_evidence<E>(",
)
replace_once(
    "src/elm.rs",
    "pub(crate) async fn read_elm_uds_responses<E>(\n    exchange: &mut E,\n    request: &TargetedDpfProbeRequest,\n) -> Result<DiagnosticResponses, String>\nwhere\n    E: ElmExchange,\n{\n    let command = uds_command(request.operation);\n    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;\n    normalize_uds_responses(&response, request.did())\n}\n",
    "pub(crate) async fn read_elm_uds_responses<E>(\n    exchange: &mut E,\n    request: &TargetedDpfProbeRequest,\n) -> Result<DiagnosticResponses, String>\nwhere\n    E: ElmExchange,\n{\n    let command = uds_command(request.operation);\n    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;\n    normalize_uds_responses(&response, request.did())\n}\n\npub(crate) async fn read_elm_ecu_identification_responses<E>(\n    exchange: &mut E,\n    request: &TargetedEcuIdentificationRequest,\n) -> Result<DiagnosticResponses, String>\nwhere\n    E: ElmExchange,\n{\n    let command = uds_command(request.operation);\n    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;\n    normalize_uds_responses(&response, request.did())\n}\n",
)
replace_once(
    "src/elm.rs",
    "    #[tokio::test]\n    async fn generic_session_executes_closed_mode01_read_without_adapter_backend() {\n        let exchange = ScriptedExchange::new([\"410000100000\\r>\", \"7E8 04 41 0C 1A F8\\r>\"]);\n        let mut session = ElmSession::new(exchange);\n\n        session.discover_support(0).await.unwrap();\n        let request = crate::prepare_read(\"engine.rpm\").unwrap();\n        let read = session.read_with_evidence(request).await.unwrap();\n\n        assert_eq!(read.payload, [0x41, 0x0c, 0x1a, 0xf8]);\n        assert_eq!(session.into_exchange().commands, [\"0100\\r\", \"010C\\r\"]);\n    }\n}",
    "    #[tokio::test]\n    async fn generic_session_executes_closed_mode01_read_without_adapter_backend() {\n        let exchange = ScriptedExchange::new([\"410000100000\\r>\", \"7E8 04 41 0C 1A F8\\r>\"]);\n        let mut session = ElmSession::new(exchange);\n\n        session.discover_support(0).await.unwrap();\n        let request = crate::prepare_read(\"engine.rpm\").unwrap();\n        let read = session.read_with_evidence(request).await.unwrap();\n\n        assert_eq!(read.payload, [0x41, 0x0c, 0x1a, 0xf8]);\n        assert_eq!(session.into_exchange().commands, [\"0100\\r\", \"010C\\r\"]);\n    }\n\n    #[tokio::test]\n    async fn generic_session_executes_only_a_canonical_ecu_identification_candidate() {\n        let catalog = crate::knowledge_db::KnowledgeCatalog::load_pinned(env!(\"CARGO_MANIFEST_DIR\")).unwrap();\n        let plan = crate::ecu_identification::EcuIdentificationPlan::from_catalog(&catalog).unwrap();\n        let candidate = plan\n            .candidates()\n            .iter()\n            .find(|candidate| candidate.did() == 0xF189)\n            .unwrap();\n        let context = crate::topology::ProtocolContext::new(\n            crate::topology::Protocol::Obd2,\n            crate::topology::AddressingContext::Physical,\n        );\n        let target = crate::topology::RequestTargetEvidence::new(\n            crate::topology::RequestTarget::concrete(\n                context.clone(),\n                crate::topology::RequestAddress::new(\"elm-header\", \"7E0\"),\n            ),\n            crate::topology::Provenance::new(\n                \"test target\",\n                crate::topology::Confidence::High,\n            )\n            .unwrap(),\n        );\n        let responder = crate::topology::ResponderIdentity::address(context, \"7E8\");\n        let request = TargetedEcuIdentificationRequest::from_evidence(\n            candidate,\n            &target,\n            &responder,\n        )\n        .unwrap();\n        let exchange = ScriptedExchange::new([\n            \"OK\\r>\",\n            \"OK\\r>\",\n            \"7E8 05 62 F1 89 31 2E 55 55\\r>\",\n            \"OK\\r>\",\n            \"OK\\r>\",\n            \"OK\\r>\",\n        ]);\n        let mut session = ElmSession::new(exchange);\n\n        let read = session\n            .read_ecu_identification_with_evidence(&request)\n            .await\n            .unwrap();\n\n        assert_eq!(\n            read.responses.as_slice()[0].payload,\n            [0x62, 0xf1, 0x89, 0x31, 0x2e]\n        );\n        assert_eq!(\n            session.into_exchange().commands,\n            [\n                \"ATSH 7E0\\r\",\n                \"ATCRA 7E8\\r\",\n                \"22F189\\r\",\n                \"ATSP0\\r\",\n                \"ATSH 7DF\\r\",\n                \"ATCRA\\r\",\n            ]\n        );\n    }\n}",
)

# BLE actor: one new closed command; no generic UDS/DID message exists.
replace_once(
    "src/ble.rs",
    "    TargetedDpfProbeRequest, TargetedReadRequest,\n};",
    "    TargetedDpfProbeRequest, TargetedEcuIdentificationRequest, TargetedReadRequest,\n};",
)
replace_once(
    "src/ble.rs",
    "impl DpfProbeOutcome {\n    fn into_result(self) -> Result<DiagnosticResponses, String> {\n        match self {\n            Self::Succeeded { responses, .. } => Ok(responses),\n            Self::Failed { error, .. } => Err(error),\n        }\n    }\n}\n",
    "impl DpfProbeOutcome {\n    fn into_result(self) -> Result<DiagnosticResponses, String> {\n        match self {\n            Self::Succeeded { responses, .. } => Ok(responses),\n            Self::Failed { error, .. } => Err(error),\n        }\n    }\n}\n\n#[derive(Debug, PartialEq)]\npub(crate) enum EcuIdentificationOutcome {\n    Succeeded {\n        responses: DiagnosticResponses,\n        observations: Vec<ResponseObservation>,\n    },\n    Failed {\n        error: String,\n        observations: Vec<ResponseObservation>,\n    },\n}\n\nimpl EcuIdentificationOutcome {\n    fn into_result(self) -> Result<DiagnosticResponses, String> {\n        match self {\n            Self::Succeeded { responses, .. } => Ok(responses),\n            Self::Failed { error, .. } => Err(error),\n        }\n    }\n}\n",
)
replace_once(
    "src/ble.rs",
    "    pub(crate) async fn read_dpf_probe_with_evidence(\n        &self,\n        request: TargetedDpfProbeRequest,\n    ) -> Result<DpfProbeOutcome, String> {\n        let (reply, result) = oneshot::channel();\n        self.sender\n            .send(SessionCommand::ReadDpfProbe { request, reply })\n            .await\n            .map_err(|_| \"diagnostic session is closed\".to_string())?;\n        result\n            .await\n            .map_err(|_| \"diagnostic session stopped before responding\".to_string())?\n    }\n",
    "    pub(crate) async fn read_dpf_probe_with_evidence(\n        &self,\n        request: TargetedDpfProbeRequest,\n    ) -> Result<DpfProbeOutcome, String> {\n        let (reply, result) = oneshot::channel();\n        self.sender\n            .send(SessionCommand::ReadDpfProbe { request, reply })\n            .await\n            .map_err(|_| \"diagnostic session is closed\".to_string())?;\n        result\n            .await\n            .map_err(|_| \"diagnostic session stopped before responding\".to_string())?\n    }\n\n    pub async fn read_ecu_identification(\n        &self,\n        request: TargetedEcuIdentificationRequest,\n    ) -> Result<DiagnosticResponses, String> {\n        self.read_ecu_identification_with_evidence(request)\n            .await?\n            .into_result()\n    }\n\n    pub(crate) async fn read_ecu_identification_with_evidence(\n        &self,\n        request: TargetedEcuIdentificationRequest,\n    ) -> Result<EcuIdentificationOutcome, String> {\n        let (reply, result) = oneshot::channel();\n        self.sender\n            .send(SessionCommand::ReadEcuIdentification { request, reply })\n            .await\n            .map_err(|_| \"diagnostic session is closed\".to_string())?;\n        result\n            .await\n            .map_err(|_| \"diagnostic session stopped before responding\".to_string())?\n    }\n",
)
replace_once(
    "src/ble.rs",
    "    ReadDpfProbe {\n        request: TargetedDpfProbeRequest,\n        reply: oneshot::Sender<Result<DpfProbeOutcome, String>>,\n    },\n    ReadStoredDtcs {",
    "    ReadDpfProbe {\n        request: TargetedDpfProbeRequest,\n        reply: oneshot::Sender<Result<DpfProbeOutcome, String>>,\n    },\n    ReadEcuIdentification {\n        request: TargetedEcuIdentificationRequest,\n        reply: oneshot::Sender<Result<EcuIdentificationOutcome, String>>,\n    },\n    ReadStoredDtcs {",
)
replace_once(
    "src/ble.rs",
    "            SessionCommand::ReadStoredDtcs { reply } => {",
    "            SessionCommand::ReadEcuIdentification { request, reply } => {\n                if let Some(error) = health.unhealthy() {\n                    let _ = reply.send(Err(error.to_owned()));\n                    continue;\n                }\n                let started = Instant::now();\n                let outcome = session.read_ecu_identification_with_evidence(request).await;\n                process_ecu_identification_outcome(\n                    &mut session,\n                    &mut health,\n                    &mut service,\n                    &mut disconnect_done,\n                    outcome,\n                    started.elapsed(),\n                    reply,\n                )\n                .await;\n            }\n            SessionCommand::ReadStoredDtcs { reply } => {",
)
replace_once(
    "src/ble.rs",
    "#[derive(Default)]\nstruct SessionHealth {",
    "async fn process_ecu_identification_outcome(\n    session: &mut DiagnosticSession,\n    health: &mut SessionHealth,\n    service: &mut RequestServiceEstimator,\n    disconnect_done: &mut bool,\n    outcome: EcuIdentificationOutcome,\n    service_time: Duration,\n    reply: oneshot::Sender<Result<EcuIdentificationOutcome, String>>,\n) {\n    if let Some(error) = health.unhealthy() {\n        let _ = reply.send(Err(error.to_owned()));\n        return;\n    }\n    match outcome {\n        EcuIdentificationOutcome::Succeeded { .. } => {\n            service.observe(service_time);\n            health.success();\n            let _ = reply.send(Ok(outcome));\n        }\n        EcuIdentificationOutcome::Failed {\n            error,\n            observations,\n        } => {\n            if health.observe(&error) {\n                let fatal = health.unhealthy().unwrap().to_owned();\n                session.disconnect_best_effort().await;\n                *disconnect_done = true;\n                let _ = reply.send(Ok(EcuIdentificationOutcome::Failed {\n                    error: fatal,\n                    observations,\n                }));\n            } else {\n                service.observe(service_time);\n                let _ = reply.send(Ok(EcuIdentificationOutcome::Failed {\n                    error,\n                    observations,\n                }));\n            }\n        }\n    }\n}\n\n#[derive(Default)]\nstruct SessionHealth {",
)
replace_once(
    "src/ble.rs",
    "    async fn read_stored_dtcs(&mut self) -> Result<DiagnosticResponses, String> {",
    "    async fn read_ecu_identification_with_evidence(\n        &mut self,\n        request: TargetedEcuIdentificationRequest,\n    ) -> EcuIdentificationOutcome {\n        let read = match self.elm_mut() {\n            Ok(session) => session.read_ecu_identification_with_evidence(&request).await,\n            Err(error) => Err(ReadEvidenceError {\n                error,\n                observations: Vec::new(),\n            }),\n        };\n        match read {\n            Ok(read) => EcuIdentificationOutcome::Succeeded {\n                responses: read.responses,\n                observations: read.observations,\n            },\n            Err(error) => EcuIdentificationOutcome::Failed {\n                error: error.error,\n                observations: error.observations,\n            },\n        }\n    }\n\n    async fn read_stored_dtcs(&mut self) -> Result<DiagnosticResponses, String> {",
)
replace_once(
    "src/ble.rs",
    "                    SessionCommand::ReadDpfProbe { reply, .. } => {\n                        let _ = reply.send(Err(\"DPF probe test request not scripted\".into()));\n                    }\n                    SessionCommand::ReadStoredDtcs { reply } => {",
    "                    SessionCommand::ReadDpfProbe { reply, .. } => {\n                        let _ = reply.send(Err(\"DPF probe test request not scripted\".into()));\n                    }\n                    SessionCommand::ReadEcuIdentification { reply, .. } => {\n                        let _ = reply.send(Err(\"ECU identification test request not scripted\".into()));\n                    }\n                    SessionCommand::ReadStoredDtcs { reply } => {",
)

print("issue #85 live patch applied")
