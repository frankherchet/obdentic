//! Inspect an exported Android evidence envelope without an adapter or vehicle.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    time::Duration,
};

use super::{coverage, Exchange, MobileCore};
use serde_json::{json, Value};

struct Offline;
impl Exchange for Offline {
    fn exchange(&mut self, _: &str, _: Duration) -> Result<String, String> {
        Err("offline report cannot access an adapter".into())
    }
}

fn open(path: &Path) -> Result<BufReader<File>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("mobile recording exceeds the 64 MiB inspection limit".into());
    }
    Ok(BufReader::new(file))
}

/// The report of one recording, with the run ledger derived from its coverage records as a
/// one-part set (REQ-038.9): `coverage` is null for a recording without coverage facts.
pub fn report(path: &Path) -> Result<Value, String> {
    inspect(open(path)?)
}

/// The report of one or more recordings. One file gives the report of that file; several give a
/// set whose `coverage` is derived from the parts of every file together, so the whole run of a
/// rotated recording is rebuilt (REQ-038.9). The argument order never changes `coverage`.
pub fn report_set(paths: &[impl AsRef<Path>]) -> Result<Value, String> {
    if let [path] = paths {
        return report(path.as_ref());
    }
    let mut files = Vec::new();
    let mut parts = Vec::new();
    for path in paths {
        let (report, part) = inspect_part(open(path.as_ref())?)?;
        files.push(report);
        parts.push(part);
    }
    let coverage = coverage::derive(&parts);
    let mismatch = coverage["knowledge_pin_mismatch"] == true;
    let issues = if mismatch {
        coverage["issues"].clone()
    } else {
        json!([])
    };
    Ok(
        json!({"schema":"OBDENTIC-MOBILE-REPORT-SET","version":1,"files":files,
        "coverage":coverage,"knowledge_pin_mismatch":mismatch,"issues":issues}),
    )
}

pub fn inspect(reader: impl BufRead) -> Result<Value, String> {
    let (mut report, part) = inspect_part(reader)?;
    let coverage = coverage::derive(std::slice::from_ref(&part));
    if coverage["knowledge_pin_mismatch"] == true {
        report["knowledge_pin_mismatch"] = json!(true);
        if let (Some(issues), Some(found)) = (
            report["issues"].as_array_mut(),
            coverage["issues"].as_array(),
        ) {
            issues.extend(found.iter().cloned());
        }
    }
    report["coverage"] = coverage;
    Ok(report)
}

/// The report of one file without coverage, and the file as a part of a run (REQ-038.9): its raw
/// `coverage.*` records in file order and how it ended.
fn inspect_part(reader: impl BufRead) -> Result<(Value, coverage::Part), String> {
    let mut coverage_part = coverage::Part::default();
    let mut core = MobileCore::new(Offline)?;
    let mut session = None;
    let mut complete = false;
    let mut outcome = Value::Null;
    let mut events = 0;
    let mut tx = 0;
    let mut rx = 0;
    let mut values = BTreeMap::new();
    let mut jobs = Vec::new();
    let mut issues = Vec::new();
    let mut smart_changes = Vec::new();
    let mut last_smart_offset = 0u64;
    let mut component_changes = Vec::new();
    let mut gaps = Gaps::default();
    let mut value_provenance = BTreeMap::new();
    let mut last_offset: Option<u64> = None;
    let mut pin_mismatch = false;
    for (index, line) in reader.lines().enumerate() {
        if complete {
            complete = false;
            issues.push("invalid recording: data after session_end marker".into());
            break;
        }
        if index >= 200_000 {
            return Err("mobile recording exceeds the event limit".into());
        }
        let line = line.map_err(|error| error.to_string())?;
        if line.len() > 1024 * 1024 {
            return Err(format!("line {} exceeds the event size limit", index + 1));
        }
        let event: Value = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(error) => {
                complete = false;
                issues.push(format!("line {}: {error}", index + 1));
                break;
            }
        };
        if index == 0 {
            if event["type"] != "session"
                || event["schema"] != "OBDENTIC-MOBILE"
                || event["version"] != 1
            {
                return Err("unsupported mobile recording header".into());
            }
            if !event["smart"].is_null() && !event["smart"].is_object() {
                issues.push("header smart snapshot is not an object".into());
            }
            if !event["component"].is_null() && !event["component"].is_object() {
                issues.push("header component snapshot is not an object".into());
            }
            session = Some(event);
            continue;
        }
        events += 1;
        if let Some(offset) = event["offset_us"].as_u64() {
            last_offset = last_offset.max(Some(offset));
        }
        match event["type"].as_str() {
            Some("session_end") => {
                complete = true;
                outcome = json!({"reason":event["reason"],"detail":event["detail"]});
            }
            Some("wire") => match event["direction"].as_str() {
                Some("TX") => tx += 1,
                Some("RX") => rx += 1,
                _ => {}
            },
            Some("action") => {
                let action = event["action"].as_str().unwrap_or("");
                let recorded = &event["result"];
                if coverage::is_coverage_action(action) {
                    // REQ-038.9: coverage facts go raw to the derivation, which validates them as
                    // a set; a record is never skipped or filtered here.
                    coverage_part.records.push(event);
                    continue;
                }
                if recorded["ok"] != true {
                    issues.push(format!("{action}: {}", recorded["error"]));
                    continue;
                }
                if action == "smart.set" {
                    // REQ-036.5: a recorded smart-analysis set change is shown as recorded. It is
                    // pure evidence of an earlier live session: never replayed through the core,
                    // never recomputed and never sent anywhere.
                    match smart_change(&event, &recorded["data"], &mut last_smart_offset) {
                        Ok(change) => smart_changes.push(change),
                        Err(reason) => issues.push(format!("smart.set: {reason}")),
                    }
                    continue;
                }
                if action == "measurement.gap" {
                    // REQ-037.3: a recorded live measurement gap is shown as recorded. It is
                    // pure evidence: never replayed, never sent anywhere and never given a cause.
                    if let Err(reason) = gaps.record(&event, &recorded["data"]) {
                        issues.push(format!("measurement.gap: {reason}"));
                    }
                    continue;
                }
                if action == "component.apply" {
                    // REQ-035.4: the recorded component snapshot (set and Knowledge revision) is
                    // shown as recorded and never replayed through the core.
                    if recorded["data"].is_object() {
                        component_changes.push(json!({"at":event["at"],
                            "offset_us":event["offset_us"],"data":recorded["data"]}));
                    } else {
                        issues.push("component.apply: data is not an object".into());
                    }
                    continue;
                }
                let offline_action = match action {
                    "read" => "replay.read",
                    "dtc.scan" => "replay.dtc.scan",
                    "adapter.throughput" => "replay.adapter.throughput",
                    "ecu.discover" => "replay.ecu.discover",
                    "knowledge.learn" => "replay.knowledge.learn",
                    "knowledge.availability" => "replay.knowledge.availability",
                    _ => continue,
                };
                let decoded: Value = serde_json::from_str(
                    &core.execute(offline_action, &recorded["data"].to_string()),
                )
                .map_err(|error| error.to_string())?;
                if decoded["ok"] != true {
                    pin_mismatch |= decoded["error"]
                        .as_str()
                        .is_some_and(|error| error.ends_with(super::knowledge::PIN_MISMATCH));
                    issues.push(format!("{action}: {}", decoded["error"]));
                } else if action == "read" {
                    // REQ-024.6: replay always sets the signal key, so two signals that share a
                    // semantic keep separate last values.
                    let key = decoded["data"]["key"]
                        .as_str()
                        .ok_or("read without key")?
                        .to_string();
                    value_provenance.insert(
                        key.clone(),
                        json!({"at":event["at"],"offset_us":event["offset_us"]}),
                    );
                    values.insert(key, decoded["data"].clone());
                } else if action == "adapter.throughput" {
                    jobs.push(
                        json!({"action":action,"at":event["at"],"data":decoded["data"],
                        "summary":crate::throughput::summary_lines(&decoded["data"])}),
                    );
                } else if action == "knowledge.availability" {
                    let (table, counts) = availability_table(&decoded["data"]);
                    jobs.push(
                        json!({"action":action,"at":event["at"],"data":decoded["data"],
                        "table":table,"counts":counts}),
                    );
                } else {
                    jobs.push(json!({"action":action,"at":event["at"],"data":decoded["data"]}));
                }
            }
            _ => {}
        }
    }
    let session = session.ok_or("empty mobile recording")?;
    if !complete {
        issues.push("interrupted recording: no session_end marker".into());
    }
    let measurement_gaps = gaps.periods();
    coverage_part.ended = if complete {
        outcome["reason"].as_str().map(str::to_owned)
    } else {
        None
    };
    let report = json!({"schema":"OBDENTIC-MOBILE-REPORT","version":1,
        "mode":session["mode"],"profile":session["profile"],
        "complete":complete,"outcome":outcome,"event_count":events,"tx_count":tx,"rx_count":rx,
        "last_values":values,"diagnostic_jobs":jobs,"smart_snapshot":session["smart"],
        "smart_changes":smart_changes,"component_snapshot":session["component"],
        "component_changes":component_changes,"measurement_gap_events":gaps.events,
        "measurement_gaps":measurement_gaps,"issues":issues,
        "knowledge_pin_mismatch":pin_mismatch,"value_provenance":value_provenance,
        "recording_end_offset_us":last_offset,
        "note":"Values and diagnostic facts are recomputed from recorded responses. Original evidence remains in the source file."});
    Ok((report, coverage_part))
}

