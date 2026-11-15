//! REQ-041 acceptance: the semantic tool bridge through the mobile core with the scripted adapter. Every
//! rejected call is proved to send nothing: the fake adapter panics on commands it does not know and the
//! tests compare its command log.
use super::*;
use crate::{
    mobile::execute_nonblocking,
    semantic_tools::{Diagnosis, DIAGNOSIS_LIMIT},
};
use std::{fs, path::PathBuf};

const KNOWLEDGE_SIGNAL: &str = "air.charge_limit_overcharge";

/// A vehicle with the engine ECU identified and an empty command log.
fn ready() -> (MobileCore<FakeExchange>, Arc<Mutex<LinkState>>) {
    let (mut core, state, _) = setup(false, &[(0x7E0, Presence::Positive)]);
    discover(&mut core);
    clear_commands(&state);
    (core, state)
}

fn call(core: &mut MobileCore<FakeExchange>, request: Value) -> Value {
    let envelope = run(core, "assistant.tool", &request.to_string());
    assert_eq!(envelope["ok"], true, "{envelope}");
    envelope["data"].clone()
}

fn begin(core: &mut MobileCore<FakeExchange>) {
    assert_eq!(run(core, "assistant.begin", "")["ok"], true);
}

fn list(core: &mut MobileCore<FakeExchange>) -> Value {
    let listing = call(core, json!({"tool":"signal_list"}));
    assert_eq!(listing["outcome"], "ok", "{listing}");
    listing
}

