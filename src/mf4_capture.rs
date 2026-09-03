use crate::capture_events::{CaptureEvent, CaptureValue};
use mdf4_rs::{writer::FileWriter, DataType, DecodedValue, FlushPolicy, MdfWriter};
use std::path::Path;
use tokio::{sync::mpsc, task};

pub const SCHEMA: &str = "OBDENTIC-MF4";
pub const VERSION: u64 = 1;

const CHANNEL_CAPACITY: usize = 64;
const FLUSH_EVERY_RECORDS: u64 = 64;
const CHUNK_BYTES: usize = 64;

const RECORD_EVENT: u64 = 1;
const RECORD_EVIDENCE_CHUNK: u64 = 2;

const FIELD_EVENT_AUDIT_UTF8: u64 = 1;
const FIELD_SEMANTIC_UTF8: u64 = 2;
const FIELD_DIAGNOSTIC_REQUEST: u64 = 3;
const FIELD_DIAGNOSTIC_RESPONSE: u64 = 4;
const FIELD_RESPONDER_UTF8: u64 = 5;
const FIELD_SELECTED_RESPONDER_UTF8: u64 = 6;
const FIELD_PROFILE_UTF8: u64 = 7;
const FIELD_UNIT_UTF8: u64 = 8;
const FIELD_SOURCE_UTF8: u64 = 9;
const FIELD_DECODER_UTF8: u64 = 10;
const FIELD_PROVENANCE_UTF8: u64 = 11;
const FIELD_ERROR_UTF8: u64 = 12;

pub type Sender = mpsc::Sender<CaptureEvent>;
pub type Writer = task::JoinHandle<Result<(), String>>;

type NativeWriter = MdfWriter<FileWriter>;

struct Layout {
    group: String,
}

/// Starts the passive MF4 sink. The sink only receives already-normalized
/// capture events and has no adapter/session capability.
pub fn start(path: &Path) -> Result<(Sender, Writer), String> {
    let path_text = path
        .to_str()
        .ok_or_else(|| "MF4 capture path must be valid UTF-8".to_string())?;
    let mut writer = MdfWriter::new(path_text)
        .map_err(|error| format!("create MF4 capture: {error}"))?
        .with_flush_policy(FlushPolicy::EveryNRecords(FLUSH_EVERY_RECORDS));
    restrict_file_permissions(path)?;
    let layout = initialize(&mut writer)?;
    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let task = task::spawn_blocking(move || write_events(writer, layout, receiver));
    Ok((sender, task))
}

fn restrict_file_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("set MF4 capture permissions: {error}"))?;
    }
    Ok(())
}

