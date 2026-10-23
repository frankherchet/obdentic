//! Closed mobile facade. Platform code supplies transport, never diagnostic commands.
use std::time::Duration;

use serde_json::{json, Value};

use crate::{
    elm::{ElmExchange, ElmSession, TargetedDpfProbeRequest, TargetedReadRequest},
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
    async fn exchange(&mut self, command: &str, timeout: Duration) -> Result<String, String> {
        self.0.exchange(command, timeout)
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
        if action == "replay.dtc.scan" {
            return replay_dtc_scan(argument);
        }
        if action == "replay.ea189.dpf.probe" {
            return replay_dpf_probe(argument);
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
            "initialize" | "inventory" | "read" | "dtc.scan" | "ea189.dpf.probe"
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
                .block_on(self.session.as_mut().unwrap().discover_support(0x40))?;
            self.initialized = true;
            return Ok(self.support());
        }
        if !self.initialized {
            return Err("initialize the adapter first".into());
        }
        match action {
            "read" => {
                let request = crate::prepare_read(argument)?;
                let read = self
                    .runtime
                    .block_on(self.session.as_mut().unwrap().read_with_evidence(request))
                    .map_err(|error| error.error)?;
                let transaction = request.complete("user", read.payload)?;
                let observations: Vec<_> = read
                    .observations
                    .into_iter()
                    .map(|observation| {
                        json!({
                            "selected_responder":observation.selected_responder,
                            "selection_error":observation.selection_error,
                            "responses":observation.responses.iter().map(|response| json!({
                                "responder":response.responder,"payload":response.payload
                            })).collect::<Vec<_>>()
                        })
                    })
                    .collect();
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
            Ok(identity) => data["vin"] = json!(identity.vin().as_str()),
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
            match self.validate_engine() {
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

    fn validate_engine(&mut self) -> Result<EcuTargetMapping, String> {
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
        let read = self
            .runtime
            .block_on(
                self.session
                    .as_mut()
                    .unwrap()
                    .read_targeted_with_evidence(&targeted),
            )
            .map_err(|e| e.error)?;
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
            .block_on(self.session.as_mut().unwrap().discover_support(0x40))
        {
            self.initialized = false;
            return Err(error);
        }
        Ok(json!({"experimental":true,"readings":readings}))
    }
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

fn catalog() -> Value {
    let mut profiles: Vec<_> = ["engine-baseline", "engine-drive", "obd2-expansion-validation"]
        .into_iter().map(|id| {
            let profile = crate::capture::profile(id).expect("built-in capture profile must parse");
            let subscriptions = profile.subscriptions().expect("built-in subscriptions must resolve");
            json!({"id":id,"description":profile.description(),
                "signals":subscriptions.iter().map(|s|s.semantic()).collect::<Vec<_>>(),
                "intervals_ms":subscriptions.iter().map(|s|s.interval().as_millis()).collect::<Vec<_>>()})
        }).collect();
    profiles.push(json!({"id":"ea189-dpf","description":"Experimental EA189 DPF probes", "experimental":true,
        "signals":crate::ea189::Ea189DpfProbe::ALL.iter().map(|p|p.semantic()).collect::<Vec<_>>()}));
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
    fn run(core: &mut MobileCore<Script>, action: &str, argument: &str) -> Value {
        serde_json::from_str(&core.execute(action, argument)).unwrap()
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