fn id_of(listing: &Value, semantic: &str) -> String {
    listing["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["semantic"] == semantic)
        .unwrap_or_else(|| panic!("{semantic} not listed: {listing}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn read(core: &mut MobileCore<FakeExchange>, id: &str) -> Value {
    call(
        core,
        json!({"tool":"signal_read","arguments":{"signal":id}}),
    )
}

fn answer_knowledge_signal(state: &Arc<Mutex<LinkState>>, answers: usize) {
    state.lock().unwrap().did_replies.insert(
        0x1356,
        (0..answers)
            .map(|_| Reply::Payload(vec![0x62, 0x13, 0x56, 0x00, 0x64]))
            .collect(),
    );
}

fn keys(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                found.insert(key.clone());
                keys(value, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys(item, found)),
        _ => {}
    }
}

struct NoTransport;
impl Exchange for NoTransport {
    fn exchange(&mut self, command: &str, _: Duration) -> Result<String, String> {
        panic!("a tool without a session sent {command:?}")
    }
}

struct TempRecording(PathBuf);
impl TempRecording {
    fn new(name: &str, lines: &[String]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "obdentic-req041-{}-{name}.jsonl",
            std::process::id()
        ));
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        Self(path)
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for TempRecording {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn header() -> String {
    json!({"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live","profile":"engine-drive"})
        .to_string()
}

fn action(action: &str, offset_us: u64, data: Value) -> String {
    json!({"type":"action","action":action,"at":"2026-11-15T10:00:00Z","offset_us":offset_us,
        "result":{"ok":true,"data":data}})
    .to_string()
}

fn end(offset_us: u64) -> String {
    json!({"type":"session_end","reason":"stopped","detail":"","offset_us":offset_us}).to_string()
}

fn rpm_recording(name: &str) -> TempRecording {
    TempRecording::new(
        name,
        &[
            header(),
            action(
                "read",
                1_000,
                json!({"semantic":"engine.rpm","response":[65,12,26,248],
                    "observations":[{"selected_responder":"7E8","responses":[]}]}),
            ),
            action(
                "dtc.scan",
                2_000,
                json!({"raw_response":"7E8 03 43 01 0C\r>","pending":{"raw_response":"7E8 02 47 00\r>"}}),
            ),
            end(40_000_000),
        ],
    )
}

fn select(core: &mut MobileCore<FakeExchange>, recording: &TempRecording) {
    assert_eq!(
        run(core, "assistant.recording.select", recording.path())["ok"],
        true
    );
}

#[test]
// REQ-041.1
fn req_041_1_interface_has_five_tools_and_info_and_listing_send_nothing() {
    let (mut core, state) = ready();
    let definitions = run(&mut core, "assistant.tools", "");
    assert_eq!(definitions["ok"], true);
    assert_eq!(definitions["data"]["version"], 1);
    let names: Vec<_> = definitions["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "vehicle_info",
            "signal_list",
            "signal_read",
            "dtc_scan",
            "recording_inspect"
        ]
    );

    let info = call(&mut core, json!({"tool":"vehicle_info"}));
    assert_eq!(info["outcome"], "ok", "{info}");
    assert_eq!(info["session"]["state"], "ready");
    assert_eq!(info["engine_target_validated"]["value"], true);
    assert!(
        info.get("vin").is_none(),
        "vehicle info must not read a VIN"
    );
    let listing = list(&mut core);
    assert!(listing["count"].as_u64().unwrap() >= 2, "{listing}");
    id_of(&listing, "engine.rpm");
    id_of(&listing, KNOWLEDGE_SIGNAL);
    assert!(
        commands(&state).is_empty(),
        "info and listing sent {:?}",
        wires(&state)
    );
}

#[test]
// REQ-041.2
fn req_041_2_ids_are_opaque_and_the_read_accepts_nothing_else() {
    let (mut core, state) = ready();
    answer_knowledge_signal(&state, 1);
    begin(&mut core);
    let listing = list(&mut core);
    let rpm = id_of(&listing, "engine.rpm");
    let knowledge = id_of(&listing, KNOWLEDGE_SIGNAL);
    for id in listing["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["id"].as_str().unwrap())
    {
        let (epoch, index) = id.strip_prefix('s').unwrap().split_once('.').unwrap();
        assert!(epoch.parse::<u64>().is_ok() && index.parse::<usize>().is_ok());
    }
    let result = read(&mut core, &rpm);
    assert_eq!(result["outcome"], "ok", "{result}");
    assert_eq!(result["value"].as_f64(), Some(1726.0));
    assert_eq!(result["signal"]["id"], rpm.as_str());
    let result = read(&mut core, &knowledge);
    assert_eq!(result["outcome"], "ok", "{result}");
    assert_eq!(result["value"].as_f64(), Some(1.0));

    clear_commands(&state);
    for argument in [
        json!({"signal":"engine.rpm"}),
        json!({"signal":KNOWLEDGE_SIGNAL}),
        json!({"signal":"0C"}),
        json!({"signal":"s0.0"}),
    ] {
        let refused = call(
            &mut core,
            json!({"tool":"signal_read","arguments":argument}),
        );
        assert_eq!(refused["outcome"], "refused", "{argument}: {refused}");
    }
    for argument in [
        json!({"signal":rpm,"did":"F190"}),
        json!({"signal":rpm,"address":"7E0"}),
        json!({"signal":rpm,"request":"010C"}),
        json!({"signal":rpm,"payload":[1]}),
        json!({"signal":rpm,"rate_ms":1}),
        json!({"signal":rpm,"session":"extended"}),
    ] {
        let refused = call(
            &mut core,
            json!({"tool":"signal_read","arguments":argument}),
        );
        assert_eq!(refused["reason"], "unsafe", "{argument}: {refused}");
    }
    assert!(commands(&state).is_empty());
}

#[test]
// REQ-041.3
fn req_041_3_refusals_come_before_traffic_and_a_new_session_expires_ids() {
    let (mut core, state) = ready();
    begin(&mut core);
    let listing = list(&mut core);
    let rpm = id_of(&listing, "engine.rpm");
    clear_commands(&state);
    for (request, reason) in [
        (json!({"tool":"horn"}), "unknown_tool"),
        (json!({"tool":7}), "malformed_arguments"),
        (json!({"tool":"signal_read"}), "malformed_arguments"),
        (
            json!({"tool":"signal_read","arguments":{"signal":3}}),
            "malformed_arguments",
        ),
        (
            json!({"tool":"vehicle_info","arguments":{"verbose":true}}),
            "unexpected_argument",
        ),
        (
            json!({"tool":"signal_read","arguments":{"signal":"s9.9"}}),
            "expired_signal",
        ),
        (
            json!({"tool":"signal_read","arguments":{"signal":"engine.rpm"}}),
            "unknown_signal",
        ),
        (json!({"tool":"signal_list","version":2}), "unsupported"),
    ] {
        let refused = call(&mut core, request.clone());
        assert_eq!(refused["outcome"], "refused", "{request}: {refused}");
        assert_eq!(refused["reason"], reason, "{request}: {refused}");
    }
    let not_json = run(&mut core, "assistant.tool", "{not json");
    assert_eq!(not_json["data"]["reason"], "malformed_arguments");
    assert!(commands(&state).is_empty());

    // Re-initializing the adapter changes the session: the id issued before is expired, not remapped.
    assert_eq!(read(&mut core, &rpm)["outcome"], "ok");
    assert_eq!(run(&mut core, "initialize", "")["ok"], true);
    clear_commands(&state);
    let expired = read(&mut core, &rpm);
    assert_eq!(expired["outcome"], "refused", "{expired}");
    assert_eq!(expired["reason"], "expired_signal");
    assert!(commands(&state).is_empty());
    let again = id_of(&list(&mut core), "engine.rpm");
    assert_ne!(again, rpm);
    assert_eq!(read(&mut core, &again)["outcome"], "ok");
}

#[test]
// REQ-041.4
fn req_041_4_live_tools_send_exactly_what_the_users_own_actions_send() {
    let (mut core, state) = ready();
    answer_knowledge_signal(&state, 4);
    begin(&mut core);
    let listing = list(&mut core);
    // The commands (with their addressing) one action sends, once the addressing is warm.
    let sent = |core: &mut MobileCore<FakeExchange>,
                step: &dyn Fn(&mut MobileCore<FakeExchange>)| {
        clear_commands(&state);
        step(core);
        commands(&state)
            .into_iter()
            .map(|command| (command.wire, command.target))
            .collect::<Vec<_>>()
    };
    let user = |action: &'static str, argument: &'static str| {
        move |core: &mut MobileCore<FakeExchange>| {
            assert_eq!(run(core, action, argument)["ok"], true, "{action}");
        }
    };
    let tool = |request: Value| {
        move |core: &mut MobileCore<FakeExchange>| {
            assert_eq!(call(core, request.clone())["outcome"], "ok", "{request}");
        }
    };
    let tool_read = |id: String| tool(json!({"tool":"signal_read","arguments":{"signal":id}}));

    sent(&mut core, &user("read", "engine.rpm"));
    let user_rpm = sent(&mut core, &user("read", "engine.rpm"));
    assert!(!user_rpm.is_empty());
    let tool_rpm = sent(&mut core, &tool_read(id_of(&listing, "engine.rpm")));
    assert_eq!(tool_rpm, user_rpm);

    sent(&mut core, &user("read", KNOWLEDGE_SIGNAL));
    let user_did = sent(&mut core, &user("read", KNOWLEDGE_SIGNAL));
    assert!(!user_did.is_empty());
    let tool_did = sent(&mut core, &tool_read(id_of(&listing, KNOWLEDGE_SIGNAL)));
    assert_eq!(tool_did, user_did);

    sent(&mut core, &user("dtc.scan", ""));
    let user_scan = sent(&mut core, &user("dtc.scan", ""));
    let tool_scan = sent(&mut core, &tool(json!({"tool":"dtc_scan"})));
    assert!(user_scan.iter().any(|(wire, _)| wire == "03\r"));
    assert_eq!(tool_scan, user_scan);
    for (wire, _) in user_rpm.iter().chain(&user_did).chain(&tool_scan) {
        // Read-only: never a clear (14), a write (2E/2F), a security access (27) or a routine (31).
        assert!(
            !["14", "27", "2E", "2F", "31"]
                .iter()
                .any(|service| wire.starts_with(service)),
            "{wire}"
        );
    }
}

#[test]
// REQ-041.5
fn req_041_5_a_tool_that_would_end_the_restricted_session_is_refused_before_traffic() {
    let (mut core, state) = active_session(3);
    begin(&mut core);
    let listing = list(&mut core);
    let session_id = id_of(&listing, SESSION_SIGNAL);
    clear_commands(&state);

    let scan = call(&mut core, json!({"tool":"dtc_scan"}));
    assert_eq!(scan["outcome"], "refused", "{scan}");
    assert_eq!(scan["reason"], "session_change");
    assert!(commands(&state).is_empty());
    assert!(core.uds.is_active());

    // Reads the session itself carries stay possible and leave it active.
    assert_eq!(read(&mut core, &session_id)["outcome"], "ok");
    assert_eq!(
        read(&mut core, &id_of(&listing, "engine.rpm"))["outcome"],
        "ok"
    );
    assert!(core.uds.is_active());
    assert!(!wires(&state).iter().any(|wire| wire == "1001\r"));

    // After an error the existing safety cleanup still returns to the default session.
    state
        .lock()
        .unwrap()
        .mode01_replies
        .push_back(Reply::NoData);
    let listing = list(&mut core);
    let failed = read(&mut core, &id_of(&listing, "engine.rpm"));
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert!(!core.uds.is_active());
    assert!(wires(&state).iter().any(|wire| wire == "1001\r"));
}

#[test]
// REQ-041.5
fn req_041_5_a_standard_read_without_a_usable_answer_ends_the_extended_session_like_a_user_read() {
    // The transport rejects malformed Mode 01 answers before decoding, so a decoded standard read that
    // is not answered is driven through the result seam with the same inputs the decoder produces.
    for response in [vec![0x41, 0x0C, 0x1A], vec![0x41, 0x0C, 0x1A, 0xF8, 0x00]] {
        let (mut core, state) = active_session(3);
        let listed = crate::mobile::assistant::Listed {
            key: "engine.rpm".into(),
            semantic: "engine.rpm".into(),
            info: json!({"source":"standard","ecu_role":null,"validation":null}),
        };
        let outcome = crate::signal_read::decode(
            "engine.rpm",
            crate::signal_read::Decoder::Standard {
                request: crate::read::prepare_read("engine.rpm").unwrap(),
                source: "user",
            },
            crate::signal_read::Evidence {
                response: Some(response),
                error: None,
            },
        );
        assert!(
            matches!(
                outcome.state,
                crate::signal_read::OutcomeState::NotAnswered { .. }
            ),
            "{outcome:?}"
        );
        assert!(core.uds.is_active());
        clear_commands(&state);

        let failed = core.answer_read("sig-1", &listed, Ok((outcome, serde_json::Map::new())));
        assert_eq!(failed["outcome"], "failed", "{failed}");
        assert_eq!(failed["reason"], "not_answered", "{failed}");
        assert!(!core.uds.is_active(), "the extended session stayed active");
        assert!(wires(&state).iter().any(|wire| wire == "1001\r"));
    }
}

#[test]
// REQ-041.5
fn req_041_5_a_read_outside_the_active_session_target_is_refused_without_ending_the_session() {
    let (mut core, state) = ready();
    begin(&mut core);
    let rpm = id_of(&list(&mut core), "engine.rpm");
    // A session on another request address than the validated engine target (as in REQ-027.6).
    let definition = session_definition(&core);
    let other = RequestTarget::concrete(
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical),
        RequestAddress::new("elm-header", "7E1"),
    );
    let responder = crate::elm::ResponderIdentity::ElmHeader("7E8".into());
    let _ = core.runtime.block_on(core.uds.enable(
        core.session.as_mut().unwrap(),
        other,
        responder,
        &[&definition],
        Instant::now(),
    ));
    assert!(core.uds.is_active() && !core.uds_matches_validated_engine());
    clear_commands(&state);

    let refused = read(&mut core, &rpm);
    assert_eq!(refused["outcome"], "refused", "{refused}");
    assert_eq!(refused["reason"], "session_change");
    assert!(commands(&state).is_empty(), "{:?}", wires(&state));
    assert!(core.uds.is_active());
}

#[test]
// REQ-041.6
fn req_041_6_one_diagnosis_has_20_reads_one_scan_60_seconds_and_cancellation() {
    let (mut core, state) = ready();
    let rpm = id_of(&list(&mut core), "engine.rpm");
    let none = call(&mut core, json!({"tool":"dtc_scan"}));
    assert_eq!(none["reason"], "no_diagnosis", "{none}");
    assert!(commands(&state).is_empty());

    begin(&mut core);
    for _ in 0..20 {
        assert_eq!(read(&mut core, &rpm)["outcome"], "ok");
    }
    clear_commands(&state);
    let over = read(&mut core, &rpm);
    assert_eq!(over["outcome"], "budget_exhausted", "{over}");
    assert_eq!(over["reason"], "read_limit");
    assert!(commands(&state).is_empty());
    assert_eq!(call(&mut core, json!({"tool":"dtc_scan"}))["outcome"], "ok");
    clear_commands(&state);
    let second = call(&mut core, json!({"tool":"dtc_scan"}));
    assert_eq!(second["outcome"], "budget_exhausted", "{second}");
    assert_eq!(second["reason"], "dtc_limit");
    assert!(commands(&state).is_empty());

    // After 60 s from the start no further live work starts.
    begin(&mut core);
    core.assistant.diagnosis = Some(Diagnosis::start(
        Instant::now() - DIAGNOSIS_LIMIT - Duration::from_secs(1),
    ));
    let late = read(&mut core, &rpm);
    assert_eq!(late["outcome"], "budget_exhausted", "{late}");
    assert_eq!(late["reason"], "time_limit");
    assert!(commands(&state).is_empty());

    // Cancellation from any thread: no further live work, and a new diagnosis starts clean.
    begin(&mut core);
    core.cancel_handle().cancel();
    let cancelled = read(&mut core, &rpm);
    assert_eq!(cancelled["outcome"], "cancelled", "{cancelled}");
    assert!(commands(&state).is_empty());
    begin(&mut core);
    assert_eq!(read(&mut core, &rpm)["outcome"], "ok");
}

#[test]
// REQ-041.6
fn req_041_6_cancelling_during_the_scan_lets_the_running_exchange_finish_and_starts_nothing_more() {
    let (mut core, state) = ready();
    begin(&mut core);
    let handle = core.cancel_handle();
    state.lock().unwrap().on_command = Some(("03\r".into(), Box::new(move || handle.cancel())));
    let scan = call(&mut core, json!({"tool":"dtc_scan"}));
    assert_eq!(scan["outcome"], "ok", "{scan}");
    assert_eq!(scan["pending"]["skipped"], "cancelled");
    let log = wires(&state);
    assert!(log.iter().any(|wire| wire == "03\r"));
    assert!(!log.iter().any(|wire| wire == "07\r"), "{log:?}");
}

#[test]
// REQ-041.7
fn req_041_7_disconnected_busy_and_unavailable_start_no_work() {
    let (mut core, state) = ready();
    answer_knowledge_signal(&state, 1);
    begin(&mut core);
    let listing = list(&mut core);
    let rpm = id_of(&listing, "engine.rpm");
    let knowledge = id_of(&listing, KNOWLEDGE_SIGNAL);
    clear_commands(&state);

    // A signal that is no longer readable is unavailable, never another signal.
    core.assistant.measured.insert(
        format!("{KNOWLEDGE_SIGNAL}@vag.ecu_target.7e0"),
        "rejected".into(),
    );
    let unavailable = read(&mut core, &knowledge);
    assert_eq!(unavailable["outcome"], "unavailable", "{unavailable}");
    core.assistant.measured.clear();

    // Busy: the session owner runs something else, the call does not queue and sends nothing.
    let shared = Mutex::new(core);
    {
        let owner = shared.lock().unwrap();
        let busy: Value = serde_json::from_str(&execute_nonblocking(
            &shared,
            "assistant.tool",
            &json!({"tool":"signal_read","arguments":{"signal":rpm}}).to_string(),
        ))
        .unwrap();
        assert_eq!(busy["ok"], true);
        assert_eq!(busy["data"]["outcome"], "busy", "{busy}");
        assert_eq!(busy["data"]["tool"], "signal_read");
        drop(owner);
    }
    assert!(commands(&state).is_empty());
    let mut core = shared.into_inner().unwrap();

    // Disconnected: every tool that needs the vehicle says so.
    core.on_disconnect();
    for request in [
        json!({"tool":"vehicle_info"}),
        json!({"tool":"signal_list"}),
        json!({"tool":"signal_read","arguments":{"signal":rpm}}),
        json!({"tool":"dtc_scan"}),
    ] {
        let result = call(&mut core, request.clone());
        assert_eq!(result["outcome"], "disconnected", "{request}: {result}");
    }
    assert!(commands(&state).is_empty());
    let none = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(none["outcome"], "unavailable", "{none}");
    assert_eq!(none["reason"], "no_recording_selected");
}

#[test]
// REQ-041.7
fn req_041_7_busy_begin_and_recording_selection_are_errors_that_change_nothing() {
    let (mut core, state) = ready();
    let old = rpm_recording("busy-old");
    let new = TempRecording::new("busy-new", &[header(), end(1_000)]);
    select(&mut core, &old);
    clear_commands(&state);

    let shared = Mutex::new(core);
    {
        let owner = shared.lock().unwrap();
        for (action, argument) in [
            ("assistant.begin", ""),
            ("assistant.recording.select", new.path()),
            ("assistant.recording.select", ""),
        ] {
            let busy: Value =
                serde_json::from_str(&execute_nonblocking(&shared, action, argument)).unwrap();
            assert_eq!(busy["ok"], false, "{action}: {busy}");
            assert!(
                busy["error"].as_str().unwrap().contains("busy"),
                "{action}: {busy}"
            );
        }
        drop(owner);
    }
    assert!(commands(&state).is_empty());
    let mut core = shared.into_inner().unwrap();

    // No diagnosis began and the earlier selection still stands: the tool reads the old recording.
    let dtc = call(&mut core, json!({"tool":"dtc_scan"}));
    assert_eq!(dtc["reason"], "no_diagnosis", "{dtc}");
    let inspected = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(inspected["signals"][0]["key"], "engine.rpm", "{inspected}");
    let free = run(&mut core, "assistant.recording.select", new.path());
    assert_eq!(free["ok"], true, "{free}");
}

#[test]
// REQ-041.7
fn req_041_7_the_availability_measurement_decides_which_signals_are_listed() {
    let (mut core, state) = ready();
    answer_knowledge_signal(&state, 1);
    let before = id_of(&list(&mut core), KNOWLEDGE_SIGNAL);
    assert_eq!(run(&mut core, "knowledge.availability", "")["ok"], true);
    clear_commands(&state);

    let listing = list(&mut core);
    let signals = listing["signals"].as_array().unwrap();
    let answered: Vec<_> = signals
        .iter()
        .filter(|signal| signal["source"] == "knowledge")
        .collect();
    assert_eq!(
        answered.len(),
        1,
        "silent signals are not listed: {listing}"
    );
    assert_eq!(answered[0]["semantic"], KNOWLEDGE_SIGNAL);
    assert_eq!(answered[0]["availability"], "answered");
    assert!(signals
        .iter()
        .any(|signal| signal["semantic"] == "engine.rpm"));
    // The measurement changed what exists: the id of the earlier listing is expired.
    begin(&mut core);
    let old = read(&mut core, &before);
    assert_eq!(old["reason"], "expired_signal", "{old}");
    assert!(commands(&state).is_empty());
}

#[test]
// REQ-041.8
fn req_041_8_results_keep_provenance_and_name_failures_instead_of_zero() {
    let (mut core, state) = ready();
    begin(&mut core);
    let rpm = id_of(&list(&mut core), "engine.rpm");
    let result = read(&mut core, &rpm);
    let provenance = &result["provenance"];
    assert_eq!(provenance["evidence"], "decoded");
    assert_eq!(provenance["source"], "live");
    assert_eq!(provenance["responder"], "7E8");
    assert!(provenance["timestamp_ms"].as_u64().unwrap() > 0);
    assert_eq!(provenance["freshness"], "fresh");
    assert!(provenance["capture"].is_null());

    state
        .lock()
        .unwrap()
        .mode01_replies
        .push_back(Reply::NoData);
    let failed = read(&mut core, &rpm);
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert!(failed.get("value").is_none(), "no value for a failed read");

    // What is inferred or unknown says so.
    let info = call(&mut core, json!({"tool":"vehicle_info"}));
    assert_eq!(info["manufacturer"]["evidence"], "inferred");
    assert_eq!(info["responders"]["evidence"], "observed");
    let (mut bare, _, _) = setup(false, &[]);
    let info = call(&mut bare, json!({"tool":"vehicle_info"}));
    assert_eq!(info["knowledge"]["evidence"], "unknown");

    // A recorded value is stale against the end of its recording.
    let recording = rpm_recording("stale");
    select(&mut core, &recording);
    let inspected = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(inspected["outcome"], "ok", "{inspected}");
    let signal = &inspected["signals"][0];
    assert_eq!(signal["provenance"]["evidence"], "decoded");
    assert_eq!(signal["provenance"]["source"], "recording");
    assert_eq!(signal["provenance"]["freshness"], "stale");
    assert_eq!(signal["provenance"]["responder"], "7E8");
    assert_eq!(signal["provenance"]["capture"]["offset_us"], 1_000);
    assert_eq!(signal["value"].as_f64(), Some(1726.0));
}

#[test]
// REQ-041.9
fn req_041_9_results_exclude_vins_serials_adapter_ids_and_raw_recordings() {
    let (mut core, _) = ready();
    begin(&mut core);
    let recording = rpm_recording("private");
    select(&mut core, &recording);
    let listing = list(&mut core);
    let rpm = id_of(&listing, "engine.rpm");
    // ECU identification (serial and part numbers) is never offered as a signal.
    assert!(listing["signals"]
        .as_array()
        .unwrap()
        .iter()
        .all(|signal| !signal["semantic"].as_str().unwrap().starts_with("ecu.")));
    let results = [
        call(&mut core, json!({"tool":"vehicle_info"})),
        listing,
        read(&mut core, &rpm),
        call(&mut core, json!({"tool":"dtc_scan"})),
        call(&mut core, json!({"tool":"recording_inspect"})),
    ];
    for result in &results {
        assert_eq!(result["outcome"], "ok", "{result}");
        let mut found = BTreeSet::new();
        keys(result, &mut found);
        for forbidden in [
            "vin",
            "raw_response",
            "response",
            "responses",
            "observations_raw",
            "serial",
            "adapter",
            "adapter_id",
        ] {
            assert!(!found.contains(forbidden), "{forbidden} in {result}");
        }
        let text = result.to_string();
        assert!(!text.contains("43 01 0C"), "raw DTC frame in {text}");
    }
}

#[test]
// REQ-041.9
fn req_041_9_recording_inspection_withholds_recorded_ecu_identification() {
    let (mut core, _) = ready();
    let catalog = KnowledgeCatalog::embedded_pinned().unwrap();
    let identification = "ecu.manufacturer_spare_part_number";
    let key = format!("{identification}@vag.ecu_target.7e0");
    let mut payload = vec![0x62, 0xF1, 0x87];
    payload.extend_from_slice(b"1K0907115XY");
    assert_eq!(payload[3..7], *b"1K09");
    let recording = TempRecording::new(
        "identification",
        &[
            header(),
            action(
                "read",
                1_000,
                json!({"knowledge_repository":catalog.pin().repository(),
                    "knowledge_revision":catalog.pin().revision(),
                    "semantic":identification,
                    "definition_id":"uds.f187.manufacturer_spare_part_number",
                    "target_id":"vag.ecu_target.7e0","request":[0x22,0xF1,0x87],
                    "response":payload,
                    "observations":[{"selected_responder":"7E8","responses":[
                        {"responder":"7E8","payload":payload}]}]}),
            ),
            action(
                "read",
                2_000,
                json!({"semantic":"engine.rpm","response":[65,12,26,248],
                    "observations":[{"selected_responder":"7E8","responses":[]}]}),
            ),
            action(
                "knowledge.availability",
                3_000,
                json!({"knowledge_repository":catalog.pin().repository(),
                    "knowledge_revision":catalog.pin().revision(),
                    "entries":[
                        {"key":key,"target":"vag.ecu_target.7e0",
                         "definition_id":"uds.f187.manufacturer_spare_part_number","response":payload},
                        {"key":"engine.rpm","target":null,"response":[65,12,26,248]}]}),
            ),
            end(40_000_000),
        ],
    );
    select(&mut core, &recording);
    let inspected = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(inspected["outcome"], "ok", "{inspected}");
    // The recording does hold the identification, so the filter is what keeps it out.
    let report = crate::mobile::report::report(&PathBuf::from(recording.path())).unwrap();
    assert!(report["last_values"].get(&key).is_some(), "{report}");
    assert_eq!(
        report["diagnostic_jobs"][0]["table"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    assert_eq!(
        inspected["signals"].as_array().unwrap().len(),
        1,
        "{inspected}"
    );
    assert_eq!(inspected["signals"][0]["key"], "engine.rpm");
    let entries = inspected["availability"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{inspected}");
    assert_eq!(entries[0]["key"], "engine.rpm");
    assert_eq!(
        inspected["availability"]["counts"]["answered"], 1,
        "{inspected}"
    );
    let text = inspected.to_string();
    // The identification value is bytes of text: "1K09" in decimal.
    assert!(!text.contains("49,75,48,57"), "{text}");
    assert!(!text.contains(identification), "{text}");
}

#[test]
// REQ-041.10
fn req_041_10_recording_inspection_is_offline_and_only_for_the_selected_recording() {
    let (mut core, state) = ready();
    let recording = rpm_recording("selected");
    let none = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(none["reason"], "no_recording_selected");
    // The model cannot choose a file.
    let chosen = call(
        &mut core,
        json!({"tool":"recording_inspect","arguments":{"path":recording.path()}}),
    );
    assert_eq!(chosen["outcome"], "refused", "{chosen}");
    let missing = run(
        &mut core,
        "assistant.recording.select",
        "/nonexistent/obdentic.jsonl",
    );
    assert_eq!(missing["ok"], false);

    select(&mut core, &recording);
    clear_commands(&state);
    let inspected = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(inspected["outcome"], "ok", "{inspected}");
    assert_eq!(inspected["signals"][0]["key"], "engine.rpm");
    assert_eq!(inspected["recording"]["complete"], true);
    let scan = &inspected["dtc_scan"];
    assert_eq!(scan["stored"][0]["codes"][0], "P010C", "{inspected}");
    assert_eq!(scan["pending"][0]["no_dtcs"], true);
    assert!(commands(&state).is_empty(), "replay touched the adapter");
}

#[test]
// REQ-041.10
fn req_041_10_a_knowledge_pin_mismatch_stops_the_inspection_with_an_explicit_error() {
    let (mut core, _) = ready();
    let recording = TempRecording::new(
        "pin",
        &[
            header(),
            action(
                "knowledge.availability",
                1_000,
                json!({"knowledge_repository":"example/other","knowledge_revision":"0000000","entries":[]}),
            ),
            end(2_000),
        ],
    );
    select(&mut core, &recording);
    let result = call(&mut core, json!({"tool":"recording_inspect"}));
    assert_eq!(result["outcome"], "failed", "{result}");
    assert_eq!(result["reason"], "knowledge_pin_mismatch");
    assert!(result.get("signals").is_none());
}

#[test]
// REQ-041.11
fn req_041_11_no_raw_write_session_or_addressing_operation_can_be_requested() {
    let (mut core, state) = ready();
    begin(&mut core);
    let rpm = id_of(&list(&mut core), "engine.rpm");
    clear_commands(&state);
    for request in [
        json!({"tool":"raw_command","arguments":{"command":"ATZ"}}),
        json!({"tool":"uds_read_did","arguments":{"did":"F190"}}),
        json!({"tool":"clear_dtc"}),
        json!({"tool":"write_memory"}),
        json!({"tool":"security_access"}),
        json!({"tool":"routine_control"}),
        json!({"tool":"coding"}),
        json!({"tool":"adaptation"}),
        json!({"tool":"actuator_test"}),
        json!({"tool":"enable_uds_session"}),
        json!({"tool":"dtc_scan","arguments":{"clear":true}}),
        json!({"tool":"signal_read","arguments":{"signal":rpm,"command":"2E"}}),
    ] {
        let refused = call(&mut core, request.clone());
        assert_eq!(refused["outcome"], "refused", "{request}: {refused}");
        assert_eq!(refused["reason"], "unsafe", "{request}: {refused}");
    }
    // Not even the platform-side actions of the closed core can be reached through the tool channel.
    for action in ["assistant.raw", "assistant.uds.enable", "assistant.write"] {
        assert_eq!(run(&mut core, action, "x")["ok"], false, "{action}");
    }
    assert!(commands(&state).is_empty());
    let definitions = run(&mut core, "assistant.tools", "")["data"].to_string();
    for word in ["raw", "write", "clear", "security", "routine"] {
        assert!(
            !definitions.contains(word),
            "{word} in the tool definitions"
        );
    }
}

#[test]
// REQ-041.12
fn req_041_12_the_bridge_needs_no_model_account_or_network_and_has_no_snapshot_tool() {
    let mut core = MobileCore::new(NoTransport).unwrap();
    assert_eq!(run(&mut core, "assistant.tools", "")["ok"], true);
    let info = run(
        &mut core,
        "assistant.tool",
        &json!({"tool":"vehicle_info"}).to_string(),
    );
    assert_eq!(info["data"]["outcome"], "disconnected");
    let snapshot = run(
        &mut core,
        "assistant.tool",
        &json!({"tool":"snapshot"}).to_string(),
    );
    assert_eq!(snapshot["data"]["reason"], "unknown_tool");

    // Recording inspection works with no session at all.
    let recording = rpm_recording("offline");
    assert_eq!(
        run(&mut core, "assistant.recording.select", recording.path())["ok"],
        true
    );
    let inspected = run(
        &mut core,
        "assistant.tool",
        &json!({"tool":"recording_inspect"}).to_string(),
    );
    assert_eq!(inspected["data"]["outcome"], "ok", "{inspected}");
}