fn initialize(writer: &mut NativeWriter) -> Result<Layout, String> {
    writer
        .init_mdf_file()
        .map_err(|error| format!("initialize MF4 file: {error}"))?;
    let group = writer
        .add_channel_group(None, |_| {})
        .map_err(|error| format!("create MF4 channel group: {error}"))?;
    writer
        .set_channel_group_name(&group, "OBDentic capture records")
        .map_err(|error| format!("name MF4 channel group: {error}"))?;
    writer
        .set_channel_group_comment(
            &group,
            "OBDentic MF4 v1. Passive normalized diagnostic capture. record_kind=1 is an event summary; record_kind=2 is a 64-byte evidence chunk. Diagnostic request/response fields are normalized diagnostic payload bytes and MUST NOT be interpreted as raw CAN frames.",
        )
        .map_err(|error| format!("comment MF4 channel group: {error}"))?;

    let time = add_channel(writer, &group, None, "time", DataType::FloatLE, 64)?;
    writer
        .set_time_channel(&time)
        .map_err(|error| format!("mark MF4 time channel: {error}"))?;
    writer
        .set_channel_unit(&time, "s")
        .map_err(|error| format!("set MF4 time unit: {error}"))?;
    let record_kind = add_channel(
        writer,
        &group,
        Some(&time),
        "record_kind",
        DataType::UnsignedIntegerLE,
        8,
    )?;
    let sequence = add_channel(
        writer,
        &group,
        Some(&record_kind),
        "event_sequence",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let event_kind = add_channel(
        writer,
        &group,
        Some(&sequence),
        "event_kind",
        DataType::UnsignedIntegerLE,
        16,
    )?;
    let field_kind = add_channel(
        writer,
        &group,
        Some(&event_kind),
        "evidence_field_kind",
        DataType::UnsignedIntegerLE,
        16,
    )?;
    let item_index = add_channel(
        writer,
        &group,
        Some(&field_kind),
        "evidence_item_index",
        DataType::UnsignedIntegerLE,
        32,
    )?;
    let chunk_index = add_channel(
        writer,
        &group,
        Some(&item_index),
        "evidence_chunk_index",
        DataType::UnsignedIntegerLE,
        32,
    )?;
    let chunk_len = add_channel(
        writer,
        &group,
        Some(&chunk_index),
        "evidence_chunk_len",
        DataType::UnsignedIntegerLE,
        8,
    )?;
    let semantic_hash = add_channel(
        writer,
        &group,
        Some(&chunk_len),
        "semantic_fnv1a64",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let requested_interval = add_channel(
        writer,
        &group,
        Some(&semantic_hash),
        "requested_interval_us",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let due = add_channel(
        writer,
        &group,
        Some(&requested_interval),
        "due_us",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let started = add_channel(
        writer,
        &group,
        Some(&due),
        "started_us",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let finished = add_channel(
        writer,
        &group,
        Some(&started),
        "finished_us",
        DataType::UnsignedIntegerLE,
        64,
    )?;
    let value = add_channel(
        writer,
        &group,
        Some(&finished),
        "decoded_numeric_value",
        DataType::FloatLE,
        64,
    )?;
    let value_kind = add_channel(
        writer,
        &group,
        Some(&value),
        "decoded_value_kind",
        DataType::UnsignedIntegerLE,
        8,
    )?;
    let data = add_channel(
        writer,
        &group,
        Some(&value_kind),
        "evidence_chunk",
        DataType::ByteArray,
        (CHUNK_BYTES * 8) as u32,
    )?;

    writer
        .set_channel_comment(
            &field_kind,
            "1=event audit UTF-8; 2=semantic UTF-8; 3=diagnostic request bytes; 4=diagnostic response bytes; 5=responder identity UTF-8; 6=selected responder UTF-8; 7=profile UTF-8; 8=unit UTF-8; 9=source UTF-8; 10=decoder UTF-8; 11=provenance UTF-8; 12=error UTF-8",
        )
        .map_err(|error| format!("comment MF4 evidence field channel: {error}"))?;
    writer
        .set_channel_comment(
            &data,
            "Fixed-size chunk storage. Only the first evidence_chunk_len bytes are part of the field; chunks are ordered by evidence_chunk_index and grouped by event_sequence/evidence_field_kind/evidence_item_index.",
        )
        .map_err(|error| format!("comment MF4 evidence chunk channel: {error}"))?;
    writer
        .set_channel_comment(
            &semantic_hash,
            "FNV-1a 64-bit convenience key for the exact semantic UTF-8 value retained as evidence_field_kind=2. The hash is not the semantic identity itself.",
        )
        .map_err(|error| format!("comment MF4 semantic channel: {error}"))?;
    writer
        .set_channel_comment(
            &value_kind,
            "0=not a decoded value; 1=number; 2=boolean; 3=enum; 4=text; 5=unavailable. Non-numeric exact values remain in the event audit envelope.",
        )
        .map_err(|error| format!("comment MF4 value kind channel: {error}"))?;

    writer
        .start_data_block_for_cg(&group, 0)
        .map_err(|error| format!("start MF4 data block: {error}"))?;
    Ok(Layout { group })
}

fn add_channel(
    writer: &mut NativeWriter,
    group: &str,
    previous: Option<&str>,
    name: &str,
    data_type: DataType,
    bit_count: u32,
) -> Result<String, String> {
    writer
        .add_channel(group, previous, |channel| {
            channel.name = Some(name.into());
            channel.data_type = data_type;
            channel.bit_count = bit_count;
        })
        .map_err(|error| format!("create MF4 channel {name}: {error}"))
}

fn write_events(
    mut writer: NativeWriter,
    layout: Layout,
    mut receiver: mpsc::Receiver<CaptureEvent>,
) -> Result<(), String> {
    let mut sequence = 0_u64;
    let mut last_time_us = 0_u64;
    let result = (|| {
        while let Some(event) = receiver.blocking_recv() {
            let time_us = event_time_us(&event, last_time_us);
            last_time_us = time_us;
            write_event(&mut writer, &layout, sequence, time_us, &event)?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| "MF4 capture event sequence exhausted".to_string())?;
        }
        Ok::<(), String>(())
    })();

    let finish = writer
        .finish_data_block(&layout.group)
        .map_err(|error| format!("finish MF4 data block: {error}"));
    let finalize = writer
        .finalize()
        .map_err(|error| format!("finalize MF4 capture: {error}"));
    match (result, finish, finalize) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(()), Ok(()))
        | (Ok(()), Err(error), Ok(()))
        | (Ok(()), Ok(()), Err(error)) => Err(error),
        (first, second, third) => Err(format!(
            "MF4 writer failed: event={:?}; finish={:?}; finalize={:?}",
            first.err(),
            second.err(),
            third.err()
        )),
    }
}

fn write_event(
    writer: &mut NativeWriter,
    layout: &Layout,
    sequence: u64,
    time_us: u64,
    event: &CaptureEvent,
) -> Result<(), String> {
    let measurement = measurement_fields(event);
    write_record(
        writer,
        layout,
        Record {
            time_us,
            record_kind: RECORD_EVENT,
            sequence,
            event_kind: event_kind(event),
            semantic_hash: measurement.semantic_hash,
            requested_interval_us: measurement.requested_interval_us,
            due_us: measurement.due_us,
            started_us: measurement.started_us,
            finished_us: measurement.finished_us,
            value: measurement.value,
            value_kind: measurement.value_kind,
            ..Record::default()
        },
    )?;

    let audit = crate::jsonl_capture::event_line(sequence, event)?;
    let audit = audit.trim_end_matches('\n');
    write_chunks(
        writer,
        layout,
        sequence,
        time_us,
        FIELD_EVENT_AUDIT_UTF8,
        0,
        audit.as_bytes(),
    )?;
    write_structured_evidence(writer, layout, sequence, time_us, event)
}

fn write_structured_evidence(
    writer: &mut NativeWriter,
    layout: &Layout,
    sequence: u64,
    time_us: u64,
    event: &CaptureEvent,
) -> Result<(), String> {
    match event {
        CaptureEvent::CaptureStarted { profile, .. } => write_optional_text(
            writer,
            layout,
            sequence,
            time_us,
            FIELD_PROFILE_UTF8,
            0,
            profile,
        ),
        CaptureEvent::SubscriptionConfigured { semantic, .. }
        | CaptureEvent::SlotsSkipped { semantic, .. } => write_text(
            writer,
            layout,
            sequence,
            time_us,
            FIELD_SEMANTIC_UTF8,
            0,
            semantic,
        ),
        CaptureEvent::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        }
        | CaptureEvent::ProtocolNegotiationObserved {
            request_payload,
            responder,
            response_payload,
        } => {
            write_chunks(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DIAGNOSTIC_REQUEST,
                0,
                request_payload,
            )?;
            write_chunks(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DIAGNOSTIC_RESPONSE,
                0,
                response_payload,
            )?;
            write_optional_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_RESPONDER_UTF8,
                0,
                responder,
            )
        }
        CaptureEvent::ResponsesObserved {
            semantic,
            request_payload,
            responses,
            selected_responder,
            selection_error,
            ..
        } => {
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_SEMANTIC_UTF8,
                0,
                semantic,
            )?;
            write_chunks(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DIAGNOSTIC_REQUEST,
                0,
                request_payload,
            )?;
            for (index, response) in responses.iter().enumerate() {
                let index = u64::try_from(index)
                    .map_err(|_| "MF4 responder index exceeds u64".to_string())?;
                write_chunks(
                    writer,
                    layout,
                    sequence,
                    time_us,
                    FIELD_DIAGNOSTIC_RESPONSE,
                    index,
                    &response.payload,
                )?;
                write_optional_text(
                    writer,
                    layout,
                    sequence,
                    time_us,
                    FIELD_RESPONDER_UTF8,
                    index,
                    &response.responder,
                )?;
            }
            write_optional_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_SELECTED_RESPONDER_UTF8,
                0,
                selected_responder,
            )?;
            write_optional_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_ERROR_UTF8,
                0,
                selection_error,
            )
        }
        CaptureEvent::ReadSucceeded {
            semantic,
            request_payload,
            response_payload,
            unit,
            source,
            profile,
            decoder,
            provenance,
            ..
        } => {
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_SEMANTIC_UTF8,
                0,
                semantic,
            )?;
            write_chunks(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DIAGNOSTIC_REQUEST,
                0,
                request_payload,
            )?;
            write_chunks(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DIAGNOSTIC_RESPONSE,
                0,
                response_payload,
            )?;
            write_text(writer, layout, sequence, time_us, FIELD_UNIT_UTF8, 0, unit)?;
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_SOURCE_UTF8,
                0,
                source,
            )?;
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_PROFILE_UTF8,
                0,
                profile,
            )?;
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_DECODER_UTF8,
                0,
                decoder,
            )?;
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_PROVENANCE_UTF8,
                0,
                provenance,
            )
        }
        CaptureEvent::ReadFailed {
            semantic,
            request_payload,
            error,
            ..
        } => {
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_SEMANTIC_UTF8,
                0,
                semantic,
            )?;
            if let Some(request_payload) = request_payload {
                write_chunks(
                    writer,
                    layout,
                    sequence,
                    time_us,
                    FIELD_DIAGNOSTIC_REQUEST,
                    0,
                    request_payload,
                )?;
            }
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_ERROR_UTF8,
                0,
                error,
            )
        }
        CaptureEvent::SessionError { error } | CaptureEvent::DiagnosticJobFailed { error, .. } => {
            write_text(
                writer,
                layout,
                sequence,
                time_us,
                FIELD_ERROR_UTF8,
                0,
                error,
            )
        }
        _ => Ok(()),
    }
}

