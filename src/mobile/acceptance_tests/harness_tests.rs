//! REQ-046 acceptance: deterministic scenarios on the virtual adapter and the real `MobileCore` / assistant policy.
//! Everything is synthetic, offline and independent of BLE, accounts, models and recordings.
use super::{
    harness_transport::{self as vt, Trace, TraceProbe, VirtualAdapter},
    run,
};
use crate::mobile::MobileCore;
use serde_json::{json, Value};
use std::time::Instant;

fn call(core: &mut MobileCore<VirtualAdapter>, request: Value) -> Value {
    let envelope = run(core, "assistant.tool", &request.to_string());
    assert_eq!(envelope["ok"], true, "{envelope}");
    envelope["data"].clone()
}

fn id_of(listing: &Value, semantic: &str) -> Option<String> {
    listing["signals"]
        .as_array()?
        .iter()
        .find(|signal| signal["semantic"] == semantic)
        .and_then(|signal| signal["id"].as_str().map(str::to_owned))
}

fn read(core: &mut MobileCore<VirtualAdapter>, id: &str) -> Value {
    call(
        core,
        json!({"tool":"signal_read","arguments":{"signal":id}}),
    )
}

/// The scripted agent of REQ-046.20: list, read RPM and MAF, scan DTCs; EGR is not advertised.
struct Run {
    trace: Vec<Trace>,
    /// Index of the first exchange of the diagnosis itself (after boot, inventory and discovery).
    diagnosis_from: usize,
    outcomes: Vec<Value>,
    listing: Value,
    egr_request: Value,
    egr_tx: usize,
}

const KNOWLEDGE: &str = "air.charge_limit_overcharge";

fn scenario() -> Run {
    let (mut core, probe) = vt::fixture();
    vt::establish(&mut core);
    let diagnosis_from = probe.len();
    assert_eq!(run(&mut core, "assistant.tools", "")["ok"], true);
    assert_eq!(run(&mut core, "assistant.begin", "")["ok"], true);
    let listing = call(&mut core, json!({"tool":"signal_list"}));
    let rpm = id_of(&listing, "engine.rpm").expect("RPM is advertised");
    let maf = id_of(&listing, "engine.maf").expect("MAF is advertised");
    let knowledge = id_of(&listing, KNOWLEDGE).expect("approved Knowledge signal is listed");
    let mut outcomes = vec![read(&mut core, &rpm)];
    // Evidence-driven follow-up: a stored EGR code makes the agent request the airflow and a Knowledge fact.
    let scan = call(&mut core, json!({"tool":"dtc_scan"}));
    let egr_finding = scan.to_string().contains("P0401");
    outcomes.push(scan);
    assert!(
        egr_finding,
        "the synthetic EGR code is the finding that drives the follow-up"
    );
    outcomes.push(read(&mut core, &maf));
    outcomes.push(read(&mut core, &knowledge));
    // The EGR position itself is not advertised: the scripted agent asks and must get an unknown outcome.
    let before = probe.len();
    let egr_request = call(
        &mut core,
        json!({"tool":"signal_read","arguments":{"signal":"engine.egr.commanded"}}),
    );
    let egr_tx = probe.len() - before;
    Run {
        trace: probe.transactions(),
        diagnosis_from,
        outcomes,
        listing,
        egr_request,
        egr_tx,
    }
}

fn comparable(run: &Run) -> Vec<(String, u64, u64, Result<String, String>)> {
    run.trace
        .iter()
        .map(|t| (t.wire.clone(), t.started_ms, t.ended_ms, t.result.clone()))
        .collect()
}

fn values(run: &Run) -> Vec<Value> {
    run.outcomes
        .iter()
        .map(|outcome| {
            // Wall-clock provenance and the process-wide signal counter are the only non-virtual fields.
            let mut outcome = outcome.clone();
            if let Some(signal) = outcome.get_mut("signal") {
                signal["id"] = Value::Null;
            }
            if let Some(provenance) = outcome.get_mut("provenance") {
                for field in ["timestamp_ms", "age_ms"] {
                    provenance[field] = Value::Null;
                }
            }
            outcome
        })
        .collect()
}

#[test]
// REQ-046.1, REQ-046.3, REQ-046.20
fn req_046_1_3_20_the_same_scenario_gives_the_same_trace_and_tool_results() {
    let first = scenario();
    let second = scenario();
    assert_eq!(comparable(&first), comparable(&second));
    assert_eq!(values(&first), values(&second));

    // REQ-046.3: the exact TX order of the diagnosis and the matching tool references.
    let wires: Vec<&str> = first.trace[first.diagnosis_from..]
        .iter()
        .map(|t| t.wire.as_str())
        .collect();
    assert_eq!(wires, EXPECTED_DIAGNOSIS_TX);
    assert_eq!(first.outcomes[0]["value"].as_f64(), Some(1726.0));
    assert_eq!(first.outcomes[0]["provenance"]["responder"], "7E8");
    assert!(first.outcomes[1].to_string().contains("P0401"));
    assert_eq!(
        first.outcomes[1]["stored"]["observations"][0]["responder"],
        "7E8"
    );
    assert_eq!(first.outcomes[2]["value"].as_f64(), Some(10.0));
    assert_eq!(first.outcomes[3]["outcome"], "ok");
    // REQ-046.21: the missing EGR fact is a typed unknown outcome, not a value, and costs no traffic.
    assert_eq!(
        first.egr_request["outcome"], "refused",
        "{}",
        first.egr_request
    );
    assert_eq!(first.egr_request["reason"], "unknown_signal");
    assert!(first.egr_request.get("value").is_none());
    assert_eq!(first.egr_tx, 0);
    // Virtual time never moves backwards; each exchange ends no earlier than it starts.
    assert!(first.trace.iter().all(|t| t.ended_ms >= t.started_ms));
}

