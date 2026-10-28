//! Adapter throughput analysis: pure statistics and multi-PID decoding shared by
//! the live job (`MobileCore`) and the offline replay (REQ-022.2, .3, .6, .7).
//!
//! Evidence JSON (recorded by the live job, re-read by replay):
//! `phase1_reads: [{semantic, elapsed_ms, response|null, error|null, observations}]`,
//! `phase2_requests: [{semantic: [..], command, elapsed_ms, payload|null, error|null, observations}]`,
//! `truncated`, `phase2_note`. Timings come from the recorded `elapsed_ms`.
use serde_json::{json, Map, Value};

use crate::ReadRequest;

pub(crate) const READS_PER_PID: usize = 10;
pub(crate) const MAX_PIDS: usize = 6;
/// Stop reading a PID after this many consecutive failures.
pub(crate) const MAX_CONSECUTIVE_ERRORS: usize = 3;

/// Multi-PID request sizes: 2, 3 and 6 (or as many as available).
pub(crate) fn phase2_sizes(available: usize) -> Vec<usize> {
    let mut sizes: Vec<usize> = [2, 3, available.min(MAX_PIDS)]
        .into_iter()
        .filter(|size| *size >= 2 && *size <= available)
        .collect();
    sizes.dedup();
    sizes
}

pub(crate) fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Median and nearest-rank 90th percentile of `samples` (unsorted).
fn median_p90(samples: &[f64]) -> (Option<f64>, Option<f64>) {
    if samples.is_empty() {
        return (None, None);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    };
    let rank = (n as f64 * 0.9).ceil() as usize;
    (Some(round1(median)), Some(round1(sorted[rank - 1])))
}

/// Decode a multi-PID reply per PID with the catalog decoders. Returns the
/// outcome (`full`, `partial`, `rejected`), per-PID values, unparsed byte count
/// and the rejection reason. A missing or malformed PID never gets a value.
fn decode_multi(
    requests: &[ReadRequest],
    payload: Option<&[u8]>,
    error: Option<&str>,
) -> (&'static str, Vec<Value>, usize, Option<String>) {
    let mut found: Vec<(&'static str, Option<f64>)> = vec![("missing", None); requests.len()];
    let mut unparsed = 0;
    let mut reason = error.map(str::to_owned);
    match payload {
        None => {
            reason.get_or_insert_with(|| "no response".into());
        }
        Some(payload) if payload.first() != Some(&0x41) => {
            reason = Some(format!("not a Mode 01 reply: {}", crate::hex(payload)));
        }
        Some(payload) => {
            let mut at = 1;
            while at < payload.len() {
                let Some(index) = requests.iter().position(|r| r.pid() == payload[at]) else {
                    break;
                };
                if found[index].0 != "missing" {
                    break;
                }
                let len = requests[index].data_len();
                if at + 1 + len > payload.len() {
                    found[index] = ("malformed", None);
                    at = payload.len();
                    break;
                }
                let mut bytes = vec![0x41, payload[at]];
                bytes.extend_from_slice(&payload[at + 1..at + 1 + len]);
                found[index] = match requests[index].value(&bytes) {
                    Ok(value) => ("ok", Some(value)),
                    Err(_) => ("malformed", None),
                };
                at += 1 + len;
            }
            unparsed = payload[at.min(payload.len())..]
                .iter()
                .filter(|byte| !matches!(byte, 0x00 | 0x55 | 0xaa))
                .count();
        }
    }
    let ok = found.iter().filter(|(status, _)| *status == "ok").count();
    let outcome = if ok == requests.len() {
        "full"
    } else if ok > 0 {
        "partial"
    } else {
        reason.get_or_insert_with(|| "no requested PID answered".into());
        "rejected"
    };
    let values = requests
        .iter()
        .zip(found)
        .map(|(request, (status, value))| {
            let mut entry = json!({"semantic":request.semantic(),"status":status});
            if let Some(value) = value {
                entry["value"] = json!(value);
                entry["unit"] = json!(request.unit());
            }
            entry
        })
        .collect();
    (outcome, values, unparsed, reason)
}

fn request_for(semantic: &Value) -> Result<ReadRequest, String> {
    crate::prepare_read(semantic.as_str().ok_or("recording semantic missing")?)
}

fn payload_of(entry: &Value, key: &str) -> Result<Option<Vec<u8>>, String> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

fn elapsed_of(entry: &Value) -> Result<f64, String> {
    entry
        .get("elapsed_ms")
        .and_then(Value::as_f64)
        .ok_or_else(|| "recording elapsed_ms missing".into())
}

/// Derive the statistics and the multi-PID decoding from recorded evidence.
pub(crate) fn derive(evidence: &Value) -> Result<Value, String> {
    let reads = evidence
        .get("phase1_reads")
        .and_then(Value::as_array)
        .ok_or("recording phase1_reads missing")?;
    // (request, ok timings, error count) in order of first appearance.
    let mut pids: Vec<(ReadRequest, Vec<f64>, usize)> = Vec::new();
    let (mut ok_total, mut elapsed_total) = (Vec::new(), 0.0);
    for read in reads {
        let request = request_for(&read["semantic"])?;
        let elapsed = elapsed_of(read)?;
        let index = match pids.iter().position(|(r, _, _)| *r == request) {
            Some(index) => index,
            None => {
                pids.push((request, Vec::new(), 0));
                pids.len() - 1
            }
        };
        elapsed_total += elapsed;
        let valid = payload_of(read, "response")?.is_some_and(|r| request.value(&r).is_ok());
        if valid {
            pids[index].1.push(elapsed);
            ok_total.push(elapsed);
        } else {
            pids[index].2 += 1;
        }
    }
    let pid_stats: Vec<_> = pids
        .iter()
        .map(|(request, samples, errors)| {
            let (median, p90) = median_p90(samples);
            json!({"semantic":request.semantic(),"pid":format!("{:02X}", request.pid()),
                "n":samples.len(),"median_ms":median,"p90_ms":p90,"errors":errors})
        })
        .collect();
    let (median, p90) = median_p90(&ok_total);
    let reads_per_s = (elapsed_total > 0.0 && !ok_total.is_empty())
        .then(|| round1(ok_total.len() as f64 / (elapsed_total / 1000.0)));

    let mut phase2 = Vec::new();
    for request in evidence
        .get("phase2_requests")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let requests = request["semantic"]
            .as_array()
            .ok_or("recording semantic list missing")?
            .iter()
            .map(request_for)
            .collect::<Result<Vec<_>, _>>()?;
        let payload = payload_of(request, "payload")?;
        let (outcome, values, unparsed, reason) = decode_multi(
            &requests,
            payload.as_deref(),
            request.get("error").and_then(Value::as_str),
        );
        phase2.push(json!({
            "pids":requests.iter().map(|r| format!("{:02X}", r.pid())).collect::<Vec<_>>(),
            "outcome":outcome,"elapsed_ms":elapsed_of(request)?,"values":values,
            "unparsed_bytes":unparsed,"reason":reason
        }));
    }
    Ok(json!({
        "phase1":{"pids":pid_stats,"overall":{"n":ok_total.len(),"median_ms":median,
            "p90_ms":p90,"reads_per_s":reads_per_s}},
        "phase2":phase2,
        "truncated":evidence.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        "phase2_note":evidence.get("phase2_note").cloned().unwrap_or(Value::Null)
    }))
}

