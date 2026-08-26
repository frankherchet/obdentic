use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub mod ble;

/// The complete, read-only request vocabulary exposed by the diagnostic core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRequest {
    signal: Signal,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SignalMetadata {
    pub semantic: &'static str,
    pub request: [u8; 2],
    pub description: &'static str,
    pub subsystem: &'static str,
    pub unit: &'static str,
    pub provenance: &'static str,
    pub confidence: &'static str,
    pub hardware_validation: &'static str,
}

const SIGNALS: [SignalMetadata; 4] = [
    SignalMetadata {
        semantic: "engine.rpm",
        request: [0x01, 0x0c],
        description: "Engine speed.",
        subsystem: "powertrain",
        unit: "rpm",
        provenance: "SAE J1979 Mode 01 PID 0C",
        confidence: "standards-derived/offline-tested",
        hardware_validation: "rust-hardware-pending",
    },
    SignalMetadata {
        semantic: "engine.coolant_temperature",
        request: [0x01, 0x05],
        description: "Engine coolant temperature.",
        subsystem: "powertrain",
        unit: "°C",
        provenance: "SAE J1979 Mode 01 PID 05",
        confidence: "standards-derived/offline-tested",
        hardware_validation: "rust-hardware-pending",
    },
    SignalMetadata {
        semantic: "vehicle.speed",
        request: [0x01, 0x0d],
        description: "Vehicle speed.",
        subsystem: "powertrain",
        unit: "km/h",
        provenance: "SAE J1979 Mode 01 PID 0D",
        confidence: "standards-derived/offline-tested",
        hardware_validation: "rust-hardware-pending",
    },
    SignalMetadata {
        semantic: "engine.maf",
        request: [0x01, 0x10],
        description: "Mass air flow rate.",
        subsystem: "powertrain",
        unit: "g/s",
        provenance: "SAE J1979 Mode 01 PID 10",
        confidence: "standards-derived/offline-tested",
        hardware_validation: "rust-hardware-pending",
    },
];

pub fn supported_signals() -> &'static [SignalMetadata] {
    &SIGNALS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    EngineRpm,
    EngineCoolantTemperature,
    VehicleSpeed,
    EngineMaf,
}

impl ReadRequest {
    pub fn bytes(self) -> [u8; 2] {
        self.metadata().request
    }

    pub(crate) fn pid(self) -> u8 {
        self.metadata().request[1]
    }

    pub(crate) fn data_len(self) -> usize {
        match self.signal {
            Signal::EngineCoolantTemperature | Signal::VehicleSpeed => 1,
            Signal::EngineRpm | Signal::EngineMaf => 2,
        }
    }

    pub fn metadata(self) -> &'static SignalMetadata {
        match self.signal {
            Signal::EngineRpm => &SIGNALS[0],
            Signal::EngineCoolantTemperature => &SIGNALS[1],
            Signal::VehicleSpeed => &SIGNALS[2],
            Signal::EngineMaf => &SIGNALS[3],
        }
    }

    fn semantic(self) -> &'static str {
        self.metadata().semantic
    }

    fn unit(self) -> &'static str {
        self.metadata().unit
    }

    fn value(self, response: &[u8]) -> Result<f64, String> {
        let expected = self.data_len() + 2;
        if response.len() != expected || response[..2] != [0x41, self.pid()] {
            return Err(format!(
                "invalid OBD-II {} response: {}",
                self.semantic(),
                hex(response)
            ));
        }
        Ok(match self.signal {
            Signal::EngineRpm => u16::from_be_bytes([response[2], response[3]]) as f64 / 4.0,
            Signal::EngineCoolantTemperature => response[2] as f64 - 40.0,
            Signal::VehicleSpeed => response[2] as f64,
            Signal::EngineMaf => u16::from_be_bytes([response[2], response[3]]) as f64 / 100.0,
        })
    }

    pub fn complete(self, source: &str, response: Vec<u8>) -> Result<Transaction, String> {
        let value = self.value(&response)?;
        Ok(Transaction {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis(),
            source: source.into(),
            semantic: self.semantic(),
            request: self.bytes().into(),
            response,
            value,
            unit: self.unit(),
        })
    }
}

pub fn prepare_read(semantic: &str) -> Result<ReadRequest, String> {
    match semantic {
        "engine.rpm" => Ok(ReadRequest {
            signal: Signal::EngineRpm,
        }),
        "engine.coolant_temperature" => Ok(ReadRequest {
            signal: Signal::EngineCoolantTemperature,
        }),
        "vehicle.speed" => Ok(ReadRequest {
            signal: Signal::VehicleSpeed,
        }),
        "engine.maf" => Ok(ReadRequest {
            signal: Signal::EngineMaf,
        }),
        _ => Err(format!(
            "read-only core rejected unsupported signal: {semantic}"
        )),
    }
}

#[derive(Debug, PartialEq)]
pub struct Transaction {
    pub timestamp_ms: u128,
    pub source: String,
    pub semantic: &'static str,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
    pub value: f64,
    pub unit: &'static str,
}