/// Targeted RPM, the re-initialization before a functional scan, stored and pending DTCs, then the targeted MAF
/// and Knowledge reads.
const EXPECTED_DIAGNOSIS_TX: [&str; 16] = [
    "ATSH 7E0\r",
    "ATCRA 7E8\r",
    "010C\r",
    "ATZ\r",
    "ATE0\r",
    "ATL0\r",
    "ATS1\r",
    "ATH1\r",
    "ATSP0\r",
    "0100\r",
    "03\r",
    "07\r",
    "ATSH 7E0\r",
    "ATCRA 7E8\r",
    "0110\r",
    "221356\r",
];

#[test]
// REQ-046.2, REQ-046.3
fn req_046_2_3_local_listing_adds_no_tx_and_one_request_is_outstanding_at_a_time() {
    let (mut core, probe) = vt::fixture();
    vt::establish(&mut core);
    assert_eq!(run(&mut core, "assistant.begin", "")["ok"], true);
    let mark = probe.len();
    call(&mut core, json!({"tool":"vehicle_info"}));
    let listing = call(&mut core, json!({"tool":"signal_list"}));
    assert_eq!(probe.len(), mark, "info and listing send nothing");
    let rpm = id_of(&listing, "engine.rpm").unwrap();
    read(&mut core, &rpm);
    assert!(probe.len() > mark);
    assert!(probe.peak_outstanding() <= 1);
}

#[test]
// REQ-046.4
fn req_046_4_an_unadvertised_pid_is_unsupported_and_sends_no_value() {
    let (mut core, probe) = vt::fixture();
    vt::establish(&mut core);
    let listing = call(&mut core, json!({"tool":"signal_list"}));
    assert!(id_of(&listing, "vehicle.speed").is_none());
    let mark = probe.len();
    let result = run(&mut core, "read", "vehicle.speed");
    assert_eq!(result["ok"], false, "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("does not advertise support"),
        "{result}"
    );
    assert!(result.get("value").is_none());
    assert_eq!(probe.len(), mark, "unsupported means no traffic");
}

#[test]
// REQ-046.21
fn req_046_21_a_fact_the_vehicle_does_not_advertise_stays_unknown() {
    let result = scenario();
    for semantic in ["engine.egr.commanded", "engine.egr.error"] {
        assert!(
            id_of(&result.listing, semantic).is_none(),
            "{semantic} must not be listed"
        );
    }
    assert_eq!(result.egr_request["outcome"], "refused");
    assert!(result.trace.iter().all(|t| t.wire != "012C\r"));
}

#[test]
// REQ-046.22
fn req_046_22_raw_unknown_and_mutating_requests_are_refused_with_zero_tx() {
    let (mut core, probe) = vt::fixture();
    vt::establish(&mut core);
    assert_eq!(run(&mut core, "assistant.begin", "")["ok"], true);
    let mark = probe.len();
    for request in [
        json!({"tool":"raw_send","arguments":{"command":"0100"}}),
        json!({"tool":"signal_read","arguments":{"signal":"22F190"}}),
        json!({"tool":"signal_read","arguments":{"signal":"engine.rpm","pid":"0C"}}),
        json!({"tool":"dtc_clear"}),
        json!({"tool":"uds_write","arguments":{"did":"F190","data":"00"}}),
    ] {
        let outcome = call(&mut core, request.clone());
        assert_eq!(outcome["outcome"], "refused", "{request}: {outcome}");
    }
    assert_eq!(probe.len(), mark, "refusals never reach the adapter");
}

#[test]
// REQ-046.5–REQ-046.18
fn req_046_5_to_18_adverse_adapter_scenarios() {
    vt::run_fault_cases();
}

#[test]
// REQ-046.24 (REQ-046.19 is the 35-case classification matrix in docs/android.md, a documentation deliverable)
fn req_046_24_the_harness_suite_runs_within_ten_seconds() {
    // The budget covers the scenario and fault runs together, without compilation.
    let started = Instant::now();
    let _ = scenario();
    let _ = scenario();
    vt::run_fault_cases();
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 10, "{elapsed:?}");
}

#[allow(dead_code)]
fn _probe_type(_: &TraceProbe) {}
