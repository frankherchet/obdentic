//! Closed mobile facade. Platform code supplies transport, never diagnostic commands.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::{
    elm::{
        ElmExchange, ElmSession, ExchangeError, TargetedDpfProbeRequest,
        TargetedEa189AshProbeRequest, TargetedEa189ExtendedProbeRequest, TargetedReadRequest,
        TransportFailure,
    },
    topology::{
        AddressingContext, Confidence, EcuRole, Protocol, ProtocolContext, Provenance,
        RequestAddress, RequestTarget, RequestTargetEvidence, ResponderIdentity, RoleAssignment,
    },
    vehicle_knowledge::EcuTargetMapping,
};

/// Implemented by the platform's synchronous BLE transport on its worker thread.
pub trait Exchange: Send {
    fn exchange(&mut self, command: &str, timeout: Duration) -> Result<String, String>;
}

struct PlatformExchange<E>(E);
impl<E: Exchange> ElmExchange for PlatformExchange<E> {
    async fn exchange(
        &mut self,
        command: &str,
        timeout: Duration,
    ) -> Result<String, ExchangeError> {
        self.0
            .exchange(command, timeout)
            .map_err(|error| TransportFailure::WriteFailed(error).into())
    }
}

/// One persistent, serialized, read-only diagnostic session.
pub struct MobileCore<E> {
    runtime: tokio::runtime::Runtime,
    session: Option<ElmSession<PlatformExchange<E>>>,
    initialized: bool,
    engine: Option<EcuTargetMapping>,
}