/// REQ-036.5: one recorded smart-analysis set change. Every recorded data field (set, reason, the
/// raw trigger readings and time stamps, latency and anything added later) is kept verbatim next
/// to the event's wallclock time and monotonic offset; the known fields are always present, null
/// when not recorded. A record is rejected, with the reason as issue, when `data` is no object,
/// `change` is not one of the closed vocabulary, `offset_us` is missing or goes backwards.
fn smart_change(event: &Value, data: &Value, last_offset: &mut u64) -> Result<Value, &'static str> {
    const CHANGES: [&str; 5] = ["start", "change", "resume", "availability", "stop"];
    let mut change = data.as_object().ok_or("data is not an object")?.clone();
    match change.get("change").and_then(Value::as_str) {
        None => return Err("change is missing"),
        Some(name) if !CHANGES.contains(&name) => return Err("change is not a known change"),
        Some(_) => {}
    }
    let offset = event["offset_us"].as_u64().ok_or("offset_us is missing")?;
    if offset < *last_offset {
        return Err("offset_us goes backwards");
    }
    *last_offset = offset;
    for key in [
        "set",
        "previous_set",
        "reason",
        "remaining_ms",
        "readings",
        "reading_at",
        "unavailable",
        "selected",
    ] {
        change.entry(key).or_insert(Value::Null);
    }
    change.insert("at".into(), event["at"].clone());
    change.insert("offset_us".into(), event["offset_us"].clone());
    Ok(Value::Object(change))
}

/// REQ-037.3: recorded measurement-gap events and the periods derived from them. A gap is only
/// ever reported as observed: `start`/`resume` open a period ("no_values"), `end` closes it with
/// the recorded outcome (`recovered` or `stopped`), and a period without an end stays `open`. A
/// `resume` marks a gap that was already open when this file began (for example after a rotation);
/// its start is not in this file and may not have been recorded at all, so no start time or
/// duration is claimed for it: only this file's own timestamps are reported.
#[derive(Default)]
struct Gaps {
    events: Vec<Value>,
    closed: Vec<Value>,
    open: Option<Value>,
    last_offset: u64,
}

