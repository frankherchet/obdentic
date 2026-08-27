use crate::capture_events::{CaptureEvent, CaptureValue, ReadTiming, SubscriptionFilterOutcome};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};

pub const SCHEMA: &str = "OBDENTIC-CAPTURE";
pub const VERSION: u64 = 1;
const CHANNEL_CAPACITY: usize = 64;
const FLUSH_EVERY: u64 = 16;

pub type Sender = mpsc::Sender<CaptureEvent>;
pub type Writer = JoinHandle<Result<(), String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStatus {
    Complete,
    Partial,
}

#[derive(Debug, PartialEq)]
pub struct ParsedCapture {
    pub events: Vec<CaptureEvent>,
    pub status: CaptureStatus,
}

/// Starts an append-only recorder on a newly created private file.
pub fn start(path: &Path) -> Result<(Sender, Writer), String> {
    let mut options = OpenOptions::new();
    options.append(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    if let Err(error) = file
        .write_all(header().as_bytes())
        .and_then(|_| file.flush())
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }

    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let task = task::spawn_blocking(move || write_events(file, receiver));
    Ok((sender, task))
}

/// Convenience owner for callers that do not need to keep the task separate.
pub struct JsonlRecorder {
    sender: Option<Sender>,
    task: Option<Writer>,
}

impl JsonlRecorder {
    pub fn start(path: &Path) -> Result<Self, String> {
        let (sender, task) = start(path)?;
        Ok(Self {
            sender: Some(sender),
            task: Some(task),
        })
    }

    pub fn sender(&self) -> &Sender {
        self.sender
            .as_ref()
            .expect("recorder sender already closed")
    }

    pub async fn close(mut self) -> Result<(), String> {
        self.sender.take();
        self.task
            .take()
            .expect("recorder task already closed")
            .await
            .map_err(|error| format!("JSONL recorder stopped unexpectedly: {error}"))?
    }
}

pub async fn close(sender: Sender, task: Writer) -> Result<(), String> {
    drop(sender);
    task.await
        .map_err(|error| format!("JSONL recorder stopped unexpectedly: {error}"))?
}

fn write_events(mut file: File, mut receiver: mpsc::Receiver<CaptureEvent>) -> Result<(), String> {
    let result = (|| {
        let mut sequence = 0_u64;
        while let Some(event) = receiver.blocking_recv() {
            let line = event_line(sequence, &event)?;
            file.write_all(line.as_bytes()).map_err(io_error)?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| "JSONL recorder sequence exhausted".to_string())?;
            if sequence.is_multiple_of(FLUSH_EVERY) {
                file.flush().map_err(io_error)?;
            }
        }
        Ok::<(), String>(())
    })();

    let close = file.flush().and_then(|_| file.sync_all()).map_err(io_error);
    match (result, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(close)) => Err(format!("{error}; close failed: {close}")),
    }
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

fn header() -> String {
    format!("{{\"schema\":\"{SCHEMA}\",\"version\":{VERSION},\"type\":\"header\"}}\n")
}

