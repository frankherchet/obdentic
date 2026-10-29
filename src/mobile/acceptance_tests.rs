use super::{Exchange, MobileCore};
use crate::{
    ecu_identification::EcuIdentificationPlan,
    knowledge_db::KnowledgeCatalog,
    topology::{
        AddressingContext, Confidence, EcuRole, Protocol, ProtocolContext, Provenance,
        RequestAddress, RequestTarget, RequestTargetEvidence, ResponderIdentity, RoleAssignment,
    },
    vehicle_knowledge::EcuTargetMapping,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
enum Reply {
    Payload(Vec<u8>),
    NoData,
    Error(String),
}

#[derive(Clone, Copy)]
enum Presence {
    Positive,
    Negative(u8),
    WrongResponder,
    NoData,
}

#[derive(Clone)]
struct Command {
    wire: String,
    target: String,
}

struct LinkState {
    commands: Vec<Command>,
    target: String,
    responders: BTreeMap<String, String>,
    presence: BTreeMap<String, Presence>,
    all_present: bool,
    identification_dids: BTreeSet<u16>,
    did_replies: BTreeMap<u16, VecDeque<Reply>>,
    enter_reply: Reply,
    leave_reply: Reply,
}

struct FakeExchange(Arc<Mutex<LinkState>>);

impl Exchange for FakeExchange {
    fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
        let mut state = self.0.lock().unwrap();
        let target = state.target.clone();
        state.commands.push(Command {
            wire: command.to_owned(),
            target,
        });
        if let Some(target) = command
            .strip_prefix("ATSH ")
            .and_then(|value| value.strip_suffix('\r'))
        {
            state.target = target.to_ascii_uppercase();
            return Ok("OK\r>".into());
        }
        if command.starts_with("ATCRA") {
            return Ok("OK\r>".into());
        }
        match command {
            "ATI\r" => return Ok("ELM327\r>".into()),
            "AT@1\r" => return Ok("CARLY-UNIVERSAL\r>".into()),
            "ATZ\r" => {
                state.target = "7DF".into();
                return Ok("ELM327\r>".into());
            }
            "ATE0\r" | "ATL0\r" | "ATS1\r" | "ATH1\r" | "ATSP0\r" => return Ok("OK\r>".into()),
            "0100\r" => return Ok(frame("7E8", &[0x41, 0x00, 0x00, 0x10, 0x00, 0x00])),
            "010C\r" => return Ok(frame("7E8", &[0x41, 0x0C, 0x1A, 0xF8])),
            "03\r" => return Ok(frame("7E8", &[0x43, 0x00])),
            "1003\r" => return Ok(reply_frame(&state.enter_reply, "7E8")),
            "1001\r" => return Ok(reply_frame(&state.leave_reply, "7E8")),
            "3E80\r" => return Ok(frame("7E8", &[0x7E, 0x00])),
            _ => {}
        }
        if let Some(did) = command
            .strip_prefix("22")
            .and_then(|value| value.strip_suffix('\r'))
            .and_then(|value| u16::from_str_radix(value, 16).ok())
        {
            let expected = state
                .responders
                .get(&state.target)
                .cloned()
                .unwrap_or_default();
            let reply = if did == 0xF187 {
                match state
                    .presence
                    .get(&state.target)
                    .copied()
                    .unwrap_or(if state.all_present {
                        Presence::Positive
                    } else {
                        Presence::NoData
                    }) {
                    Presence::Positive => Reply::Payload(vec![0x62, 0xF1, 0x87, b'1', b'2', b'3']),
                    Presence::Negative(nrc) => Reply::Payload(vec![0x7F, 0x22, nrc]),
                    Presence::WrongResponder => {
                        let responder = if expected == "7E8" { "7E9" } else { "7E8" };
                        return Ok(frame(responder, &[0x62, 0xF1, 0x87, b'1', b'2', b'3']));
                    }
                    Presence::NoData => Reply::NoData,
                }
            } else if let Some(reply) = state
                .did_replies
                .get_mut(&did)
                .and_then(VecDeque::pop_front)
            {
                reply
            } else if state.identification_dids.contains(&did) && target_is_present(&state) {
                Reply::Payload(vec![0x62, (did >> 8) as u8, did as u8, b'1', b'2', b'3'])
            } else {
                Reply::NoData
            };
            return match reply {
                Reply::Payload(payload) => Ok(frame(&expected, &payload)),
                Reply::NoData => Ok("NO DATA\r>".into()),
                Reply::Error(error) => Err(error),
            };
        }
        panic!("unexpected fake adapter command: {command:?}")
    }
}

