//! Inspect an exported Android evidence envelope without an adapter or vehicle.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    time::Duration,
};

use obdentic::mobile::{Exchange, MobileCore};
use serde_json::{json, Value};

struct Offline;
impl Exchange for Offline {
    fn exchange(&mut self, _: &str, _: Duration) -> Result<String, String> {
        Err("offline report cannot access an adapter".into())
    }
}

fn report(path: &Path) -> Result<Value, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("mobile recording exceeds the 64 MiB inspection limit".into());
    }
    inspect(BufReader::new(file))
}

fn inspect(reader: impl BufRead) -> Result<Value, String> {
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
            session = Some(event);
            continue;
        }
        events += 1;
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
                if recorded["ok"] != true {
                    issues.push(format!("{action}: {}", recorded["error"]));
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
                    issues.push(format!("{action}: {}", decoded["error"]));
                } else if action == "read" {
                    // REQ-024.6: replay always sets the signal key, so two signals that share a
                    // semantic keep separate last values.
                    let key = decoded["data"]["key"]
                        .as_str()
                        .ok_or("read without key")?
                        .to_string();
                    values.insert(key, decoded["data"].clone());
                } else if action == "adapter.throughput" {
                    jobs.push(
                        json!({"action":action,"at":event["at"],"data":decoded["data"],
                        "summary":obdentic::throughput::summary_lines(&decoded["data"])}),
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
    Ok(json!({"schema":"OBDENTIC-MOBILE-REPORT","version":1,
        "mode":session["mode"],"profile":session["profile"],
        "complete":complete,"outcome":outcome,"event_count":events,"tx_count":tx,"rx_count":rx,
        "last_values":values,"diagnostic_jobs":jobs,"issues":issues,
        "note":"Values and diagnostic facts are recomputed from recorded responses. Original evidence remains in the source file."}))
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

fn main() {
    let mut args = std::env::args_os().skip(1);
    let result = match (args.next(), args.next()) {
        (Some(path), None) => report(Path::new(&path)),
        _ => Err("usage: obdentic-mobile-report <android-recording.jsonl>".into()),
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
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
        let revision = obdentic::knowledge_db::KnowledgeCatalog::embedded_pinned()
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
        let catalog = obdentic::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
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
}
