use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub(crate) mod adapter;
pub mod audit;
pub mod ble;
pub mod cache_validation;
pub mod capability;
pub mod capture;
pub mod capture_events;
pub mod capture_replay;
pub mod capture_report;
pub mod capture_tui;
pub mod compound_fact;
pub mod diagnostic_job;
pub mod dpf_report;
pub mod dtc;
pub mod ea189;
pub mod ecu_identification;
pub(crate) mod elm;
pub mod evidence;
pub mod functional_discovery;
pub mod identity;
pub mod jsonl_capture;
pub mod knowledge_db;
pub mod layout_observation;
pub mod protocol;
pub mod runtime_actor;
pub mod runtime_reducer;
pub mod runtime_state;
pub mod safety;
pub mod scheduler;
pub mod semantic_snapshot;
pub mod subscription_policy;
pub mod telemetry;
pub mod topology;
pub mod tui;
pub mod vehicle;
pub mod vehicle_cache;
pub mod vehicle_knowledge;

pub use identity::{
    decode_mode09_pid02, IdentityDecodeError, IdentitySource, Provenance, VehicleIdentity, Vin,
    VinError, MODE09_PID02_REQUEST,
};
pub use vehicle::{supported_profiles, ProfileMetadata, SignalMetadata};

/// The complete, read-only request vocabulary exposed by the diagnostic core.
#[derive(Clone, Copy, Debug)]
pub struct ReadRequest {
    signal: &'static vehicle::SignalDefinition,
}

impl PartialEq for ReadRequest {
    fn eq(&self, other: &Self) -> bool {
        self.metadata().semantic == other.metadata().semantic
    }
}

impl Eq for ReadRequest {}

impl ReadRequest {
    pub fn bytes(self) -> [u8; 2] {
        match self.operation() {
            protocol::ReadOperation::Mode01(mode) => mode.bytes(),
            protocol::ReadOperation::UdsReadDataByIdentifier { .. } => {
                unreachable!("semantic ReadRequest must use Mode01")
            }
        }
    }

    pub(crate) fn pid(self) -> u8 {
        match self.operation() {
            protocol::ReadOperation::Mode01(mode) => mode.pid(),
            protocol::ReadOperation::UdsReadDataByIdentifier { .. } => {
                unreachable!("semantic ReadRequest must use Mode01")
            }
        }
    }

    pub(crate) fn data_len(self) -> usize {
        match self.operation() {
            protocol::ReadOperation::Mode01(mode) => mode.data_len(),
            protocol::ReadOperation::UdsReadDataByIdentifier { .. } => {
                unreachable!("semantic ReadRequest must use Mode01")
            }
        }
    }

    pub fn metadata(self) -> &'static SignalMetadata {
        self.signal.metadata()
    }

    fn semantic(self) -> &'static str {
        self.metadata().semantic
    }

    fn unit(self) -> &'static str {
        self.metadata().unit
    }

    fn profile(self) -> &'static str {
        self.metadata().profile
    }

    fn value(self, response: &[u8]) -> Result<f64, String> {
        self.operation()
            .validate_response(response, self.semantic())
            .map_err(|error| error.to_string())?;
        self.signal.decode(response)
    }

    pub(crate) fn operation(self) -> protocol::ReadOperation {
        protocol::ReadOperation::Mode01(self.signal.request())
    }

    pub fn complete(self, source: &str, response: Vec<u8>) -> Result<Transaction, String> {
        let value = self.value(&response)?;
        Ok(Transaction {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis(),
            source: source.into(),
            profile: self.profile(),
            semantic: self.semantic(),
            request: self.bytes().into(),
            response,
            value,
            unit: self.unit(),
        })
    }
}

pub fn prepare_read(semantic: &str) -> Result<ReadRequest, String> {
    vehicle::signal(semantic)
        .map(|signal| ReadRequest { signal })
        .ok_or_else(|| format!("read-only core rejected unsupported signal: {semantic}"))
}

pub fn supported_signals() -> &'static [vehicle::SignalDefinition] {
    vehicle::signals()
}

#[derive(Debug, PartialEq)]
pub struct Transaction {
    timestamp_ms: u128,
    source: String,
    profile: &'static str,
    semantic: &'static str,
    request: Vec<u8>,
    response: Vec<u8>,
    value: f64,
    unit: &'static str,
}