impl<E: Exchange> MobileCore<E> {
    pub fn new(exchange: E) -> Result<Self, String> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|error| error.to_string())?,
            session: Some(ElmSession::new(PlatformExchange(exchange))),
            initialized: false,
            engine: None,
        })
    }

    /// No caller-supplied wire bytes, addresses or write capabilities are accepted.
    pub fn execute(&mut self, action: &str, argument: &str) -> String {
        let result = self.dispatch(action, argument);
        match result {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error}),
        }
        .to_string()
    }

    fn dispatch(&mut self, action: &str, argument: &str) -> Result<Value, String> {
        if action == "catalog" {
            return Ok(catalog());
        }
        if action == "vin.describe" {
            return Ok(crate::vin_identity::describe(argument));
        }
        if action == "replay.dtc.scan" {
            return replay_dtc_scan(argument);
        }
        if action == "replay.ea189.dpf.probe" {
            return replay_dpf_probe(argument);
        }
        if action == "replay.ea189.extended.probe" {
            return replay_ea189_extended_probe(argument);
        }
        if action == "replay.ea189.ash.probe" {
            return replay_ea189_ash_probe(argument);
        }
        if action == "replay.read" {
            let mut recorded: Value = serde_json::from_str(argument).map_err(|e| e.to_string())?;
            let semantic = recorded
                .get("semantic")
                .and_then(Value::as_str)
                .ok_or("recording semantic missing")?;
            let request = crate::prepare_read(semantic)?;
            let response: Vec<u8> = serde_json::from_value(
                recorded
                    .get("response")
                    .cloned()
                    .ok_or("recording response missing")?,
            )
            .map_err(|e| e.to_string())?;
            let transaction = request.complete("replay", response)?;
            recorded["value"] = json!(transaction.value());
            recorded["unit"] = json!(transaction.unit());
            recorded["replay"] = json!(true);
            return Ok(recorded);
        }
        if !matches!(
            action,
            "initialize"
                | "inventory"
                | "read"
                | "dtc.scan"
                | "ea189.dpf.probe"
                | "ea189.extended.probe"
                | "ea189.ash.probe"
        ) {
            return Err(format!("read-only mobile core rejected action: {action}"));
        }
        if action != "read" && !argument.is_empty() {
            return Err("this closed action accepts no argument".into());
        }
        if action == "initialize" {
            self.initialized = false;
            self.engine = None;
            let mut exchange = self
                .session
                .take()
                .ok_or("session unavailable")?
                .into_exchange();
            let setup = self
                .runtime
                .block_on(crate::adapter::initialize_carly(&mut exchange));
            self.session = Some(ElmSession::new(exchange));
            setup?;
            self.runtime
                .block_on(self.session.as_mut().unwrap().discover_support(0x60))?;
            self.initialized = true;
            return Ok(self.support());
        }
        if !self.initialized {
            return Err("initialize the adapter first".into());
        }
        match action {
            "read" => {
                let request = crate::prepare_read(argument)?;
                let read = if let Some(mapping) = &self.engine {
                    if !self
                        .session
                        .as_ref()
                        .unwrap()
                        .supported()
                        .supports_pid(request.pid())
                    {
                        return Err(format!("vehicle does not advertise support for {argument}"));
                    }
                    let targeted = TargetedReadRequest::new(
                        request,
                        mapping.target().target().clone(),
                        crate::elm::ResponderIdentity::ElmHeader(
                            mapping
                                .expected_responder()
                                .value()
                                .ok_or("validated engine responder missing")?
                                .into(),
                        ),
                    )?;
                    self.runtime.block_on(
                        self.session
                            .as_mut()
                            .unwrap()
                            .read_targeted_with_evidence(&targeted),
                    )
                } else {
                    self.runtime
                        .block_on(self.session.as_mut().unwrap().read_with_evidence(request))
                }
                .map_err(|error| error.error)?;
                let transaction = request.complete("user", read.payload)?;
                let observations = response_observations_json(&read.observations);
                Ok(
                    json!({"semantic":transaction.semantic(),"value":transaction.value(),
                    "unit":transaction.unit(),"timestamp_ms":transaction.timestamp_ms(),
                    "response":transaction.response(),"observations":observations}),
                )
            }
            "inventory" => self.inventory(),
            "dtc.scan" => {
                let responses = self
                    .runtime
                    .block_on(self.session.as_mut().unwrap().read_stored_dtcs())?;
                let observations: Vec<_> = responses.as_slice().iter().map(|response| {
                    match crate::dtc::decode_mode03_payload(&response.payload) {
                        Ok(decoded) => json!({"responder":response.responder.as_ref().map(|r|r.as_str()),
                            "codes":decoded.dtcs().iter().map(ToString::to_string).collect::<Vec<_>>(),
                            "no_dtcs":decoded.is_no_dtcs()}),
                        Err(error) => json!({"responder":response.responder.as_ref().map(|r|r.as_str()),"error":error.to_string()}),
                    }
                }).collect();
                Ok(
                    json!({"observations":observations,"raw_response":responses.raw_response(),
                    "responses":diagnostic_evidence(&responses),
                    "errors":diagnostic_errors(&responses)}),
                )
            }
            "ea189.dpf.probe" => self.dpf(),
            "ea189.extended.probe" => self.ea189_extended(),
            "ea189.ash.probe" => self.ea189_ash_probe(),
            _ => unreachable!(),
        }
    }

    fn support(&self) -> Value {
        let support = self.session.as_ref().unwrap().supported();
        json!({"signals":crate::supported_signals().iter().map(|signal|json!({
            "semantic":signal.metadata().semantic,
            "status":format!("{:?}",support.status(signal.metadata().request[1])).to_lowercase()
        })).collect::<Vec<_>>(),"responders":support.discovery.iter().filter_map(|d|d.responder.as_ref().map(|r|r.as_str())).collect::<std::collections::BTreeSet<_>>()})
    }

    fn inventory(&mut self) -> Result<Value, String> {
        // Do not retain target evidence from an earlier inventory if this
        // discovery no longer observes the expected engine responder.
        self.engine = None;
        let identity = self
            .runtime
            .block_on(self.session.as_mut().unwrap().identify());
        let mut data = self.support();
        match identity {
            Ok(identity) => {
                data["vin"] = json!(identity.vin().as_str());
                data["vin_identity"] = crate::vin_identity::describe(identity.vin().as_str());
            }
            Err(error) => data["identity_error"] = json!(error),
        }
        if self
            .session
            .as_ref()
            .unwrap()
            .supported()
            .discovery
            .iter()
            .any(|d| {
                d.responder
                    .as_ref()
                    .is_some_and(|r| r.as_str().eq_ignore_ascii_case("7E8"))
            })
        {
            match self.validate_engine(&mut data) {
                Ok(mapping) => {
                    self.engine = Some(mapping);
                    data["engine_target_validated"] = json!(true);
                }
                Err(error) => {
                    self.engine = None;
                    data["engine_target_error"] = json!(error);
                }
            }
        }
        Ok(data)
    }

    fn validate_engine(&mut self, data: &mut Value) -> Result<EcuTargetMapping, String> {
        // Same closed validation rule as observed_inventory: observed 7E8 + targeted RPM.
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let target =
            RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0"));
        let request = crate::prepare_read("engine.rpm")?;
        let targeted = TargetedReadRequest::new(
            request,
            target.clone(),
            crate::elm::ResponderIdentity::ElmHeader("7E8".into()),
        )?;
        data["engine_validation_request"] = json!({
            "semantic":"engine.rpm",
            "request_payload":request.bytes(),
            "target":"7E0",
            "expected_responder":"7E8"
        });
        let read = match self.runtime.block_on(
            self.session
                .as_mut()
                .unwrap()
                .read_targeted_with_evidence(&targeted),
        ) {
            Ok(read) => {
                data["engine_validation_observations"] =
                    json!(response_observations_json(&read.observations));
                read
            }
            Err(error) => {
                data["engine_validation_observations"] =
                    json!(response_observations_json(&error.observations));
                return Err(error.error);
            }
        };
        request.complete("user", read.payload)?;
        let provenance = Provenance::new(
            "targeted engine.rpm Mode 01 validation",
            Confidence::Verified,
        )
        .map_err(|e| e.to_string())?;
        Ok(EcuTargetMapping::new(
            RoleAssignment::new(EcuRole::Engine, provenance.clone()),
            RequestTargetEvidence::new(target, provenance),
            ResponderIdentity::address(context, "7E8"),
        ))
    }

    fn dpf(&mut self) -> Result<Value, String> {
        let mapping = self
            .engine
            .as_ref()
            .ok_or("DPF requires an inventory with validated engine target evidence")?;
        let mut readings = Vec::new();
        for probe in crate::ea189::Ea189DpfProbe::ALL {
            let request = TargetedDpfProbeRequest::from_mapping(probe, mapping)?;
            let outcome = self.runtime.block_on(
                self.session
                    .as_mut()
                    .unwrap()
                    .read_dpf_probe_with_evidence(&request),
            );
            readings.push(match outcome {
                Ok(read) => {
                    let matching: Vec<_> = read.responses.as_slice().iter().filter(|r|r.responder.as_ref().is_some_and(|r|r.as_str().eq_ignore_ascii_case("7E8"))).collect();
                    let payload = match matching.first() {
                        None => Err("expected responder 7E8 absent".to_string()),
                        Some(first) if matching.iter().any(|r|r.payload != first.payload) => Err("conflicting DPF responses from expected responder".into()),
                        Some(_) if !read.responses.errors().is_empty() => Err(format!("incomplete DPF response: {}",read.responses.errors()[0].error)),
                        Some(first) => Ok(first.payload.as_slice()),
                    };
                    let mut common = json!({
                        "semantic":probe.semantic(),
                        "experimental":true,
                        "raw_response":read.responses.raw_response(),
                        "responses":diagnostic_evidence(&read.responses),
                        "response_errors":diagnostic_errors(&read.responses)
                    });
                    match payload.and_then(|payload|
                        crate::ea189::decode_experimental_dpf(probe,payload).map_err(|e|e.to_string())) {
                        Ok(reading) => {
                            common["value"] = json!(reading.value());
                            common["unit"] = json!(reading.unit());
                            common
                        }
                        Err(error) => {
                            common["error"] = json!(error);
                            common
                        }
                    }
                }
                Err(error) => json!({"semantic":probe.semantic(),"error":error.error,"experimental":true,
                    "raw_response":null,
                    "responses":error.observations.iter().flat_map(|observation| observation.responses.iter()).map(|response|json!({
                        "responder":response.responder,"payload":response.payload
                    })).collect::<Vec<_>>(),
                    "response_errors":[]}),
            });
        }
        // DPF uses physical addressing; restore with complete closed adapter setup.
        let mut exchange = self.session.take().unwrap().into_exchange();
        let restore = self
            .runtime
            .block_on(crate::adapter::initialize_carly(&mut exchange));
        self.session = Some(ElmSession::new(exchange));
        if let Err(error) = restore {
            self.initialized = false;
            return Err(format!("DPF restoration failed: {error}"));
        }
        if let Err(error) = self
            .runtime
            .block_on(self.session.as_mut().unwrap().discover_support(0x60))
        {
            self.initialized = false;
            return Err(error);
        }
        Ok(json!({"experimental":true,"readings":readings}))
    }

    fn ea189_extended(&mut self) -> Result<Value, String> {
        let mapping = self.engine.as_ref().ok_or(
            "EA189 extended probes require an inventory with validated engine target evidence",
        )?;
        let mut readings = Vec::with_capacity(crate::ea189::EA189_EXTENDED_PROBE_COUNT);
        for signal in crate::ea189::Ea189ExtendedSignal::ALL {
            let request = TargetedEa189ExtendedProbeRequest::from_mapping(signal, mapping)?;
            let outcome = self.runtime.block_on(
                self.session
                    .as_mut()
                    .unwrap()
                    .read_ea189_extended_probe_with_evidence(&request),
            );
            readings.push(match outcome {
                Ok(read) => {
                    let matching: Vec<_> = read.responses.as_slice().iter().filter(|response| {
                        response.responder.as_ref().is_some_and(|responder| {
                            responder.as_str().eq_ignore_ascii_case("7E8")
                        })
                    }).collect();
                    let payload = match matching.first() {
                        None => Err("expected responder 7E8 absent".to_string()),
                        Some(first) if matching.iter().any(|response| response.payload != first.payload) => {
                            Err("conflicting EA189 extended responses from expected responder".into())
                        }
                        Some(_) if !read.responses.errors().is_empty() => {
                            Err(format!("incomplete UDS response: {}", read.responses.errors()[0].error))
                        }
                        Some(first) => Ok(first.payload.as_slice()),
                    };
                    let mut result = extended_signal_metadata(signal);
                    result["raw_response"] = json!(read.responses.raw_response());
                    result["responses"] = diagnostic_evidence(&read.responses);
                    result["response_errors"] = diagnostic_errors(&read.responses);
                    match payload.and_then(|payload| {
                        crate::ea189::decode_experimental_extended(signal, payload)
                            .map_err(|error| error.to_string())
                    }) {
                        Ok(reading) => {
                            result["value"] = json!(reading.value());
                            result["unit"] = json!(reading.unit());
                            result
                        }
                        Err(error) => {
                            result["error"] = json!(error);
                            result
                        }
                    }
                }
                Err(error) => {
                    let mut result = extended_signal_metadata(signal);
                    result["error"] = json!(error.error);
                    result["raw_response"] = Value::Null;
                    result["responses"] = json!(error.observations.iter()
                        .flat_map(|observation| observation.responses.iter())
                        .map(|response| json!({"responder":response.responder,"payload":response.payload}))
                        .collect::<Vec<_>>());
                    result["response_errors"] = json!([]);
                    result
                }
            });
        }
        let mut exchange = self.session.take().unwrap().into_exchange();
        let restore = self
            .runtime
            .block_on(crate::adapter::initialize_carly(&mut exchange));
        self.session = Some(ElmSession::new(exchange));
        if let Err(error) = restore {
            self.initialized = false;
            return Err(format!("EA189 extended restoration failed: {error}"));
        }
        if let Err(error) = self
            .runtime
            .block_on(self.session.as_mut().unwrap().discover_support(0x60))
        {
            self.initialized = false;
            return Err(error);
        }
        Ok(json!({"experimental":true,"readings":readings}))
    }

    fn ea189_ash_probe(&mut self) -> Result<Value, String> {
        let mapping = self
            .engine
            .as_ref()
            .ok_or("EA189 ash probes require an inventory with validated engine target evidence")?;
        let mut readings = Vec::with_capacity(crate::ea189::EA189_ASH_CANDIDATE_PROBE_COUNT);
        for candidate in crate::ea189::Ea189AshCandidate::ALL {
            let request = TargetedEa189AshProbeRequest::from_mapping(candidate, mapping)?;
            let outcome = self.runtime.block_on(
                self.session
                    .as_mut()
                    .unwrap()
                    .read_ea189_ash_probe_with_evidence(&request),
            );
            let mut reading = match outcome {
                Ok(read) => ash_candidate_result(candidate, &read.responses),
                Err(error) => {
                    let mut result = ash_candidate_metadata(candidate);
                    result["error"] = json!(error.error);
                    result["raw_response"] = Value::Null;
                    result["responses"] = json!(error.observations.iter()
                        .flat_map(|observation| observation.responses.iter())
                        .map(|response| json!({"responder":response.responder,"payload":response.payload}))
                        .collect::<Vec<_>>());
                    result["response_errors"] = json!([]);
                    result["observations"] = json!(response_observations_json(&error.observations));
                    result
                }
            };
            reading["timestamp_ms"] = json!(wallclock_timestamp_ms());
            readings.push(reading);
        }

        // Restore Carly's normal functional setup after the bounded physical reads.
        let mut exchange = self.session.take().unwrap().into_exchange();
        let restore = self
            .runtime
            .block_on(crate::adapter::initialize_carly(&mut exchange));
        self.session = Some(ElmSession::new(exchange));
        if let Err(error) = restore {
            self.initialized = false;
            return Err(format!("EA189 ash probe restoration failed: {error}"));
        }
        if let Err(error) = self
            .runtime
            .block_on(self.session.as_mut().unwrap().discover_support(0x60))
        {
            self.initialized = false;
            return Err(error);
        }
        Ok(json!({"experimental":true,"readings":readings}))
    }
}

