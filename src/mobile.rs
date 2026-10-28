//! Closed mobile facade. Platform code supplies transport, never diagnostic commands.
use std::time::Duration;

use serde_json::{json, Value};

use crate::{
    elm::{ElmExchange, ElmSession, ExchangeError, TargetedReadRequest, TransportFailure},
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

    /// Full Carly initialization. Functional operations re-initialize on
    /// their own after targeted ones (`ElmSession`, #215/#221).
    fn reinitialize(&mut self) -> Result<(), String> {
        self.initialized = false;
        let session = self.session.as_mut().ok_or("session unavailable")?;
        self.runtime.block_on(async {
            // The mobile core only drives Carly: verify it before the generic reset.
            crate::adapter::verify_carly(session.exchange_mut()).await?;
            session.reinitialize().await
        })?;
        self.initialized = true;
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
        if !matches!(action, "initialize" | "inventory" | "read" | "dtc.scan") {
            return Err(format!("read-only mobile core rejected action: {action}"));
        }
        if action != "read" && !argument.is_empty() {
            return Err("this closed action accepts no argument".into());
        }
        if action == "initialize" {
            self.initialized = false;
            self.engine = None;
            self.reinitialize()?;
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

fn catalog() -> Value {
    let profiles: Vec<_> = ["engine-baseline", "engine-drive", "obd2-expansion-validation", "obd2-extended-validation"]
        .into_iter().map(|id| {
            let profile = crate::capture::profile(id).expect("built-in capture profile must parse");
            let subscriptions = profile.subscriptions().expect("built-in subscriptions must resolve");
            json!({"id":id,"description":profile.description(),
                "signals":subscriptions.iter().map(|s|s.semantic()).collect::<Vec<_>>(),
                "intervals_ms":subscriptions.iter().map(|s|s.interval().as_millis()).collect::<Vec<_>>()})
        }).collect();
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

    fn run<E: Exchange>(core: &mut MobileCore<E>, action: &str, argument: &str) -> Value {
        serde_json::from_str(&core.execute(action, argument)).unwrap()
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
        for action in [
            "dtc.clear",
            "04",
            "raw",
            "ATSH7E0",
            "write",
            "22F190",
            "ea189.dpf.probe",
            "replay.ea189.ash.probe",
            "vw.cluster.odometer",
            "replay.vw.cluster.odometer",
        ] {
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
    fn validated_engine_routes_reads_by_exact_responder_and_preserves_observations() {
        let mut core = core(&[
            ("0100\r", "7E8 06 41 00 00 10 00 00\r>"),
            ("0902\r", "NO DATA\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r7E9 04 41 0C 0C 80\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r7E9 04 41 0C 0C 80\r>"),
            ("010C\r", "7E9 04 41 0C 0C 80\r>"),
            ("010C\r", "7E8 04 41 0C 00 00\r7E8 04 41 0C FF FF\r>"),
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
    fn dtc_scan_preserves_negative_result() {
        let mut core = core(&[("03\r", "7E8 03 43 01 0C\r>")]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
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
        // second inventory re-initializes with the generic ELM reset (no
        // adapter identity check); the new discovery has no 7E8.
        *core.session.as_mut().unwrap().exchange_mut() =
            PlatformExchange(Script(VecDeque::from([
                ("ATZ\r", "ELM327\r>"),
                ("ATE0\r", "OK\r>"),
                ("ATL0\r", "OK\r>"),
                ("ATS1\r", "OK\r>"),
                ("ATH1\r", "OK\r>"),
                ("ATSP0\r", "OK\r>"),
                ("0100\r", "7E9 06 41 00 00 10 00 00\r>"),
                ("0100\r", "NO DATA\r>"),
            ])));
        assert_eq!(run(&mut core, "inventory", "")["ok"], true);
        assert!(core.engine.is_none());
    }

    impl Exchange for crate::test_support::NoFunctionalAfterRestore {
        fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
            Ok(self.respond(command))
        }
    }

    use crate::test_support::{CommandLog as Log, NoFunctionalAfterRestore};

    fn broken_core() -> (MobileCore<NoFunctionalAfterRestore>, Log) {
        let (model, log) = NoFunctionalAfterRestore::new();
        (MobileCore::new(model).unwrap(), log)
    }

    fn atz_count(log: &Log) -> usize {
        NoFunctionalAfterRestore::atz_count(log)
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