impl Gaps {
    fn record(&mut self, event: &Value, data: &Value) -> Result<(), &'static str> {
        let object = data.as_object().ok_or("data is not an object")?;
        let change = object
            .get("change")
            .and_then(Value::as_str)
            .ok_or("change is missing")?;
        let reason = object
            .get("reason")
            .and_then(Value::as_str)
            .ok_or("reason is missing")?;
        match (change, reason) {
            ("start" | "resume", "no_values") | ("end", "recovered" | "stopped") => {}
            ("start" | "resume" | "end", _) => return Err("reason does not fit the change"),
            _ => return Err("change is not a known change"),
        }
        // The recorder writes `at` as epoch milliseconds (number); a text time stamp from other
        // writers stays accepted. Either is carried over verbatim.
        let at = &event["at"];
        if !(at.is_number() || at.as_str().is_some_and(|text| !text.is_empty())) {
            return Err("at is missing");
        }
        let offset = event["offset_us"].as_u64().ok_or("offset_us is missing")?;
        if offset < self.last_offset {
            return Err("offset_us goes backwards");
        }
        match (change, self.open.is_some()) {
            ("start", true) => return Err("start while a gap is already open"),
            ("resume", true) => return Err("resume while a gap is already open"),
            ("end", false) => return Err("end without an open gap"),
            _ => {}
        }
        self.last_offset = offset;
        let mut recorded = object.clone();
        recorded.insert("at".into(), json!(at));
        recorded.insert("offset_us".into(), json!(offset));
        self.events.push(Value::Object(recorded));
        if change == "end" {
            let mut period = self.open.take().unwrap_or(Value::Null);
            period["end_at"] = json!(at);
            period["end_offset_us"] = json!(offset);
            period["outcome"] = json!(reason);
            if period["started_before_this_file"] == false {
                let duration = offset - period["start_offset_us"].as_u64().unwrap_or(offset);
                period["duration_us"] = json!(duration);
                period["duration_ms"] = json!(duration / 1000);
            }
            self.closed.push(period);
        } else {
            self.open = Some(json!({"start_at":at,"start_offset_us":offset,
                "started_before_this_file":change == "resume",
                "end_at":null,"end_offset_us":null,"outcome":"open",
                "duration_us":null,"duration_ms":null}));
        }
        Ok(())
    }

    fn periods(&self) -> Vec<Value> {
        self.closed
            .iter()
            .chain(self.open.iter())
            .cloned()
            .collect()
    }
}