pub fn record(path: &Path, transaction: &Transaction) -> Result<(), String> {
    if transaction.source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(transaction.semantic)?;
    if transaction.request != request.bytes() || transaction.unit != request.unit() {
        return Err("recording request or unit does not match its semantic signal".into());
    }
    let value = request.value(&transaction.response)?;
    if transaction.value != value {
        return Err("recording value does not match its raw response".into());
    }
    let transport = "ble-elm327-ffe1";
    for (field, value) in [
        ("transport", transport),
        ("source", transaction.source.as_str()),
        ("semantic", transaction.semantic),
        ("unit", transaction.unit),
    ] {
        reject_record_control(field, value)?;
    }
    let contents = format!(
        "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\t{transport}\ntimestamp_ms\t{}\nsource\t{}\nsemantic\t{}\nrequest\t{}\nresponse\t{}\nvalue\t{}\nunit\t{}\n",
        transaction.timestamp_ms,
        transaction.source,
        request.semantic(),
        hex(&transaction.request),
        hex(&transaction.response),
        value,
        request.unit(),
    );
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = std::io::Write::write_all(&mut file, contents.as_bytes())
        .and_then(|_| std::io::Write::flush(&mut file))
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    drop(file);

    let result = fs::hard_link(&temporary, path).and_then(|_| fs::remove_file(&temporary));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

pub fn replay(path: &Path) -> Result<Transaction, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut lines = contents.lines();
    if lines.next() != Some("OBDENTIC\t1") {
        return Err("unsupported recording format".into());
    }
    let mut fields = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once('\t')
            .ok_or_else(|| "malformed recording field".to_string())?;
        if ![
            "profile",
            "transport",
            "timestamp_ms",
            "source",
            "semantic",
            "request",
            "response",
            "value",
            "unit",
        ]
        .contains(&name)
        {
            return Err(format!("recording contains unsupported field {name}"));
        }
        reject_record_control("field name", name)?;
        reject_record_control(name, value)?;
        if fields.insert(name, value).is_some() {
            return Err(format!("recording contains duplicate {name}"));
        }
    }
    if required(&fields, "profile")? != "obd2-v1" {
        return Err("recording profile or unit is unsupported".into());
    }
    if required(&fields, "transport")? != "ble-elm327-ffe1" {
        return Err("recording transport is unsupported".into());
    }
    let source = required(&fields, "source")?;
    if source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(required(&fields, "semantic")?)?;
    if required(&fields, "request")? != hex(&request.bytes())
        || required(&fields, "unit")? != request.unit()
    {
        return Err("recording request or unit does not match its semantic signal".into());
    }
    let response = parse_hex(required(&fields, "response")?)?;
    let mut transaction = request.complete(source, response)?;
    let stored_value = required(&fields, "value")?
        .parse::<f64>()
        .map_err(|error| error.to_string())?;
    if stored_value != transaction.value {
        return Err("recording value does not match its raw response".into());
    }
    transaction.timestamp_ms = required(&fields, "timestamp_ms")?
        .parse::<u128>()
        .map_err(|error| error.to_string())?;
    Ok(transaction)
}

fn reject_record_control(field: &str, value: &str) -> Result<(), String> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\t' | b'\r' | b'\n'))
    {
        return Err(format!("recording {field} contains a tab or newline"));
    }
    Ok(())
}