/// Evidence keys copied next to the derived summary.
const EVIDENCE_KEYS: [&str; 4] = [
    "phase1_reads",
    "phase2_requests",
    "limit_s",
    "total_elapsed_ms",
];

/// Summary plus the raw evidence it was derived from.
pub(crate) fn with_evidence(evidence: &Value) -> Result<Value, String> {
    let mut result = derive(evidence)?;
    let object: &mut Map<String, Value> = result.as_object_mut().unwrap();
    for key in EVIDENCE_KEYS {
        if let Some(value) = evidence.get(key) {
            object.insert(key.into(), value.clone());
        }
    }
    Ok(result)
}

/// `replay.adapter.throughput`: re-derive from a recorded action result, no transport.
pub(crate) fn replay(argument: &str) -> Result<Value, String> {
    let recorded: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let mut result = with_evidence(&recorded)?;
    result["replay"] = json!(true);
    Ok(result)
}

/// One-line-per-item text summary for the desktop report.
pub fn summary_lines(result: &Value) -> Vec<String> {
    let f = |value: &Value, unit: &str| {
        value
            .as_f64()
            .map_or("-".to_string(), |v| format!("{v:.1} {unit}"))
    };
    let overall = &result["phase1"]["overall"];
    let mut lines = vec![format!(
        "overall: n={} median {} p90 {} {}{}",
        overall["n"],
        f(&overall["median_ms"], "ms"),
        f(&overall["p90_ms"], "ms"),
        f(&overall["reads_per_s"], "reads/s"),
        if result["truncated"] == true {
            " (truncated)"
        } else {
            ""
        }
    )];
    for pid in result["phase1"]["pids"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  {} (PID {}): n={} median {} p90 {} errors {}",
            pid["semantic"].as_str().unwrap_or("?"),
            pid["pid"].as_str().unwrap_or("?"),
            pid["n"],
            f(&pid["median_ms"], "ms"),
            f(&pid["p90_ms"], "ms"),
            pid["errors"]
        ));
    }
    for request in result["phase2"].as_array().into_iter().flatten() {
        lines.push(format!(
            "multi-PID x{}: {} in {}{}",
            request["pids"].as_array().map_or(0, Vec::len),
            request["outcome"].as_str().unwrap_or("?"),
            f(&request["elapsed_ms"], "ms"),
            request["reason"]
                .as_str()
                .map_or(String::new(), |reason| format!(" ({reason})"))
        ));
    }
    if let Some(note) = result["phase2_note"].as_str() {
        lines.push(format!("multi-PID not run: {note}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_022_2_median_p90_and_reads_per_second_from_recorded_timings() {
        let reads: Vec<_> = (0..10)
            .map(|i| {
                json!({"semantic":"engine.rpm","elapsed_ms":100.0 + 10.0 * i as f64,
                "response":[65,12,31,64]})
            })
            .chain([json!({"semantic":"engine.rpm","elapsed_ms":50.0,"error":"NO DATA"})])
            .collect();
        let derived = derive(&json!({"phase1_reads":reads})).unwrap();
        let pid = &derived["phase1"]["pids"][0];
        assert_eq!(
            (pid["n"].clone(), pid["errors"].clone()),
            (json!(10), json!(1))
        );
        assert_eq!(pid["median_ms"], 145.0);
        assert_eq!(pid["p90_ms"], 180.0);
        // 10 ok reads over 1450 + 50 ms.
        assert_eq!(derived["phase1"]["overall"]["reads_per_s"], 6.7);
        assert_eq!(phase2_sizes(6), [2, 3, 6]);
        assert_eq!(phase2_sizes(4), [2, 3, 4]);
        assert_eq!(phase2_sizes(2), [2]);
        assert!(phase2_sizes(1).is_empty());
    }
}