fn target_is_present(state: &LinkState) -> bool {
    matches!(
        state
            .presence
            .get(&state.target)
            .copied()
            .unwrap_or(if state.all_present {
                Presence::Positive
            } else {
                Presence::NoData
            }),
        Presence::Positive | Presence::Negative(_)
    )
}

fn frame(responder: &str, payload: &[u8]) -> String {
    let bytes = payload
        .iter()
        .map(|byte| format!(" {byte:02X}"))
        .collect::<String>();
    format!("{responder} {:02X}{bytes}\r>", payload.len())
}

fn reply_frame(reply: &Reply, responder: &str) -> String {
    match reply {
        Reply::Payload(payload) => frame(responder, payload),
        Reply::NoData => "NO DATA\r>".into(),
        Reply::Error(error) => panic!("unexpected configured transport error: {error}"),
    }
}

fn setup(
    all_present: bool,
    presence: &[(u16, Presence)],
) -> (
    MobileCore<FakeExchange>,
    Arc<Mutex<LinkState>>,
    KnowledgeCatalog,
) {
    let catalog = KnowledgeCatalog::embedded_pinned().unwrap();
    let responders = catalog
        .ecu_targets()
        .iter()
        .filter(|target| target.manufacturer() == "vag")
        .map(|target| {
            (
                format!("{:03X}", target.request_id()),
                format!("{:03X}", target.response_id()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let identification_dids = EcuIdentificationPlan::from_catalog(&catalog)
        .unwrap()
        .candidates()
        .iter()
        .map(|candidate| candidate.did())
        .collect();
    let presence = presence
        .iter()
        .map(|(request_id, behavior)| (format!("{request_id:03X}"), *behavior))
        .collect();
    let state = Arc::new(Mutex::new(LinkState {
        commands: Vec::new(),
        target: "7DF".into(),
        responders,
        presence,
        all_present,
        identification_dids,
        did_replies: BTreeMap::new(),
        enter_reply: Reply::Payload(vec![0x50, 0x03, 0, 0x32, 1, 0xF4]),
        leave_reply: Reply::Payload(vec![0x50, 0x01]),
    }));
    let mut core = MobileCore::new(FakeExchange(state.clone())).unwrap();
    assert_eq!(run(&mut core, "initialize", "")["ok"], true);

    // Test-only vehicle inventory seed; it stands in for the validated result of inventory().
    let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
    let target = RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0"));
    let provenance = Provenance::new("synthetic test inventory", Confidence::Verified).unwrap();
    core.vehicle_manufacturer = Some("vag".into());
    core.engine = Some(EcuTargetMapping::new(
        RoleAssignment::new(EcuRole::Engine, provenance.clone()),
        RequestTargetEvidence::new(target, provenance),
        ResponderIdentity::address(context, "7E8"),
    ));
    (core, state, catalog)
}

fn run<E: Exchange>(core: &mut MobileCore<E>, action: &str, argument: &str) -> Value {
    serde_json::from_str(&core.execute(action, argument)).unwrap()
}

fn discover(core: &mut MobileCore<FakeExchange>) -> Value {
    let result = run(core, "ecu.discover", "");
    assert_eq!(result["ok"], true, "{result}");
    result["data"].clone()
}

fn commands(state: &Arc<Mutex<LinkState>>) -> Vec<Command> {
    state.lock().unwrap().commands.clone()
}

fn diagnostic_dids(state: &Arc<Mutex<LinkState>>) -> Vec<(String, u16)> {
    commands(state)
        .into_iter()
        .filter_map(|command| {
            command
                .wire
                .strip_prefix("22")
                .and_then(|value| value.strip_suffix('\r'))
                .and_then(|value| u16::from_str_radix(value, 16).ok())
                .map(|did| (command.target, did))
        })
        .collect()
}

fn clear_commands(state: &Arc<Mutex<LinkState>>) {
    state.lock().unwrap().commands.clear();
}

#[test]
// REQ-023.1, REQ-023.2, REQ-023.3, REQ-023.5
fn req_023_1_2_3_5_discovery_is_targeted_and_keeps_response_evidence() {
    let (mut core, state, catalog) = setup(
        false,
        &[
            (0x7E0, Presence::Positive),
            (0x7E1, Presence::Negative(0x31)),
            (0x714, Presence::WrongResponder),
        ],
    );
    let result = discover(&mut core);
    let targets = result["targets"].as_array().unwrap();
    let target_count = catalog
        .ecu_targets()
        .iter()
        .filter(|target| target.manufacturer() == "vag")
        .count();
    assert_eq!(target_count, 18);
    assert_eq!(targets.len(), target_count);

    let f187 = diagnostic_dids(&state)
        .into_iter()
        .filter(|(_, did)| *did == 0xF187)
        .collect::<Vec<_>>();
    assert_eq!(f187.len(), target_count);
    let expected_targets = catalog
        .ecu_targets()
        .iter()
        .filter(|target| target.manufacturer() == "vag")
        .map(|target| format!("{:03X}", target.request_id()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        f187.iter()
            .map(|(target, _)| target.clone())
            .collect::<BTreeSet<_>>(),
        expected_targets
    );

    let target = |id: &str| {
        targets
            .iter()
            .find(|target| target["target_id"] == id)
            .unwrap()
    };
    assert_eq!(target("vag.ecu_target.7e0")["status"], "present");
    assert_eq!(target("vag.ecu_target.7e1")["status"], "present");
    assert_eq!(target("vag.ecu_target.7e1")["observations"][0]["nrc"], 0x31);
    assert_eq!(target("vag.ecu_target.714")["status"], "not_observed");
    assert_eq!(target("vag.ecu_target.714")["absent"], false);
    assert!(target("vag.ecu_target.7e0")["identity"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |item| item["semantic"] == "ecu.manufacturer_spare_part_number"
                && item["value"] == json!([49, 50, 51])
                && item["response"] == json!([0x62, 0xF1, 0x87, 49, 50, 51])
        ));

    let candidates = EcuIdentificationPlan::from_catalog(&catalog).unwrap();
    let dids = candidates
        .candidates()
        .iter()
        .map(|candidate| candidate.did())
        .collect::<Vec<_>>();
    let requests = diagnostic_dids(&state);
    for did in dids {
        for target in ["7E0", "7E1"] {
            assert_eq!(
                requests
                    .iter()
                    .filter(|(address, request)| address == target && *request == did)
                    .count(),
                1
            );
        }
    }
    assert!(requests.iter().all(|(_, did)| *did != 0xF190));
}

#[test]
// REQ-023.4
fn req_023_4_discovery_uses_reads_only_and_next_functional_job_restores_addressing() {
    let (mut core, state, _) = setup(true, &[]);
    clear_commands(&state);
    discover(&mut core);
    assert_eq!(state.lock().unwrap().target, "7DF");
    let discovery_commands = commands(&state);
    let services = discovery_commands
        .iter()
        .filter(|command| !command.wire.starts_with("AT"))
        .map(|command| command.wire.as_str())
        .collect::<Vec<_>>();
    assert!(!services.is_empty());
    assert!(services.iter().all(|service| service.starts_with("22")));

    clear_commands(&state);
    let scan = run(&mut core, "dtc.scan", "");
    assert_eq!(scan["ok"], true, "{scan}");
    let read = commands(&state)
        .into_iter()
        .find(|command| command.wire == "03\r")
        .unwrap();
    assert_eq!(read.target, "7DF");
}

#[test]
// REQ-024.1, REQ-024.2, REQ-024.3, REQ-024.5
fn req_024_1_2_3_5_catalog_and_reads_are_applicable_default_only_and_fail_closed() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    let discovery = discover(&mut core);
    let signals = discovery["signals"].as_array().unwrap();
    let signal = signals
        .iter()
        .find(|signal| signal["semantic"] == "air.charge_limit_overcharge")
        .unwrap();
    assert_eq!(signal["unit"], "g");
    assert_eq!(signal["group"], "air");
    assert_eq!(signal["validation"], "not_validated");
    assert_eq!(signal["validation_label"], "nicht abgeglichen");
    assert!(signals
        .iter()
        .all(|signal| signal["extended_session_required"] != true));

    state.lock().unwrap().did_replies.insert(
        0x1356,
        VecDeque::from([
            Reply::Payload(vec![0x62, 0x13, 0x56, 0x00, 0x64]),
            Reply::Payload(vec![0x7F, 0x22, 0x31]),
            Reply::Payload(vec![0x62, 0x13, 0x57, 0x00, 0x64]),
        ]),
    );
    let value = run(&mut core, "read", "air.charge_limit_overcharge");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["data"]["value"], 1.0);
    assert_eq!(value["data"]["validation_label"], "nicht abgeglichen");
    let negative = run(&mut core, "read", "air.charge_limit_overcharge");
    assert_eq!(negative["data"]["status"], "unavailable");
    assert_eq!(negative["data"]["nrc"], 0x31);
    assert!(negative["data"]["value"].is_null());
    let malformed = run(&mut core, "read", "air.charge_limit_overcharge");
    assert_eq!(malformed["data"]["status"], "unavailable");
    assert!(malformed["data"]["value"].is_null());
    assert_eq!(
        diagnostic_dids(&state)
            .iter()
            .filter(|(_, did)| *did == 0x1356)
            .count(),
        3
    );

    let before = diagnostic_dids(&state).len();
    let flagged = run(&mut core, "read", "air.intercooler_outlet_temperature");
    assert_eq!(flagged["ok"], false);
    assert_eq!(diagnostic_dids(&state).len(), before);
}

#[test]
// #256: a Knowledge signal whose semantic is also a standard PID is addressed by its own key
fn issue_256_knowledge_signal_sharing_a_pid_semantic_is_read_by_its_key_not_as_the_pid() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    let discovery = discover(&mut core);
    let signal = discovery["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["definition_id"] == "vag.ea189.oil.oil_temperature")
        .unwrap();
    assert_eq!(signal["semantic"], "engine.oil_temperature");
    assert_eq!(signal["key"], "engine.oil_temperature@vag.ecu_target.7e0");

    state.lock().unwrap().did_replies.insert(
        0x11BE,
        VecDeque::from([Reply::Payload(vec![0x62, 0x11, 0xBE, 0x04, 0x00])]),
    );
    let read = run(
        &mut core,
        "read",
        "engine.oil_temperature@vag.ecu_target.7e0",
    );
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["data"]["semantic"], "engine.oil_temperature");
    assert!(read["data"]["value"].is_number(), "{read}");
    assert_eq!(
        diagnostic_dids(&state)
            .iter()
            .filter(|(_, did)| *did == 0x11BE)
            .count(),
        1
    );

    let unknown = run(
        &mut core,
        "read",
        "engine.oil_temperature@vag.ecu_target.7e1",
    );
    assert_eq!(unknown["ok"], false, "{unknown}");
}

#[test]
// #256: with several ECUs present, signals that repeat a semantic per ECU still have unique keys
fn issue_256_every_catalog_signal_key_is_unique_across_all_present_ecus() {
    let (mut core, _state, _) = setup(true, &[]);
    let discovery = discover(&mut core);
    let keys = discovery["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["key"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let unique = keys.iter().collect::<std::collections::HashSet<_>>();
    assert!(keys.len() > 1);
    assert_eq!(unique.len(), keys.len(), "duplicate signal keys: {keys:?}");
}

#[test]
// REQ-026.1, REQ-026.2
fn req_026_1_2_learning_probes_each_candidate_once_without_session_change() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    let candidates = core
        .knowledge
        .as_ref()
        .unwrap()
        .learning_candidates()
        .unwrap();
    assert!(candidates.len() > 2);
    let mut replies = BTreeMap::<u16, VecDeque<Reply>>::new();
    let answered = candidates
        .iter()
        .find(|candidate| {
            candidate
                .definition
                .response()
                .exact_payload_length()
                .is_some()
        })
        .unwrap();
    let mut positive = vec![
        0x62,
        (answered.definition.operation().did() >> 8) as u8,
        answered.definition.operation().did() as u8,
    ];
    positive.resize(
        positive.len()
            + answered
                .definition
                .response()
                .exact_payload_length()
                .unwrap(),
        0,
    );
    replies.insert(
        answered.definition.operation().did(),
        VecDeque::from([Reply::Payload(positive)]),
    );
    let rejected = candidates
        .iter()
        .find(|candidate| candidate.definition.id() != answered.definition.id())
        .unwrap();
    replies.insert(
        rejected.definition.operation().did(),
        VecDeque::from([Reply::Payload(vec![0x7F, 0x22, 0x31])]),
    );
    state.lock().unwrap().did_replies = replies;
    clear_commands(&state);

    let result = run(&mut core, "knowledge.learn", "");
    assert_eq!(result["ok"], true, "{result}");
    let actual = diagnostic_dids(&state);
    assert_eq!(actual.len(), candidates.len());
    for candidate in &candidates {
        assert_eq!(
            actual
                .iter()
                .filter(|(_, did)| *did == candidate.definition.operation().did())
                .count(),
            1
        );
    }
    assert!(commands(&state)
        .iter()
        .filter(|command| !command.wire.starts_with("22"))
        .all(|command| {
            !["1001\r", "1003\r", "3E80\r", "2701\r", "3101\r"].contains(&command.wire.as_str())
        }));
    let results = result["data"]["results"].as_array().unwrap();
    assert!(results
        .iter()
        .any(|result| result["status"] == "answered" && result["validation"] == "unconfirmed"));
    assert!(results
        .iter()
        .any(|result| result["status"] == "rejected" && result["nrc"] == 0x31));
    assert!(results
        .iter()
        .any(|result| result["status"] == "not_answered"));
}

#[test]
// REQ-026.3
fn req_026_3_zero_limit_and_link_loss_keep_a_partial_result() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    clear_commands(&state);
    let zero = core.learn_knowledge(Duration::ZERO).unwrap();
    assert_eq!(zero["results"].as_array().unwrap().len(), 0);
    assert_eq!(zero["truncated"], true);
    assert!(diagnostic_dids(&state).is_empty());

    let first_did = core
        .knowledge
        .as_ref()
        .unwrap()
        .learning_candidates()
        .unwrap()[0]
        .definition
        .operation()
        .did();
    state.lock().unwrap().did_replies.insert(
        first_did,
        VecDeque::from([Reply::Error(
            "Bluetooth connection failed: test link loss".into(),
        )]),
    );
    let partial = run(&mut core, "knowledge.learn", "");
    assert_eq!(partial["ok"], true, "{partial}");
    assert_eq!(partial["data"]["stopped"], true);
    assert_eq!(partial["data"]["results"].as_array().unwrap().len(), 1);
}

#[test]
// REQ-027.1, REQ-027.2, REQ-027.3, REQ-027.4, REQ-027.5
fn req_027_1_2_3_4_5_mobile_session_lifecycle_reads_and_replays_raw_response() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    let definition = core
        .knowledge
        .as_ref()
        .unwrap()
        .learning_candidates()
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.definition.semantic() == "air.intercooler_outlet_temperature")
        .unwrap()
        .definition;
    let did = definition.operation().did();
    let mut positive = vec![0x62, (did >> 8) as u8, did as u8];
    positive.resize(
        positive.len() + definition.response().exact_payload_length().unwrap(),
        0,
    );
    state.lock().unwrap().did_replies.insert(
        did,
        VecDeque::from([
            Reply::Payload(positive),
            Reply::Payload(vec![0x7F, 0x22, 0x31]),
        ]),
    );
    clear_commands(&state);

    let enabled = run(&mut core, "uds.enable", "");
    assert_eq!(enabled["data"]["active"], true, "{enabled}");
    assert!(commands(&state)
        .iter()
        .any(|command| command.wire == "1003\r"));
    let active_catalog = run(&mut core, "catalog", "");
    let active_signal = active_catalog["data"]["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["semantic"] == "air.intercooler_outlet_temperature")
        .unwrap();
    assert_eq!(active_signal["protocol"], "UDS");
    assert_eq!(active_signal["extended_session_required"], true);
    let start = Instant::now();
    let _ = core.runtime.block_on(core.uds.tick(
        core.session.as_mut().unwrap(),
        start + Duration::from_secs(2),
    ));
    assert!(commands(&state)
        .iter()
        .any(|command| command.wire == "3E80\r"));

    let read = run(&mut core, "read", "air.intercooler_outlet_temperature");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["data"]["uds"]["active"], true);
    assert_eq!(read["data"]["uds_state_after_read"]["state"], "active");
    assert!(core.uds.is_active(), "state after live read: {read}");
    assert!(!read["data"]["value"].is_null());
    let recorded = read["data"].clone();
    let replay = run(&mut core, "replay.read", &recorded.to_string());
    assert_eq!(replay["ok"], true, "{replay}");
    assert_eq!(replay["data"]["value"], read["data"]["value"]);
    assert_eq!(replay["data"]["replay"], true);
    assert!(core.uds.is_active(), "state after replay: {replay}");

    let rejected = run(&mut core, "read", "air.intercooler_outlet_temperature");
    assert_eq!(rejected["ok"], true, "{rejected}");
    assert_eq!(rejected["data"]["status"], "unavailable");
    assert_eq!(rejected["data"]["nrc"], 0x31);
    assert_eq!(rejected["data"]["uds"]["active"], true);
    assert!(
        rejected["data"]["observations"][0]["responses"][0]["payload"] == json!([0x7F, 0x22, 0x31])
    );
    assert!(rejected["data"]["uds_session_observations"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|observation| observation["responses"].as_array().into_iter().flatten())
        .any(|response| response["payload"] == json!([0x50, 0x01])));
    let rejected_replay = run(&mut core, "replay.read", &rejected["data"].to_string());
    assert_eq!(rejected_replay["ok"], true, "{rejected_replay}");
    assert_eq!(rejected_replay["data"]["status"], "unavailable");
    assert_eq!(rejected_replay["data"]["nrc"], 0x31);

    let scan = run(&mut core, "dtc.scan", "");
    assert_eq!(scan["ok"], true, "{scan}");
    let commands = commands(&state);
    let leave = commands
        .iter()
        .rposition(|command| command.wire == "1001\r")
        .unwrap();
    let dtc = commands
        .iter()
        .position(|command| command.wire == "03\r")
        .unwrap();
    assert!(leave < dtc);
    assert_eq!(commands[dtc].target, "7DF");
    let inactive_catalog = run(&mut core, "catalog", "");
    assert_eq!(inactive_catalog["data"]["uds"]["active"], false);
    assert!(!inactive_catalog["data"]["signals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|signal| signal["semantic"] == "air.intercooler_outlet_temperature"));
}

#[test]
// REQ-027.5
fn req_027_5_negative_activation_stays_off_and_never_offers_candidates() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    state.lock().unwrap().enter_reply = Reply::Payload(vec![0x7F, 0x10, 0x22]);
    clear_commands(&state);
    let result = run(&mut core, "uds.enable", "");
    assert_eq!(result["data"]["active"], false);
    assert!(result["data"]["error"]
        .as_str()
        .unwrap()
        .contains("7F 10 22"));
    assert_eq!(
        run(&mut core, "catalog", "")["data"]["uds"]["active"],
        false
    );
    assert!(!commands(&state)
        .iter()
        .any(|command| command.wire.starts_with("3E80")));
    assert!(!commands(&state)
        .iter()
        .any(|command| command.wire == "1001\r"));
}

#[test]
// REQ-024.4, REQ-027.5
fn req_024_4_malformed_offline_replay_does_not_touch_active_vehicle_session() {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    let enabled = run(&mut core, "uds.enable", "");
    assert_eq!(enabled["data"]["active"], true, "{enabled}");
    clear_commands(&state);

    let replay = run(&mut core, "replay.read", "{}");

    assert_eq!(replay["ok"], false, "{replay}");
    assert!(commands(&state).is_empty());
    assert_eq!(replay["uds"]["active"], true);
}