/// REQ-030.10: one row per availability entry (key, target, status, NRC or reason, value, unit,
/// implausible) plus the number of entries per status.
fn availability_table(data: &Value) -> (Vec<Value>, BTreeMap<String, u32>) {
    let mut counts = BTreeMap::new();
    let table = data["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            *counts
                .entry(entry["status"].as_str().unwrap_or("unknown").to_owned())
                .or_insert(0) += 1;
            json!({"key":entry["key"],"target":entry["target"],"status":entry["status"],
                "detail":if entry["nrc"].is_null() {&entry["reason"]} else {&entry["nrc"]},
                "value":entry["value"],"unit":entry["unit"],
                "implausible":entry["implausible"] == true})
        })
        .collect();
    (table, counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recomputes_read_and_reports_interrupted_recording() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"demo\",\"profile\":\"engine-drive\"}\n",
            "{\"type\":\"action\",\"action\":\"read\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128],\"value\":99999}}}\n"
        );
        let value = inspect(recording.as_bytes()).unwrap();
        assert_eq!(value["last_values"]["engine.rpm"]["value"], 800.0);
        assert_eq!(value["mode"], "demo");
        assert_eq!(value["complete"], false);
        assert_eq!(value["issues"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn req_024_6_report_keeps_signals_sharing_a_semantic_apart() {
        let revision = crate::knowledge_db::KnowledgeCatalog::embedded_pinned()
            .unwrap()
            .pin()
            .revision()
            .to_owned();
        // Synthetic: standard PID 5C and the Knowledge oil temperature share one semantic.
        let pid = json!({"type":"action","action":"read","result":{"ok":true,
            "data":{"semantic":"engine.oil_temperature","response":[65,92,120]}}});
        let knowledge = json!({"type":"action","action":"read","result":{"ok":true,"data":{
            "knowledge_repository":"frankherchet/obdentic-knowledge","knowledge_revision":revision,
            "semantic":"engine.oil_temperature","definition_id":"vag.ea189.oil.oil_temperature",
            "target_id":"vag.ecu_target.7e0","request":[34,17,190],"response":[98,17,190,14,47],
            "observations":[{"selected_responder":"7E8","responses":[{"responder":"7E8","payload":[98,17,190,14,47]}]}]}}});
        let recording = format!(
            "{}\n{}\n{}\n",
            json!({"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live"}),
            pid,
            knowledge
        );
        let report = inspect(recording.as_bytes()).unwrap();
        let values = report["last_values"].as_object().unwrap();
        assert_eq!(values.len(), 2, "{report}");
        assert_eq!(values["engine.oil_temperature"]["value"], 80.0);
        let knowledge = &values["engine.oil_temperature@vag.ecu_target.7e0"];
        assert!((knowledge["value"].as_f64().unwrap() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn req_022_6_throughput_summary_is_rederived_offline_from_recorded_evidence() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\"}\n",
            "{\"type\":\"action\",\"action\":\"adapter.throughput\",\"result\":{\"ok\":true,\"data\":{",
            "\"phase1_reads\":[",
            "{\"semantic\":\"engine.rpm\",\"elapsed_ms\":100.0,\"response\":[65,12,31,64]},",
            "{\"semantic\":\"engine.rpm\",\"elapsed_ms\":200.0,\"response\":[65,12,31,64]}],",
            "\"phase2_requests\":[{\"semantic\":[\"engine.rpm\",\"vehicle.speed\"],\"elapsed_ms\":150.0,",
            "\"payload\":[65,12,31,64,13,50]}],\"phase1\":{\"overall\":{\"reads_per_s\":999}}}}}\n",
            "{\"type\":\"session_end\",\"reason\":\"completed\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        let job = &report["diagnostic_jobs"][0];
        assert_eq!(job["data"]["phase1"]["overall"]["median_ms"], 150.0);
        assert_eq!(job["data"]["phase1"]["overall"]["reads_per_s"], 6.7);
        assert_eq!(job["data"]["phase2"][0]["outcome"], "full");
        assert!(job["summary"][0].as_str().unwrap().contains("6.7 reads/s"));
    }

    #[test]
    fn rejects_unknown_format_and_keeps_corrupt_tail_partial() {
        assert!(inspect(b"{\"type\":\"session\"}\n".as_slice()).is_err());
        let value = inspect(
            concat!(
                "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1}\n",
                "{\"type\":\"wire\",\"direction\":\"TX\"}\n",
                "{truncated\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(value["tx_count"], 1);
        assert_eq!(value["complete"], false);
        assert_eq!(value["issues"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn terminal_marker_rejects_appended_actions() {
        let value = inspect(concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n",
            "{\"type\":\"action\",\"action\":\"read\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128]}}}\n"
        ).as_bytes()).unwrap();
        assert_eq!(value["complete"], false);
        assert!(value["last_values"].as_object().unwrap().is_empty());
    }

    #[test]
    fn actions_of_removed_features_are_skipped_without_replay_or_issue() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"demo\"}\n",
            "{\"type\":\"action\",\"action\":\"ea189.ash.probe\",\"result\":{\"ok\":true,\"data\":{\"readings\":[]}}}\n",
            "{\"type\":\"action\",\"action\":\"vw.cluster.odometer\",\"result\":{\"ok\":true,\"data\":{}}}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(report["complete"], true);
        assert!(report["issues"].as_array().unwrap().is_empty());
        assert!(report["diagnostic_jobs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn req_028_3_and_10_rotated_automatic_file_with_early_header_is_complete_and_rederived() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\",\"profile\":\"session-auto\",\"selected\":[],\"vehicle\":{}}\n",
            "{\"type\":\"wire\",\"direction\":\"TX\",\"text\":\"ATZ\"}\n",
            "{\"type\":\"action\",\"action\":\"initialize\",\"result\":{\"ok\":true,\"data\":{\"signals\":[]}}}\n",
            "{\"type\":\"action\",\"action\":\"catalog\",\"result\":{\"ok\":true,\"data\":{\"signals\":[]}}}\n",
            "{\"type\":\"action\",\"action\":\"inventory\",\"result\":{\"ok\":true,\"data\":{\"responders\":[]}}}\n",
            "{\"type\":\"action\",\"action\":\"read\",\"argument\":\"engine.rpm\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,31,64]}}}\n",
            "{\"type\":\"session_end\",\"reason\":\"rotated\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(report["complete"], true);
        assert_eq!(report["outcome"]["reason"], "rotated");
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["last_values"]["engine.rpm"]["value"], 2000.0);
    }

    #[test]
    fn req_030_10_availability_table_and_counts_are_rederived_from_recorded_responses() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let data = json!({"knowledge_repository":catalog.pin().repository(),
            "knowledge_revision":catalog.pin().revision(),"entries":[
            {"key":"engine.oil_temperature@vag.ecu_target.7e0","target":"vag.ecu_target.7e0",
             "definition_id":"vag.ea189.oil.oil_temperature","status":"answered",
             "response":[98,17,190,4,0],"value":999.0},
            {"key":"dpf.oil_ash_volume@vag.ecu_target.7e0","target":"vag.ecu_target.7e0",
             "definition_id":"vag.ea189.dpf.oil_ash_volume","status":"rejected",
             "response":[127,34,49],"nrc":49},
            {"key":"engine.rpm","target":"vag.ecu_target.7e0","status":"answered",
             "response":[65,12,26,248]}]});
        let recording = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live"}"#,
            json!({"type":"action","action":"knowledge.availability","result":{"ok":true,"data":data}}),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        let job = &report["diagnostic_jobs"][0];
        assert_eq!(job["counts"], json!({"answered":2,"rejected":1}));
        let rows = job["table"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert!((rows[0]["value"].as_f64().unwrap() + 170.7).abs() < 1e-6);
        assert_eq!(rows[0]["unit"], "°C");
        assert_eq!(rows[0]["implausible"], true);
        assert_eq!(rows[1]["status"], "rejected");
        assert_eq!(rows[1]["detail"], 49);
        assert_eq!(rows[1]["implausible"], false);
        assert_eq!(rows[2]["value"], 1726.0);
        assert_eq!(rows[2]["target"], "vag.ecu_target.7e0");
    }

    fn smart_event(at: &str, offset_us: u64, data: Value) -> String {
        json!({"type":"action","action":"smart.set","argument":"","at":at,"offset_us":offset_us,
            "result":{"ok":true,"data":data}})
        .to_string()
    }

    #[test]
    fn req_036_5_smart_set_changes_are_reported_in_order_with_raw_trigger_readings() {
        // Synthetic recording: rotated continuation file whose header carries the snapshot.
        let first = smart_event(
            "2026-11-14T10:00:00.000Z",
            1_000,
            json!({"change":"resume","set":"cooling","previous_set":"dpf","reason":"resumption",
                "remaining_ms":180000,
                "readings":{"engine.coolant_temperature":{"value":79.0,"age_ms":400,
                "reading_at":"2026-11-14T09:59:59.600Z"},"vehicle.speed":null},
                "reading_at":"2026-11-14T10:00:00.000Z","unavailable":["vehicle.speed"],
                "selected":["engine.coolant_temperature","engine.rpm"]}),
        );
        let second = smart_event(
            "2026-11-14T10:00:05.250Z",
            5_251_000,
            json!({"change":"change","set":"dpf","previous_set":"cooling",
                "reason":"dpf.field_regeneration_request_status","remaining_ms":null,
                "readings":{"dpf.field_regeneration_request_status":{"value":2,"age_ms":150,
                "reading_at":"2026-11-14T10:00:05.100Z"}},
                "reading_at":"2026-11-14T10:00:05.250Z","unavailable":[],
                "selected":["dpf.field_regeneration_request_status"]}),
        );
        let third = smart_event(
            "2026-11-14T10:03:00.000Z",
            180_000_000,
            json!({"change":"stop","set":null,"previous_set":"dpf","reason":"stop",
                "remaining_ms":null,"readings":{},"reading_at":null,"unavailable":[],
                "selected":[]}),
        );
        let recording = format!(
            "{}\n{first}\n{{\"type\":\"wire\",\"direction\":\"TX\",\"text\":\"010C\"}}\n{second}\n{third}\n{}\n",
            r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live","profile":"smart-analysis","smart":{"set":"dpf","started_at":"2026-11-14T09:58:00.000Z"}}"#,
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["complete"], true);
        assert_eq!(report["event_count"], 5);
        assert_eq!(report["tx_count"], 1);
        assert_eq!(report["smart_snapshot"]["set"], "dpf");
        // smart.set is evidence only: it creates neither a job nor a last value.
        assert!(report["diagnostic_jobs"].as_array().unwrap().is_empty());
        assert!(report["last_values"].as_object().unwrap().is_empty());
        let changes = report["smart_changes"].as_array().unwrap();
        let order: Vec<_> = changes
            .iter()
            .map(|c| c["change"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["resume", "change", "stop"]);
        let sets: Vec<_> = changes.iter().map(|c| c["set"].as_str()).collect();
        assert_eq!(sets, [Some("cooling"), Some("dpf"), None]);
        assert_eq!(changes[0]["at"], "2026-11-14T10:00:00.000Z");
        assert_eq!(changes[0]["offset_us"], 1_000);
        assert_eq!(changes[0]["remaining_ms"], 180_000);
        assert_eq!(
            changes[0]["readings"]["engine.coolant_temperature"]["value"],
            79.0
        );
        assert!(changes[0]["readings"]["vehicle.speed"].is_null());
        assert_eq!(changes[0]["unavailable"], json!(["vehicle.speed"]));
        assert_eq!(
            changes[0]["selected"],
            json!(["engine.coolant_temperature", "engine.rpm"])
        );
        assert_eq!(changes[1]["previous_set"], "cooling");
        assert_eq!(
            changes[1]["reason"],
            "dpf.field_regeneration_request_status"
        );
        // The raw (unrecomputed) trigger value and its reading time stamp are kept verbatim.
        let raw = &changes[1]["readings"]["dpf.field_regeneration_request_status"];
        assert_eq!(raw["value"], 2);
        assert_eq!(raw["age_ms"], 150);
        assert_eq!(raw["reading_at"], "2026-11-14T10:00:05.100Z");
        assert_eq!(changes[1]["reading_at"], "2026-11-14T10:00:05.250Z");
        assert_eq!(changes[2]["previous_set"], "dpf");
        assert!(changes[2]["offset_us"].as_u64() > changes[1]["offset_us"].as_u64());
    }

    #[test]
    fn req_036_5_recordings_without_smart_events_keep_empty_additive_fields_and_legacy_actions() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"demo\"}\n",
            "{\"type\":\"action\",\"action\":\"smart.unknown\",\"result\":{\"ok\":true,\"data\":{\"change\":\"x\"}}}\n",
            "{\"type\":\"action\",\"action\":\"vw.cluster.odometer\",\"result\":{\"ok\":true,\"data\":{}}}\n",
            "{\"type\":\"action\",\"action\":\"component.unknown\",\"result\":{\"ok\":true,\"data\":{\"component\":\"dpf\"}}}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["complete"], true);
        assert!(report["smart_changes"].as_array().unwrap().is_empty());
        assert!(report["smart_snapshot"].is_null());
        assert!(report["component_changes"].as_array().unwrap().is_empty());
        assert!(report["component_snapshot"].is_null());
        assert!(report["diagnostic_jobs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn req_035_4_component_snapshot_and_applies_are_reported_in_order_without_replay() {
        let apply = |at: &str, offset_us: u64, component: &str| {
            json!({"type":"action","action":"component.apply","argument":component,"at":at,
                "offset_us":offset_us,"result":{"ok":true,"data":{"component":component,
                "selected":["engine.rpm"],"knowledge_revision":"synthetic-revision"}}})
            .to_string()
        };
        let recording = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live","component":{"component":"cooling","selected":["engine.coolant_temperature"],"knowledge_revision":"synthetic-revision"}}"#,
            apply("2026-11-14T10:00:00.000Z", 100, "dpf"),
            smart_event(
                "2026-11-14T10:00:01.000Z",
                1_000_100,
                json!({"change":"start","set":"dpf","previous_set":null,"reason":"start",
                    "remaining_ms":null,"readings":{},"reading_at":null,"unavailable":[],
                    "selected":[]})
            ),
            apply("2026-11-14T10:00:02.000Z", 2_000_100, "turbo"),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["complete"], true);
        assert_eq!(report["component_snapshot"]["component"], "cooling");
        let applies = report["component_changes"].as_array().unwrap();
        let order: Vec<_> = applies
            .iter()
            .map(|c| c["data"]["component"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["dpf", "turbo"]);
        assert_eq!(applies[0]["at"], "2026-11-14T10:00:00.000Z");
        assert_eq!(applies[0]["offset_us"], 100);
        assert_eq!(applies[1]["offset_us"], 2_000_100);
        assert_eq!(
            applies[1]["data"]["knowledge_revision"],
            "synthetic-revision"
        );
        assert_eq!(applies[1]["data"]["selected"], json!(["engine.rpm"]));
        // Evidence only: no jobs or last values, and smart changes stay separate.
        assert!(report["diagnostic_jobs"].as_array().unwrap().is_empty());
        assert!(report["last_values"].as_object().unwrap().is_empty());
        assert_eq!(report["smart_changes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn req_035_4_malformed_component_records_are_issues_and_never_changes() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\",\"component\":\"dpf\"}\n",
            "{\"type\":\"action\",\"action\":\"component.apply\",\"result\":{\"ok\":true,\"data\":[]}}\n",
            "{\"type\":\"action\",\"action\":\"component.apply\",\"result\":{\"ok\":false,\"error\":\"refused\"}}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["component_changes"].as_array().unwrap().is_empty());
        assert_eq!(
            report["issues"],
            json!([
                "header component snapshot is not an object",
                "component.apply: data is not an object",
                "component.apply: \"refused\""
            ])
        );
    }

    #[test]
    fn req_036_5_interrupted_smart_recording_is_partial_only_for_the_missing_end_marker() {
        let recording = format!(
            "{}\n{}\n",
            r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live"}"#,
            smart_event(
                "2026-11-14T10:00:00.000Z",
                10,
                json!({"change":"start","set":"dpf","previous_set":null,"reason":"start",
                    "remaining_ms":null,"readings":{},"reading_at":{},"unavailable":[]})
            )
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(report["complete"], false);
        assert_eq!(
            report["issues"],
            json!(["interrupted recording: no session_end marker"])
        );
        assert_eq!(report["smart_changes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn req_036_5_malformed_smart_events_are_issues_and_never_changes() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\",\"smart\":3}\n",
            "{\"type\":\"action\",\"action\":\"smart.set\",\"result\":{\"ok\":true,\"data\":[]}}\n",
            "{\"type\":\"action\",\"action\":\"smart.set\",\"result\":{\"ok\":true,\"data\":{\"set\":\"dpf\"}}}\n",
            "{\"type\":\"action\",\"action\":\"smart.set\",\"result\":{\"ok\":false,\"error\":\"refused\"}}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["smart_changes"].as_array().unwrap().is_empty());
        assert_eq!(report["complete"], true);
        assert_eq!(
            report["issues"],
            json!([
                "header smart snapshot is not an object",
                "smart.set: data is not an object",
                "smart.set: change is missing",
                "smart.set: \"refused\""
            ])
        );
    }

    #[test]
    fn req_036_5_smart_change_keeps_every_recorded_field_and_rejects_bad_order_or_vocabulary() {
        let ok = |at: &str, offset_us: u64, change: &str, extra: Value| {
            let mut data = json!({"change":change,"set":"egr","reason":"rotation"});
            data["latency_ms"] = extra;
            smart_event(at, offset_us, data)
        };
        let recording = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live"}"#,
            ok("2026-11-14T10:00:00.000Z", 500, "start", json!(120)),
            // Equal offsets are accepted: two changes can share one monotonic tick.
            ok("2026-11-14T10:00:00.001Z", 500, "availability", json!(null)),
            // Going backwards, an unknown change and a missing offset are rejected.
            ok("2026-11-14T10:00:00.002Z", 499, "change", json!(1)),
            ok("2026-11-14T10:00:00.003Z", 600, "trigger", json!(1)),
            r#"{"type":"action","action":"smart.set","at":"2026-11-14T10:00:00.004Z","result":{"ok":true,"data":{"change":"stop"}}}"#,
            ok("2026-11-14T10:00:00.005Z", 700, "stop", json!(7)),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(
            report["issues"],
            json!([
                "smart.set: offset_us goes backwards",
                "smart.set: change is not a known change",
                "smart.set: offset_us is missing"
            ])
        );
        assert_eq!(report["complete"], true);
        let changes = report["smart_changes"].as_array().unwrap();
        let order: Vec<_> = changes
            .iter()
            .map(|c| c["change"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["start", "availability", "stop"]);
        // A field the report does not know is carried over unchanged.
        assert_eq!(changes[0]["latency_ms"], 120);
        assert_eq!(changes[2]["latency_ms"], 7);
        assert_eq!(changes[0]["at"], "2026-11-14T10:00:00.000Z");
        assert_eq!(changes[0]["offset_us"], 500);
        // Known fields that were not recorded stay present as null.
        assert!(changes[0]["readings"].is_null());
        assert!(changes[0]["previous_set"].is_null());
    }

    fn gap_event(at: &str, offset_us: u64, change: &str, reason: &str) -> String {
        json!({"type":"action","action":"measurement.gap","argument":"","at":at,
            "offset_us":offset_us,
            "result":{"ok":true,"data":{"change":change,"reason":reason}}})
        .to_string()
    }

    const HEADER: &str =
        r#"{"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"live"}"#;

    #[test]
    fn req_037_3_gap_start_and_recovery_report_timestamps_duration_and_outcome() {
        let recording = format!(
            "{HEADER}\n{}\n{}\n{{\"type\":\"wire\",\"direction\":\"RX\",\"text\":\"410C0C80\"}}\n{}\n{}\n",
            gap_event("2026-11-14T10:00:10.000Z", 10_000_000, "start", "no_values"),
            gap_event("2026-11-14T10:00:42.500Z", 42_500_000, "end", "recovered"),
            gap_event("2026-11-14T10:01:00.000Z", 60_000_000, "start", "no_values"),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["complete"], true);
        assert_eq!(report["rx_count"], 1);
        assert!(report["diagnostic_jobs"].as_array().unwrap().is_empty());
        assert!(report["last_values"].as_object().unwrap().is_empty());
        let events = report["measurement_gap_events"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["change"], "start");
        assert_eq!(events[0]["reason"], "no_values");
        assert_eq!(events[1]["at"], "2026-11-14T10:00:42.500Z");
        let gaps = report["measurement_gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0]["start_at"], "2026-11-14T10:00:10.000Z");
        assert_eq!(gaps[0]["end_at"], "2026-11-14T10:00:42.500Z");
        assert_eq!(gaps[0]["duration_us"], 32_500_000);
        assert_eq!(gaps[0]["duration_ms"], 32_500);
        assert_eq!(gaps[0]["outcome"], "recovered");
        assert_eq!(gaps[0]["started_before_this_file"], false);
        // The gap still open at the end of the recording claims neither end nor duration.
        assert_eq!(gaps[1]["outcome"], "open");
        assert!(gaps[1]["end_at"].is_null());
        assert!(gaps[1]["duration_ms"].is_null());
    }

    #[test]
    fn req_037_3_stopped_gap_closes_without_claiming_recovery() {
        let recording = format!(
            "{HEADER}\n{}\n{}\n{}\n",
            gap_event("2026-11-14T10:00:00.000Z", 0, "start", "no_values"),
            gap_event("2026-11-14T10:00:12.000Z", 12_000_000, "end", "stopped"),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        let gaps = report["measurement_gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["outcome"], "stopped");
        assert_eq!(gaps[0]["duration_ms"], 12_000);
    }

    #[test]
    fn req_037_3_rotated_file_carries_an_open_gap_through_resume() {
        // A file that begins with a resume: the gap was already open when it began, so only
        // this file's timestamps are reported and no duration is claimed from an unseen start.
        let recording = format!(
            "{HEADER}\n{}\n{}\n{}\n",
            gap_event("2026-11-14T10:05:00.000Z", 1_000, "resume", "no_values"),
            gap_event("2026-11-14T10:05:08.000Z", 8_001_000, "end", "recovered"),
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        let gaps = report["measurement_gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["started_before_this_file"], true);
        assert_eq!(gaps[0]["start_at"], "2026-11-14T10:05:00.000Z");
        assert_eq!(gaps[0]["outcome"], "recovered");
        assert!(gaps[0]["duration_ms"].is_null());
        // A resume that is never closed stays open (the recording was interrupted).
        let open = format!(
            "{HEADER}\n{}\n",
            gap_event("2026-11-14T10:05:00.000Z", 1_000, "resume", "no_values")
        );
        let report = inspect(open.as_bytes()).unwrap();
        assert_eq!(report["complete"], false);
        assert_eq!(report["measurement_gaps"][0]["outcome"], "open");
        assert_eq!(report["measurement_gap_events"][0]["change"], "resume");
    }

    #[test]
    fn req_037_3_old_recordings_without_gaps_report_empty_fields_and_keep_smart_events() {
        let recording = concat!(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\"}\n",
            "{\"type\":\"action\",\"action\":\"read\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128]}}}\n",
            "{\"type\":\"session_end\",\"reason\":\"stopped\"}\n"
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert!(report["measurement_gaps"].as_array().unwrap().is_empty());
        assert!(report["measurement_gap_events"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(report["last_values"]["engine.rpm"]["value"], 800.0);
        // Gap events beside smart events do not disturb either list or the raw evidence.
        let mixed = format!(
            "{HEADER}\n{}\n{}\n{}\n{}\n",
            smart_event(
                "2026-11-14T10:00:00.000Z",
                5,
                json!({"change":"start","set":"dpf","reason":"start"})
            ),
            gap_event("2026-11-14T10:00:01.000Z", 1_000_000, "start", "no_values"),
            r#"{"type":"wire","direction":"TX","text":"010C"}"#,
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(mixed.as_bytes()).unwrap();
        assert!(report["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(report["smart_changes"].as_array().unwrap().len(), 1);
        assert_eq!(report["measurement_gaps"].as_array().unwrap().len(), 1);
        assert_eq!(report["tx_count"], 1);
    }

    #[test]
    fn req_037_3_malformed_or_inconsistent_gap_events_are_issues_and_never_gaps() {
        let missing_offset = r#"{"type":"action","action":"measurement.gap","at":"2026-11-14T10:00:00.000Z","result":{"ok":true,"data":{"change":"start","reason":"no_values"}}}"#;
        let recording = format!(
            "{HEADER}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            r#"{"type":"action","action":"measurement.gap","result":{"ok":true,"data":[]}}"#,
            r#"{"type":"action","action":"measurement.gap","result":{"ok":true,"data":{"reason":"no_values"}}}"#,
            r#"{"type":"action","action":"measurement.gap","result":{"ok":false,"error":"refused"}}"#,
            gap_event("2026-11-14T10:00:00.000Z", 100, "end", "recovered"),
            gap_event("2026-11-14T10:00:00.001Z", 200, "start", "ignition_off"),
            gap_event("2026-11-14T10:00:00.002Z", 300, "pause", "no_values"),
            missing_offset,
            gap_event("2026-11-14T10:00:00.003Z", 400, "start", "no_values"),
            gap_event("2026-11-14T10:00:00.004Z", 500, "start", "no_values"),
            gap_event("2026-11-14T10:00:00.005Z", 300, "end", "stopped"),
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(
            report["issues"],
            json!([
                "measurement.gap: data is not an object",
                "measurement.gap: change is missing",
                "measurement.gap: \"refused\"",
                "measurement.gap: end without an open gap",
                "measurement.gap: reason does not fit the change",
                "measurement.gap: change is not a known change",
                "measurement.gap: offset_us is missing",
                "measurement.gap: start while a gap is already open",
                "measurement.gap: offset_us goes backwards",
                "interrupted recording: no session_end marker"
            ])
        );
        let gaps = report["measurement_gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["start_offset_us"], 400);
        assert_eq!(gaps[0]["outcome"], "open");
    }

    #[test]
    fn req_037_3_recorder_epoch_millisecond_at_is_accepted_and_carried_over_verbatim() {
        let gap = |at: u64, offset_us: u64, change: &str, reason: &str| {
            json!({"type":"action","action":"measurement.gap","argument":"","at":at,
                "offset_us":offset_us,
                "result":{"ok":true,"data":{"change":change,"reason":reason}}})
            .to_string()
        };
        let bad_at = r#"{"type":"action","action":"measurement.gap","at":true,"offset_us":99000000,"result":{"ok":true,"data":{"change":"start","reason":"no_values"}}}"#;
        let recording = format!(
            "{HEADER}\n{}\n{}\n{}\n{}\n",
            gap(1_794_650_410_000, 10_000_000, "start", "no_values"),
            gap(1_794_650_442_500, 42_500_000, "end", "recovered"),
            bad_at,
            r#"{"type":"session_end","reason":"stopped"}"#
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(
            report["issues"],
            json!(["measurement.gap: at is missing"]),
            "{report}"
        );
        let events = report["measurement_gap_events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["at"], 1_794_650_410_000u64);
        assert_eq!(events[1]["at"], 1_794_650_442_500u64);
        let gaps = report["measurement_gaps"].as_array().unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["start_at"], 1_794_650_410_000u64);
        assert_eq!(gaps[0]["end_at"], 1_794_650_442_500u64);
        assert_eq!(gaps[0]["outcome"], "recovered");
        assert_eq!(gaps[0]["duration_us"], 32_500_000);
        assert_eq!(gaps[0]["duration_ms"], 32_500);
    }

    // REQ-038.9: coverage facts in the recording, the report of one file and of a set of parts.

    use super::coverage::fixtures::{boundary, read, reads, resume, secs, start, RUN};

    /// The fixture records carry a test pin; recordings of this build carry the embedded one.
    fn pinned(mut records: Vec<Value>) -> Vec<Value> {
        let pin = crate::knowledge_db::KnowledgeCatalog::embedded_pinned()
            .unwrap()
            .pin()
            .revision()
            .to_owned();
        for record in &mut records {
            if record["result"]["data"]["knowledge_revision"].is_string() {
                record["result"]["data"]["knowledge_revision"] = json!(pin);
            }
        }
        records
    }

    fn recording(records: Vec<Value>, end: Option<&str>) -> String {
        let mut text = String::from(
            "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"live\"}\n",
        );
        for record in pinned(records) {
            text.push_str(&format!("{record}\n"));
        }
        if let Some(reason) = end {
            text.push_str(&format!(
                "{{\"type\":\"session_end\",\"reason\":\"{reason}\"}}\n"
            ));
        }
        text
    }

    fn three_files() -> Vec<String> {
        let mut first = vec![start(RUN, 0)];
        first.extend(reads("engine.rpm", 0, 20));
        let first_count = first.len() as u64;
        let mut second = vec![resume(RUN, 2, first_count, secs(21))];
        second.extend(reads("engine.rpm", 22, 40));
        let second_count = second.len() as u64;
        let mut third = vec![resume(RUN, 3, second_count, secs(41))];
        third.extend(reads("engine.rpm", 42, 60));
        third.push(boundary(RUN, "run_end", secs(60)));
        vec![
            recording(first, Some("rotated")),
            recording(second, Some("rotated")),
            recording(third, Some("disconnect")),
        ]
    }

    fn write_files(name: &str, files: &[String]) -> Vec<std::path::PathBuf> {
        let directory =
            std::env::temp_dir().join(format!("obdentic-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        files
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let path = directory.join(format!("part-{index}.jsonl"));
                std::fs::write(&path, text).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn req_038_9_report_of_one_file_derives_its_run_as_a_one_part_set() {
        let mut records = vec![start(RUN, 0)];
        records.extend(reads("engine.rpm", 0, 60));
        records.push(boundary(RUN, "run_end", secs(60)));
        let value = inspect(recording(records, Some("disconnect")).as_bytes()).unwrap();
        let run = &value["coverage"]["runs"][0];
        assert_eq!(run["run_complete"], true);
        assert_eq!(run["figures"], "exact");
        assert_eq!(run["entries"][0]["valid_time_ms"], 60_000);
        assert_eq!(value["knowledge_pin_mismatch"], false);
    }

    #[test]
    fn req_038_9_report_of_an_older_recording_has_no_coverage() {
        let value = inspect(
            concat!(
                "{\"type\":\"session\",\"schema\":\"OBDENTIC-MOBILE\",\"version\":1,\"mode\":\"demo\"}\n",
                "{\"type\":\"action\",\"action\":\"read\",\"result\":{\"ok\":true,\"data\":{\"semantic\":\"engine.rpm\",\"response\":[65,12,12,128]}}}\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(value["coverage"], Value::Null);
        assert_eq!(value["issues"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn req_038_9_report_of_a_set_rebuilds_the_whole_run_in_any_argument_order() {
        let files = three_files();
        let paths = write_files("report-set", &files);
        let ordered = report_set(&paths).unwrap();
        assert_eq!(ordered["schema"], "OBDENTIC-MOBILE-REPORT-SET");
        assert_eq!(ordered["version"], 1);
        assert_eq!(ordered["files"].as_array().unwrap().len(), 3);
        for file in ordered["files"].as_array().unwrap() {
            assert!(file.get("coverage").is_none());
            // A rotated file is a complete recording; that never implies a complete run.
            assert_eq!(file["complete"], true);
        }
        let run = &ordered["coverage"]["runs"][0];
        assert_eq!(run["run_complete"], true);
        assert_eq!(run["partial"], false);
        assert_eq!(run["figures"], "exact");
        assert_eq!(run["entries"][0]["valid_time_ms"], 60_000);
        assert_eq!(run["entries"][0]["longest_segment_ms"], 60_000);
        let shuffled = report_set(&[paths[2].clone(), paths[0].clone(), paths[1].clone()]).unwrap();
        assert_eq!(shuffled["coverage"], ordered["coverage"]);
    }

    #[test]
    fn req_038_9_report_of_one_rotated_file_or_an_incomplete_set_is_partial() {
        let paths = write_files("report-partial", &three_files());
        let single = report_set(&paths[..1]).unwrap();
        assert_eq!(single["coverage"]["runs"][0]["partial"], true);
        assert_eq!(
            single["coverage"]["runs"][0]["figures"],
            "supplied_parts_only"
        );
        assert_eq!(single["complete"], true);
        let missing_middle = report_set(&[paths[0].clone(), paths[2].clone()]).unwrap();
        let run = &missing_middle["coverage"]["runs"][0];
        assert_eq!(run["partial"], true);
        assert_eq!(run["missing_parts"], json!([2]));
        assert_eq!(run["entries"][0]["valid_time_ms"], 38_000);
    }

    #[test]
    fn req_038_9_report_of_the_same_file_twice_is_an_invalid_duplicate_part() {
        let paths = write_files("report-duplicate", &three_files());
        let set = report_set(&[paths[0].clone(), paths[0].clone()]).unwrap();
        assert_eq!(set["coverage"]["runs"][0]["coverage_invalid"], true);
        assert!(set["coverage"]["runs"][0].get("entries").is_none());
    }

    #[test]
    fn req_038_9_report_hands_over_a_malformed_coverage_record_instead_of_skipping_it() {
        let mut broken = boundary(RUN, "gap", secs(12));
        broken["result"]["ok"] = json!(false);
        let records = vec![
            start(RUN, 0),
            read("engine.rpm", 10),
            broken,
            read("engine.rpm", 15),
            boundary(RUN, "run_end", secs(15)),
        ];
        let value = inspect(recording(records, Some("disconnect")).as_bytes()).unwrap();
        let run = &value["coverage"]["runs"][0];
        assert_eq!(run["coverage_invalid"], true);
        assert!(run.get("entries").is_none());
        // The other action records of the file are reported as before.
        assert_eq!(value["event_count"], 6);
    }

    #[test]
    fn req_038_9_report_derives_the_facts_before_an_unreadable_line_and_ends_the_part_interrupted()
    {
        let mut text = recording(
            vec![start(RUN, 0), read("engine.rpm", 0), read("engine.rpm", 5)],
            None,
        );
        text.push_str("{truncated\n");
        let value = inspect(text.as_bytes()).unwrap();
        assert_eq!(value["complete"], false);
        let run = &value["coverage"]["runs"][0];
        assert_eq!(run["parts"][0]["ended"], Value::Null);
        assert_eq!(run["partial_reasons"], json!(["no_run_end"]));
        assert_eq!(run["entries"][0]["valid_time_ms"], 5_000);
    }

    #[test]
    fn req_038_9_report_stops_on_a_knowledge_pin_mismatch_of_any_part() {
        let mut files = three_files();
        files[2] = files[2].replacen(
            crate::knowledge_db::KnowledgeCatalog::embedded_pinned()
                .unwrap()
                .pin()
                .revision(),
            "0000000000000000000000000000000000000000",
            1,
        );
        let paths = write_files("report-pin", &files);
        let set = report_set(&paths).unwrap();
        assert_eq!(set["knowledge_pin_mismatch"], true);
        assert_eq!(set["coverage"]["runs"], json!([]));
        assert!(set["issues"][0]
            .as_str()
            .unwrap()
            .ends_with(super::super::knowledge::PIN_MISMATCH));
        let single = inspect(files[2].as_bytes()).unwrap();
        assert_eq!(single["knowledge_pin_mismatch"], true);
        assert!(single["issues"]
            .to_string()
            .contains(super::super::knowledge::PIN_MISMATCH));
    }
}
