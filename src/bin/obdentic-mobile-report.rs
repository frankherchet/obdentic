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
                    "ea189.dpf.probe" => "replay.ea189.dpf.probe",
                    "ea189.extended.probe" => "replay.ea189.extended.probe",
                    _ => continue,
                };
                let decoded: Value = serde_json::from_str(
                    &core.execute(offline_action, &recorded["data"].to_string()),
                )
                .map_err(|error| error.to_string())?;
                if decoded["ok"] != true {
                    issues.push(format!("{action}: {}", decoded["error"]));
                } else if action == "read" {
                    let semantic = decoded["data"]["semantic"]
                        .as_str()
                        .ok_or("read without semantic")?
                        .to_string();
                    values.insert(semantic, decoded["data"].clone());
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
    fn recomputes_the_closed_ea189_extended_job_offline() {
        let readings: Vec<_> = obdentic::ea189::Ea189ExtendedSignal::ALL
            .into_iter()
            .map(|signal| {
                let definition = signal.definition();
                let [_, high, low] = definition.request_bytes();
                let data = vec![0; definition.data_len()];
                let mut response = format!("7E8 {:02X} 62 {high:02X} {low:02X}", data.len() + 3);
                for byte in data {
                    response.push_str(&format!(" {byte:02X}"));
                }
                json!({
                    "semantic":signal.semantic(),
                    "request":format!("22 {high:02X} {low:02X}"),
                    "raw_response":format!("{response}\r>"),
                    "value":999
                })
            })
            .collect();
        let recording = format!(
            "{}\n{}\n{}\n",
            json!({"type":"session","schema":"OBDENTIC-MOBILE","version":1,"mode":"demo","profile":"ea189-extended"}),
            json!({"type":"action","action":"ea189.extended.probe","result":{"ok":true,"data":{"experimental":true,"readings":readings}}}),
            json!({"type":"session_end","reason":"stopped"})
        );
        let report = inspect(recording.as_bytes()).unwrap();
        assert_eq!(report["complete"], true);
        assert!(report["issues"].as_array().unwrap().is_empty());
        assert_eq!(
            report["diagnostic_jobs"][0]["data"]["readings"]
                .as_array()
                .unwrap()
                .len(),
            obdentic::ea189::EA189_EXTENDED_PROBE_COUNT
        );
        let ash = &report["diagnostic_jobs"][0]["data"]["readings"][6];
        assert_eq!(ash["semantic"], "dpf.oil_ash_volume");
        assert_eq!(ash["value"], 0.0);
        assert_eq!(ash["replay"], true);
    }
}
