use std::{fs, process::Command};

// REQ-045.12, REQ-045.19: an interrupted audit is inspectable without an agent or vehicle.
#[test]
fn interrupted_session_is_reported_as_incomplete() {
    let path = std::env::temp_dir().join(format!("obdentic-audit-{}.jsonl", std::process::id()));
    fs::write(&path, concat!(
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":1,\"wall_ms\":1,\"mono_ns\":1,\"type\":\"session_start\",\"data\":{}}\n",
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":2,\"wall_ms\":2,\"mono_ns\":2,\"type\":\"request_intent\",\"request_id\":\"r1\",\"data\":{}}\n"
    )).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_obdentic-mobile-report"))
        .arg("--agent-audit")
        .arg(&path)
        .output()
        .unwrap();
    let _ = fs::remove_file(path);
    assert!(result.status.success(), "offline audit inspection failed");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["audit_status"], "incomplete");
    assert_eq!(report["last_evidenced_action"], "request_intent");
    assert_eq!(report["dispatch_status"], "unknown");
}

// REQ-045.12, REQ-045.19: a recorded dispatch remains evidenced even if its response is interrupted.
#[test]
fn dispatched_request_is_not_reported_as_unknown() {
    let path =
        std::env::temp_dir().join(format!("obdentic-dispatched-{}.jsonl", std::process::id()));
    fs::write(&path, concat!(
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":1,\"wall_ms\":1,\"mono_ns\":1,\"type\":\"session_start\",\"data\":{}}\n",
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":2,\"wall_ms\":2,\"mono_ns\":2,\"type\":\"request_intent\",\"request_id\":\"r1\",\"data\":{}}\n",
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":3,\"wall_ms\":3,\"mono_ns\":3,\"type\":\"request_dispatched\",\"request_id\":\"r1\",\"data\":{}}\n"
    )).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_obdentic-mobile-report"))
        .arg("--agent-audit")
        .arg(&path)
        .output()
        .unwrap();
    let _ = fs::remove_file(path);
    assert!(result.status.success(), "offline audit inspection failed");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["audit_status"], "incomplete");
    assert_eq!(report["dispatch_status"], "dispatched");
}

fn event(seq: u64, kind: &str, extra: &str) -> String {
    format!("{{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":{seq},\"wall_ms\":{seq},\"mono_ns\":{seq},\"type\":\"{kind}\"{extra},\"data\":{{}}}}\n")
}

fn report(name: &str, text: &str) -> serde_json::Value {
    let path = std::env::temp_dir().join(format!(
        "obdentic-audit-{name}-{}.jsonl",
        std::process::id()
    ));
    fs::write(&path, text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_obdentic-mobile-report"))
        .arg("--agent-audit")
        .arg(&path)
        .output()
        .unwrap();
    let _ = fs::remove_file(path);
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

// REQ-045.19: a complete audit is distinguished from the diagnosis outcome.
#[test]
fn complete_audit_reports_the_diagnosis_outcome_separately() {
    let end = event(2, "session_end", "")
        .replace("\"data\":{}", "\"data\":{\"diagnosis_status\":\"failed\"}");
    let r = report("complete", &(event(1, "session_start", "") + &end));
    assert_eq!(r["audit_status"], "complete");
    assert_eq!(r["diagnosis_status"], "failed");
}

// REQ-045.12: a sequence gap or a torn line is never complete.
#[test]
fn sequence_gap_and_torn_line_are_incomplete() {
    let gap = report(
        "gap",
        &(event(1, "session_start", "") + &event(3, "session_end", "")),
    );
    assert_eq!(gap["audit_status"], "incomplete");
    let torn = report(
        "torn",
        &(event(1, "session_start", "") + "{\"schema\":\"obdentic-agent-au"),
    );
    assert_eq!(torn["audit_status"], "incomplete");
    assert_eq!(torn["last_evidenced_action"], "session_start");
}

// REQ-045.20: a missing linked recording stays unresolved and the transcript stays readable.
#[test]
fn missing_recording_reference_is_unresolved_but_readable() {
    let tool = event(2, "tool_end", ",\"tool_call_id\":\"c1\"").replace(
        "\"data\":{}",
        "\"data\":{\"recording_ref\":\"missing.jsonl\"}",
    );
    let intent = event(2, "tool_intent", ",\"tool_call_id\":\"c1\"");
    let tool = tool.replace("\"seq\":2", "\"seq\":3");
    let r = report(
        "ref",
        &(event(1, "session_start", "") + &intent + &tool + &event(4, "session_end", "")),
    );
    assert_eq!(r["audit_status"], "complete");
    assert_eq!(r["source_references"][0]["status"], "unresolved");
    assert_eq!(r["events"].as_array().unwrap().len(), 4);
}

// REQ-045.9: inspecting only reads the audit; the file stays byte-identical and no work is started.
#[test]
fn inspection_leaves_the_audit_untouched() {
    let text = event(1, "session_start", "") + &event(2, "request_intent", ",\"request_id\":\"r\"");
    let path = std::env::temp_dir().join(format!("obdentic-audit-ro-{}.jsonl", std::process::id()));
    fs::write(&path, &text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_obdentic-mobile-report"))
        .arg("--agent-audit")
        .arg(&path)
        .output()
        .unwrap();
    let after = fs::read_to_string(&path).unwrap();
    let _ = fs::remove_file(path);
    assert!(out.status.success());
    assert_eq!(after, text);
}

// REQ-045.18: the prefix of a process killed mid-stream (open request, partial delta, torn line) is incomplete, with no invented end.
#[test]
fn killed_mid_stream_shows_the_prefix_and_no_terminal_response() {
    let text = event(1, "session_start", "")
        + &event(2, "request_intent", ",\"request_id\":\"r\"")
        + &event(3, "response_delta", ",\"request_id\":\"r\"")
        + "{\"schema\":\"obdentic-agent-au";
    let r = report("killed", &text);
    assert_eq!(r["audit_status"], "incomplete");
    assert_eq!(r["last_evidenced_action"], "response_delta");
    assert_eq!(r["events"].as_array().unwrap().len(), 3);
}