fn event_line(sequence: u64, event: &CaptureEvent) -> Result<String, String> {
    let mut object = format!("{{\"schema\":\"{SCHEMA}\",\"version\":{VERSION},\"type\":",);
    match event {
        CaptureEvent::CaptureStarted {
            wallclock_ms,
            profile,
        } => {
            object.push_str("\"capture_started\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"wallclock_ms\":");
            push_option_u64(&mut object, *wallclock_ms);
            object.push_str(",\"profile\":");
            push_option_string(&mut object, profile.as_deref());
        }
        CaptureEvent::SessionInitialized => {
            simple_event(&mut object, "session_initialized", sequence)
        }
        CaptureEvent::SubscriptionConfigured {
            semantic,
            requested_interval_us,
            filter,
        } => {
            object.push_str("\"subscription_configured\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"semantic\":");
            push_string(&mut object, semantic);
            object.push_str(",\"requested_interval_us\":");
            object.push_str(&requested_interval_us.to_string());
            object.push_str(",\"filter_outcome\":");
            push_string(&mut object, filter_outcome_name(*filter));
        }
        CaptureEvent::SupportDiscovery {
            request_payload,
            response_payload,
        } => {
            object.push_str("\"support_discovery\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"request_payload\":");
            push_string(&mut object, &hex(request_payload));
            object.push_str(",\"response_payload\":");
            push_string(&mut object, &hex(response_payload));
        }
        CaptureEvent::ReadSucceeded {
            semantic,
            requested_interval_us,
            due_us,
            started_us,
            finished_us,
            request_payload,
            response_payload,
            value,
            unit,
            source,
            profile,
            decoder,
            provenance,
        } => {
            object.push_str("\"read_succeeded\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"semantic\":");
            push_string(&mut object, semantic);
            object.push_str(",\"requested_interval_us\":");
            object.push_str(&requested_interval_us.to_string());
            object.push_str(",\"due_us\":");
            object.push_str(&due_us.to_string());
            object.push_str(",\"started_us\":");
            object.push_str(&started_us.to_string());
            object.push_str(",\"finished_us\":");
            object.push_str(&finished_us.to_string());
            object.push_str(",\"request_payload\":");
            push_string(&mut object, &hex(request_payload));
            object.push_str(",\"response_payload\":");
            push_string(&mut object, &hex(response_payload));
            object.push_str(",\"value\":");
            push_value(&mut object, value)?;
            object.push_str(",\"unit\":");
            push_string(&mut object, unit);
            object.push_str(",\"source\":");
            push_string(&mut object, source);
            object.push_str(",\"profile\":");
            push_string(&mut object, profile);
            object.push_str(",\"decoder\":");
            push_string(&mut object, decoder);
            object.push_str(",\"provenance\":");
            push_string(&mut object, provenance);
        }
        CaptureEvent::ReadFailed {
            semantic,
            requested_interval_us,
            timing,
            request_payload,
            error,
        } => {
            object.push_str("\"read_failed\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"semantic\":");
            push_string(&mut object, semantic);
            object.push_str(",\"requested_interval_us\":");
            object.push_str(&requested_interval_us.to_string());
            push_timing(&mut object, *timing);
            object.push_str(",\"request_payload\":");
            match request_payload {
                Some(payload) => push_string(&mut object, &hex(payload)),
                None => object.push_str("null"),
            }
            object.push_str(",\"error\":");
            push_string(&mut object, error);
        }
        CaptureEvent::SlotsSkipped {
            semantic,
            count,
            first_due_us,
            last_due_us,
        } => {
            object.push_str("\"slots_skipped\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"semantic\":");
            push_string(&mut object, semantic);
            object.push_str(",\"count\":");
            object.push_str(&count.to_string());
            object.push_str(",\"first_due_us\":");
            object.push_str(&first_due_us.to_string());
            object.push_str(",\"last_due_us\":");
            object.push_str(&last_due_us.to_string());
        }
        CaptureEvent::SessionError { error } => {
            object.push_str("\"session_error\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"error\":");
            push_string(&mut object, error);
        }
        CaptureEvent::ShutdownRequested => {
            simple_event(&mut object, "shutdown_requested", sequence)
        }
        CaptureEvent::SessionStopped { offset_us } => {
            object.push_str("\"session_stopped\",\"sequence\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\"offset_us\":");
            object.push_str(&offset_us.to_string());
        }
    }
    object.push_str("}\n");
    Ok(object)
}

fn simple_event(object: &mut String, event_type: &str, sequence: u64) {
    object.push('"');
    object.push_str(event_type);
    object.push_str("\",\"sequence\":");
    object.push_str(&sequence.to_string());
}

fn push_timing(object: &mut String, timing: Option<ReadTiming>) {
    object.push_str(",\"due_us\":");
    object.push_str(
        &timing
            .map(|timing| timing.due_us.to_string())
            .unwrap_or_else(|| "null".into()),
    );
    object.push_str(",\"started_us\":");
    object.push_str(
        &timing
            .map(|timing| timing.started_us.to_string())
            .unwrap_or_else(|| "null".into()),
    );
    object.push_str(",\"finished_us\":");
    object.push_str(
        &timing
            .map(|timing| timing.finished_us.to_string())
            .unwrap_or_else(|| "null".into()),
    );
}

