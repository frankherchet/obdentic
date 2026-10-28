//! Closed mobile facade. Platform code supplies transport, never diagnostic commands.
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{
    elm::{
        ElmExchange, ElmSession, ExchangeError, TargetedMultiReadRequest, TargetedReadRequest,
        TransportFailure,
    },
    runtime_state::Activity,
    safety::{OperationRequest, SafetyPolicy},
    throughput,
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

/// Upper bound of the adapter throughput job (REQ-022.5).
const THROUGHPUT_LIMIT: Duration = Duration::from_secs(60);

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
        if action == "replay.adapter.throughput" {
            return throughput::replay(argument);
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
            "initialize" | "inventory" | "read" | "dtc.scan" | "adapter.throughput"
        ) {
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
                let read = if self.engine.is_some() {
                    if !self
                        .session
                        .as_ref()
                        .unwrap()
                        .supported()
                        .supports_pid(request.pid())
                    {
                        return Err(format!("vehicle does not advertise support for {argument}"));
                    }
                    let targeted = self.targeted_read(request)?;
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
            "adapter.throughput" => self.adapter_throughput(THROUGHPUT_LIMIT),
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

    fn engine_target(&self) -> Result<(RequestTarget, crate::elm::ResponderIdentity), String> {
        let mapping = self
            .engine
            .as_ref()
            .ok_or("no validated engine target; run the inventory first")?;
        let responder = crate::elm::ResponderIdentity::ElmHeader(
            mapping
                .expected_responder()
                .value()
                .ok_or("validated engine responder missing")?
                .into(),
        );
        Ok((mapping.target().target().clone(), responder))
    }

    fn targeted_read(&self, request: crate::ReadRequest) -> Result<TargetedReadRequest, String> {
        let (target, responder) = self.engine_target()?;
        TargetedReadRequest::new(request, target, responder)
    }

    /// Closed adapter throughput job (REQ-022). The mobile core has no
    /// cancellation: the job ends by itself after `limit` (60 s). The platform's
    /// stop/disconnect closes the transport, which fails the pending read and
    /// ends the job with a partial recording.
    fn adapter_throughput(&mut self, limit: Duration) -> Result<Value, String> {
        SafetyPolicy::read_only()
            .authorize_activity(Activity::Diagnose, OperationRequest::AdapterThroughput)
            .map_err(|error| error.to_string())?;
        let (target, responder) = self.engine_target()?;
        let support = self.session.as_ref().unwrap().supported();
        let mut selected: Vec<crate::ReadRequest> = Vec::new();
        for signal in crate::supported_signals() {
            let request = crate::prepare_read(signal.metadata().semantic)?;
            if selected.len() < throughput::MAX_PIDS
                && request.fixed_length()
                && support.supports_pid(request.pid())
                && !selected.iter().any(|known| known.pid() == request.pid())
            {
                selected.push(request);
            }
        }
        if selected.is_empty() {
            return Err("vehicle advertises no catalog PID for the throughput job".into());
        }
        let elapsed_ms =
            |since: Instant| throughput::round1(since.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        let mut truncated = false;
        let mut reads = Vec::new();
        'phase1: for request in &selected {
            let mut consecutive_errors = 0;
            for _ in 0..throughput::READS_PER_PID {
                if started.elapsed() >= limit {
                    truncated = true;
                    break 'phase1;
                }
                let targeted =
                    TargetedReadRequest::new(*request, target.clone(), responder.clone())?;
                let at = Instant::now();
                let result = self.runtime.block_on(
                    self.session
                        .as_mut()
                        .unwrap()
                        .read_targeted_with_evidence(&targeted),
                );
                let elapsed = elapsed_ms(at);
                let (payload, error, observations) = match result {
                    Ok(read) => (
                        Some(read.payload),
                        None,
                        response_observations_json(&read.observations),
                    ),
                    Err(error) => (
                        None,
                        Some(error.error),
                        response_observations_json(&error.observations),
                    ),
                };
                consecutive_errors = if payload.is_some() {
                    0
                } else {
                    consecutive_errors + 1
                };
                reads.push(json!({"semantic":request.semantic(),"elapsed_ms":elapsed,
                    "response":payload,"error":error,"observations":observations}));
                if consecutive_errors >= throughput::MAX_CONSECUTIVE_ERRORS {
                    break;
                }
            }
        }
        let mut phase2 = Vec::new();
        let mut note = Value::Null;
        if selected.len() < 2 {
            note = json!("fewer than 2 advertised PIDs");
        } else if !truncated {
            for size in throughput::phase2_sizes(selected.len()) {
                if started.elapsed() >= limit {
                    truncated = true;
                    break;
                }
                let pids = &selected[..size];
                let multi = TargetedMultiReadRequest::new(pids, target.clone(), responder.clone())?;
                let at = Instant::now();
                let result = self.runtime.block_on(
                    self.session
                        .as_mut()
                        .unwrap()
                        .read_multi_targeted_with_evidence(&multi),
                );
                let elapsed = elapsed_ms(at);
                let (payload, error, observations) = match result {
                    Ok(read) => (
                        Some(read.payload),
                        None,
                        response_observations_json(&read.observations),
                    ),
                    Err(error) => (
                        None,
                        Some(error.error),
                        response_observations_json(&error.observations),
                    ),
                };
                phase2.push(
                    json!({"semantic":pids.iter().map(|r| r.semantic()).collect::<Vec<_>>(),
                    "command":multi.command().trim(),"elapsed_ms":elapsed,"payload":payload,
                    "error":error,"observations":observations}),
                );
            }
        }
        throughput::with_evidence(&json!({
            "phase1_reads":reads,"phase2_requests":phase2,"phase2_note":note,
            "truncated":truncated,"limit_s":limit.as_secs(),"total_elapsed_ms":elapsed_ms(started)
        }))
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

    // ---- REQ-022: adapter throughput job ----

    const PAGE_ALL: &str = "7E8 06 41 00 18 39 00 00\r>"; // 0C 05 0D 10 04 0B
    const PAGE_TWO: &str = "7E8 06 41 00 08 10 00 00\r>"; // 0C 05
    const PAGE_ONE: &str = "7E8 06 41 00 00 10 00 00\r>"; // 0C
    const PHASE2_FULL: [(&str, &str); 3] = [
        ("010C05\r", "7E8 06 41 0C 1F 40 05 5A\r>"),
        (
            "010C050D\r",
            "7E8 10 08 41 0C 1F 40 05 5A\r7E8 21 0D 32 00 00 00 00\r>",
        ),
        (
            "010C050D10040B\r",
            "7E8 10 0F 41 0C 1F 40 05 5A\r7E8 21 0D 32 10 01 F4 04 80\r7E8 22 0B 64 00 00 00 00 00\r>",
        ),
    ];

    /// Initialized core with a validated engine target and a scripted throughput run.
    fn throughput_core(
        page: &'static str,
        pids: &[(&'static str, &'static str)],
        phase2: &[(&'static str, &'static str)],
    ) -> MobileCore<Script> {
        let mut extra = vec![
            ("0100\r", page),
            ("0902\r", "NO DATA\r>"),
            ("ATSH 7E0\r", "OK\r>"),
            ("ATCRA 7E8\r", "OK\r>"),
            ("010C\r", "7E8 04 41 0C 1F 40\r>"),
        ];
        for (command, response) in pids {
            extra.extend(std::iter::repeat_n((*command, *response), 10));
        }
        extra.extend(phase2.iter().copied());
        let mut core = core_with_pages(&[("0100\r", page)], &extra);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        assert_eq!(
            run(&mut core, "inventory", "")["data"]["engine_target_validated"],
            true
        );
        core
    }

    const ALL_PIDS: [(&str, &str); 6] = [
        ("010C\r", "7E8 04 41 0C 1F 40\r>"),
        ("0105\r", "7E8 03 41 05 5A\r>"),
        ("010D\r", "7E8 03 41 0D 32\r>"),
        ("0110\r", "7E8 04 41 10 01 F4\r>"),
        ("0104\r", "7E8 03 41 04 80\r>"),
        ("010B\r", "7E8 03 41 0B 64\r>"),
    ];

    fn script_remaining(core: &mut MobileCore<Script>) -> usize {
        core.session.as_mut().unwrap().exchange_mut().0 .0.len()
    }

    #[test]
    fn req_022_2_phase1_reads_each_advertised_pid_ten_times_with_one_tx_per_read() {
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &PHASE2_FULL);
        let result = run(&mut core, "adapter.throughput", "");
        assert_eq!(result["ok"], true, "{result}");
        let data = &result["data"];
        let pids = data["phase1"]["pids"].as_array().unwrap();
        let names: Vec<_> = pids
            .iter()
            .map(|p| p["semantic"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "engine.rpm",
                "engine.coolant_temperature",
                "vehicle.speed",
                "engine.maf",
                "engine.load",
                "engine.intake_manifold_pressure"
            ]
        );
        assert!(pids.iter().all(|p| p["n"] == 10 && p["errors"] == 0));
        assert_eq!(pids[0]["pid"], "0C");
        assert_eq!(data["phase1"]["overall"]["n"], 60);
        assert!(data["phase1"]["overall"]["median_ms"].is_number());
        assert!(data["phase1"]["overall"]["p90_ms"].is_number());
        assert_eq!(data["phase1_reads"].as_array().unwrap().len(), 60);
        assert_eq!(data["phase1_reads"][0]["response"], json!([65, 12, 31, 64]));
        assert_eq!(data["truncated"], false);
        // Every scripted TX (60 single reads, 3 multi-PID) was used, none extra.
        assert_eq!(script_remaining(&mut core), 0);
    }

    #[test]
    fn req_022_3_multi_pid_requests_with_two_three_and_six_pids_are_full() {
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &PHASE2_FULL);
        let data = run(&mut core, "adapter.throughput", "")["data"].clone();
        let phase2 = data["phase2"].as_array().unwrap();
        assert_eq!(phase2.len(), 3);
        assert_eq!(
            phase2
                .iter()
                .map(|r| r["pids"].as_array().unwrap().len())
                .collect::<Vec<_>>(),
            [2, 3, 6]
        );
        for request in phase2 {
            assert_eq!(request["outcome"], "full", "{request}");
            assert!(request["elapsed_ms"].is_number());
        }
        let values = phase2[2]["values"].as_array().unwrap();
        assert_eq!(values[0]["value"], 2000.0);
        assert_eq!(values[1]["value"], 50.0);
        assert_eq!(values[0]["unit"], "rpm");
        assert!(values.iter().all(|v| v["status"] == "ok"));
        assert_eq!(script_remaining(&mut core), 0);
    }

    #[test]
    fn req_022_3_and_7_partial_rejected_and_malformed_replies_never_invent_values() {
        let phase2 = [
            // 05 missing from the reply.
            ("010C05\r", "7E8 04 41 0C 1F 40\r>"),
            // Truncated tail: 0D announced without its data byte.
            (
                "010C050D\r",
                "7E8 10 07 41 0C 1F 40 05 5A\r7E8 21 0D 00 00 00 00 00\r>",
            ),
            // Rejected by the vehicle.
            ("010C050D10040B\r", "NO DATA\r>"),
        ];
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &phase2);
        let data = run(&mut core, "adapter.throughput", "")["data"].clone();
        let phase2 = data["phase2"].as_array().unwrap();
        assert_eq!(phase2[0]["outcome"], "partial");
        assert_eq!(phase2[0]["values"][0]["status"], "ok");
        assert_eq!(phase2[0]["values"][1]["status"], "missing");
        assert!(phase2[0]["values"][1].get("value").is_none());
        assert_eq!(phase2[1]["outcome"], "partial");
        assert_eq!(phase2[1]["values"][1]["value"], 50.0);
        assert_eq!(phase2[1]["values"][2]["status"], "malformed");
        assert!(phase2[1]["values"][2].get("value").is_none());
        assert_eq!(phase2[2]["outcome"], "rejected");
        assert!(phase2[2]["reason"].as_str().unwrap().contains("NO DATA"));
        assert!(phase2[2]["values"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["status"] == "missing" && v.get("value").is_none()));
    }

    #[test]
    fn req_022_3_negative_response_is_rejected() {
        let phase2 = [
            ("010C05\r", "7E8 03 7F 01 12\r>"),
            ("010C050D\r", "NO DATA\r>"),
            ("010C050D10040B\r", "NO DATA\r>"),
        ];
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &phase2);
        let data = run(&mut core, "adapter.throughput", "")["data"].clone();
        assert_eq!(data["phase2"][0]["outcome"], "rejected");
        assert!(data["phase2"][0]["reason"].as_str().unwrap().contains("7F"));
    }

    #[test]
    fn req_022_3_and_4_only_advertised_pids_are_used() {
        let pids = [ALL_PIDS[0], ALL_PIDS[1]];
        let mut core = throughput_core(PAGE_TWO, &pids, &PHASE2_FULL[..1]);
        let data = run(&mut core, "adapter.throughput", "")["data"].clone();
        assert_eq!(data["phase1"]["pids"].as_array().unwrap().len(), 2);
        assert_eq!(data["phase2"].as_array().unwrap().len(), 1);
        assert_eq!(data["phase2"][0]["pids"], json!(["0C", "05"]));
        // The script has no entry for any other PID; an extra TX would panic.
        assert_eq!(script_remaining(&mut core), 0);
    }

    #[test]
    fn req_022_3_fewer_than_two_advertised_pids_runs_phase_one_only() {
        let mut core = throughput_core(PAGE_ONE, &ALL_PIDS[..1], &[]);
        let data = run(&mut core, "adapter.throughput", "")["data"].clone();
        assert_eq!(data["phase1"]["overall"]["n"], 10);
        assert_eq!(data["phase2"], json!([]));
        assert_eq!(data["phase2_note"], "fewer than 2 advertised PIDs");
        assert_eq!(script_remaining(&mut core), 0);
    }

    #[test]
    fn req_022_1_and_4_job_needs_session_and_engine_target_and_accepts_no_argument() {
        let mut fresh = MobileCore::new(Script(VecDeque::new())).unwrap();
        assert_eq!(run(&mut fresh, "adapter.throughput", "")["ok"], false);

        // Initialized but no validated engine target: refused, nothing sent.
        let mut core = core(&[]);
        assert_eq!(run(&mut core, "initialize", "")["ok"], true);
        let refused = run(&mut core, "adapter.throughput", "");
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("engine target"));

        // No caller bytes: any argument is rejected before anything is sent.
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &PHASE2_FULL);
        for argument in ["010C", "engine.rpm", "0100"] {
            assert_eq!(run(&mut core, "adapter.throughput", argument)["ok"], false);
        }
        assert_eq!(script_remaining(&mut core), 60 + 3);
    }

    #[test]
    fn req_022_6_replay_rederives_the_live_result_without_transport() {
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &PHASE2_FULL);
        let live = run(&mut core, "adapter.throughput", "")["data"].clone();
        let mut offline = MobileCore::new(Script(VecDeque::new())).unwrap();
        let replay = run(&mut offline, "replay.adapter.throughput", &live.to_string());
        assert_eq!(replay["ok"], true, "{replay}");
        let mut replayed = replay["data"].clone();
        assert_eq!(
            replayed.as_object_mut().unwrap().remove("replay"),
            Some(json!(true))
        );
        assert_eq!(replayed, live);
        // A tampered summary is ignored: statistics come from the evidence.
        let mut tampered = live.clone();
        tampered["phase1"]["overall"]["n"] = json!(1);
        tampered["phase2"][0]["outcome"] = json!("rejected");
        let again = run(
            &mut offline,
            "replay.adapter.throughput",
            &tampered.to_string(),
        );
        assert_eq!(again["data"]["phase1"], live["phase1"]);
        assert_eq!(again["data"]["phase2"], live["phase2"]);
        assert_eq!(
            run(&mut offline, "replay.adapter.throughput", "{}")["ok"],
            false
        );
    }

    #[test]
    fn req_022_5_job_is_time_bounded_and_reports_truncation() {
        assert_eq!(THROUGHPUT_LIMIT, Duration::from_secs(60));
        let mut core = throughput_core(PAGE_ALL, &ALL_PIDS, &PHASE2_FULL);
        let data = core.adapter_throughput(Duration::ZERO).unwrap();
        assert_eq!(data["truncated"], true);
        assert_eq!(data["phase1_reads"], json!([]));
        assert_eq!(data["phase2"], json!([]));
        // Nothing was sent once the bound was reached.
        assert_eq!(script_remaining(&mut core), 60 + 3);
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