fn wallclock_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn ash_candidate_metadata(candidate: crate::ea189::Ea189AshCandidate) -> Value {
    let definition = candidate.definition();
    json!({
        "candidate":definition.candidate(),
        "did":format!("{:04X}", definition.did()),
        "request":crate::hex(&definition.request_bytes()),
        "name":definition.name(),
        "experimental":true,
        "target":"7E0",
        "expected_responder":"7E8",
        "source_version":crate::ea189::EA189_ASH_SOURCE_VERSION,
        "source_profile":crate::ea189::EA189_ASH_SOURCE_PROFILE,
        "hardware_validation":crate::ea189::EA189_ASH_HARDWARE_VALIDATION,
        "source_hypothesis":{
            "data_length":definition.data_len(),
            "endianness":"big",
            "signed":definition.signed(),
            "multiplier":definition.multiplier(),
            "offset":0.0,
            "unit":definition.unit()
        }
    })
}

fn ash_candidate_result(
    candidate: crate::ea189::Ea189AshCandidate,
    responses: &crate::elm::DiagnosticResponses,
) -> Value {
    let mut result = ash_candidate_metadata(candidate);
    result["raw_response"] = json!(responses.raw_response());
    result["responses"] = diagnostic_evidence(responses);
    result["response_errors"] = diagnostic_errors(responses);
    let matching: Vec<_> = responses
        .as_slice()
        .iter()
        .filter(|response| {
            response
                .responder
                .as_ref()
                .is_some_and(|responder| responder.as_str().eq_ignore_ascii_case("7E8"))
        })
        .collect();
    let payload = match matching.first() {
        None => Err("expected responder 7E8 absent".to_string()),
        Some(first)
            if matching
                .iter()
                .any(|response| response.payload != first.payload) =>
        {
            Err("conflicting ash-candidate responses from expected responder".into())
        }
        Some(_) if !responses.errors().is_empty() => Err(format!(
            "incomplete UDS response: {}",
            responses.errors()[0].error
        )),
        Some(first) => Ok(first.payload.as_slice()),
    };
    let selection_error = payload.as_ref().err().cloned();
    let target = crate::elm::ResponderIdentity::ElmHeader("7E8".into());
    result["observations"] = json!(response_observations_json(&[
        responses.observation_for_target(&target, selection_error),
    ]));

    match payload {
        Err(error) => result["error"] = json!(error),
        Ok(payload) if payload.starts_with(&[0x7f, 0x22]) && payload.len() == 3 => {
            result["outcome"] = json!("negative_response");
            result["nrc"] = json!(format!("0x{:02X}", payload[2]));
            result["error"] = json!(format!("negative UDS response (NRC 0x{:02X})", payload[2]));
        }
        Ok(payload) if payload.len() < 3 => {
            result["error"] = json!("UDS response is shorter than service and DID")
        }
        Ok(payload) if payload[0] != 0x62 => {
            result["error"] = json!(format!(
                "unexpected UDS response service 0x{:02X}",
                payload[0]
            ))
        }
        Ok(payload) => {
            let actual_did = u16::from_be_bytes([payload[1], payload[2]]);
            if actual_did != candidate.id() {
                result["actual_did"] = json!(format!("{:04X}", actual_did));
                result["error"] = json!(format!(
                    "UDS response DID {actual_did:04X} does not match requested DID {:04X}",
                    candidate.id()
                ));
            } else {
                let data = &payload[3..];
                result["response_data"] = json!(data);
                result["response_width_bytes"] = json!(data.len());
                let expected_width = candidate.definition().data_len();
                if data.len() != expected_width {
                    result["error"] = json!(format!(
                        "positive response width {} differs from the {}-byte source hypothesis",
                        data.len(),
                        expected_width
                    ));
                } else {
                    result["outcome"] = json!("positive_candidate");
                }
            }
        }
    }
    result
}

fn extended_signal_metadata(signal: crate::ea189::Ea189ExtendedSignal) -> Value {
    let definition = signal.definition();
    json!({
        "semantic":definition.semantic(),
        "did":format!("{:04X}", definition.did()),
        "request":crate::hex(&definition.request_bytes()),
        "name":definition.name(),
        "experimental":true,
        "target":"7E0",
        "expected_responder":"7E8",
        "source_version":crate::ea189::EA189_EXTENDED_SOURCE_VERSION,
        "source_profile":crate::ea189::EA189_EXTENDED_SOURCE_PROFILE,
        "hardware_validation":crate::ea189::EA189_EXTENDED_HARDWARE_VALIDATION,
        "source_definition":{
            "data_length":definition.data_len(),
            "signed":definition.signed(),
            "multiplier":definition.multiplier(),
            "offset":definition.offset(),
            "unit":definition.unit()
        }
    })
}