fn push_value(object: &mut String, value: &CaptureValue) -> Result<(), String> {
    match value {
        CaptureValue::Number(value) if value.is_finite() => {
            object.push_str("{\"type\":\"number\",\"value\":");
            object.push_str(&value.to_string());
            object.push('}');
        }
        CaptureValue::Number(_) => return Err("capture value cannot be NaN or infinite".into()),
        CaptureValue::Boolean(value) => {
            object.push_str("{\"type\":\"boolean\",\"value\":");
            object.push_str(if *value { "true}" } else { "false}" });
        }
        CaptureValue::Enum(value) => {
            object.push_str("{\"type\":\"enum\",\"value\":");
            push_string(object, value);
            object.push('}');
        }
        CaptureValue::Text(value) => {
            object.push_str("{\"type\":\"text\",\"value\":");
            push_string(object, value);
            object.push('}');
        }
        CaptureValue::Unavailable { reason } => {
            object.push_str("{\"type\":\"unavailable\",\"reason\":");
            push_string(object, reason);
            object.push('}');
        }
    }
    Ok(())
}

fn push_option_u64(object: &mut String, value: Option<u64>) {
    match value {
        Some(value) => object.push_str(&value.to_string()),
        None => object.push_str("null"),
    }
}

fn push_option_string(object: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_string(object, value),
        None => object.push_str("null"),
    }
}

fn push_string(object: &mut String, value: &str) {
    object.push('"');
    for character in value.chars() {
        match character {
            '"' => object.push_str("\\\""),
            '\\' => object.push_str("\\\\"),
            '\n' => object.push_str("\\n"),
            '\r' => object.push_str("\\r"),
            '\t' => object.push_str("\\t"),
            '\u{08}' => object.push_str("\\b"),
            '\u{0c}' => object.push_str("\\f"),
            character if character.is_control() => {
                object.push_str(&format!("\\u{:04X}", character as u32))
            }
            character => object.push(character),
        }
    }
    object.push('"');
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn read(path: &Path) -> Result<ParsedCapture, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut lines = contents.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| "JSONL capture is empty".to_string())?;
    let header = parse_object(header_line, 1)?;
    expect_schema(&header, 1)?;
    if string_field(&header, "type", 1)? != "header" || header.len() != 3 {
        return Err("line 1: malformed JSONL capture header".into());
    }

    let mut events = Vec::new();
    let mut expected_sequence = 0_u64;
    let mut stopped = false;
    for (line_number, line) in lines.enumerate() {
        let line_number = line_number + 2;
        if line.trim().is_empty() {
            return Err(format!("line {line_number}: empty JSONL record"));
        }
        let object = parse_object(line, line_number)?;
        expect_schema(&object, line_number)?;
        let sequence = u64_field(&object, "sequence", line_number)?;
        if sequence != expected_sequence {
            return Err(format!(
                "line {line_number}: sequence {sequence} expected {expected_sequence}"
            ));
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| format!("line {line_number}: sequence exhausted"))?;
        let event = parse_event(&object, line_number)?;
        if matches!(event, CaptureEvent::SessionStopped { .. }) {
            if stopped {
                return Err(format!(
                    "line {line_number}: duplicate session_stopped event"
                ));
            }
            stopped = true;
        } else if stopped {
            return Err(format!("line {line_number}: event follows session_stopped"));
        }
        events.push(event);
    }

    Ok(ParsedCapture {
        events,
        status: if stopped {
            CaptureStatus::Complete
        } else {
            CaptureStatus::Partial
        },
    })
}

pub fn read_events(path: &Path) -> Result<Vec<CaptureEvent>, String> {
    Ok(read(path)?.events)
}

fn expect_schema(object: &Object, line_number: usize) -> Result<(), String> {
    if string_field(object, "schema", line_number)? != SCHEMA {
        return Err(format!(
            "line {line_number}: unsupported JSONL capture schema"
        ));
    }
    if integer_field(object, "version", line_number)? != VERSION {
        return Err(format!(
            "line {line_number}: unsupported JSONL capture version"
        ));
    }
    Ok(())
}

