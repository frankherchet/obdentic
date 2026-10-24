//! Closed mobile facade. Platform code supplies transport, never diagnostic commands.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::{
    elm::{
        ElmExchange, ElmSession, ExchangeError, TargetedClusterOdometerRequest,
        TargetedDpfProbeRequest, TargetedEa189AshProbeRequest, TargetedEa189ExtendedProbeRequest,
        TargetedReadRequest, TransportFailure,
    },
    runtime_state::Activity,
    safety::{OperationRequest, SafetyPolicy},
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
    /// VIN read by the latest inventory of this session; gates the cluster job.
    inventory_vin: Option<String>,
    /// A targeted operation ran since the last full Carly (re-)initialization.
    /// Its bare `ATSH 7DF`/`ATCRA` restore does not reliably bring functional
    /// responses back on Carly, so functional operations re-initialize first (#215).
    functional_dirty: bool,
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
            inventory_vin: None,
            functional_dirty: false,
        })
    }

    /// The one proven way to (re-)establish functional addressing: the closed
    /// Carly setup plus support discovery. `restore_label` prefixes setup errors.
    fn reinitialize(&mut self, restore_label: Option<&str>) -> Result<(), String> {
        self.initialized = false;
        let mut exchange = self
            .session
            .take()
            .ok_or("session unavailable")?
            .into_exchange();
        let setup = self
            .runtime
            .block_on(crate::adapter::initialize_carly(&mut exchange));
        self.session = Some(ElmSession::new(exchange));
        setup.map_err(|error| match restore_label {
            Some(label) => format!("{label} failed: {error}"),
            None => error,
        })?;
        self.runtime.block_on(
            self.session
                .as_mut()
                .unwrap()
                .discover_support(crate::vehicle::highest_catalog_page()),
        )?;
        self.initialized = true;
        self.functional_dirty = false;
        Ok(())
    }

    /// Untargeted operations need functional responses; re-initialize only if a
    /// targeted operation ran since the last initialization (#215).
    fn ensure_functional(&mut self) -> Result<(), String> {
        if self.functional_dirty {
            self.reinitialize(None)?;
        }
        Ok(())
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
        if action == "replay.vw.cluster.odometer" {
            return replay_vw_cluster_odometer(argument);
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
                | "vw.cluster.odometer"
        ) {
            return Err(format!("read-only mobile core rejected action: {action}"));
        }
        if action != "read" && !argument.is_empty() {
            return Err("this closed action accepts no argument".into());
        }
        if action == "initialize" {
            self.initialized = false;
            self.engine = None;
            self.inventory_vin = None;
            self.reinitialize(None)?;
            return Ok(self.support());
        }
        if !self.initialized {
            return Err("initialize the adapter first".into());
        }
        match action {
            "read" => {
                let request = crate::prepare_read(argument)?;
                if self.engine.is_none() {
                    self.ensure_functional()?;
                } else {
                    self.functional_dirty = true;
                }
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
                self.ensure_functional()?;
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
            "vw.cluster.odometer" => self.vw_cluster_odometer(),
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
        self.inventory_vin = None;
        self.ensure_functional()?;
        let identity = self
            .runtime
            .block_on(self.session.as_mut().unwrap().identify());
        let mut data = self.support();
        match identity {
            Ok(identity) => {
                self.inventory_vin = Some(identity.vin().as_str().to_owned());
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
        self.functional_dirty = true;
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
        self.reinitialize(Some("DPF restoration"))?;
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
        self.reinitialize(Some("EA189 extended restoration"))?;
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
        self.reinitialize(Some("EA189 ash probe restoration"))?;
        Ok(json!({"experimental":true,"readings":readings}))
    }

    /// REQ-016: one user-started `22 2203` to `714`; no argument, retry or fallback.
    fn vw_cluster_odometer(&mut self) -> Result<Value, String> {
        let vin = self.inventory_vin.as_deref().ok_or(
            "Kombiinstrument-km-Stand: zuerst ein Inventar mit VIN erfassen (nichts gesendet)",
        )?;
        if !crate::vin_identity::is_vw_group_vin(vin) {
            return Err(
                "Kombiinstrument-km-Stand: nur für Fahrzeuge der VW-Gruppe (nichts gesendet)"
                    .into(),
            );
        }
        SafetyPolicy::read_only()
            .authorize_activity(Activity::Diagnose, OperationRequest::VwClusterOdometer)
            .map_err(|error| error.to_string())?;
        let request = TargetedClusterOdometerRequest::vw_cluster();
        self.functional_dirty = true;
        let read = self.runtime.block_on(
            self.session
                .as_mut()
                .unwrap()
                .read_cluster_odometer_with_evidence(&request),
        );
        let mut result = cluster_result(read.responses.as_ref(), read.error.as_deref());
        result["timestamp_ms"] = json!(wallclock_timestamp_ms());
        if let Some(error) = read.restore_error {
            self.initialized = false;
            result["restore_error"] = json!(error);
        }
        Ok(result)
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

/// Shared by the live job and its offline replay, so both decode identically.
fn cluster_result(
    responses: Option<&crate::elm::DiagnosticResponses>,
    error: Option<&str>,
) -> Value {
    use crate::vw_cluster as cluster;
    let mut result = json!({
        "semantic":cluster::SEMANTIC,
        "experimental":true,
        "target":cluster::TARGET,
        "responder":cluster::RESPONDER,
        "did":format!("{:04X}", cluster::DID),
        "request":"22 22 03",
        "source":cluster::SOURCE,
        "source_provenance":cluster::PROVENANCE,
        "classification":cluster::CLASSIFICATION,
        "hardware_validation":cluster::HARDWARE_VALIDATION,
    });
    let Some(responses) = responses else {
        result["outcome"] = json!(cluster::Outcome::NoResponse.as_str());
        result["error"] = json!(error.unwrap_or("no response"));
        result["raw_response"] = Value::Null;
        result["responses"] = json!([]);
        result["response_errors"] = json!([]);
        result["observations"] = json!([]);
        return result;
    };
    let reading = cluster::interpret(responses);
    result["outcome"] = json!(reading.outcome.as_str());
    result["raw_response"] = json!(responses.raw_response());
    result["responses"] = diagnostic_evidence(responses);
    result["response_errors"] = diagnostic_errors(responses);
    let selection_error = match reading.outcome {
        cluster::Outcome::NoResponse
        | cluster::Outcome::OtherResponder
        | cluster::Outcome::Malformed => reading.error.clone(),
        _ => None,
    };
    let expected = crate::elm::ResponderIdentity::ElmHeader(cluster::RESPONDER.into());
    result["observations"] = json!(response_observations_json(&[
        responses.observation_for_target(&expected, selection_error)
    ]));
    if let Some(km) = reading.value_km {
        result["value_km"] = json!(km);
    }
    if let Some(nrc) = reading.nrc {
        result["nrc"] = json!(format!("0x{nrc:02X}"));
    }
    if let Some(data) = reading.data {
        result["response_data"] = json!(data);
    }
    if let Some(error) = reading.error {
        result["error"] = json!(error);
    }
    result
}

fn replay_vw_cluster_odometer(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    if recorded
        .get("semantic")
        .and_then(Value::as_str)
        .is_some_and(|semantic| semantic != crate::vw_cluster::SEMANTIC)
    {
        return Err("recording is not a VW cluster odometer read".into());
    }
    let raw_response = recorded
        .get("raw_response")
        .and_then(Value::as_str)
        .filter(|raw| !raw.trim().is_empty());
    let mut result = match raw_response {
        None => cluster_result(
            None,
            Some(
                recorded
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("recording raw_response missing"),
            ),
        ),
        Some(raw) => cluster_result(
            Some(&crate::elm::normalize_uds_responses(
                raw,
                crate::vw_cluster::DID,
            )?),
            None,
        ),
    };
    result["replay"] = json!(true);
    if let Some(timestamp) = recorded.get("timestamp_ms") {
        result["timestamp_ms"] = timestamp.clone();
    }
    Ok(result)
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
        core_with_pages(&[("0100\r", "7E8 06 41 00 00 10 00 00\r>")], extra)
    }
    fn core_with_pages(
        pages: &[(&'static str, &'static str)],
        extra: &[(&'static str, &'static str)],
    ) -> MobileCore<Script> {
        let mut script = VecDeque::from([
            ("ATI\r", "ELM327\r>"),
            ("AT@1\r", "CARLY-UNIVERSAL\r>"),
            ("ATZ\r", "ELM327\r>"),
            ("ATE0\r", "OK\r>"),
            ("ATL0\r", "OK\r>"),
            ("ATS1\r", "OK\r>"),
            ("ATH1\r", "OK\r>"),
            ("ATSP0\r", "OK\r>"),
        ]);
        script.extend(pages.iter().copied());
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

    const VW_VIN_RESPONSE: &str =
        "7E8 10 14 49 02 01 57 56 57\r7E8 21 5A 5A 5A 31 4A 5A 58\r7E8 22 57 30 30 30 30 30 31\r>";
    const BMW_VIN_RESPONSE: &str =
        "7E8 10 14 49 02 01 57 42 41\r7E8 21 5A 5A 5A 31 4A 5A 58\r7E8 22 57 30 30 30 30 30 31\r>";

    struct ClusterExchange {
        commands: Vec<String>,
        vin_response: &'static str,
        cluster_response: Result<&'static str, &'static str>,
    }

    impl ClusterExchange {
        fn new(vin_response: &'static str, cluster_response: &'static str) -> Self {
            Self {
                commands: Vec::new(),
                vin_response,
                cluster_response: Ok(cluster_response),
            }
        }
    }

    impl Exchange for ClusterExchange {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            self.commands.push(command.to_owned());
            match command.trim() {
                "ATI" | "ATZ" => Ok("ELM327\r>".into()),
                "AT@1" => Ok("CARLY-UNIVERSAL\r>".into()),
                "ATE0" | "ATL0" | "ATS1" | "ATH1" | "ATSP0" | "ATSH 7DF" | "ATCRA" | "ATSH 714"
                | "ATCRA 77E" | "ATSH 7E0" | "ATCRA 7E8" => Ok("OK\r>".into()),
                "0100" => Ok("7E8 06 41 00 00 00 00 00\r>".into()),
                "010C" => Ok("7E8 04 41 0C 1F 40\r>".into()),
                "0902" => Ok(self.vin_response.into()),
                "222203" => self.cluster_response.map(Into::into).map_err(Into::into),
                unexpected => panic!("unexpected command {unexpected}"),
            }
        }
    }

    /// Initialize and run an inventory so the VIN gate has evidence.
    fn cluster_core(
        vin_response: &'static str,
        cluster_response: &'static str,
    ) -> MobileCore<ClusterExchange> {
        let mut core =
            MobileCore::new(ClusterExchange::new(vin_response, cluster_response)).unwrap();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(run(&mut core, "inventory", "")["ok"], true);
        core
    }

    fn sent<E>(core: &mut MobileCore<E>, extract: impl FnOnce(E) -> Vec<String>) -> Vec<String>
    where
        E: Exchange,
    {
        extract(core.session.take().unwrap().into_exchange().0)
    }

    fn cluster_commands(core: &mut MobileCore<ClusterExchange>) -> Vec<String> {
        sent(core, |exchange| exchange.commands)
    }

    /// Commands after the inventory's engine validation (last `010C` plus its
    /// two-command restore), i.e. only the cluster job's traffic.
    fn job_traffic(commands: &[String]) -> Vec<String> {
        let start = commands.iter().rposition(|c| c.trim() == "010C").unwrap() + 3;
        commands[start..]
            .iter()
            .map(|c| c.trim().to_owned())
            .collect()
    }

    #[test]
    fn req_016_1_vw_vin_sends_exactly_one_targeted_read_and_decodes_the_value() {
        let mut core = cluster_core(VW_VIN_RESPONSE, "77E 05 62 22 03 30 39\r>");
        let result = run(&mut core, "vw.cluster.odometer", "");
        assert_eq!(result["ok"], true, "{result}");
        let data = &result["data"];
        assert_eq!(data["outcome"], "value");
        assert_eq!(data["value_km"], 123450);
        assert_eq!(data["target"], "714");
        assert_eq!(data["responder"], "77E");
        assert_eq!(data["did"], "2203");
        assert_eq!(
            data["source"],
            "VW Kombiinstrument DID 2203 (experimentell)"
        );
        assert_eq!(data["classification"], "COMMUNITY");
        assert_eq!(data["hardware_validation"], "pending");
        assert_eq!(data["responses"][0]["responder"], "77E");
        assert!(data["timestamp_ms"].is_number());
        let commands = cluster_commands(&mut core);
        assert_eq!(
            job_traffic(&commands),
            ["ATSH 714", "ATCRA 77E", "222203", "ATSH 7DF", "ATCRA"]
        );
    }

    #[test]
    fn req_016_2_non_vw_vin_or_missing_inventory_sends_nothing() {
        let mut bmw = cluster_core(BMW_VIN_RESPONSE, "77E 05 62 22 03 30 39\r>");
        let refused = run(&mut bmw, "vw.cluster.odometer", "");
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("VW-Gruppe"));

        let mut no_inventory =
            MobileCore::new(ClusterExchange::new(VW_VIN_RESPONSE, "unused")).unwrap();
        assert_eq!(run(&mut no_inventory, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut no_inventory, "vw.cluster.odometer", "")["ok"],
            false
        );
        // Dropping the inventory evidence closes the gate again.
        let mut stale = cluster_core(VW_VIN_RESPONSE, "unused");
        stale.inventory_vin = None;
        assert_eq!(run(&mut stale, "vw.cluster.odometer", "")["ok"], false);

        for core in [&mut bmw, &mut no_inventory, &mut stale] {
            let commands = cluster_commands(core);
            assert!(!commands
                .iter()
                .any(|c| c.contains("222203") || c.contains("ATSH 714")));
        }
    }

    #[test]
    fn req_016_2_the_action_accepts_no_argument() {
        let mut core = cluster_core(VW_VIN_RESPONSE, "77E 05 62 22 03 30 39\r>");
        for argument in ["2203", "714", "22 22 03"] {
            let result = run(&mut core, "vw.cluster.odometer", argument);
            assert_eq!(result["ok"], false);
            assert!(result["error"].as_str().unwrap().contains("no argument"));
        }
        assert!(!cluster_commands(&mut core)
            .iter()
            .any(|c| c.contains("222203")));
    }

    #[test]
    fn req_016_3_only_one_22_2203_and_no_session_security_or_routine_service() {
        let mut core = cluster_core(VW_VIN_RESPONSE, "77E 05 62 22 03 30 39\r>");
        assert_eq!(run(&mut core, "vw.cluster.odometer", "")["ok"], true);
        let commands = cluster_commands(&mut core);
        assert_eq!(commands.iter().filter(|c| c.trim() == "222203").count(), 1);
        for command in &commands {
            let command = command.trim();
            assert!(
                !(command.starts_with("10")
                    || command.starts_with("27")
                    || command.starts_with("31")),
                "forbidden service sent: {command}"
            );
        }
        // Functional addressing is restored last.
        assert_eq!(commands.last().unwrap().trim(), "ATCRA");
    }

    #[test]
    fn req_016_4_negative_missing_and_foreign_responses_never_yield_a_value() {
        for (response, outcome) in [
            ("77E 03 7F 22 31\r>", "negative_response"),
            ("NO DATA\r>", "no_response"),
            ("7E8 05 62 22 03 30 39\r>", "other_responder"),
        ] {
            let mut core = cluster_core(VW_VIN_RESPONSE, response);
            let result = run(&mut core, "vw.cluster.odometer", "");
            assert_eq!(result["ok"], true, "{result}");
            let data = &result["data"];
            assert_eq!(data["outcome"], outcome, "{response}");
            assert!(!data.as_object().unwrap().contains_key("value_km"));
            assert!(data["raw_response"].as_str().unwrap().len() > 2);
            if outcome == "negative_response" {
                assert_eq!(data["nrc"], "0x31");
                assert!(data["error"].as_str().unwrap().contains("0x31"));
            }
            let commands = cluster_commands(&mut core);
            assert_eq!(commands.iter().filter(|c| c.trim() == "222203").count(), 1);
            assert_eq!(commands.last().unwrap().trim(), "ATCRA");
        }
    }

    #[test]
    fn req_016_4_a_transport_failure_ends_without_value_and_restores_addressing() {
        let mut core = cluster_core(VW_VIN_RESPONSE, "unused");
        let mut exchange = core.session.take().unwrap().into_exchange().0;
        exchange.cluster_response = Err("Carly command timed out: 222203");
        core.session = Some(ElmSession::new(PlatformExchange(exchange)));
        let result = run(&mut core, "vw.cluster.odometer", "");
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["data"]["outcome"], "no_response");
        assert!(result["data"]["error"]
            .as_str()
            .unwrap()
            .contains("timed out"));
        assert_eq!(result["data"]["raw_response"], Value::Null);
        assert_eq!(cluster_commands(&mut core).last().unwrap().trim(), "ATCRA");
    }

    #[test]
    fn req_016_5_other_payload_lengths_are_kept_raw_without_a_value() {
        for (response, width) in [
            ("77E 06 62 22 03 00 12 D6\r>", 3),
            ("77E 04 62 22 03 12\r>", 1),
            ("77E 03 62 22 03\r>", 0),
        ] {
            let mut core = cluster_core(VW_VIN_RESPONSE, response);
            let result = run(&mut core, "vw.cluster.odometer", "");
            let data = &result["data"];
            assert_eq!(data["outcome"], "unparseable_length", "{result}");
            assert_eq!(data["response_data"].as_array().unwrap().len(), width);
            assert!(!data.as_object().unwrap().contains_key("value_km"));
            assert!(data["error"].as_str().unwrap().contains("nicht auswertbar"));
        }
    }

    #[test]
    fn req_016_7_replay_redecodes_the_recorded_raw_response_without_transport() {
        let mut core = cluster_core(VW_VIN_RESPONSE, "77E 05 62 22 03 30 39\r>");
        let live = run(&mut core, "vw.cluster.odometer", "");
        let recorded = live["data"].to_string();
        struct Panics;
        impl Exchange for Panics {
            fn exchange(&mut self, _: &str, _: Duration) -> Result<String, String> {
                panic!("replay must not use the transport")
            }
        }
        let mut offline = MobileCore::new(Panics).unwrap();
        let replay = run(&mut offline, "replay.vw.cluster.odometer", &recorded);
        assert_eq!(replay["ok"], true, "{replay}");
        assert_eq!(replay["data"]["value_km"], 123450);
        assert_eq!(replay["data"]["outcome"], "value");
        assert_eq!(replay["data"]["replay"], true);
        assert_eq!(replay["data"]["timestamp_ms"], live["data"]["timestamp_ms"]);

        let negative = json!({"raw_response":"77E 03 7F 22 31\r>"}).to_string();
        let replay = run(&mut offline, "replay.vw.cluster.odometer", &negative);
        assert_eq!(replay["data"]["outcome"], "negative_response");
        assert!(!replay["data"].as_object().unwrap().contains_key("value_km"));
        let other = json!({"semantic":"engine.rpm","raw_response":"x"}).to_string();
        assert_eq!(
            run(&mut offline, "replay.vw.cluster.odometer", &other)["ok"],
            false
        );
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
    /// Pages 0100..01A0 chained by continuation bits; PID A6 advertised.
    const A6_PAGES: [(&str, &str); 6] = [
        ("0100\r", "7E8 06 41 00 00 00 00 01\r>"),
        ("0120\r", "7E8 06 41 20 00 00 00 01\r>"),
        ("0140\r", "7E8 06 41 40 00 00 00 01\r>"),
        ("0160\r", "7E8 06 41 60 00 00 00 01\r>"),
        ("0180\r", "7E8 06 41 80 00 00 00 01\r>"),
        ("01A0\r", "7E8 06 41 A0 04 00 00 00\r>"),
    ];

    #[test]
    fn req_015_1_odometer_is_discovered_through_page_a0_and_decoded() {
        let mut core = core_with_pages(&A6_PAGES, &[("01A6\r", "7E8 06 41 A6 00 12 D6 87\r>")]);
        let support = run(&mut core, "initialize", "");
        assert_eq!(support["ok"], true);
        let odometer = support["data"]["signals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|signal| signal["semantic"] == "vehicle.odometer")
            .unwrap();
        assert_eq!(odometer["status"], "supported");
        let read = run(&mut core, "read", "vehicle.odometer");
        assert_eq!(read["ok"], true);
        assert_eq!(read["data"]["value"], 123456.7);
        assert_eq!(read["data"]["unit"], "km");
    }

    #[test]
    fn req_015_2_unadvertised_odometer_sends_no_request() {
        // Page 00 has no continuation bit; the script has no 01A6 entry and
        // would panic on any unexpected transport call.
        let mut core = core(&[]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let read = run(&mut core, "read", "vehicle.odometer");
        assert_eq!(read["ok"], false);
        assert!(read["error"]
            .as_str()
            .unwrap()
            .contains("does not advertise"));
    }

    #[test]
    fn req_015_3_failed_odometer_reads_are_errors() {
        for response in [
            "NO DATA\r>",
            "7E8 03 7F 01 12\r>",
            "7E8 05 41 A6 00 12 D6\r>",
        ] {
            let mut core = core_with_pages(&A6_PAGES, &[("01A6\r", response)]);
            assert_eq!(run(&mut core, "initialize", "")["ok"], true);
            assert_eq!(
                run(&mut core, "read", "vehicle.odometer")["ok"],
                false,
                "{response}"
            );
        }
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
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r7E9 04 41 0C 0C 80\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E9 04 41 0C 0C 80\r>"),
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 00 00\r7E8 04 41 0C FF FF\r>"),
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
            ("ATSH 7DF\r", "OK\r>"),
            ("ATCRA\r", "OK\r>"),
        ]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut core, "inventory", "")["data"]["engine_target_validated"],
            true
        );
        assert!(core.engine.is_some());

        // The targeted validation dirtied functional addressing (#215), so the
        // second inventory re-initializes; the new discovery has no 7E8.
        core.session = Some(ElmSession::new(PlatformExchange(Script(VecDeque::from([
            ("ATI\r", "ELM327\r>"),
            ("AT@1\r", "CARLY-UNIVERSAL\r>"),
            ("ATZ\r", "ELM327\r>"),
            ("ATE0\r", "OK\r>"),
            ("ATL0\r", "OK\r>"),
            ("ATS1\r", "OK\r>"),
            ("ATH1\r", "OK\r>"),
            ("ATSP0\r", "OK\r>"),
            ("0100\r", "7E9 06 41 00 00 10 00 00\r>"),
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

    /// Carly model (#215): after an explicit `ATSH 7DF` + bare `ATCRA` restore,
    /// functional `0100`/`0902` answer `NO DATA` until the next `ATZ` init.
    struct NoFunctionalAfterRestore {
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        broken: bool,
        searching: bool,
    }

    impl Exchange for NoFunctionalAfterRestore {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            let command = command.trim();
            self.log.lock().unwrap().push(command.to_owned());
            Ok(match command {
                "ATI" => "ELM327\r>".into(),
                "AT@1" => "CARLY-UNIVERSAL\r>".into(),
                "ATZ" => {
                    self.broken = false;
                    "ELM327\r>".into()
                }
                "ATSP0" => {
                    self.searching = true;
                    "OK\r>".into()
                }
                "ATCRA" => {
                    self.broken = true;
                    "OK\r>".into()
                }
                c if c.starts_with("AT") => "OK\r>".into(),
                "0100" | "0902" if self.broken => "NO DATA\r>".into(),
                "0100" if self.searching => {
                    self.searching = false;
                    "SEARCHING...\r7E8 06 41 00 98 3B A0 12 00\r>".into()
                }
                "0100" => "7E8 06 41 00 98 3B A0 12 00\r>".into(),
                "0902" => "7E8 10 14 49 02 01 57 56 57\r7E8 21 5A 5A 5A 31 4B 5A 41\r7E8 22 4D 31 32 33 34 35 36\r>".into(),
                "010C" => "7E8 04 41 0C 1F 40\r>".into(),
                other => panic!("unexpected command {other}"),
            })
        }
    }

    type Log = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

    fn broken_core() -> (MobileCore<NoFunctionalAfterRestore>, Log) {
        let log = Log::default();
        let core = MobileCore::new(NoFunctionalAfterRestore {
            log: log.clone(),
            broken: false,
            searching: false,
        })
        .unwrap();
        (core, log)
    }

    fn atz_count(log: &Log) -> usize {
        log.lock().unwrap().iter().filter(|c| *c == "ATZ").count()
    }

    #[test]
    fn issue_215_inventory_after_targeted_operations_reinitializes_before_functional_requests() {
        let (mut core, log) = broken_core();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let first = run(&mut core, "inventory", "");
        assert!(first["data"]["identity_error"].is_null(), "{first}");
        assert_eq!(first["data"]["vin"], "WVWZZZ1KZAM123456");
        assert_eq!(first["data"]["engine_target_validated"], true);

        // Targeted live polling leaves the adapter in the restored state.
        assert_eq!(run(&mut core, "read", "engine.rpm")["ok"], true);
        assert_eq!(atz_count(&log), 1);

        let second = run(&mut core, "inventory", "");
        assert!(second["data"]["identity_error"].is_null(), "{second}");
        assert_eq!(second["data"]["vin"], "WVWZZZ1KZAM123456");
        assert_eq!(atz_count(&log), 2);
    }

    #[test]
    fn issue_215_consecutive_targeted_reads_do_not_reinitialize() {
        let (mut core, log) = broken_core();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let inventory = run(&mut core, "inventory", "");
        assert_eq!(inventory["data"]["engine_target_validated"], true);
        for _ in 0..3 {
            assert_eq!(run(&mut core, "read", "engine.rpm")["ok"], true);
        }
        assert_eq!(atz_count(&log), 1);
    }

    #[test]
    fn issue_215_inventory_directly_after_initialize_does_not_reinitialize() {
        let (mut core, log) = broken_core();
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let inventory = run(&mut core, "inventory", "");
        assert_eq!(inventory["data"]["vin"], "WVWZZZ1KZAM123456");
        assert_eq!(atz_count(&log), 1);
    }
}