fn response_observations_json(observations: &[crate::elm::ResponseObservation]) -> Vec<Value> {
    observations
        .iter()
        .map(|observation| {
            json!({
                "selected_responder":observation.selected_responder,
                "selection_error":observation.selection_error,
                "responses":observation.responses.iter().map(|response| json!({
                    "responder":response.responder,"payload":response.payload
                })).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn diagnostic_evidence(responses: &crate::elm::DiagnosticResponses) -> Value {
    json!(responses
        .as_slice()
        .iter()
        .map(|response| json!({
            "responder":response.responder.as_ref().map(|r|r.as_str()),
            "payload":response.payload
        }))
        .collect::<Vec<_>>())
}

fn diagnostic_errors(responses: &crate::elm::DiagnosticResponses) -> Value {
    json!(responses
        .errors()
        .iter()
        .map(|error| json!({
            "responder":error.responder.as_ref().map(|r|r.as_str()),
            "error":error.error
        }))
        .collect::<Vec<_>>())
}

fn replay_dtc_scan(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let raw_response = recorded
        .get("raw_response")
        .and_then(Value::as_str)
        .filter(|raw| !raw.trim().is_empty())
        .ok_or("recording raw_response missing")?;
    let responses = crate::elm::normalize_mode03_responses(raw_response)?;
    let observations: Vec<_> = responses
        .as_slice()
        .iter()
        .map(
            |response| match crate::dtc::decode_mode03_payload(&response.payload) {
                Ok(decoded) => json!({
                    "responder":response.responder.as_ref().map(|r|r.as_str()),
                    "codes":decoded.dtcs().iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "no_dtcs":decoded.is_no_dtcs()
                }),
                Err(error) => json!({
                    "responder":response.responder.as_ref().map(|r|r.as_str()),
                    "error":error.to_string()
                }),
            },
        )
        .collect();

    Ok(json!({
        "observations":observations,
        "raw_response":raw_response,
        "responses":diagnostic_evidence(&responses),
        "errors":diagnostic_errors(&responses),
        "replay":true
    }))
}

fn replay_dpf_probe(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let readings = recorded
        .get("readings")
        .and_then(Value::as_array)
        .ok_or("recording DPF readings missing")?;
    let replayed = readings
        .iter()
        .map(replay_dpf_reading)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"experimental":true,"replay":true,"readings":replayed}))
}

fn replay_dpf_reading(recorded: &Value) -> Result<Value, String> {
    let semantic = recorded
        .get("semantic")
        .and_then(Value::as_str)
        .ok_or("recorded DPF semantic missing")?;
    let probe = crate::ea189::Ea189DpfProbe::ALL
        .into_iter()
        .find(|probe| probe.semantic() == semantic)
        .ok_or_else(|| format!("unsupported recorded DPF semantic: {semantic}"))?;
    let raw_response = recorded
        .get("raw_response")
        .and_then(Value::as_str)
        .filter(|raw| !raw.trim().is_empty());
    let Some(raw_response) = raw_response else {
        return Ok(json!({
            "semantic":semantic,
            "error":"recording raw_response missing",
            "experimental":true,
            "raw_response":null,
            "responses":[],
            "response_errors":[]
        }));
    };

    let responses = crate::elm::normalize_uds_responses(raw_response, probe.id())?;
    let matching: Vec<_> = responses
        .as_slice()
        .iter()
        .filter(|response| {
            response
                .responder
                .as_ref()
                .is_some_and(|responder| responder.as_str().eq_ignore_ascii_case("7E8"))
        })
        .collect();
    let payload = match matching.first() {
        None => Err("expected responder 7E8 absent".to_string()),
        Some(first)
            if matching
                .iter()
                .any(|response| response.payload != first.payload) =>
        {
            Err("conflicting DPF responses from expected responder".into())
        }
        Some(_) if !responses.errors().is_empty() => Err(format!(
            "incomplete DPF response: {}",
            responses.errors()[0].error
        )),
        Some(first) => Ok(first.payload.as_slice()),
    };
    let common = json!({
        "semantic":semantic,
        "experimental":true,
        "raw_response":raw_response,
        "responses":diagnostic_evidence(&responses),
        "response_errors":diagnostic_errors(&responses)
    });
    match payload.and_then(|payload| {
        crate::ea189::decode_experimental_dpf(probe, payload).map_err(|error| error.to_string())
    }) {
        Ok(reading) => {
            let mut result = common;
            result["value"] = json!(reading.value());
            result["unit"] = json!(reading.unit());
            Ok(result)
        }
        Err(error) => {
            let mut result = common;
            result["error"] = json!(error);
            Ok(result)
        }
    }
}

fn replay_ea189_extended_probe(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let readings = recorded
        .get("readings")
        .and_then(Value::as_array)
        .ok_or("recording EA189 extended readings missing")?;
    if readings.len() != crate::ea189::EA189_EXTENDED_PROBE_COUNT {
        return Err("recording does not contain the fixed EA189 extended read set".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for reading in readings {
        let semantic = reading
            .get("semantic")
            .and_then(Value::as_str)
            .ok_or("recorded EA189 extended semantic missing")?;
        if !crate::ea189::Ea189ExtendedSignal::ALL
            .iter()
            .any(|signal| signal.semantic() == semantic)
            || !seen.insert(semantic)
        {
            return Err(format!(
                "recording does not match the fixed EA189 extended read set: {semantic}"
            ));
        }
    }
    let replayed = readings
        .iter()
        .map(replay_ea189_extended_reading)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"experimental":true,"replay":true,"readings":replayed}))
}

fn replay_ea189_extended_reading(recorded: &Value) -> Result<Value, String> {
    let semantic = recorded
        .get("semantic")
        .and_then(Value::as_str)
        .ok_or("recorded EA189 extended semantic missing")?;
    let signal = crate::ea189::Ea189ExtendedSignal::ALL
        .into_iter()
        .find(|signal| signal.semantic() == semantic)
        .ok_or_else(|| format!("unsupported recorded EA189 extended semantic: {semantic}"))?;
    let request = recorded
        .get("request")
        .and_then(Value::as_str)
        .ok_or("recorded EA189 extended request missing")?;
    let expected_request = crate::hex(&signal.request_bytes());
    if request != expected_request {
        return Err(format!(
            "recorded EA189 extended request for {semantic} does not match its fixed read"
        ));
    }
    let raw_response = recorded
        .get("raw_response")
        .and_then(Value::as_str)
        .filter(|raw| !raw.trim().is_empty());
    let Some(raw_response) = raw_response else {
        let mut result = extended_signal_metadata(signal);
        result["error"] = json!("recording raw_response missing");
        result["raw_response"] = Value::Null;
        result["responses"] = json!([]);
        result["response_errors"] = json!([]);
        result["replay"] = json!(true);
        return Ok(result);
    };

    let responses = crate::elm::normalize_uds_responses(raw_response, signal.id())?;
    let matching: Vec<_> = responses
        .as_slice()
        .iter()
        .filter(|response| {
            response
                .responder
                .as_ref()
                .is_some_and(|responder| responder.as_str().eq_ignore_ascii_case("7E8"))
        })
        .collect();
    let payload = match matching.first() {
        None => Err("expected responder 7E8 absent".to_string()),
        Some(first)
            if matching
                .iter()
                .any(|response| response.payload != first.payload) =>
        {
            Err("conflicting EA189 extended responses from expected responder".into())
        }
        Some(_) if !responses.errors().is_empty() => Err(format!(
            "incomplete UDS response: {}",
            responses.errors()[0].error
        )),
        Some(first) => Ok(first.payload.as_slice()),
    };
    let mut result = extended_signal_metadata(signal);
    result["raw_response"] = json!(raw_response);
    result["responses"] = diagnostic_evidence(&responses);
    result["response_errors"] = diagnostic_errors(&responses);
    result["replay"] = json!(true);
    match payload.and_then(|payload| {
        crate::ea189::decode_experimental_extended(signal, payload)
            .map_err(|error| error.to_string())
    }) {
        Ok(reading) => {
            result["value"] = json!(reading.value());
            result["unit"] = json!(reading.unit());
        }
        Err(error) => result["error"] = json!(error),
    }
    Ok(result)
}

fn replay_ea189_ash_probe(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let readings = recorded
        .get("readings")
        .and_then(Value::as_array)
        .ok_or("recording EA189 ash-candidate readings missing")?;
    if readings.len() != crate::ea189::EA189_ASH_CANDIDATE_PROBE_COUNT {
        return Err("recording does not contain the fixed EA189 ash-candidate read set".into());
    }
    for (index, reading) in readings.iter().enumerate() {
        let candidate = reading
            .get("candidate")
            .and_then(Value::as_str)
            .ok_or("recorded EA189 ash-candidate identifier missing")?;
        let expected = crate::ea189::Ea189AshCandidate::ALL[index];
        if candidate != expected.candidate() {
            return Err(format!(
                "recording does not match the fixed EA189 ash-candidate read order: {candidate}"
            ));
        }
    }
    let replayed = readings
        .iter()
        .map(replay_ea189_ash_reading)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"experimental":true,"replay":true,"readings":replayed}))
}

fn replay_ea189_ash_reading(recorded: &Value) -> Result<Value, String> {
    let candidate_name = recorded
        .get("candidate")
        .and_then(Value::as_str)
        .ok_or("recorded EA189 ash-candidate identifier missing")?;
    let candidate = crate::ea189::Ea189AshCandidate::ALL
        .into_iter()
        .find(|candidate| candidate.candidate() == candidate_name)
        .ok_or_else(|| format!("unsupported EA189 ash candidate: {candidate_name}"))?;
    let request = recorded
        .get("request")
        .and_then(Value::as_str)
        .ok_or("recorded EA189 ash-candidate request missing")?;
    let expected_request = crate::hex(&candidate.request_bytes());
    if request != expected_request {
        return Err(format!(
            "recorded EA189 ash candidate {candidate_name} request does not match its fixed read"
        ));
    }
    let raw_response = recorded
        .get("raw_response")
        .and_then(Value::as_str)
        .filter(|raw| !raw.trim().is_empty());
    let Some(raw_response) = raw_response else {
        let mut result = ash_candidate_metadata(candidate);
        result["error"] = json!("recording raw_response missing");
        result["raw_response"] = Value::Null;
        result["responses"] = json!([]);
        result["response_errors"] = json!([]);
        result["observations"] = json!([]);
        result["replay"] = json!(true);
        if let Some(timestamp) = recorded.get("timestamp_ms") {
            result["timestamp_ms"] = timestamp.clone();
        }
        return Ok(result);
    };

    let responses = crate::elm::normalize_uds_responses(raw_response, candidate.id())?;
    let mut result = ash_candidate_result(candidate, &responses);
    result["replay"] = json!(true);
    if let Some(timestamp) = recorded.get("timestamp_ms") {
        result["timestamp_ms"] = timestamp.clone();
    }
    Ok(result)
}