fn parse_event(object: &Object, line_number: usize) -> Result<CaptureEvent, String> {
    let event_type = string_field(object, "type", line_number)?;
    match event_type.as_str() {
        "capture_started" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "wallclock_ms",
                    "profile",
                ],
                line_number,
            )?;
            Ok(CaptureEvent::CaptureStarted {
                wallclock_ms: optional_u64_field(object, "wallclock_ms", line_number)?,
                profile: optional_string_field(object, "profile", line_number)?,
            })
        }
        "session_initialized" => {
            fields_exact(
                object,
                &["schema", "version", "type", "sequence"],
                line_number,
            )?;
            Ok(CaptureEvent::SessionInitialized)
        }
        "subscription_configured" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "semantic",
                    "requested_interval_us",
                    "filter_outcome",
                ],
                line_number,
            )?;
            Ok(CaptureEvent::SubscriptionConfigured {
                semantic: string_field(object, "semantic", line_number)?,
                requested_interval_us: u64_field(object, "requested_interval_us", line_number)?,
                filter: parse_filter_outcome(object, line_number)?,
            })
        }
        "support_discovery" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "request_payload",
                    "response_payload",
                ],
                line_number,
            )?;
            Ok(CaptureEvent::SupportDiscovery {
                request_payload: parse_hex(
                    &string_field(object, "request_payload", line_number)?,
                    line_number,
                )?,
                response_payload: parse_hex(
                    &string_field(object, "response_payload", line_number)?,
                    line_number,
                )?,
            })
        }
        "read_succeeded" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "semantic",
                    "requested_interval_us",
                    "due_us",
                    "started_us",
                    "finished_us",
                    "request_payload",
                    "response_payload",
                    "value",
                    "unit",
                    "source",
                    "profile",
                    "decoder",
                    "provenance",
                ],
                line_number,
            )?;
            let due_us = u64_field(object, "due_us", line_number)?;
            let started_us = u64_field(object, "started_us", line_number)?;
            let finished_us = u64_field(object, "finished_us", line_number)?;
            if !(due_us <= started_us && started_us <= finished_us) {
                return Err(format!("line {line_number}: non-monotonic read timing"));
            }
            Ok(CaptureEvent::ReadSucceeded {
                semantic: string_field(object, "semantic", line_number)?,
                requested_interval_us: u64_field(object, "requested_interval_us", line_number)?,
                due_us,
                started_us,
                finished_us,
                request_payload: parse_hex(
                    &string_field(object, "request_payload", line_number)?,
                    line_number,
                )?,
                response_payload: parse_hex(
                    &string_field(object, "response_payload", line_number)?,
                    line_number,
                )?,
                value: parse_value(
                    object.get("value").expect("fields_exact checked value"),
                    line_number,
                )?,
                unit: string_field(object, "unit", line_number)?,
                source: string_field(object, "source", line_number)?,
                profile: string_field(object, "profile", line_number)?,
                decoder: string_field(object, "decoder", line_number)?,
                provenance: string_field(object, "provenance", line_number)?,
            })
        }
        "read_failed" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "semantic",
                    "requested_interval_us",
                    "due_us",
                    "started_us",
                    "finished_us",
                    "request_payload",
                    "error",
                ],
                line_number,
            )?;
            let timing = match (
                optional_u64_field(object, "due_us", line_number)?,
                optional_u64_field(object, "started_us", line_number)?,
                optional_u64_field(object, "finished_us", line_number)?,
            ) {
                (Some(due_us), Some(started_us), Some(finished_us)) => {
                    if !(due_us <= started_us && started_us <= finished_us) {
                        return Err(format!(
                            "line {line_number}: non-monotonic failed-read timing"
                        ));
                    }
                    Some(ReadTiming::new(due_us, started_us, finished_us))
                }
                (None, None, None) => None,
                _ => return Err(format!("line {line_number}: incomplete failed-read timing")),
            };
            let request_payload = match object.get("request_payload") {
                Some(Value::Null) => None,
                Some(Value::String(value)) => Some(parse_hex(value, line_number)?),
                _ => {
                    return Err(format!(
                        "line {line_number}: request_payload must be a hex string or null"
                    ))
                }
            };
            Ok(CaptureEvent::ReadFailed {
                semantic: string_field(object, "semantic", line_number)?,
                requested_interval_us: u64_field(object, "requested_interval_us", line_number)?,
                timing,
                request_payload,
                error: string_field(object, "error", line_number)?,
            })
        }
        "slots_skipped" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "semantic",
                    "count",
                    "first_due_us",
                    "last_due_us",
                ],
                line_number,
            )?;
            Ok(CaptureEvent::SlotsSkipped {
                semantic: string_field(object, "semantic", line_number)?,
                count: u64_field(object, "count", line_number)?,
                first_due_us: u64_field(object, "first_due_us", line_number)?,
                last_due_us: u64_field(object, "last_due_us", line_number)?,
            })
        }
        "session_error" => {
            fields_exact(
                object,
                &["schema", "version", "type", "sequence", "error"],
                line_number,
            )?;
            Ok(CaptureEvent::SessionError {
                error: string_field(object, "error", line_number)?,
            })
        }
        "shutdown_requested" => {
            fields_exact(
                object,
                &["schema", "version", "type", "sequence"],
                line_number,
            )?;
            Ok(CaptureEvent::ShutdownRequested)
        }
        "session_stopped" => {
            fields_exact(
                object,
                &["schema", "version", "type", "sequence", "offset_us"],
                line_number,
            )?;
            Ok(CaptureEvent::SessionStopped {
                offset_us: u64_field(object, "offset_us", line_number)?,
            })
        }
        _ => Err(format!(
            "line {line_number}: unknown capture event type {event_type:?}"
        )),
    }
}