impl Transaction {
    pub fn timestamp_ms(&self) -> u128 {
        self.timestamp_ms
    }

    pub fn with_timestamp_ms(mut self, timestamp_ms: u128) -> Self {
        self.timestamp_ms = timestamp_ms;
        self
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn profile(&self) -> &'static str {
        self.profile
    }

    pub fn semantic(&self) -> &'static str {
        self.semantic
    }

    pub fn request(&self) -> &[u8] {
        &self.request
    }

    pub fn response(&self) -> &[u8] {
        &self.response
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn unit(&self) -> &'static str {
        self.unit
    }
}

pub(crate) trait DiagnosticTransport {
    async fn read(&mut self, request: ReadRequest) -> Result<Vec<u8>, String>;
}

pub(crate) async fn read_transaction<T>(
    transport: &mut T,
    request: ReadRequest,
) -> Result<Transaction, String>
where
    T: DiagnosticTransport,
{
    request.complete("user", transport.read(request).await?)
}

pub fn record(path: &Path, transaction: &Transaction) -> Result<(), String> {
    if transaction.source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(transaction.semantic)?;
    if transaction.profile != request.profile()
        || transaction.request != request.bytes()
        || transaction.unit != request.unit()
    {
        return Err("recording profile, request or unit does not match its semantic signal".into());
    }
    let value = request.value(&transaction.response)?;
    if transaction.value != value {
        return Err("recording value does not match its raw response".into());
    }
    let transport = "ble-elm327-ffe1";
    for (field, value) in [
        ("transport", transport),
        ("source", transaction.source.as_str()),
        ("profile", transaction.profile),
        ("semantic", transaction.semantic),
        ("unit", transaction.unit),
    ] {
        reject_record_control(field, value)?;
    }
    let contents = format!(
        "OBDENTIC\t1\nprofile\t{}\ntransport\t{transport}\ntimestamp_ms\t{}\nsource\t{}\nsemantic\t{}\nrequest\t{}\nresponse\t{}\nvalue\t{}\nunit\t{}\n",
        transaction.profile,
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

pub async fn replay(path: &Path) -> Result<Transaction, String> {
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
    if required(&fields, "transport")? != "ble-elm327-ffe1" {
        return Err("recording transport is unsupported".into());
    }
    let source = required(&fields, "source")?;
    if source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(required(&fields, "semantic")?)?;
    if required(&fields, "profile")? != request.profile()
        || required(&fields, "request")? != hex(&request.bytes())
        || required(&fields, "unit")? != request.unit()
    {
        return Err("recording profile, request or unit does not match its semantic signal".into());
    }
    let response = parse_hex(required(&fields, "response")?)?;
    let mut transport = ReplayTransport {
        response: Some(response),
    };
    let mut transaction = read_transaction(&mut transport, request).await?;
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

struct ReplayTransport {
    response: Option<Vec<u8>>,
}

impl DiagnosticTransport for ReplayTransport {
    async fn read(&mut self, _request: ReadRequest) -> Result<Vec<u8>, String> {
        self.response
            .take()
            .ok_or_else(|| "recording has no remaining response".into())
    }
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
        let hardware_observed = [
            "engine.throttle_position",
            "vehicle.distance_with_mil_on",
            "engine.fuel_rail_gauge_pressure",
            "vehicle.warmups_since_dtc_clear",
            "vehicle.distance_since_dtc_clear",
            "vehicle.ambient_air_temperature",
            "engine.throttle_actuator.commanded",
        ];
        let expected = [
            (
                "engine.rpm",
                [0x01, 0x0c],
                "rpm",
                0.0,
                16383.75,
                "powertrain",
            ),
            (
                "engine.coolant_temperature",
                [0x01, 0x05],
                "°C",
                -40.0,
                215.0,
                "powertrain",
            ),
            (
                "vehicle.speed",
                [0x01, 0x0d],
                "km/h",
                0.0,
                255.0,
                "powertrain",
            ),
            ("engine.maf", [0x01, 0x10], "g/s", 0.0, 655.35, "powertrain"),
            ("engine.load", [0x01, 0x04], "%", 0.0, 100.0, "powertrain"),
            (
                "engine.intake_manifold_pressure",
                [0x01, 0x0b],
                "kPa",
                0.0,
                255.0,
                "powertrain",
            ),
            (
                "engine.intake_air_temperature",
                [0x01, 0x0f],
                "°C",
                -40.0,
                215.0,
                "powertrain",
            ),
            (
                "engine.egr.commanded",
                [0x01, 0x2c],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.egr.error",
                [0x01, 0x2d],
                "%",
                -100.0,
                99.21875,
                "powertrain",
            ),
            (
                "engine.runtime",
                [0x01, 0x1f],
                "s",
                0.0,
                65535.0,
                "powertrain",
            ),
            (
                "vehicle.accelerator_pedal_d",
                [0x01, 0x49],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "vehicle.accelerator_pedal_e",
                [0x01, 0x4a],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.relative_throttle",
                [0x01, 0x45],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.barometric_pressure",
                [0x01, 0x33],
                "kPa",
                0.0,
                255.0,
                "powertrain",
            ),
            (
                "engine.control_module_voltage",
                [0x01, 0x42],
                "V",
                0.0,
                65.535,
                "powertrain",
            ),
            (
                "engine.throttle_position",
                [0x01, 0x11],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "vehicle.distance_with_mil_on",
                [0x01, 0x21],
                "km",
                0.0,
                65535.0,
                "diagnostics",
            ),
            (
                "engine.fuel_rail_gauge_pressure",
                [0x01, 0x23],
                "kPa",
                0.0,
                655350.0,
                "powertrain",
            ),
            (
                "vehicle.warmups_since_dtc_clear",
                [0x01, 0x30],
                "count",
                0.0,
                255.0,
                "diagnostics",
            ),
            (
                "vehicle.distance_since_dtc_clear",
                [0x01, 0x31],
                "km",
                0.0,
                65535.0,
                "diagnostics",
            ),
            (
                "vehicle.ambient_air_temperature",
                [0x01, 0x46],
                "°C",
                -40.0,
                215.0,
                "environment",
            ),
            (
                "engine.throttle_actuator.commanded",
                [0x01, 0x4c],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
        ];
        assert_eq!(supported_signals().len(), expected.len());

        for (definition, (semantic, bytes, unit, minimum, maximum, subsystem)) in
            supported_signals().iter().zip(expected)
        {
            let metadata = definition.metadata();
            let request = prepare_read(semantic).unwrap();
            assert_eq!(request.metadata(), metadata);
            assert_eq!(request.bytes(), bytes);
            assert_eq!(metadata.semantic, semantic);
            assert_eq!(metadata.profile, "obd2-v1");
            assert_eq!(metadata.protocol, "OBD-II Mode 01");
            assert_eq!(metadata.unit, unit);
            assert_eq!((metadata.minimum, metadata.maximum), (minimum, maximum));
            assert_eq!(metadata.subsystem, subsystem);
            assert!(!metadata.decoder.is_empty());
            assert!(metadata.description.ends_with('.'));
            assert!(metadata.provenance.starts_with("SAE J1979"));
            assert_eq!(metadata.confidence, "standards-derived/offline-tested");
            assert_eq!(
                metadata.hardware_validation,
                if hardware_observed.contains(&semantic) {
                    "rust-hardware-observed"
                } else {
                    "rust-hardware-pending"
                }
            );
        }
    }

    #[test]
    fn profile_catalog_keeps_ea189_empty_until_evidence_exists() {
        assert_eq!(
            supported_profiles()
                .iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>(),
            ["obd2-v1", "vw-ea189-v1"]
        );
        let ea189 = &supported_profiles()[1];
        assert_eq!(ea189.confidence, "experimental");
        assert_eq!(ea189.hardware_validation, "hardware-evidence-required");
        assert!(prepare_read("dpf.diff_pressure").is_err());
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

    #[tokio::test]
    async fn replay_recomputes_each_signal_from_raw_bytes() {
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
            let replayed = replay(&path).await.unwrap();
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
        assert!(replay(&path).await.is_err());
        fs::remove_file(path).unwrap();

        let path = temp_path("wrong-value");
        fs::write(
            &path,
            "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\tengine.rpm\nrequest\t01 0C\nresponse\t41 0C 00 00\nvalue\t999\nunit\trpm\n",
        )
        .unwrap();
        assert!(replay(&path).await.is_err());
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
            profile: "obd2-v1",
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
            profile: "obd2-v1",
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