fn write_optional_text(
    writer: &mut NativeWriter,
    layout: &Layout,
    sequence: u64,
    time_us: u64,
    field_kind: u64,
    item_index: u64,
    value: &Option<String>,
) -> Result<(), String> {
    if let Some(value) = value {
        write_text(
            writer, layout, sequence, time_us, field_kind, item_index, value,
        )?;
    }
    Ok(())
}

fn write_text(
    writer: &mut NativeWriter,
    layout: &Layout,
    sequence: u64,
    time_us: u64,
    field_kind: u64,
    item_index: u64,
    value: &str,
) -> Result<(), String> {
    write_chunks(
        writer,
        layout,
        sequence,
        time_us,
        field_kind,
        item_index,
        value.as_bytes(),
    )
}

fn write_chunks(
    writer: &mut NativeWriter,
    layout: &Layout,
    sequence: u64,
    time_us: u64,
    field_kind: u64,
    item_index: u64,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.is_empty() {
        return write_record(
            writer,
            layout,
            Record {
                time_us,
                record_kind: RECORD_EVIDENCE_CHUNK,
                sequence,
                field_kind,
                item_index,
                ..Record::default()
            },
        );
    }
    for (chunk_index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let mut data = vec![0_u8; CHUNK_BYTES];
        data[..chunk.len()].copy_from_slice(chunk);
        write_record(
            writer,
            layout,
            Record {
                time_us,
                record_kind: RECORD_EVIDENCE_CHUNK,
                sequence,
                field_kind,
                item_index,
                chunk_index: u64::try_from(chunk_index)
                    .map_err(|_| "MF4 evidence chunk index exceeds u64".to_string())?,
                chunk_len: chunk.len() as u64,
                data,
                ..Record::default()
            },
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct Record {
    time_us: u64,
    record_kind: u64,
    sequence: u64,
    event_kind: u64,
    field_kind: u64,
    item_index: u64,
    chunk_index: u64,
    chunk_len: u64,
    semantic_hash: u64,
    requested_interval_us: u64,
    due_us: u64,
    started_us: u64,
    finished_us: u64,
    value: f64,
    value_kind: u64,
    data: Vec<u8>,
}

fn write_record(
    writer: &mut NativeWriter,
    layout: &Layout,
    mut record: Record,
) -> Result<(), String> {
    if record.data.is_empty() {
        record.data = vec![0_u8; CHUNK_BYTES];
    }
    writer
        .write_record(
            &layout.group,
            &[
                DecodedValue::Float(record.time_us as f64 / 1_000_000.0),
                DecodedValue::UnsignedInteger(record.record_kind),
                DecodedValue::UnsignedInteger(record.sequence),
                DecodedValue::UnsignedInteger(record.event_kind),
                DecodedValue::UnsignedInteger(record.field_kind),
                DecodedValue::UnsignedInteger(record.item_index),
                DecodedValue::UnsignedInteger(record.chunk_index),
                DecodedValue::UnsignedInteger(record.chunk_len),
                DecodedValue::UnsignedInteger(record.semantic_hash),
                DecodedValue::UnsignedInteger(record.requested_interval_us),
                DecodedValue::UnsignedInteger(record.due_us),
                DecodedValue::UnsignedInteger(record.started_us),
                DecodedValue::UnsignedInteger(record.finished_us),
                DecodedValue::Float(record.value),
                DecodedValue::UnsignedInteger(record.value_kind),
                DecodedValue::ByteArray(record.data),
            ],
        )
        .map_err(|error| format!("write MF4 record: {error}"))
}

#[derive(Default)]
struct MeasurementFields {
    semantic_hash: u64,
    requested_interval_us: u64,
    due_us: u64,
    started_us: u64,
    finished_us: u64,
    value: f64,
    value_kind: u64,
}

fn measurement_fields(event: &CaptureEvent) -> MeasurementFields {
    match event {
        CaptureEvent::ReadSucceeded {
            semantic,
            requested_interval_us,
            due_us,
            started_us,
            finished_us,
            value,
            ..
        } => {
            let (value, value_kind) = match value {
                CaptureValue::Number(value) => (*value, 1),
                CaptureValue::Boolean(value) => (u8::from(*value) as f64, 2),
                CaptureValue::Enum(_) => (f64::NAN, 3),
                CaptureValue::Text(_) => (f64::NAN, 4),
                CaptureValue::Unavailable { .. } => (f64::NAN, 5),
            };
            MeasurementFields {
                semantic_hash: fnv1a64(semantic.as_bytes()),
                requested_interval_us: *requested_interval_us,
                due_us: *due_us,
                started_us: *started_us,
                finished_us: *finished_us,
                value,
                value_kind,
            }
        }
        CaptureEvent::ReadFailed {
            semantic,
            requested_interval_us,
            timing,
            ..
        } => MeasurementFields {
            semantic_hash: fnv1a64(semantic.as_bytes()),
            requested_interval_us: *requested_interval_us,
            due_us: timing.map_or(0, |timing| timing.due_us),
            started_us: timing.map_or(0, |timing| timing.started_us),
            finished_us: timing.map_or(0, |timing| timing.finished_us),
            value: f64::NAN,
            value_kind: 0,
        },
        _ => MeasurementFields {
            value: f64::NAN,
            ..MeasurementFields::default()
        },
    }
}

fn event_time_us(event: &CaptureEvent, previous: u64) -> u64 {
    let observed = match event {
        CaptureEvent::ResponsesObserved { offset_us, .. } => *offset_us,
        CaptureEvent::ReadSucceeded { finished_us, .. } => Some(*finished_us),
        CaptureEvent::ReadFailed { timing, .. } => timing.map(|timing| timing.finished_us),
        CaptureEvent::SlotsSkipped { last_due_us, .. } => Some(*last_due_us),
        CaptureEvent::SessionStopped { offset_us } => Some(*offset_us),
        _ => None,
    };
    observed.unwrap_or(previous).max(previous)
}

fn event_kind(event: &CaptureEvent) -> u64 {
    match event {
        CaptureEvent::CaptureStarted { .. } => 1,
        CaptureEvent::SessionInitialized => 2,
        CaptureEvent::SubscriptionConfigured { .. } => 3,
        CaptureEvent::SupportDiscovery { .. } => 4,
        CaptureEvent::ProtocolNegotiationObserved { .. } => 5,
        CaptureEvent::ResponsesObserved { .. } => 6,
        CaptureEvent::ReadSucceeded { .. } => 7,
        CaptureEvent::ReadFailed { .. } => 8,
        CaptureEvent::SlotsSkipped { .. } => 9,
        CaptureEvent::SessionError { .. } => 10,
        CaptureEvent::RuntimeStateChanged { .. } => 11,
        CaptureEvent::ShutdownRequested => 12,
        CaptureEvent::SessionStopped { .. } => 13,
        CaptureEvent::DiagnosticJobStarted { .. } => 14,
        CaptureEvent::DiagnosticJobStep { .. } => 15,
        CaptureEvent::DiagnosticJobCompleted { .. } => 16,
        CaptureEvent::DiagnosticJobFailed { .. } => 17,
        CaptureEvent::DiagnosticJobCancelled { .. } => 18,
        CaptureEvent::DtcTransportObserved { .. } => 19,
        CaptureEvent::DtcObservation { .. } => 20,
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        std::env::temp_dir().join(format!("obdentic-mf4-{}-{nonce}.mf4", std::process::id()))
    }

    #[tokio::test]
    async fn writes_a_parseable_mf4_with_true_read_time_and_diagnostic_evidence() {
        let path = temp_path();
        let (sender, task) = start(&path).expect("start MF4 writer");
        sender
            .send(CaptureEvent::capture_started(
                Some(1_700_000_000_000),
                Some("test".into()),
            ))
            .await
            .unwrap();
        sender
            .send(CaptureEvent::ReadSucceeded {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                due_us: 1_000_000,
                started_us: 1_000_100,
                finished_us: 1_000_300,
                request_payload: vec![0x01, 0x0c],
                response_payload: vec![0x41, 0x0c, 0x1f, 0x40],
                value: CaptureValue::Number(2_000.0),
                unit: "rpm".into(),
                source: "obdii".into(),
                profile: "standard".into(),
                decoder: "sae-j1979".into(),
                provenance: "SAE J1979".into(),
            })
            .await
            .unwrap();
        drop(sender);
        task.await.unwrap().expect("finish MF4 writer");

        let bytes = std::fs::read(&path).expect("read MF4 file");
        assert!(bytes.starts_with(b"MDF     "));
        let parsed = mdf4_rs::MDF::from_file(path.to_str().unwrap()).expect("parse MF4 file");
        assert_eq!(parsed.channel_groups().len(), 1);
        let group = &parsed.channel_groups()[0];
        assert_eq!(group.channels().len(), 16);
        assert!(group.channels()[0].values().unwrap().len() >= 2);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn semantic_hash_is_stable() {
        assert_eq!(fnv1a64(b"engine.rpm"), 0xc6f08fcafe7d7828);
    }
}