fn parse_value(value: &Value, line_number: usize) -> Result<CaptureValue, String> {
    let object = match value {
        Value::Object(object) => object,
        _ => {
            return Err(format!(
                "line {line_number}: capture value must be an object"
            ))
        }
    };
    let value_type = string_field(object, "type", line_number)?;
    match value_type.as_str() {
        "number" => {
            fields_exact(object, &["type", "value"], line_number)?;
            match object.get("value") {
                Some(Value::Number(value)) => value
                    .parse::<f64>()
                    .map_err(|_| format!("line {line_number}: number value must be numeric"))
                    .and_then(|value| {
                        if value.is_finite() {
                            Ok(CaptureValue::Number(value))
                        } else {
                            Err(format!("line {line_number}: number value must be finite"))
                        }
                    }),
                _ => Err(format!("line {line_number}: number value must be numeric")),
            }
        }
        "boolean" => {
            fields_exact(object, &["type", "value"], line_number)?;
            match object.get("value") {
                Some(Value::Bool(value)) => Ok(CaptureValue::Boolean(*value)),
                _ => Err(format!(
                    "line {line_number}: boolean value must be true or false"
                )),
            }
        }
        "enum" => {
            fields_exact(object, &["type", "value"], line_number)?;
            Ok(CaptureValue::Enum(string_field(
                object,
                "value",
                line_number,
            )?))
        }
        "text" => {
            fields_exact(object, &["type", "value"], line_number)?;
            Ok(CaptureValue::Text(string_field(
                object,
                "value",
                line_number,
            )?))
        }
        "unavailable" => {
            fields_exact(object, &["type", "reason"], line_number)?;
            Ok(CaptureValue::Unavailable {
                reason: string_field(object, "reason", line_number)?,
            })
        }
        _ => Err(format!(
            "line {line_number}: unknown capture value type {value_type:?}"
        )),
    }
}

fn fields_exact(object: &Object, fields: &[&str], line_number: usize) -> Result<(), String> {
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(format!(
            "line {line_number}: malformed or unsupported capture fields"
        ));
    }
    Ok(())
}

fn string_field(object: &Object, name: &str, line_number: usize) -> Result<String, String> {
    match object.get(name) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(format!("line {line_number}: field {name} must be a string")),
    }
}

fn optional_string_field(
    object: &Object,
    name: &str,
    line_number: usize,
) -> Result<Option<String>, String> {
    match object.get(name) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) => Ok(None),
        _ => Err(format!(
            "line {line_number}: field {name} must be a string or null"
        )),
    }
}

