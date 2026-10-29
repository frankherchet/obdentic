use crate::{
    ecu_identification::{
        EcuIdentificationPlan, IdentificationCandidate, IdentificationObservation,
        IdentificationResponseEvidence, IdentificationResultStatus,
    },
    effective_knowledge::{EffectiveVehicleKnowledge, ObservedEcuFacts},
    elm::{ReadEvidence, ReadEvidenceError},
    knowledge_db::{
        AsciiTrim, Endian, HardwareValidationStatus, KnowledgeCatalog, KnowledgeDecoder,
        KnowledgeDefinition, KnowledgeEcuTarget,
    },
    topology::{
        AddressingContext, Confidence, Protocol, ProtocolContext, Provenance, RequestAddress,
        RequestTarget, RequestTargetEvidence, ResponderIdentity,
    },
};
use serde_json::{json, Value};
use std::time::Duration;

pub(super) const LEARNING_LIMIT: Duration = Duration::from_secs(120);
pub(super) const PRESENCE_DID: u16 = 0xF187;

#[derive(Clone)]
pub(super) struct CandidateTarget {
    pub(super) id: String,
    pub(super) role: String,
    pub(super) manufacturer: String,
    pub(super) request_id: u16,
    pub(super) response_id: u16,
    pub(super) target: RequestTarget,
    pub(super) responder: ResponderIdentity,
    pub(super) provenance: Provenance,
}

impl CandidateTarget {
    fn from_catalog(target: &KnowledgeEcuTarget, repository: &str) -> Result<Self, String> {
        if target.request_id() > 0x7ff || target.response_id() > 0x7ff {
            return Err(format!(
                "Knowledge target {} is not an 11-bit CAN address",
                target.id()
            ));
        }
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let request = RequestTarget::concrete(
            context.clone(),
            RequestAddress::new("elm-header", format!("{:03X}", target.request_id())),
        );
        let responder =
            ResponderIdentity::address(context, format!("{:03X}", target.response_id()));
        let provenance = Provenance::new(
            format!("pinned ECU target {} from {repository}", target.id()),
            Confidence::Medium,
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            id: target.id().to_owned(),
            role: target.role().to_owned(),
            manufacturer: target.manufacturer().to_ascii_lowercase(),
            request_id: target.request_id(),
            response_id: target.response_id(),
            target: request,
            responder,
            provenance,
        })
    }

    pub(super) fn request_evidence(&self) -> RequestTargetEvidence {
        RequestTargetEvidence::new(self.target.clone(), self.provenance.clone())
    }

    pub(super) fn elm_responder(&self) -> crate::elm::ResponderIdentity {
        crate::elm::ResponderIdentity::ElmHeader(format!("{:03X}", self.response_id))
    }

    fn context(&self) -> ProtocolContext {
        self.target.context().clone()
    }

    pub(super) fn header(&self) -> String {
        format!("{:03X}", self.request_id)
    }

    pub(super) fn response_header(&self) -> String {
        format!("{:03X}", self.response_id)
    }
}

pub(super) struct EcuState {
    pub(super) candidate: CandidateTarget,
    pub(super) present: bool,
    pub(super) identity: Vec<IdentificationObservation>,
}

pub(super) struct MobileKnowledge {
    pub(super) catalog: KnowledgeCatalog,
    pub(super) plan: EcuIdentificationPlan,
    pub(super) targets: Vec<EcuState>,
    pub(super) manufacturer: String,
}

impl MobileKnowledge {
    pub(super) fn embedded(manufacturer: &str) -> Result<Self, String> {
        let catalog = KnowledgeCatalog::embedded_pinned().map_err(|error| error.to_string())?;
        Self::new(catalog, manufacturer)
    }

