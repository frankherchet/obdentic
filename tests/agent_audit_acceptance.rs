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
        .arg("--agent-audit").arg(&path).output().unwrap();
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
    let path = std::env::temp_dir().join(format!("obdentic-dispatched-{}.jsonl", std::process::id()));
    fs::write(&path, concat!(
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":1,\"wall_ms\":1,\"mono_ns\":1,\"type\":\"session_start\",\"data\":{}}\n",
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":2,\"wall_ms\":2,\"mono_ns\":2,\"type\":\"request_intent\",\"request_id\":\"r1\",\"data\":{}}\n",
        "{\"schema\":\"obdentic-agent-audit\",\"version\":1,\"session_id\":\"synthetic\",\"seq\":3,\"wall_ms\":3,\"mono_ns\":3,\"type\":\"request_dispatched\",\"request_id\":\"r1\",\"data\":{}}\n"
    )).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_obdentic-mobile-report"))
        .arg("--agent-audit").arg(&path).output().unwrap();
    let _ = fs::remove_file(path);
    assert!(result.status.success(), "offline audit inspection failed");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["audit_status"], "incomplete");
    assert_eq!(report["dispatch_status"], "dispatched");
}