fn required<'a>(fields: &'a BTreeMap<&str, &str>, name: &str) -> Result<&'a str, String> {
    fields
        .get(name)
        .copied()
        .ok_or_else(|| format!("recording is missing {name}"))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_hex(value: &str) -> Result<Vec<u8>, String> {
    value
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).map_err(|error| error.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-{label}-{}-{nonce}.tsv",
            std::process::id()
        ))
    }

    #[test]
    fn closed_read_requests_decode_standard_mode01_values() {
        for (semantic, bytes, response, value, unit) in [
            (
                "engine.rpm",
                [0x01, 0x0c],
                &[0x41, 0x0c, 0x1a, 0xf8][..],
                1726.0,
                "rpm",
            ),
            (
                "engine.coolant_temperature",
                [0x01, 0x05],
                &[0x41, 0x05, 0x5a][..],
                50.0,
                "°C",
            ),
            (
                "vehicle.speed",
                [0x01, 0x0d],
                &[0x41, 0x0d, 0x64][..],
                100.0,
                "km/h",
            ),
            (
                "engine.maf",
                [0x01, 0x10],
                &[0x41, 0x10, 0x01, 0xf4][..],
                5.0,
                "g/s",
            ),
        ] {
            let request = prepare_read(semantic).unwrap();
            assert_eq!(request.bytes(), bytes);
            assert_eq!(request.data_len(), response.len() - 2);
            let transaction = request.complete("user", response.into()).unwrap();
            assert_eq!(transaction.semantic, semantic);
            assert_eq!(transaction.value, value);
            assert_eq!(transaction.unit, unit);
        }

        assert_eq!(
            prepare_read("dtc.clear"),
            Err("read-only core rejected unsupported signal: dtc.clear".into())
        );
    }

    #[test]
    fn supported_signal_metadata_matches_the_closed_request_vocabulary() {
        let expected = [
            ("engine.rpm", [0x01, 0x0c], "rpm"),
            ("engine.coolant_temperature", [0x01, 0x05], "°C"),
            ("vehicle.speed", [0x01, 0x0d], "km/h"),
            ("engine.maf", [0x01, 0x10], "g/s"),
        ];
        assert_eq!(supported_signals().len(), expected.len());

        for (metadata, (semantic, bytes, unit)) in supported_signals().iter().zip(expected) {
            let request = prepare_read(semantic).unwrap();
            assert_eq!(request.metadata(), metadata);
            assert_eq!(request.bytes(), bytes);
            assert_eq!(metadata.semantic, semantic);
            assert_eq!(metadata.unit, unit);
            assert_eq!(metadata.subsystem, "powertrain");
            assert!(metadata.description.ends_with('.'));
            assert!(metadata.provenance.starts_with("SAE J1979"));
            assert_eq!(metadata.confidence, "standards-derived/offline-tested");
            assert_eq!(metadata.hardware_validation, "rust-hardware-pending");
        }
    }

    #[test]
    fn decoders_cover_standard_raw_bounds_and_reject_wrong_responses() {
        for (semantic, response, value) in [
            ("engine.rpm", &[0x41, 0x0c, 0xff, 0xff][..], 16383.75),
            ("engine.coolant_temperature", &[0x41, 0x05, 0x00][..], -40.0),
            ("engine.coolant_temperature", &[0x41, 0x05, 0xff][..], 215.0),
            ("vehicle.speed", &[0x41, 0x0d, 0xff][..], 255.0),
            ("engine.maf", &[0x41, 0x10, 0xff, 0xff][..], 655.35),
        ] {
            assert_eq!(
                prepare_read(semantic)
                    .unwrap()
                    .complete("user", response.into())
                    .unwrap()
                    .value,
                value
            );
        }

        for semantic in [
            "engine.rpm",
            "engine.coolant_temperature",
            "vehicle.speed",
            "engine.maf",
        ] {
            assert!(prepare_read(semantic)
                .unwrap()
                .complete("user", vec![0x41, 0xff])
                .is_err());
        }
    }

    #[test]
    fn replay_recomputes_each_signal_from_raw_bytes() {
        for (semantic, request, response, value, unit) in [
            ("engine.rpm", "01 0C", "41 0C 00 00", 0.0, "rpm"),
            (
                "engine.coolant_temperature",
                "01 05",
                "41 05 5A",
                50.0,
                "°C",
            ),
            ("vehicle.speed", "01 0D", "41 0D 64", 100.0, "km/h"),
            ("engine.maf", "01 10", "41 10 01 F4", 5.0, "g/s"),
        ] {
            let path = temp_path(semantic);
            fs::write(
                &path,
                format!(
                    "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\t{semantic}\nrequest\t{request}\nresponse\t{response}\nvalue\t{value}\nunit\t{unit}\n"
                ),
            )
            .unwrap();
            let replayed = replay(&path).unwrap();
            assert_eq!(replayed.timestamp_ms, 1);
            assert_eq!(replayed.value, value);
            fs::remove_file(path).unwrap();
        }

        let path = temp_path("inconsistent");
        fs::write(
            &path,
            "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\tengine.maf\nrequest\t01 0C\nresponse\t41 10 01 F4\nvalue\t5\nunit\trpm\n",
        )
        .unwrap();
        assert!(replay(&path).is_err());
        fs::remove_file(path).unwrap();

        let path = temp_path("wrong-value");
        fs::write(
            &path,
            "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\tengine.rpm\nrequest\t01 0C\nresponse\t41 0C 00 00\nvalue\t999\nunit\trpm\n",
        )
        .unwrap();
        assert!(replay(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn record_is_private_and_does_not_overwrite_or_leave_temp_on_failure() {
        let path = temp_path("existing");
        let temporary = path.with_extension("tmp");
        fs::write(&path, "keep this recording").unwrap();
        let mut transaction = Transaction {
            timestamp_ms: 1,
            source: "user".into(),
            semantic: "engine.rpm",
            request: vec![0x01, 0x0c],
            response: vec![0x41, 0x0c, 0x00, 0x00],
            value: 0.0,
            unit: "rpm",
        };

        assert!(record(&path, &transaction).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep this recording");
        assert!(!temporary.exists());

        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            record(&path, &transaction).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::remove_file(path).unwrap();
        }

        transaction.value = 1.0;
        let path = temp_path("wrong-value");
        assert!(record(&path, &transaction).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn record_rejects_tabs_and_newlines_in_text_fields() {
        let path = temp_path("invalid");
        let transaction = Transaction {
            timestamp_ms: 1,
            source: "user\tspoof".into(),
            semantic: "engine.rpm",
            request: vec![0x01, 0x0c],
            response: vec![0x41, 0x0c, 0x00, 0x00],
            value: 0.0,
            unit: "rpm",
        };

        assert!(record(&path, &transaction).is_err());
        assert!(!path.exists());
        assert!(!path.with_extension("tmp").exists());
    }
}