    pub(super) fn new(catalog: KnowledgeCatalog, manufacturer: &str) -> Result<Self, String> {
        let manufacturer = manufacturer.to_ascii_lowercase();
        let targets = catalog
            .ecu_targets()
            .iter()
            .filter(|target| target.manufacturer().eq_ignore_ascii_case(&manufacturer))
            .map(|target| {
                Ok(EcuState {
                    candidate: CandidateTarget::from_catalog(target, catalog.pin().repository())?,
                    present: false,
                    identity: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if targets.is_empty() {
            return Err(format!(
                "pinned Knowledge has no ECU targets for {manufacturer:?}"
            ));
        }
        let plan = EcuIdentificationPlan::from_catalog(&catalog)?;
        if !plan
            .candidates()
            .iter()
            .any(|candidate| candidate.did() == PRESENCE_DID)
        {
            return Err("pinned Knowledge has no standard F187 identification candidate".into());
        }
        Ok(Self {
            catalog,
            plan,
            targets,
            manufacturer,
        })
    }

    pub(super) fn presence_candidate(&self) -> &IdentificationCandidate {
        self.plan
            .candidates()
            .iter()
            .find(|candidate| candidate.did() == PRESENCE_DID)
            .expect("catalog plan was checked for the standard F187 candidate")
    }

    pub(super) fn candidate(&self, target_id: &str) -> Option<&CandidateTarget> {
        self.targets
            .iter()
            .find(|target| target.candidate.id == target_id)
            .map(|target| &target.candidate)
    }

    pub(super) fn record(&mut self, target_id: &str, observation: IdentificationObservation) {
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.candidate.id == target_id)
        {
            target.present |= observation_marks_presence(&target.candidate, &observation);
            target.identity.push(observation);
        }
    }

    pub(super) fn present_targets(&self) -> impl Iterator<Item = &EcuState> {
        self.targets.iter().filter(|target| {
            target.present
                && target.identity.iter().any(|observation| {
                    observation.status() == IdentificationResultStatus::Supported
                })
        })
    }

    pub(super) fn observations_json(&self) -> Value {
        json!({
            "knowledge_repository": self.catalog.pin().repository(),
            "knowledge_revision": self.catalog.pin().revision(),
            "vehicle_manufacturer": &self.manufacturer,
            "targets": self.targets.iter().map(target_json).collect::<Vec<_>>()
        })
    }

    pub(super) fn effective(&self) -> Result<EffectiveVehicleKnowledge, String> {
        EffectiveVehicleKnowledge::resolve(&self.catalog, self.present_targets().map(target_facts))
    }

    pub(super) fn signal_catalog(
        &self,
        can_read: impl Fn(&CandidateTarget, &KnowledgeDefinition) -> bool,
    ) -> Result<Vec<Value>, String> {
        let effective = self.effective()?;
        let mut signals = Vec::new();
        for ecu in effective.ecus() {
            let target = self
                .candidate(ecu.ecu_id())
                .ok_or_else(|| format!("effective ECU {} has no pinned target", ecu.ecu_id()))?;
            for resolution in ecu.semantics() {
                let Some(definition) = resolution.selected_definition(&self.catalog) else {
                    continue;
                };
                if definition.requires_extended_session() && !can_read(target, definition) {
                    continue;
                }
                signals.push(signal_json(target, definition));
            }
        }
        signals.sort_by(|left, right| {
            left["semantic"]
                .as_str()
                .cmp(&right["semantic"].as_str())
                .then_with(|| left["target_id"].as_str().cmp(&right["target_id"].as_str()))
        });
        Ok(signals)
    }

    pub(super) fn select_signal(
        &self,
        key: &str,
        can_read: impl Fn(&CandidateTarget, &KnowledgeDefinition) -> bool,
    ) -> Result<ResolvedDid, String> {
        let (semantic, only_target) = match key.rsplit_once('@') {
            Some((semantic, target)) => (semantic, Some(target)),
            None => (key, None),
        };
        let effective = self.effective()?;
        let mut found = Vec::new();
        for ecu in effective.ecus() {
            let Some(resolution) = ecu.semantic(semantic) else {
                continue;
            };
            if only_target
                .is_some_and(|only| self.candidate(ecu.ecu_id()).is_none_or(|t| t.id != only))
            {
                continue;
            }
            let Some(definition) = resolution.selected_definition(&self.catalog) else {
                continue;
            };
            let target = self
                .candidate(ecu.ecu_id())
                .ok_or_else(|| format!("effective ECU {} has no pinned target", ecu.ecu_id()))?;
            if !definition.requires_extended_session() || can_read(target, definition) {
                found.push(resolved_did(target, definition));
            }
        }
        match found.len() {
            0 => Err(format!(
                "Knowledge signal is not applicable to an identified ECU: {key}"
            )),
            1 => Ok(found.remove(0)),
            _ => Err(format!(
                "Knowledge signal matches more than one identified ECU: {key}"
            )),
        }
    }

    pub(super) fn learning_candidates(&self) -> Result<Vec<ResolvedDid>, String> {
        let effective = self.effective()?;
        let mut candidates = Vec::new();
        for ecu in effective.ecus() {
            let target = self
                .candidate(ecu.ecu_id())
                .ok_or_else(|| format!("effective ECU {} has no pinned target", ecu.ecu_id()))?;
            for resolution in ecu.semantics() {
                let Some(definition) = resolution.selected_definition(&self.catalog) else {
                    continue;
                };
                if definition.requires_extended_session() {
                    candidates.push(resolved_did(target, definition));
                }
            }
        }
        candidates.sort_by(|left, right| {
            left.target_id
                .cmp(&right.target_id)
                .then_with(|| left.definition.id().cmp(right.definition.id()))
        });
        Ok(candidates)
    }

    pub(super) fn learning_candidate_count_for(&self, target_id: &str) -> Result<usize, String> {
        Ok(self
            .learning_candidates()?
            .iter()
            .filter(|candidate| candidate.target_id == target_id)
            .count())
    }

    pub(super) fn learning_candidates_for_target(
        &self,
        target_id: &str,
    ) -> Result<Vec<ResolvedDid>, String> {
        Ok(self
            .learning_candidates()?
            .into_iter()
            .filter(|candidate| candidate.target_id == target_id)
            .collect())
    }
}

pub(super) fn manufacturer_from_vin(vin: &str) -> Option<String> {
    let identity = crate::vin_identity::describe(vin);
    match identity.get("make_hint").and_then(Value::as_str) {
        Some("Volkswagen" | "Audi") => Some("vag".into()),
        _ => None,
    }
}

pub(super) struct ResolvedDid {
    pub(super) target_id: String,
    pub(super) candidate: CandidateTarget,
    pub(super) definition: KnowledgeDefinition,
}

fn resolved_did(target: &CandidateTarget, definition: &KnowledgeDefinition) -> ResolvedDid {
    ResolvedDid {
        target_id: target.id.clone(),
        candidate: target.clone(),
        definition: definition.clone(),
    }
}

fn target_facts(target: &EcuState) -> ObservedEcuFacts {
    ObservedEcuFacts::new(target.candidate.id.clone())
        .and_then(|facts| {
            facts.with_fact(
                crate::knowledge_db::FingerprintField::VehicleManufacturer,
                target.candidate.manufacturer.clone(),
            )
        })
        .and_then(|facts| {
            facts.with_fact(
                crate::knowledge_db::FingerprintField::EcuLogicalRole,
                target.candidate.role.clone(),
            )
        })
        .expect("pinned target ids and normalized fields are validated")
}

/// #256: the identity of a live signal. A Knowledge semantic can coincide with a standard PID or with
/// the same semantic of another ECU, so a Knowledge signal is addressed as `<semantic>@<target id>`.
pub(super) fn signal_key(semantic: &str, target_id: &str) -> String {
    format!("{semantic}@{target_id}")
}

fn signal_json(target: &CandidateTarget, definition: &KnowledgeDefinition) -> Value {
    let id_parts = definition.id().split('.').collect::<Vec<_>>();
    let validation = hardware_validation(definition);
    json!({
        "semantic":definition.semantic(),
        "key":signal_key(definition.semantic(), &target.id),
        "definition_id":definition.id(),
        "target_id":target.id,
        "title":definition.description().unwrap_or(definition.semantic()),
        "ecu":target.role,
        "group":id_parts.get(2).copied().unwrap_or("Knowledge"),
        "unit":definition.unit().unwrap_or(""),
        "protocol":if definition.requires_extended_session() {"UDS"} else {"VW"},
        "supported":null,
        "validation":validation,
        "validation_label":validation_label(validation),
        "extended_session_required":definition.requires_extended_session()
    })
}

pub(super) fn target_json(target: &EcuState) -> Value {
    let identity = target
        .identity
        .iter()
        .filter(|observation| observation.status() == IdentificationResultStatus::Supported)
        .map(|observation| {
            json!({
                "semantic":observation.semantic(),
                "definition_id":observation.definition_id(),
                "did":observation_did(observation),
                "value":observation.value().map(<[u8]>::to_vec),
                "response":observation.responses().iter().find(|response| response.responder().and_then(ResponderIdentity::value).is_some_and(|value|value.eq_ignore_ascii_case(&target.candidate.response_header()))).map(|response|response.payload().to_vec())
            })
        })
        .collect::<Vec<_>>();
    json!({
        "target_id":target.candidate.id,
        "role":target.candidate.role,
        "manufacturer":target.candidate.manufacturer,
        "request_id":target.candidate.header(),
        "response_id":target.candidate.response_header(),
        "status":if target.present {"present"} else {"not_observed"},
        "absent":false,
        "identity":identity,
        "observations":target.identity.iter().map(identification_json).collect::<Vec<_>>()
    })
}

pub(super) fn identification_json(observation: &IdentificationObservation) -> Value {
    json!({
        "semantic":observation.semantic(),
        "definition_id":observation.definition_id(),
        "did":observation_did(observation),
        "request":observation.request(),
        "status":identification_status(observation.status()),
        "nrc":observation.nrc(),
        "value":observation.value(),
        "responses":observation.responses().iter().map(|response| json!({
            "responder":response.responder().and_then(ResponderIdentity::value),
            "payload":response.payload()
        })).collect::<Vec<_>>(),
        "errors":observation.errors()
    })
}

fn observation_did(observation: &IdentificationObservation) -> u16 {
    let request = observation.request();
    u16::from_be_bytes([request[1], request[2]])
}

fn observation_marks_presence(
    target: &CandidateTarget,
    observation: &IdentificationObservation,
) -> bool {
    observation_did(observation) == PRESENCE_DID
        && observation.responses().iter().any(|response| {
            response
                .responder()
                .and_then(ResponderIdentity::value)
                .is_some_and(|value| value.eq_ignore_ascii_case(&target.response_header()))
                && (response.payload().starts_with(&[0x62, 0xF1, 0x87])
                    || (response.payload().starts_with(&[0x7F, 0x22])
                        && response.payload().len() == 3))
        })
}

pub(super) fn identification_observation(
    catalog: &KnowledgeCatalog,
    target: &CandidateTarget,
    candidate: &IdentificationCandidate,
    result: Result<ReadEvidence, ReadEvidenceError>,
) -> Result<(IdentificationObservation, bool), String> {
    let definition = catalog
        .definition(candidate.definition_id())
        .ok_or_else(|| {
            format!(
                "identification definition {} is missing",
                candidate.definition_id()
            )
        })?;
    let mut responses = Vec::new();
    let mut errors = Vec::new();
    let (payload, response_observations, transport_error) = match result {
        Ok(read) => (Some(read.payload), read.observations, None),
        Err(error) => (None, error.observations, Some(error.error)),
    };
    for observation in response_observations {
        for response in observation.responses {
            let responder = response
                .responder
                .map(|value| ResponderIdentity::address(target.context(), value));
            responses.push(IdentificationResponseEvidence::new(
                responder,
                response.payload,
            ));
        }
    }
    if let Some(error) = transport_error {
        errors.push(error);
    }
    let (status, nrc, value) = match payload.as_deref() {
        Some(response) if response.starts_with(&[0x7f, 0x22]) && response.len() == 3 => (
            identification_status_for_nrc(response[2]),
            Some(response[2]),
            None,
        ),
        Some(response) => match definition.validate_response(response) {
            Ok(value) => (
                IdentificationResultStatus::Supported,
                None,
                Some(value.to_vec()),
            ),
            Err(error) => {
                errors.push(error);
                (IdentificationResultStatus::Malformed, None, None)
            }
        },
        None => {
            let error = errors
                .last()
                .cloned()
                .unwrap_or_else(|| "no response observed before request timeout".into());
            if errors.is_empty() {
                errors.push(error.clone());
            }
            let status = if error.to_ascii_lowercase().contains("timeout")
                || error.to_ascii_lowercase().contains("timed out")
            {
                IdentificationResultStatus::Timeout
            } else {
                IdentificationResultStatus::Unavailable
            };
            (status, None, None)
        }
    };
    let observation = IdentificationObservation::new(
        target.target.clone(),
        target.responder.clone(),
        candidate.semantic(),
        candidate.definition_id(),
        candidate.definition_version(),
        catalog.pin().repository(),
        catalog.pin().revision(),
        candidate.request_bytes(),
        status,
        responses,
        nrc,
        value,
        errors,
    )?;
    let present = observation_marks_presence(target, &observation);
    Ok((observation, present))
}

pub(super) fn target_from_catalog(
    catalog: &KnowledgeCatalog,
    target_id: &str,
) -> Result<CandidateTarget, String> {
    let target = catalog
        .ecu_targets()
        .iter()
        .find(|target| target.id() == target_id)
        .ok_or_else(|| format!("unknown pinned Knowledge ECU target: {target_id}"))?;
    CandidateTarget::from_catalog(target, catalog.pin().repository())
}

pub(super) fn replay_read(recorded: &Value) -> Result<Value, String> {
    let catalog = KnowledgeCatalog::embedded_pinned().map_err(|error| error.to_string())?;
    require_revision(recorded, &catalog)?;
    let semantic = recorded
        .get("semantic")
        .and_then(Value::as_str)
        .ok_or("recording semantic missing")?;
    let definition_id = recorded
        .get("definition_id")
        .and_then(Value::as_str)
        .ok_or("Knowledge recording definition_id missing")?;
    let target_id = recorded
        .get("target_id")
        .and_then(Value::as_str)
        .ok_or("Knowledge recording target_id missing")?;
    let definition = catalog
        .definition(definition_id)
        .filter(|definition| definition.semantic() == semantic)
        .ok_or("recorded Knowledge definition does not match semantic")?;
    validate_applicability(&catalog, target_id, definition)?;
    let target = target_from_catalog(&catalog, target_id)?;
    if definition.requires_extended_session() {
        let uds = recorded
            .get("uds")
            .ok_or("session-required Knowledge replay is missing read-time UDS state")?;
        if uds.get("state").and_then(Value::as_str) != Some("active")
            || uds.get("active").and_then(Value::as_bool) != Some(true)
            || uds.get("target").and_then(Value::as_str) != Some(target.header().as_str())
            || uds.get("responder").and_then(Value::as_str)
                != Some(target.response_header().as_str())
            || !uds
                .get("signals")
                .and_then(Value::as_array)
                .is_some_and(|signals| {
                    signals
                        .iter()
                        .any(|id| id.as_str() == Some(definition.id()))
                })
        {
            return Err(
                "session-required Knowledge replay was not read in its authorized ECU session"
                    .into(),
            );
        }
    }
    let response = recorded_evidence_payload(recorded, &target, definition)?;
    let mut result = decode_read_result(
        &catalog,
        target_id,
        definition,
        response.as_deref(),
        recorded.get("error").and_then(Value::as_str),
        true,
    );
    for field in [
        "observations",
        "uds",
        "uds_state_after_read",
        "uds_session_observations",
        "timestamp_ms",
    ] {
        if let Some(value) = recorded.get(field) {
            result[field] = value.clone();
        }
    }
    Ok(result)
}

pub(super) fn replay_learning(recorded: &Value) -> Result<Value, String> {
    let catalog = KnowledgeCatalog::embedded_pinned().map_err(|error| error.to_string())?;
    require_revision(recorded, &catalog)?;
    let results = recorded
        .get("results")
        .and_then(Value::as_array)
        .ok_or("learning recording results missing")?;
    let mut decoded = Vec::with_capacity(results.len());
    for result in results {
        require_revision(result, &catalog)?;
        let semantic = result
            .get("semantic")
            .and_then(Value::as_str)
            .ok_or("learning result semantic missing")?;
        let definition_id = result
            .get("definition_id")
            .and_then(Value::as_str)
            .ok_or("learning result definition_id missing")?;
        let target_id = result
            .get("target_id")
            .and_then(Value::as_str)
            .ok_or("learning result target_id missing")?;
        let definition = catalog
            .definition(definition_id)
            .filter(|definition| definition.semantic() == semantic)
            .ok_or("learning definition does not match semantic")?;
        if !definition.requires_extended_session() {
            return Err("learning accepts only session-flagged Knowledge definitions".into());
        }
        validate_applicability(&catalog, target_id, definition)?;
        let target = target_from_catalog(&catalog, target_id)?;
        let response = recorded_evidence_payload(result, &target, definition)?;
        decoded.push(decode_learning_result(
            &catalog,
            target_id,
            definition,
            response.as_deref(),
            result.get("error").and_then(Value::as_str),
        ));
    }
    Ok(json!({
        "knowledge_repository":catalog.pin().repository(),
        "knowledge_revision":catalog.pin().revision(),
        "results":decoded,
        "replay":true
    }))
}

pub(super) fn replay_discovery(recorded: &Value) -> Result<Value, String> {
    let catalog = KnowledgeCatalog::embedded_pinned().map_err(|error| error.to_string())?;
    require_revision(recorded, &catalog)?;
    let recorded_targets = recorded
        .get("targets")
        .and_then(Value::as_array)
        .ok_or("discovery recording targets missing")?;
    let plan = EcuIdentificationPlan::from_catalog(&catalog)?;
    let mut states = Vec::with_capacity(recorded_targets.len());
    let mut target_ids = std::collections::BTreeSet::new();
    for recorded_target in recorded_targets {
        let target_id = recorded_target
            .get("target_id")
            .and_then(Value::as_str)
            .ok_or("discovery result target_id missing")?;
        if !target_ids.insert(target_id) {
            return Err(format!("duplicate recorded ECU target: {target_id}"));
        }
        let candidate_target = target_from_catalog(&catalog, target_id)?;
        let input = recorded_target
            .get("observations")
            .and_then(Value::as_array)
            .ok_or("discovery result observations missing")?;
        let mut state = EcuState {
            candidate: candidate_target.clone(),
            present: false,
            identity: Vec::with_capacity(input.len()),
        };
        let mut definition_ids = std::collections::BTreeSet::new();
        for item in input {
            let definition_id = item
                .get("definition_id")
                .and_then(Value::as_str)
                .ok_or("discovery observation definition_id missing")?;
            if !definition_ids.insert(definition_id) {
                return Err(format!(
                    "duplicate ECU identification response for {definition_id}"
                ));
            }
            let request = item
                .get("request")
                .cloned()
                .ok_or("discovery observation request missing")?;
            let candidate = plan
                .candidates()
                .iter()
                .find(|candidate| candidate.definition_id() == definition_id)
                .ok_or("discovery observation is not a canonical identification candidate")?;
            if serde_json::from_value::<Vec<u8>>(request).map_err(|error| error.to_string())?
                != candidate.request_bytes()
            {
                return Err("discovery observation request does not match its definition".into());
            }
            let definition = catalog
                .definition(definition_id)
                .ok_or("discovery definition missing from pinned Knowledge")?;
            if item.get("semantic").and_then(Value::as_str) != Some(definition.semantic()) {
                return Err("discovery observation semantic does not match its definition".into());
            }
            let responses = item
                .get("responses")
                .and_then(Value::as_array)
                .ok_or("discovery observation responses missing")?;
            let replay = classify_replay_response(definition, &candidate_target, responses)?;
            let observation = IdentificationObservation::new(
                candidate_target.target.clone(),
                candidate_target.responder.clone(),
                candidate.semantic(),
                candidate.definition_id(),
                candidate.definition_version(),
                catalog.pin().repository(),
                catalog.pin().revision(),
                candidate.request_bytes(),
                replay.status,
                replay
                    .responses
                    .iter()
                    .map(|response| {
                        let responder =
                            response
                                .get("responder")
                                .and_then(Value::as_str)
                                .map(|value| {
                                    ResponderIdentity::address(candidate_target.context(), value)
                                });
                        let payload = response
                            .get("payload")
                            .cloned()
                            .map(|payload| {
                                serde_json::from_value::<Vec<u8>>(payload)
                                    .map_err(|error| error.to_string())
                            })
                            .transpose()?
                            .ok_or("discovery response payload missing")?;
                        Ok(IdentificationResponseEvidence::new(responder, payload))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                replay.nrc,
                replay.value,
                replay.error.into_iter().collect(),
            )?;
            state.present |= observation_marks_presence(&candidate_target, &observation);
            state.identity.push(observation);
        }
        states.push(state);
    }

    let manufacturer = recorded
        .get("vehicle_manufacturer")
        .and_then(Value::as_str)
        .or_else(|| {
            states
                .first()
                .map(|state| state.candidate.manufacturer.as_str())
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    let targets = states.iter().map(target_json).collect::<Vec<_>>();
    let signals = if manufacturer.is_empty() {
        Vec::new()
    } else {
        let mut knowledge = MobileKnowledge::new(catalog.clone(), &manufacturer)?;
        for state in states {
            if state.candidate.manufacturer != manufacturer {
                return Err("recorded ECU target does not match vehicle manufacturer".into());
            }
            if let Some(slot) = knowledge
                .targets
                .iter_mut()
                .find(|candidate| candidate.candidate.id == state.candidate.id)
            {
                *slot = state;
            }
        }
        knowledge.signal_catalog(|_, definition| !definition.requires_extended_session())?
    };
    Ok(json!({
        "knowledge_repository":catalog.pin().repository(),
        "knowledge_revision":catalog.pin().revision(),
        "vehicle_manufacturer":manufacturer,
        "targets":targets,
        "signals":signals,
        "replay":true
    }))
}

pub(super) fn decode_read_result(
    catalog: &KnowledgeCatalog,
    target_id: &str,
    definition: &KnowledgeDefinition,
    response: Option<&[u8]>,
    error: Option<&str>,
    replay: bool,
) -> Value {
    let parsed = decode_definition(definition, response);
    let (status, nrc, value, decode_error) = match parsed {
        Ok(Some(value)) => ("available", None, value, None),
        Ok(None) => ("unavailable", None, Value::Null, None),
        Err(DecodeFailure::NegativeResponse(nrc)) => (
            "unavailable",
            Some(nrc),
            Value::Null,
            Some("negative response".to_owned()),
        ),
        Err(DecodeFailure::Invalid(error)) => ("unavailable", None, Value::Null, Some(error)),
    };
    let mut result = json!({
        "knowledge_repository":catalog.pin().repository(),
        "knowledge_revision":catalog.pin().revision(),
        "target_id":target_id,
        "semantic":definition.semantic(),
        "definition_id":definition.id(),
        "did":definition.operation().did(),
        "request":[0x22,(definition.operation().did()>>8) as u8,definition.operation().did() as u8],
        "response":response,
        "status":status,
        "nrc":nrc,
        "unit":definition.unit().unwrap_or(""),
        "validation":hardware_validation(definition),
        "validation_label":validation_label(hardware_validation(definition)),
        "replay":replay
    });
    if status == "available" {
        result["value"] = value;
    } else {
        result["error"] = json!(decode_error
            .or_else(|| error.map(str::to_owned))
            .unwrap_or_else(|| "Knowledge DID unavailable".into()));
    }
    result
}

pub(super) fn decode_learning_result(
    catalog: &KnowledgeCatalog,
    target_id: &str,
    definition: &KnowledgeDefinition,
    response: Option<&[u8]>,
    error: Option<&str>,
) -> Value {
    let parsed = decode_definition(definition, response);
    let (status, nrc, value, error) = match parsed {
        Ok(Some(value)) => ("answered", None, value, None),
        Ok(None) => ("not_answered", None, Value::Null, error.map(str::to_owned)),
        Err(DecodeFailure::NegativeResponse(nrc)) => ("rejected", Some(nrc), Value::Null, None),
        Err(DecodeFailure::Invalid(error)) => ("not_answered", None, Value::Null, Some(error)),
    };
    let mut result = json!({
        "knowledge_repository":catalog.pin().repository(),
        "knowledge_revision":catalog.pin().revision(),
        "target_id":target_id,
        "semantic":definition.semantic(),
        "definition_id":definition.id(),
        "did":definition.operation().did(),
        "request":[0x22,(definition.operation().did()>>8) as u8,definition.operation().did() as u8],
        "response":response,
        "status":status,
        "nrc":nrc,
        "unit":definition.unit().unwrap_or(""),
        "validation":"unconfirmed",
        "validation_state":hardware_validation(definition),
        "validation_label":validation_label(hardware_validation(definition)),
        "observations":[]
    });
    if status == "answered" {
        result["value"] = value;
    } else if let Some(error) = error {
        result["error"] = json!(error);
    }
    result
}

enum DecodeFailure {
    NegativeResponse(u8),
    Invalid(String),
}

fn decode_definition(
    definition: &KnowledgeDefinition,
    response: Option<&[u8]>,
) -> Result<Option<Value>, DecodeFailure> {
    let Some(response) = response else {
        return Ok(None);
    };
    if response.starts_with(&[0x7f, 0x22]) {
        return if response.len() == 3 {
            Err(DecodeFailure::NegativeResponse(response[2]))
        } else {
            Err(DecodeFailure::Invalid(
                "malformed UDS negative response".into(),
            ))
        };
    }
    let payload = definition
        .validate_response(response)
        .map_err(DecodeFailure::Invalid)?;
    decode_value(definition.decoder(), payload)
        .map(Some)
        .map_err(DecodeFailure::Invalid)
}

fn decode_value(decoder: &KnowledgeDecoder, payload: &[u8]) -> Result<Value, String> {
    match decoder {
        KnowledgeDecoder::OpaqueBytes => Ok(json!(payload)),
        KnowledgeDecoder::Ascii { trim } => {
            if !payload.is_ascii() {
                return Err("Knowledge ASCII decoder received non-ASCII bytes".into());
            }
            let trim_byte = |byte: u8| match trim {
                AsciiTrim::None => false,
                AsciiTrim::Space => byte == b' ',
                AsciiTrim::Nul => byte == 0,
                AsciiTrim::SpaceAndNul => matches!(byte, 0 | b' '),
            };
            let end = payload
                .iter()
                .rposition(|byte| !trim_byte(*byte))
                .map_or(0, |index| index + 1);
            let payload = &payload[..end];
            if payload.is_empty() || payload.iter().any(u8::is_ascii_control) {
                return Err("Knowledge ASCII decoder received empty or control bytes".into());
            }
            Ok(json!(
                std::str::from_utf8(payload).map_err(|error| error.to_string())?
            ))
        }
        KnowledgeDecoder::LinearInteger {
            width_bytes,
            endian,
            signed,
            scale,
            offset,
        } => {
            if payload.len() != usize::from(*width_bytes) {
                return Err(format!(
                    "Knowledge linear decoder expected {width_bytes} bytes, received {}",
                    payload.len()
                ));
            }
            let mut raw = 0u64;
            match endian {
                Endian::Big => {
                    for byte in payload {
                        raw = (raw << 8) | u64::from(*byte);
                    }
                }
                Endian::Little => {
                    for (shift, byte) in payload.iter().enumerate() {
                        raw |= u64::from(*byte) << (shift * 8);
                    }
                }
            }
            let bit_count = u32::from(*width_bytes) * 8;
            let integer = if *signed && raw & (1u64 << (bit_count - 1)) != 0 {
                (raw as i128) - (1i128 << bit_count)
            } else {
                raw as i128
            };
            let value = (integer as f64) * scale + offset;
            if !value.is_finite() {
                return Err("Knowledge linear decoder produced a non-finite value".into());
            }
            Ok(json!(value))
        }
    }
}

fn hardware_validation(definition: &KnowledgeDefinition) -> &'static str {
    match definition.hardware_validation().status() {
        HardwareValidationStatus::NotValidated => "not_validated",
        HardwareValidationStatus::Validated => "validated",
        HardwareValidationStatus::NotApplicable => "not_applicable",
    }
}

fn validation_label(validation: &str) -> &'static str {
    match validation {
        "not_validated" => "nicht abgeglichen",
        "validated" => "validiert",
        "not_applicable" => "nicht anwendbar",
        _ => "unbekannt",
    }
}

fn identification_status(status: IdentificationResultStatus) -> &'static str {
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

fn recorded_evidence_payload(
    item: &Value,
    target: &CandidateTarget,
    definition: &KnowledgeDefinition,
) -> Result<Option<Vec<u8>>, String> {
    let request = item
        .get("request")
        .cloned()
        .ok_or("Knowledge recording request missing")?;
    if serde_json::from_value::<Vec<u8>>(request).map_err(|error| error.to_string())?
        != definition.operation().request_bytes()
    {
        return Err("recorded Knowledge request does not match its definition".into());
    }
    let observations = item
        .get("observations")
        .and_then(Value::as_array)
        .ok_or("Knowledge recording observations missing")?;
    let expected = target.response_header();
    let mut payloads = Vec::new();
    for observation in observations {
        let responses = observation
            .get("responses")
            .and_then(Value::as_array)
            .ok_or("Knowledge response observations missing")?;
        for response in responses {
            if response
                .get("responder")
                .and_then(Value::as_str)
                .is_some_and(|responder| responder.eq_ignore_ascii_case(&expected))
            {
                let payload = response
                    .get("payload")
                    .cloned()
                    .ok_or("Knowledge response payload missing")?;
                payloads.push(
                    serde_json::from_value::<Vec<u8>>(payload)
                        .map_err(|error| error.to_string())?,
                );
            }
        }
    }
    payloads.sort();
    payloads.dedup();
    if payloads.len() > 1 {
        return Err("conflicting Knowledge responses from the expected ECU".into());
    }
    let evidence = payloads.pop();
    let recorded = item
        .get("response")
        .filter(|response| !response.is_null())
        .cloned()
        .map(|response| {
            serde_json::from_value::<Vec<u8>>(response).map_err(|error| error.to_string())
        })
        .transpose()?;
    if recorded != evidence {
        return Err("recorded Knowledge response does not match normalized ECU evidence".into());
    }
    Ok(evidence)
}

fn require_revision(recorded: &Value, catalog: &KnowledgeCatalog) -> Result<(), String> {
    let revision = recorded
        .get("knowledge_revision")
        .and_then(Value::as_str)
        .ok_or("recording knowledge_revision missing")?;
    if revision != catalog.pin().revision() {
        return Err("recording Knowledge revision does not match the embedded pin".into());
    }
    let repository = recorded
        .get("knowledge_repository")
        .and_then(Value::as_str)
        .ok_or("recording knowledge_repository missing")?;
    if repository != catalog.pin().repository() {
        return Err("recording Knowledge repository does not match the embedded pin".into());
    }
    Ok(())
}

fn validate_applicability(
    catalog: &KnowledgeCatalog,
    target_id: &str,
    definition: &KnowledgeDefinition,
) -> Result<(), String> {
    let target = target_from_catalog(catalog, target_id)?;
    let effective =
        EffectiveVehicleKnowledge::resolve(catalog, [target_facts_from_candidate(&target)?])?;
    let selected = effective
        .ecu(target_id)
        .and_then(|ecu| ecu.semantic(definition.semantic()))
        .and_then(|resolution| resolution.selected_definition_id());
    if selected != Some(definition.id()) {
        return Err("recorded Knowledge definition is not applicable to its ECU target".into());
    }
    Ok(())
}

fn target_facts_from_candidate(target: &CandidateTarget) -> Result<ObservedEcuFacts, String> {
    ObservedEcuFacts::new(target.id.clone())?
        .with_fact(
            crate::knowledge_db::FingerprintField::VehicleManufacturer,
            target.manufacturer.clone(),
        )?
        .with_fact(
            crate::knowledge_db::FingerprintField::EcuLogicalRole,
            target.role.clone(),
        )
}

struct ReplayIdentification {
    status: IdentificationResultStatus,
    nrc: Option<u8>,
    value: Option<Vec<u8>>,
    error: Option<String>,
    responses: Vec<Value>,
}

fn classify_replay_response(
    definition: &KnowledgeDefinition,
    target: &CandidateTarget,
    responses: &[Value],
) -> Result<ReplayIdentification, String> {
    let expected = target.response_header();
    let matching = responses
        .iter()
        .filter(|response| {
            response
                .get("responder")
                .and_then(Value::as_str)
                .is_some_and(|responder| responder.eq_ignore_ascii_case(&expected))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut payloads = matching
        .iter()
        .map(|response| {
            response
                .get("payload")
                .cloned()
                .ok_or_else(|| "discovery response payload missing".to_owned())
                .and_then(|payload| {
                    serde_json::from_value::<Vec<u8>>(payload).map_err(|error| error.to_string())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    payloads.sort();
    payloads.dedup();
    if payloads.len() > 1 {
        return Err("conflicting discovery responses from the expected ECU".into());
    }
    let payload = payloads.into_iter().next();
    let Some(payload) = payload else {
        return Ok(ReplayIdentification {
            status: IdentificationResultStatus::Unavailable,
            nrc: None,
            value: None,
            error: Some("no response observed".into()),
            responses: responses.to_vec(),
        });
    };
    if payload.starts_with(&[0x7f, 0x22]) && payload.len() == 3 {
        return Ok(ReplayIdentification {
            status: identification_status_for_nrc(payload[2]),
            nrc: Some(payload[2]),
            value: None,
            error: None,
            responses: matching,
        });
    }
    match definition.validate_response(&payload) {
        Ok(value) => Ok(ReplayIdentification {
            status: IdentificationResultStatus::Supported,
            nrc: None,
            value: Some(value.to_vec()),
            error: None,
            responses: matching,
        }),
        Err(error) => Ok(ReplayIdentification {
            status: IdentificationResultStatus::Malformed,
            nrc: None,
            value: None,
            error: Some(error),
            responses: matching,
        }),
    }
}

fn identification_status_for_nrc(nrc: u8) -> IdentificationResultStatus {
    match nrc {
        0x11 | 0x31 => IdentificationResultStatus::Unsupported,
        0x22 | 0x33 | 0x7e | 0x7f => IdentificationResultStatus::Unavailable,
        _ => IdentificationResultStatus::NegativeResponse,
    }
}

pub(super) fn read_evidence_json(result: &Result<ReadEvidence, ReadEvidenceError>) -> Vec<Value> {
    let observations = match result {
        Ok(read) => &read.observations,
        Err(error) => &error.observations,
    };
    observations
        .iter()
        .map(|observation| {
            json!({
                "selected_responder":observation.selected_responder,
                "selection_error":observation.selection_error,
                "responses":observation.responses.iter().map(|response|json!({
                    "responder":response.responder,
                    "payload":response.payload
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

pub(super) fn learning_result(
    catalog: &KnowledgeCatalog,
    resolved: &ResolvedDid,
    result: Result<ReadEvidence, ReadEvidenceError>,
) -> Value {
    let response = result.as_ref().ok().map(|read| read.payload.as_slice());
    let error = result.as_ref().err().map(|error| error.error.as_str());
    let mut decoded = decode_learning_result(
        catalog,
        &resolved.target_id,
        &resolved.definition,
        response,
        error,
    );
    decoded["request"] = json!([
        0x22,
        (resolved.definition.operation().did() >> 8) as u8,
        resolved.definition.operation().did() as u8
    ]);
    decoded["observations"] = json!(read_evidence_json(&result));
    decoded
}

pub(super) fn read_result(
    catalog: &KnowledgeCatalog,
    resolved: &ResolvedDid,
    result: Result<ReadEvidence, ReadEvidenceError>,
) -> Value {
    let response = result.as_ref().ok().map(|read| read.payload.as_slice());
    let error = result.as_ref().err().map(|error| error.error.as_str());
    let mut decoded = decode_read_result(
        catalog,
        &resolved.target_id,
        &resolved.definition,
        response,
        error,
        false,
    );
    decoded["observations"] = json!(read_evidence_json(&result));
    decoded["timestamp_ms"] = json!(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis()));
    decoded
}

pub(super) fn definition_read_request(
    resolved: &ResolvedDid,
) -> (
    KnowledgeDefinition,
    RequestTarget,
    crate::elm::ResponderIdentity,
) {
    (
        resolved.definition.clone(),
        resolved.candidate.target.clone(),
        resolved.candidate.elm_responder(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> KnowledgeCatalog {
        KnowledgeCatalog::embedded_pinned().unwrap()
    }

    #[test]
    fn req_024_2_knowledge_decoder_handles_signed_scaled_and_ascii_values() {
        assert_eq!(
            decode_value(
                &KnowledgeDecoder::LinearInteger {
                    width_bytes: 2,
                    endian: Endian::Big,
                    signed: true,
                    scale: 0.01,
                    offset: 0.0,
                },
                &[0xff, 0x9c],
            )
            .unwrap(),
            json!(-1.0)
        );
        assert_eq!(
            decode_value(
                &KnowledgeDecoder::Ascii {
                    trim: AsciiTrim::SpaceAndNul
                },
                b"part  \0",
            )
            .unwrap(),
            json!("part")
        );
        assert!(decode_value(
            &KnowledgeDecoder::LinearInteger {
                width_bytes: 2,
                endian: Endian::Big,
                signed: false,
                scale: 1.0,
                offset: 0.0,
            },
            &[0x01],
        )
        .is_err());
    }

    #[test]
    fn req_024_1_effective_knowledge_selects_only_matching_default_definitions() {
        let catalog = catalog();
        let target = target_from_catalog(&catalog, "vag.ecu_target.7e0").unwrap();
        let effective = EffectiveVehicleKnowledge::resolve(
            &catalog,
            [target_facts_from_candidate(&target).unwrap()],
        )
        .unwrap();
        let ecu = effective.ecu("vag.ecu_target.7e0").unwrap();
        let normal = ecu.semantic("air.charge_limit_overcharge").unwrap();
        assert_eq!(
            normal.selected_definition_id(),
            Some("vag.ea189.air.charge_limit_overcharge")
        );
        let flagged = ecu.semantic("air.intercooler_outlet_temperature").unwrap();
        assert_eq!(
            flagged.selected_definition_id(),
            Some("vag.ea189_session.air.intercooler_outlet_temperature")
        );
        assert!(!catalog
            .definition(normal.selected_definition_id().unwrap())
            .unwrap()
            .requires_extended_session());
        assert!(catalog
            .definition(flagged.selected_definition_id().unwrap())
            .unwrap()
            .requires_extended_session());
        let raw_status = catalog.semantic("dpf.service_regeneration_status").unwrap();
        assert!(raw_status.unit().is_none());
        assert_eq!(signal_json(&target, raw_status)["unit"], "");
    }

    #[test]
    fn req_026_2_knowledge_results_preserve_nrc_and_timeout_as_not_answered() {
        let catalog = catalog();
        let definition = catalog
            .definition("vag.ea189_session.air.intercooler_outlet_temperature")
            .unwrap();
        let negative = decode_learning_result(
            &catalog,
            "vag.ecu_target.7e0",
            definition,
            Some(&[0x7f, 0x22, 0x31]),
            None,
        );
        assert_eq!(negative["status"], "rejected");
        assert_eq!(negative["nrc"], 0x31);
        assert!(negative["value"].is_null());
        let missing = decode_learning_result(
            &catalog,
            "vag.ecu_target.7e0",
            definition,
            None,
            Some("timeout"),
        );
        assert_eq!(missing["status"], "not_answered");
        assert!(missing["value"].is_null());
    }
}