fn integer_field(object: &Object, name: &str, line_number: usize) -> Result<u64, String> {
    match object.get(name) {
        Some(Value::Number(value))
            if !value
                .bytes()
                .any(|byte| matches!(byte, b'.' | b'e' | b'E' | b'-')) =>
        {
            value
                .parse::<u64>()
                .map_err(|_| format!("line {line_number}: field {name} is out of range"))
        }
        _ => Err(format!(
            "line {line_number}: field {name} must be an integer"
        )),
    }
}

fn u64_field(object: &Object, name: &str, line_number: usize) -> Result<u64, String> {
    integer_field(object, name, line_number)
}

fn optional_u64_field(
    object: &Object,
    name: &str,
    line_number: usize,
) -> Result<Option<u64>, String> {
    match object.get(name) {
        Some(Value::Null) => Ok(None),
        Some(Value::Number(_)) => Ok(Some(integer_field(object, name, line_number)?)),
        _ => Err(format!(
            "line {line_number}: field {name} must be an integer or null"
        )),
    }
}

fn filter_outcome_name(outcome: SubscriptionFilterOutcome) -> &'static str {
    match outcome {
        SubscriptionFilterOutcome::Scheduled => "scheduled",
        SubscriptionFilterOutcome::Unsupported => "unsupported",
        SubscriptionFilterOutcome::Unknown => "unknown",
    }
}

fn parse_filter_outcome(
    object: &Object,
    line_number: usize,
) -> Result<SubscriptionFilterOutcome, String> {
    match string_field(object, "filter_outcome", line_number)?.as_str() {
        "scheduled" => Ok(SubscriptionFilterOutcome::Scheduled),
        "unsupported" => Ok(SubscriptionFilterOutcome::Unsupported),
        "unknown" => Ok(SubscriptionFilterOutcome::Unknown),
        outcome => Err(format!(
            "line {line_number}: unknown subscription filter outcome {outcome:?}"
        )),
    }
}

fn parse_hex(value: &str, line_number: usize) -> Result<Vec<u8>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::new();
    for token in value.split(' ') {
        if token.is_empty()
            || token.len() != 2
            || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "line {line_number}: invalid uppercase-space-separated hex"
            ));
        }
        if token.bytes().any(|byte| byte.is_ascii_lowercase()) {
            return Err(format!(
                "line {line_number}: hex payload must use uppercase digits"
            ));
        }
        bytes.push(
            u8::from_str_radix(token, 16).map_err(|_| {
                format!("line {line_number}: invalid uppercase-space-separated hex")
            })?,
        );
    }
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Object(Object),
}

type Object = BTreeMap<String, Value>;