fn catalog() -> Value {
    let mut profiles: Vec<_> = ["engine-baseline", "engine-drive", "obd2-expansion-validation", "obd2-extended-validation"]
        .into_iter().map(|id| {
            let profile = crate::capture::profile(id).expect("built-in capture profile must parse");
            let subscriptions = profile.subscriptions().expect("built-in subscriptions must resolve");
            json!({"id":id,"description":profile.description(),
                "signals":subscriptions.iter().map(|s|s.semantic()).collect::<Vec<_>>(),
                "intervals_ms":subscriptions.iter().map(|s|s.interval().as_millis()).collect::<Vec<_>>()})
        }).collect();
    profiles.push(json!({"id":"ea189-dpf","description":"Experimental EA189 DPF probes", "experimental":true,
        "signals":crate::ea189::Ea189DpfProbe::ALL.iter().map(|p|p.semantic()).collect::<Vec<_>>()}));
    profiles.push(json!({
        "id":"ea189-extended",
        "description":"Explicit experimental EA189 additional engine reads",
        "experimental":true,
        "default":false,
        "action":"ea189.extended.probe",
        "expected_reading_count":crate::ea189::EA189_EXTENDED_PROBE_COUNT,
        "signals":[],
        "probe_semantics":crate::ea189::Ea189ExtendedSignal::ALL.iter().map(|signal|signal.semantic()).collect::<Vec<_>>(),
        "source_version":crate::ea189::EA189_EXTENDED_SOURCE_VERSION,
        "source_profile":crate::ea189::EA189_EXTENDED_SOURCE_PROFILE,
        "hardware_validation":crate::ea189::EA189_EXTENDED_HARDWARE_VALIDATION
    }));
    profiles.push(json!({
        "id":"ea189-ash-test",
        "title":"Testlauf: EA189 Asche (ohne Sitzungswechsel)",
        "description":"Zwei feste Aschekandidaten im aktuellen Diagnosekontext erfassen; keine Werteinterpretation.",
        "type":"test_sequence",
        "experimental":true,
        "default":false,
        "signals":[]
    }));
    json!({"signals":crate::supported_signals().iter().map(|s| {let m=s.metadata();json!({
        "semantic":m.semantic,"title":m.description,"unit":m.unit,"description":m.description,
        "minimum":m.minimum,"maximum":m.maximum,"subsystem":m.subsystem,"profile":m.profile,
        "hardware_validation":m.hardware_validation
    })}).collect::<Vec<_>>(),"profiles":profiles})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Script(VecDeque<(&'static str, &'static str)>);
    impl Exchange for Script {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            let (expected, response) = self.0.pop_front().expect("unexpected transport call");
            assert_eq!(command, expected);
            Ok(response.into())
        }
    }
    fn core(extra: &[(&'static str, &'static str)]) -> MobileCore<Script> {
        let mut script = VecDeque::from([
            ("ATI\r", "ELM327\r>"),
            ("AT@1\r", "CARLY-UNIVERSAL\r>"),
            ("ATZ\r", "ELM327\r>"),
            ("ATE0\r", "OK\r>"),
            ("ATL0\r", "OK\r>"),
            ("ATS1\r", "OK\r>"),
            ("ATH1\r", "OK\r>"),
            ("ATSP0\r", "OK\r>"),
            ("0100\r", "7E8 06 41 00 00 10 00 00\r>"),
        ]);
        script.extend(extra.iter().copied());
        MobileCore::new(Script(script)).unwrap()
    }

    #[derive(Default)]
    struct ExtendedExchange {
        commands: Vec<String>,
        oem_reads: Vec<u16>,
    }

    impl Exchange for ExtendedExchange {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            self.commands.push(command.to_owned());
            let command = command.trim();
            Ok(match command {
                "ATI" | "ATZ" => "ELM327\r>".into(),
                "AT@1" => "CARLY-UNIVERSAL\r>".into(),
                "ATE0" | "ATL0" | "ATS1" | "ATH1" | "ATSP0" | "ATSH 7E0" | "ATCRA 7E8"
                | "ATSH 7DF" | "ATCRA" => "OK\r>".into(),
                "0100" => "7E8 06 41 00 00 00 00 00\r>".into(),
                "0902" => "NO DATA\r>".into(),
                "010C" => "7E8 04 41 0C 1F 40\r>".into(),
                request => {
                    let signal = crate::ea189::Ea189ExtendedSignal::ALL
                        .into_iter()
                        .find(|signal| {
                            crate::hex(&signal.request_bytes()).replace(' ', "") == request
                        })
                        .expect("only a closed EA189 extended request should be sent");
                    self.oem_reads.push(signal.id());
                    let definition = signal.definition();
                    let mut payload = vec![0x62, (signal.id() >> 8) as u8, signal.id() as u8];
                    payload.resize(3 + definition.data_len(), 0);
                    format!("7E8 {:02X} {}\r>", payload.len(), crate::hex(&payload))
                }
            })
        }
    }

    struct AshExchange {
        commands: Vec<String>,
        response_1153: &'static str,
        response_178c: &'static str,
    }

    impl AshExchange {
        fn new(response_1153: &'static str, response_178c: &'static str) -> Self {
            Self {
                commands: Vec::new(),
                response_1153,
                response_178c,
            }
        }
    }

    impl Exchange for AshExchange {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            self.commands.push(command.to_owned());
            Ok(match command.trim() {
                "ATI" | "ATZ" => "ELM327\r>".into(),
                "AT@1" => "CARLY-UNIVERSAL\r>".into(),
                "ATE0" | "ATL0" | "ATS1" | "ATH1" | "ATSP0" | "ATSH 7E0" | "ATCRA 7E8"
                | "ATSH 7DF" | "ATCRA" => "OK\r>".into(),
                "0100" => "7E8 06 41 00 00 00 00 00\r>".into(),
                "0902" => "NO DATA\r>".into(),
                "010C" => "7E8 04 41 0C 1F 40\r>".into(),
                "221153" => self.response_1153.into(),
                "22178C" => self.response_178c.into(),
                unexpected => panic!("unexpected command {unexpected}"),
            })
        }
    }

    fn run<E: Exchange>(core: &mut MobileCore<E>, action: &str, argument: &str) -> Value {
        serde_json::from_str(&core.execute(action, argument)).unwrap()
    }
    fn ash_recording(raw_1153: &str, raw_178c: &str) -> Value {
        json!({"readings":[
            {"candidate":"ash_volume_1153","request":"22 11 53","raw_response":raw_1153,"timestamp_ms":1000},
            {"candidate":"ash_mass_178c","request":"22 17 8C","raw_response":raw_178c,"timestamp_ms":2000}
        ]})
    }
    #[test]
    fn closed_actions_reject_writes_raw_commands_and_uninitialized_reads() {
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        for action in ["dtc.clear", "04", "raw", "ATSH7E0", "write", "22F190"] {
            assert_eq!(run(&mut core, action, "")["ok"], false);
        }
        assert_eq!(run(&mut core, "read", "engine.rpm")["ok"], false);
        assert_eq!(
            run(&mut core, "catalog", "")["data"]["signals"]
                .as_array()
                .unwrap()
                .len(),
            crate::supported_signals().len()
        );
        let catalog = run(&mut core, "catalog", "");
        let extended = &catalog["data"]["profiles"];
        let profile = extended
            .as_array()
            .unwrap()
            .iter()
            .find(|profile| profile["id"] == "ea189-extended")
            .unwrap();
        assert_eq!(profile["default"], false);
        assert_eq!(profile["action"], "ea189.extended.probe");
        assert_eq!(profile["signals"], json!([]));
        assert_eq!(
            profile["expected_reading_count"],
            crate::ea189::EA189_EXTENDED_PROBE_COUNT
        );
        let ash = extended
            .as_array()
            .unwrap()
            .iter()
            .find(|profile| profile["id"] == "ea189-ash-test")
            .unwrap();
        assert_eq!(ash["type"], "test_sequence");
        assert_eq!(ash["signals"], json!([]));
        assert!(!ash.as_object().unwrap().contains_key("action"));
    }

    #[test]
    fn extended_job_requires_inventory_and_uses_only_the_closed_targeted_read_set() {
        let mut without_target = core(&[]);
        assert_eq!(run(&mut without_target, "initialize", "")["ok"], true);
        let rejected = run(&mut without_target, "ea189.extended.probe", "");
        assert_eq!(rejected["ok"], false);
        assert!(rejected["error"]
            .as_str()
            .unwrap()
            .contains("validated engine target"));

        let mut core = MobileCore::new(ExtendedExchange::default()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&core.execute("initialize", "")).unwrap()["ok"],
            true
        );
        let inventory = serde_json::from_str::<Value>(&core.execute("inventory", "")).unwrap();
        assert_eq!(inventory["data"]["engine_target_validated"], true);
        let result =
            serde_json::from_str::<Value>(&core.execute("ea189.extended.probe", "")).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        let readings = result["data"]["readings"].as_array().unwrap();
        assert_eq!(readings.len(), crate::ea189::EA189_EXTENDED_PROBE_COUNT);
        assert_eq!(readings[6]["semantic"], "dpf.oil_ash_volume");
        assert_eq!(readings[6]["value"], 0.0);
        assert_eq!(readings[6]["request"], "22 11 53");
        assert_eq!(
            readings[6]["source_version"],
            crate::ea189::EA189_EXTENDED_SOURCE_VERSION
        );
        assert_eq!(readings[6]["responses"][0]["responder"], "7E8");
        let exchange = core.session.take().unwrap().into_exchange().0;
        assert_eq!(
            exchange.oem_reads.len(),
            crate::ea189::EA189_EXTENDED_PROBE_COUNT
        );
        assert_eq!(
            exchange
                .oem_reads
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            crate::ea189::EA189_EXTENDED_PROBE_COUNT
        );
        assert!(!exchange
            .commands
            .iter()
            .any(|command| command.trim() == "1003"));
    }

    #[test]
    fn ash_probe_requires_validated_engine_and_sends_only_two_fixed_reads() {
        let mut without_target = core(&[]);
        assert_eq!(run(&mut without_target, "initialize", "")["ok"], true);
        assert_eq!(run(&mut without_target, "ea189.ash.probe", "")["ok"], false);

        let mut core = MobileCore::new(AshExchange::new(
            "7E8 05 62 11 53 00 0C\r>",
            "7E8 05 62 17 8C 00 0C\r>",
        ))
        .unwrap();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let inventory = run(&mut core, "inventory", "");
        assert_eq!(inventory["data"]["engine_target_validated"], true);
        let result = run(&mut core, "ea189.ash.probe", "");
        assert_eq!(result["ok"], true, "{result}");
        let readings = result["data"]["readings"].as_array().unwrap();
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0]["candidate"], "ash_volume_1153");
        assert_eq!(readings[0]["request"], "22 11 53");
        assert_eq!(readings[0]["outcome"], "positive_candidate");
        assert_eq!(readings[0]["response_width_bytes"], 2);
        assert_eq!(readings[0]["response_data"], json!([0, 12]));
        assert_eq!(readings[0]["target"], "7E0");
        assert_eq!(readings[0]["expected_responder"], "7E8");
        assert_eq!(readings[0]["source_hypothesis"]["signed"], true);
        assert_eq!(readings[1]["candidate"], "ash_mass_178c");
        assert_eq!(readings[1]["request"], "22 17 8C");
        assert_eq!(readings[1]["outcome"], "positive_candidate");
        assert_eq!(readings[1]["source_hypothesis"]["signed"], false);
        for reading in readings {
            assert_eq!(reading["experimental"], true);
            assert!(!reading.as_object().unwrap().contains_key("value"));
            assert!(reading["timestamp_ms"].is_number());
        }

        let exchange = core.session.take().unwrap().into_exchange().0;
        let diagnostic_commands: Vec<_> = exchange
            .commands
            .iter()
            .map(|command| command.trim())
            .filter(|command| !command.starts_with("AT"))
            .collect();
        assert_eq!(
            diagnostic_commands,
            ["0100", "0100", "0902", "010C", "221153", "22178C", "0100"]
        );
        assert!(!exchange
            .commands
            .iter()
            .any(|command| command.trim() == "1003"));
    }

    #[test]
    fn ash_probe_keeps_negative_and_unexpected_responder_evidence_without_values() {
        let mut core = MobileCore::new(AshExchange::new(
            "7E8 03 7F 22 31\r>",
            "7E9 05 62 17 8C 00 0C\r>",
        ))
        .unwrap();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut core, "inventory", "")["data"]["engine_target_validated"],
            true
        );
        let result = run(&mut core, "ea189.ash.probe", "");
        let readings = result["data"]["readings"].as_array().unwrap();
        assert_eq!(readings[0]["nrc"], "0x31");
        assert_eq!(readings[0]["outcome"], "negative_response");
        assert_eq!(
            readings[0]["responses"][0]["payload"],
            json!([0x7f, 0x22, 0x31])
        );
        assert!(!readings[0].as_object().unwrap().contains_key("value"));
        assert!(readings[1]["error"]
            .as_str()
            .unwrap()
            .contains("expected responder 7E8 absent"));
        assert_eq!(readings[1]["responses"][0]["responder"], "7E9");
        assert_eq!(
            readings[1]["observations"][0]["selection_error"],
            "expected responder 7E8 absent"
        );
        assert!(!readings[1].as_object().unwrap().contains_key("value"));
    }

    #[test]
    fn ash_probe_preserves_truncated_and_wrong_width_responses() {
        let mut core = MobileCore::new(AshExchange::new(
            "7E8 06 62 11 53 00 0C\r>",
            "7E8 06 62 17 8C 00 0C 01\r>",
        ))
        .unwrap();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut core, "inventory", "")["data"]["engine_target_validated"],
            true
        );
        let result = run(&mut core, "ea189.ash.probe", "");
        let readings = result["data"]["readings"].as_array().unwrap();
        assert!(readings[0]["error"]
            .as_str()
            .unwrap()
            .contains("truncated UDS"));
        assert!(readings[0]["raw_response"]
            .as_str()
            .unwrap()
            .contains("62 11 53"));
        assert!(!readings[0]["response_errors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(readings[1]["error"].as_str().unwrap().contains("width 3"));
        assert_eq!(readings[1]["response_width_bytes"], 3);
        assert_eq!(readings[1]["response_data"], json!([0, 12, 1]));
        assert!(!readings[1].as_object().unwrap().contains_key("value"));
    }
    #[test]
    fn initialized_reads_reuse_session_and_preserve_responder() {
        let mut core = core(&[("010C\r", "7E8 04 41 0C 1F 40\r>")]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let read = run(&mut core, "read", "engine.rpm");
        assert_eq!(read["data"]["value"], 2000.0);
        assert_eq!(
            read["data"]["observations"][0]["responses"][0]["responder"],
            "7E8"
        );
        assert_eq!(run(&mut core, "read", "dtc.clear")["ok"], false);
        assert_eq!(run(&mut core, "read", "vehicle.speed")["ok"], false);
    }

    #[test]
    fn validated_engine_routes_reads_by_exact_responder_and_preserves_observations() {
        let mut core = core(&[
            ("0100\r", "7E8 06 41 00 00 10 00 00\r>"),
            ("0902\r", "NO DATA\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r7E9 04 41 0C 0C 80\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r7E9 04 41 0C 0C 80\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E9 04 41 0C 0C 80\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 00 00\r7E8 04 41 0C FF FF\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
        ]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let inventory = run(&mut core, "inventory", "");
        assert_eq!(inventory["data"]["engine_target_validated"], true);
        assert_eq!(
            inventory["data"]["engine_validation_request"]["request_payload"],
            json!([0x01, 0x0c])
        );
        assert_eq!(
            inventory["data"]["engine_validation_request"]["target"],
            "7E0"
        );
        assert_eq!(
            inventory["data"]["engine_validation_request"]["expected_responder"],
            "7E8"
        );
        assert_eq!(
            inventory["data"]["engine_validation_observations"][0]["selected_responder"],
            "7E8"
        );
        let validation_responses = inventory["data"]["engine_validation_observations"][0]
            ["responses"]
            .as_array()
            .unwrap();
        assert_eq!(validation_responses.len(), 2);
        assert_eq!(validation_responses[0]["responder"], "7E8");
        assert_eq!(
            validation_responses[0]["payload"],
            json!([0x41, 0x0c, 0x1f, 0x40])
        );
        assert_eq!(validation_responses[1]["responder"], "7E9");
        assert_eq!(
            validation_responses[1]["payload"],
            json!([0x41, 0x0c, 0x0c, 0x80])
        );

        let read = run(&mut core, "read", "engine.rpm");
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(read["data"]["value"], 2000.0);
        assert_eq!(read["data"]["observations"][0]["selected_responder"], "7E8");

        let mixed = run(&mut core, "read", "engine.rpm");
        assert_eq!(mixed["ok"], true, "{mixed}");
        assert_eq!(mixed["data"]["value"], 2000.0);
        assert_eq!(
            mixed["data"]["observations"][0]["selected_responder"],
            "7E8"
        );
        assert_eq!(
            mixed["data"]["observations"][0]["responses"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            mixed["data"]["observations"][0]["responses"][1]["responder"],
            "7E9"
        );

        let unexpected = run(&mut core, "read", "engine.rpm");
        assert_eq!(unexpected["ok"], false);
        assert!(unexpected["error"]
            .as_str()
            .unwrap()
            .contains("unexpected responder"));

        let conflicting = run(&mut core, "read", "engine.rpm");
        assert_eq!(conflicting["ok"], false);
        assert!(conflicting["error"]
            .as_str()
            .unwrap()
            .contains("conflicting responses from responder 7E8"));
    }

    #[test]
    fn engine_inventory_failure_keeps_targeted_response_evidence() {
        let mut core = core(&[
            ("0100\r", "7E8 06 41 00 00 10 00 00\r>"),
            ("0902\r", "NO DATA\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E9 04 41 0C 0C 80\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
        ]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);

        let inventory = run(&mut core, "inventory", "");

        assert_eq!(inventory["ok"], true);
        assert!(inventory["data"]["engine_target_error"]
            .as_str()
            .unwrap()
            .contains("unexpected responder 7E9"));
        assert_eq!(
            inventory["data"]["engine_validation_request"]["request_payload"],
            json!([0x01, 0x0c])
        );
        let observation = &inventory["data"]["engine_validation_observations"][0];
        assert!(observation["selected_responder"].is_null());
        assert!(observation["selection_error"]
            .as_str()
            .unwrap()
            .contains("unexpected responder 7E9"));
        assert_eq!(observation["responses"][0]["responder"], "7E9");
        assert_eq!(
            observation["responses"][0]["payload"],
            json!([0x41, 0x0c, 0x0c, 0x80])
        );
    }
    #[test]
    fn dtc_scan_preserves_negative_result_and_dpf_requires_target_evidence() {
        let mut core = core(&[("03\r", "7E8 03 43 01 0C\r>")]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(run(&mut core, "ea189.dpf.probe", "")["ok"], false);
        let scan = run(&mut core, "dtc.scan", "");
        assert_eq!(scan["data"]["observations"][0]["codes"][0], "P010C");
        assert_eq!(run(&mut core, "dtc.scan", "04")["ok"], false);
    }
    #[test]
    fn offline_replay_redecodes_raw_response_without_vehicle_traffic() {
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        let recorded =
            json!({"semantic":"engine.rpm","response":[65,12,31,64],"value":999,"unit":"wrong"});
        let replay = run(&mut core, "replay.read", &recorded.to_string());
        assert_eq!(replay["data"]["value"], 2000.0);
        assert_eq!(replay["data"]["unit"], "rpm");
        assert_eq!(
            run(
                &mut core,
                "replay.read",
                r#"{"semantic":"engine.rpm","response":[65,13,1]}"#
            )["ok"],
            false
        );
        assert_eq!(
            run(
                &mut core,
                "replay.read",
                r#"{"semantic":"dtc.clear","response":[68]}"#
            )["ok"],
            false
        );
    }

    #[test]
    fn replay_dtc_scan_corrects_recorded_codes_from_raw_evidence_without_transport() {
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        let recorded = json!({
            "raw_response":"7E8 03 43 01 0C\r>",
            "observations":[{"responder":"7E8","codes":["P0000"]}]
        });
        let replay = run(&mut core, "replay.dtc.scan", &recorded.to_string());
        assert_eq!(replay["ok"], true);
        assert_eq!(replay["data"]["observations"][0]["codes"][0], "P010C");
        assert_eq!(replay["data"]["observations"][0]["responder"], "7E8");
        assert_eq!(
            replay["data"]["responses"][0]["payload"],
            json!([0x43, 0x01, 0x0c])
        );
        assert_eq!(replay["data"]["replay"], true);
        assert_eq!(
            run(&mut core, "replay.dtc.scan", r#"{"observations":[]}"#)["ok"],
            false
        );

        let malformed = run(
            &mut core,
            "replay.dtc.scan",
            r#"{"raw_response":"7E8 03 43 01 GG"}"#,
        );
        assert_eq!(malformed["ok"], true);
        assert!(!malformed["data"]["errors"].as_array().unwrap().is_empty());
        assert!(malformed["data"]["observations"][0].get("error").is_some());
        assert!(malformed["data"]["observations"][0].get("codes").is_none());
    }

    #[test]
    fn replay_dpf_probe_corrects_values_and_keeps_incomplete_evidence_clear() {
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        let recorded = json!({
            "experimental":true,
            "readings":[{
                "semantic":"dpf.soot_mass_calculated",
                "value":999,
                "unit":"wrong",
                "experimental":true,
                "raw_response":"7E8 05 62 11 4F 01 F4\r>"
            }]
        });
        let replay = run(&mut core, "replay.ea189.dpf.probe", &recorded.to_string());
        let reading = &replay["data"]["readings"][0];
        assert_eq!(replay["ok"], true);
        assert_eq!(reading["value"], 5.0);
        assert_eq!(reading["unit"], "g");
        assert_eq!(reading["responses"][0]["responder"], "7E8");
        assert_eq!(
            reading["responses"][0]["payload"],
            json!([0x62, 0x11, 0x4f, 0x01, 0xf4])
        );

        let malformed = run(
            &mut core,
            "replay.ea189.dpf.probe",
            r#"{"readings":[{"semantic":"dpf.soot_mass_calculated","value":999,"raw_response":"7E8 05 62 11 4F 01"}]}"#,
        );
        let malformed_reading = &malformed["data"]["readings"][0];
        assert_eq!(malformed["ok"], true);
        assert!(malformed_reading.get("error").is_some());
        assert!(malformed_reading.get("value").is_none());
        assert!(!malformed_reading["response_errors"]
            .as_array()
            .unwrap()
            .is_empty());

        let missing = run(
            &mut core,
            "replay.ea189.dpf.probe",
            r#"{"readings":[{"semantic":"dpf.soot_mass_calculated","value":999}]}"#,
        );
        assert_eq!(missing["ok"], true);
        assert_eq!(
            missing["data"]["readings"][0]["error"],
            "recording raw_response missing"
        );
        assert!(missing["data"]["readings"][0].get("value").is_none());
    }

    #[test]
    fn replay_extended_probe_redecodes_only_the_exact_read_set_and_rejects_spoofed_request() {
        let readings: Vec<_> = crate::ea189::Ea189ExtendedSignal::ALL
            .into_iter()
            .map(|signal| {
                let definition = signal.definition();
                let request = crate::hex(&signal.request_bytes());
                let data = if signal == crate::ea189::Ea189ExtendedSignal::OilAshVolume {
                    vec![0x00, 0x64]
                } else {
                    vec![0; definition.data_len()]
                };
                let mut payload = vec![0x62, (signal.id() >> 8) as u8, signal.id() as u8];
                payload.extend(data);
                json!({
                    "semantic":signal.semantic(),
                    "request":request,
                    "value":999,
                    "raw_response":format!("7E8 {:02X} {}\r>",payload.len(),crate::hex(&payload))
                })
            })
            .collect();
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        let replay = run(
            &mut core,
            "replay.ea189.extended.probe",
            &json!({"readings":readings}).to_string(),
        );
        assert_eq!(replay["ok"], true, "{replay}");
        let ash = &replay["data"]["readings"][6];
        assert_eq!(ash["value"], 1.0);
        assert_eq!(ash["unit"], "liters");
        assert_eq!(
            ash["responses"][0]["payload"],
            json!([0x62, 0x11, 0x53, 0, 0x64])
        );
        assert_eq!(ash["replay"], true);

        let recorded = json!({"readings":[{
            "semantic":"dpf.oil_ash_volume",
            "request":"22 11 54",
            "raw_response":"7E8 05 62 11 53 00 64\r>"
        }]});
        assert_eq!(
            run(
                &mut core,
                "replay.ea189.extended.probe",
                &recorded.to_string()
            )["ok"],
            false
        );

        let mut negative = json!({"readings":crate::ea189::Ea189ExtendedSignal::ALL.iter().map(|signal| {
            let definition = signal.definition();
            let raw_response = if *signal == crate::ea189::Ea189ExtendedSignal::OilAshVolume {
                "7E8 03 7F 22 31\r>".to_owned()
            } else {
                let mut payload = vec![0x62, (signal.id() >> 8) as u8, signal.id() as u8];
                payload.resize(3 + definition.data_len(), 0);
                format!("7E8 {:02X} {}\r>", payload.len(), crate::hex(&payload))
            };
            json!({"semantic":signal.semantic(),"request":crate::hex(&signal.request_bytes()),"raw_response":raw_response})
        }).collect::<Vec<_>>()});
        let result = run(
            &mut core,
            "replay.ea189.extended.probe",
            &negative.to_string(),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(
            result["data"]["readings"][6]["error"],
            "negative UDS response (NRC 0x31)"
        );
        assert!(result["data"]["readings"][6].get("value").is_none());

        negative["readings"].as_array_mut().unwrap().pop();
        assert_eq!(
            run(
                &mut core,
                "replay.ea189.extended.probe",
                &negative.to_string()
            )["ok"],
            false
        );
    }

    #[test]
    fn replay_ash_probe_preserves_raw_positive_and_negative_results_without_decoding() {
        let mut core = MobileCore::new(Script(VecDeque::new())).unwrap();
        let recorded = ash_recording("7E8 05 62 11 53 00 64\r>", "7E8 03 7F 22 31\r>");
        let replay = run(&mut core, "replay.ea189.ash.probe", &recorded.to_string());
        assert_eq!(replay["ok"], true, "{replay}");
        let readings = replay["data"]["readings"].as_array().unwrap();
        assert_eq!(readings[0]["outcome"], "positive_candidate");
        assert_eq!(readings[0]["response_data"], json!([0, 100]));
        assert_eq!(readings[0]["response_width_bytes"], 2);
        assert_eq!(readings[0]["timestamp_ms"], 1000);
        assert_eq!(readings[0]["replay"], true);
        assert!(!readings[0].as_object().unwrap().contains_key("value"));
        assert_eq!(readings[1]["nrc"], "0x31");
        assert_eq!(readings[1]["raw_response"], "7E8 03 7F 22 31\r>");
        assert_eq!(readings[1]["timestamp_ms"], 2000);
        assert_eq!(readings[1]["replay"], true);
        assert!(!readings[1].as_object().unwrap().contains_key("value"));

        let wrong_responder = ash_recording("7E9 05 62 11 53 00 64\r>", "7E8 05 62 17 8C 00 64\r>");
        let replay = run(
            &mut core,
            "replay.ea189.ash.probe",
            &wrong_responder.to_string(),
        );
        assert_eq!(
            replay["data"]["readings"][0]["error"],
            "expected responder 7E8 absent"
        );
        assert_eq!(
            replay["data"]["readings"][0]["responses"][0]["responder"],
            "7E9"
        );

        let truncated = ash_recording("7E8 06 62 11 53 00 64\r>", "7E8 05 62 17 8C 00 64\r>");
        let replay = run(&mut core, "replay.ea189.ash.probe", &truncated.to_string());
        assert!(replay["data"]["readings"][0]["error"]
            .as_str()
            .unwrap()
            .contains("truncated UDS"));
        assert!(!replay["data"]["readings"][0]["response_errors"]
            .as_array()
            .unwrap()
            .is_empty());

        let wrong_width = ash_recording("7E8 06 62 11 53 00 64 01\r>", "7E8 05 62 17 8C 00 64\r>");
        let replay = run(
            &mut core,
            "replay.ea189.ash.probe",
            &wrong_width.to_string(),
        );
        assert!(replay["data"]["readings"][0]["error"]
            .as_str()
            .unwrap()
            .contains("width 3"));
        assert!(!replay["data"]["readings"][0]
            .as_object()
            .unwrap()
            .contains_key("value"));

        let mut spoofed = ash_recording("7E8 05 62 11 53 00 64\r>", "7E8 05 62 17 8C 00 64\r>");
        spoofed["readings"][0]["request"] = json!("22 F1 90");
        assert_eq!(
            run(&mut core, "replay.ea189.ash.probe", &spoofed.to_string())["ok"],
            false
        );
        spoofed = ash_recording("7E8 05 62 11 53 00 64\r>", "7E8 05 62 17 8C 00 64\r>");
        spoofed["readings"].as_array_mut().unwrap().swap(0, 1);
        assert_eq!(
            run(&mut core, "replay.ea189.ash.probe", &spoofed.to_string())["ok"],
            false
        );
    }

    #[test]
    fn inventory_drops_old_engine_target_when_new_discovery_has_no_responder() {
        let mut core = core(&[
            ("0100\r", "7E8 06 41 00 00 10 00 00\r>"),
            ("0902\r", "NO DATA\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r>"),
            ("ATSP0\r", "OK\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
        ]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut core, "inventory", "")["data"]["engine_target_validated"],
            true
        );
        assert!(core.engine.is_some());

        core.session = Some(ElmSession::new(PlatformExchange(Script(VecDeque::from([
            ("0100\r", "NO DATA\r>"),
        ])))));
        assert_eq!(run(&mut core, "inventory", "")["ok"], true);
        assert!(core.engine.is_none());
    }

    #[test]
    fn dpf_probes_are_closed_experimental_and_negative_responses_are_not_values() {
        struct NegativeDpf;
        impl Exchange for NegativeDpf {
            fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
                Ok(match command.trim() {
                    "ATI" | "ATZ" => "ELM327\r>".into(),
                    "AT@1" => "CARLY-UNIVERSAL\r>".into(),
                    "0100" => "7E8 06 41 00 00 10 00 00\r>".into(),
                    "0902" => "NO DATA\r>".into(),
                    "010C" => "7E8 04 41 0C 00 00\r>".into(),
                    "ATE0" | "ATL0" | "ATS1" | "ATH1" | "ATSP0" | "ATSH 7E0" | "ATCRA 7E8"
                    | "ATSH 7DF" | "ATCRA" => "OK\r>".into(),
                    probe
                        if crate::ea189::Ea189DpfProbe::ALL
                            .iter()
                            .any(|p| probe == crate::hex(&p.request_bytes()).replace(' ', "")) =>
                    {
                        "7E8 03 7F 22 31\r>".into()
                    }
                    unexpected => panic!("unexpected command {unexpected}"),
                })
            }
        }
        let mut core = MobileCore::new(NegativeDpf).unwrap();
        let initialized: Value = serde_json::from_str(&core.execute("initialize", "")).unwrap();
        assert_eq!(initialized["ok"], true);
        let inventory: Value = serde_json::from_str(&core.execute("inventory", "")).unwrap();
        assert_eq!(inventory["data"]["engine_target_validated"], true);
        let dpf: Value = serde_json::from_str(&core.execute("ea189.dpf.probe", "")).unwrap();
        assert_eq!(dpf["ok"], true);
        assert_eq!(dpf["data"]["readings"].as_array().unwrap().len(), 7);
        for reading in dpf["data"]["readings"].as_array().unwrap() {
            assert_eq!(reading["experimental"], true);
            assert!(reading.get("error").is_some());
            assert!(reading.get("value").is_none());
            assert!(reading.get("raw_response").is_some());
            assert!(!reading["responses"].as_array().unwrap().is_empty());
        }
    }
}
