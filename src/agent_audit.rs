//! Offline inspection of the independent agent-session JSONL audit.
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::Path};

/// Inspect only evidenced events; malformed or interrupted tails are never treated as complete.
pub fn inspect(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|_| "audit cannot be read".to_owned())?;
    let mut events = Vec::new();
    let mut session = None;
    let mut last_time = 0u64;
    let mut damaged = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let mut open = HashMap::new();
    let mut ended = false;
    let mut diagnosis = Value::Null;
    let mut references = Vec::new();
    for (expected, line) in (1u64..).zip(bytes.split_inclusive(|b| *b == b'\n')) {
        if !line.ends_with(b"\n") {
            break;
        }
        let event: Value = match serde_json::from_slice(&line[..line.len() - 1]) {
            Ok(v) => v,
            Err(_) => {
                damaged = true;
                break;
            }
        };
        let id = event.get("session_id").and_then(Value::as_str);
        let kind = event.get("type").and_then(Value::as_str);
        let mono = event.get("mono_ns").and_then(Value::as_u64);
        let valid = event.get("schema").and_then(Value::as_str) == Some("obdentic-agent-audit")
            && event.get("version").and_then(Value::as_u64) == Some(1)
            && event.get("seq").and_then(Value::as_u64) == Some(expected)
            && event.get("wall_ms").and_then(Value::as_u64).is_some()
            && mono.is_some_and(|m| m >= last_time)
            && id.is_some_and(|id| !id.is_empty() && session.as_deref().is_none_or(|s| s == id))
            && kind.is_some()
            && event.get("data").is_some_and(Value::is_object)
            && !ended
            && (expected != 1 || kind == Some("session_start"));
        if !valid {
            damaged = true;
            break;
        }
        if session.is_none() {
            session = id.map(str::to_owned);
        }
        last_time = mono.unwrap();
        let kind = kind.unwrap();
        let key = match kind {
            "request_intent" | "request_dispatched" | "request_end" => {
                event.get("request_id").and_then(Value::as_str)
            }
            "tool_intent" | "tool_dispatched" | "tool_end" => {
                event.get("tool_call_id").and_then(Value::as_str)
            }
            _ => None,
        };
        if matches!(kind, "request_intent" | "tool_intent") {
            if let Some(key) = key {
                if open
                    .insert((kind == "tool_intent", key.to_owned()), false)
                    .is_some()
                {
                    damaged = true;
                    break;
                }
            } else {
                damaged = true;
                break;
            }
        } else if matches!(kind, "request_dispatched" | "tool_dispatched") {
            if let Some(key) = key {
                if let Some(dispatched) = open.get_mut(&(kind == "tool_dispatched", key.to_owned()))
                {
                    if *dispatched {
                        damaged = true;
                        break;
                    }
                    *dispatched = true;
                } else {
                    damaged = true;
                    break;
                }
            } else {
                damaged = true;
                break;
            }
        } else if matches!(kind, "request_end" | "tool_end") {
            if let Some(key) = key {
                if open.remove(&(kind == "tool_end", key.to_owned())).is_none() {
                    damaged = true;
                    break;
                }
            } else {
                damaged = true;
                break;
            }
        } else if kind == "session_end" {
            ended = true;
            diagnosis = event["data"]
                .get("diagnosis_status")
                .cloned()
                .unwrap_or(Value::Null);
        }
        let data = &event["data"];
        let evidence = data.get("evidence_ref").cloned().or_else(|| {
            data.get("recording_ref")
                .and_then(Value::as_str)
                .map(|reference| json!({"recording_ref": reference}))
        });
        if let Some(evidence) = evidence {
            let reference = evidence.get("recording_ref").and_then(Value::as_str);
            let resolved = reference.is_some_and(|reference| {
                let source = Path::new(reference);
                source.components().count() == 1
                    && matches!(
                        source.components().next(),
                        Some(std::path::Component::Normal(_))
                    )
                    && path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(source)
                        .is_file()
            });
            references.push(json!({"seq": expected, "evidence_ref": evidence,
                "status": if resolved { "resolved" } else { "unresolved" }}));
        }
        events.push(event);
    }
    let last = events.last().and_then(|e| e["type"].as_str());
    let complete = ended && open.is_empty() && !damaged;
    Ok(json!({
        "audit_status": if complete { "complete" } else { "incomplete" },
        "diagnosis_status": diagnosis,
        "last_evidenced_action": last,
        "dispatch_status": if open.is_empty() { "not_applicable" } else if open.values().all(|sent| *sent) { "dispatched" } else { "unknown" },
        "events": events,
        "source_references": references,
    }))
}