fn parse_object(line: &str, line_number: usize) -> Result<Object, String> {
    let mut parser = Parser::new(line, line_number);
    let value = parser.value()?;
    parser.whitespace();
    if !parser.done() {
        return Err(parser.error("trailing characters"));
    }
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(parser.error("record must be a JSON object")),
    }
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
    line: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str, line: usize) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
            line,
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.whitespace();
        match self.peek() {
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'{') => self.object().map(Value::Object),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.number(),
            _ => Err(self.error("expected a JSON value")),
        }
    }

    fn object(&mut self) -> Result<Object, String> {
        self.expect(b'{')?;
        self.whitespace();
        let mut object = BTreeMap::new();
        if self.consume(b'}') {
            return Ok(object);
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            if object.insert(key.clone(), self.value()?).is_some() {
                return Err(self.error("duplicate object field"));
            }
            self.whitespace();
            if self.consume(b'}') {
                return Ok(object);
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut result = String::new();
        loop {
            let byte = self
                .take()
                .ok_or_else(|| self.error("unterminated JSON string"))?;
            match byte {
                b'"' => return Ok(result),
                b'\\' => {
                    let escaped = self
                        .take()
                        .ok_or_else(|| self.error("unterminated JSON escape"))?;
                    match escaped {
                        b'"' => result.push('"'),
                        b'\\' => result.push('\\'),
                        b'/' => result.push('/'),
                        b'b' => result.push('\u{08}'),
                        b'f' => result.push('\u{0c}'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'u' => result.push(self.unicode_escape()?),
                        _ => return Err(self.error("invalid JSON escape")),
                    }
                }
                byte if byte < 0x20 => return Err(self.error("control character in JSON string")),
                byte if byte.is_ascii() => result.push(byte as char),
                _ => {
                    self.position -= 1;
                    let character = std::str::from_utf8(&self.input[self.position..])
                        .map_err(|_| self.error("invalid UTF-8 in JSON string"))?
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("invalid UTF-8 in JSON string"))?;
                    result.push(character);
                    self.position += character.len_utf8();
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self
                .take()
                .ok_or_else(|| self.error("short unicode escape"))?;
            value = value * 16
                + (byte as char)
                    .to_digit(16)
                    .ok_or_else(|| self.error("invalid unicode escape"))?;
        }
        char::from_u32(value).ok_or_else(|| self.error("invalid unicode code point"))
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.position;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => {
                self.position += 1;
            }
            Some(byte) if (b'1'..=b'9').contains(&byte) => {
                self.position += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.position += 1;
                }
            }
            _ => return Err(self.error("invalid JSON number")),
        }
        if self.consume(b'.') {
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err(self.error("invalid JSON number"));
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.position += 1;
            }
        }
        if self.peek().is_some_and(|byte| byte == b'e' || byte == b'E') {
            self.position += 1;
            if !self.consume(b'+') {
                self.consume(b'-');
            }
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err(self.error("invalid JSON number"));
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.position += 1;
            }
        }
        let value = std::str::from_utf8(&self.input[start..self.position])
            .map_err(|_| self.error("invalid JSON number"))?;
        if !value
            .parse::<f64>()
            .ok()
            .is_some_and(|value| value.is_finite())
        {
            return Err(self.error("invalid JSON number"));
        }
        Ok(Value::Number(value.into()))
    }

    fn literal(&mut self, expected: &[u8], value: Value) -> Result<Value, String> {
        if self
            .input
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            Ok(value)
        } else {
            Err(self.error("invalid JSON literal"))
        }
    }

    fn whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {:?}", expected as char)))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn take(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn done(&self) -> bool {
        self.position == self.input.len()
    }

    fn error(&self, message: &str) -> String {
        format!(
            "line {}: {message} at byte {}",
            self.line,
            self.position + 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_events::{CaptureEvent, SubscriptionFilterOutcome};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-jsonl-{label}-{}-{nonce}.jsonl",
            std::process::id()
        ))
    }

    async fn finish(sender: Sender, writer: Writer) {
        close(sender, writer).await.unwrap();
    }

    fn events() -> Vec<CaptureEvent> {
        vec![
            CaptureEvent::capture_started(Some(1_700_000_000_000), Some("engine-baseline".into())),
            CaptureEvent::SessionInitialized,
            CaptureEvent::subscription_configured(
                "engine.rpm",
                250_000,
                SubscriptionFilterOutcome::Scheduled,
            ),
            CaptureEvent::support_discovery(
                vec![0x01, 0x00],
                vec![0x41, 0x00, 0x80, 0x00, 0x00, 0x01],
            ),
            CaptureEvent::ReadSucceeded {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                due_us: 10,
                started_us: 12,
                finished_us: 15,
                request_payload: vec![0x01, 0x0c],
                response_payload: vec![0x41, 0x0c, 0x1a, 0xf8],
                value: CaptureValue::Number(1726.0),
                unit: "rpm".into(),
                source: "user".into(),
                profile: "obd2-v1".into(),
                decoder: "((A * 256) + B) / 4".into(),
                provenance: "SAE J1979 Mode 01 PID 0C".into(),
            },
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: Some(ReadTiming::new(20, 21, 22)),
                request_payload: Some(vec![0x01, 0x0c]),
                error: "timeout\nline".into(),
            },
            CaptureEvent::slots_skipped("engine.rpm", 2, 30, 280_000),
            CaptureEvent::SessionError {
                error: "session stopped".into(),
            },
            CaptureEvent::ShutdownRequested,
            CaptureEvent::SessionStopped { offset_us: 300_000 },
        ]
    }

    #[tokio::test]
    async fn round_trips_every_capture_event_variant_and_preserves_order() {
        let path = temp_path("roundtrip");
        let (sender, writer) = start(&path).unwrap();
        let expected = events();
        for event in expected.iter().cloned() {
            sender.send(event).await.unwrap();
        }
        finish(sender, writer).await;

        let parsed = read(&path).unwrap();
        assert_eq!(parsed.status, CaptureStatus::Complete);
        assert_eq!(parsed.events, expected);
        assert_eq!(
            parsed
                .events
                .iter()
                .enumerate()
                .map(|(index, _)| index as u64)
                .collect::<Vec<_>>(),
            (0..expected.len() as u64).collect::<Vec<_>>()
        );
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn preserves_each_subscription_filter_outcome() {
        let path = temp_path("subscription-outcomes");
        let (sender, writer) = start(&path).unwrap();
        let expected = vec![
            CaptureEvent::subscription_configured(
                "engine.rpm",
                250_000,
                SubscriptionFilterOutcome::Scheduled,
            ),
            CaptureEvent::subscription_configured(
                "engine.maf",
                500_000,
                SubscriptionFilterOutcome::Unsupported,
            ),
            CaptureEvent::subscription_configured(
                "engine.load",
                500_000,
                SubscriptionFilterOutcome::Unknown,
            ),
        ];
        for event in expected.iter().cloned() {
            sender.send(event).await.unwrap();
        }
        finish(sender, writer).await;

        assert_eq!(read_events(&path).unwrap(), expected);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"filter_outcome\":\"scheduled\""));
        assert!(contents.contains("\"filter_outcome\":\"unsupported\""));
        assert!(contents.contains("\"filter_outcome\":\"unknown\""));
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn emits_deterministic_header_hex_and_sequence_bytes() {
        let path = temp_path("bytes");
        let (sender, writer) = start(&path).unwrap();
        sender
            .send(CaptureEvent::SupportDiscovery {
                request_payload: vec![0x01, 0xab],
                response_payload: vec![0x41, 0x00, 0x00, 0xff],
            })
            .await
            .unwrap();
        finish(sender, writer).await;
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"header\"}\n{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"support_discovery\",\"sequence\":0,\"request_payload\":\"01 AB\",\"response_payload\":\"41 00 00 FF\"}\n"
        );
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn closes_by_draining_queued_events_and_accepts_partial_files() {
        let path = temp_path("partial");
        let (sender, writer) = start(&path).unwrap();
        for event in events().into_iter().take(3) {
            sender.send(event).await.unwrap();
        }
        finish(sender, writer).await;
        assert_eq!(read(&path).unwrap().status, CaptureStatus::Partial);
        assert_eq!(read_events(&path).unwrap().len(), 3);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_bad_versions_malformed_lines_and_noncanonical_hex() {
        let path = temp_path("invalid");
        fs::write(
            &path,
            "{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":2,\"type\":\"header\"}\n",
        )
        .unwrap();
        assert!(read(&path).unwrap_err().contains("line 1"));
        fs::write(
            &path,
            "{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"header\"}\nnot-json\n",
        )
        .unwrap();
        assert!(read(&path).unwrap_err().contains("line 2"));
        fs::write(
            &path,
            "{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"header\"}\n{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"support_discovery\",\"sequence\":0,\"request_payload\":\"01 ab\",\"response_payload\":\"\"}\n",
        )
        .unwrap();
        assert!(read(&path).unwrap_err().contains("line 2"));
        fs::write(
            &path,
            "{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"header\"}\n{\"schema\":\"OBDENTIC-CAPTURE\",\"version\":1,\"type\":\"subscription_configured\",\"sequence\":0,\"semantic\":\"engine.rpm\",\"requested_interval_us\":250000,\"filter_outcome\":\"invalid\"}\n",
        )
        .unwrap();
        assert!(read(&path)
            .unwrap_err()
            .contains("unknown subscription filter outcome"));
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn refuses_overwrite_and_uses_private_permissions() {
        let path = temp_path("private");
        let (sender, writer) = start(&path).unwrap();
        finish(sender, writer).await;
        assert!(start(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(path).unwrap();
    }
}
