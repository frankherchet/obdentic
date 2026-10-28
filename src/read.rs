//! The complete, read-only request vocabulary exposed by the diagnostic core:
//! [`ReadRequest`], the [`Transaction`] it produces, and the private
//! [`DiagnosticTransport`] seam used to complete one.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{protocol, vehicle};

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
        self.signal.request().bytes()
    }

    pub(crate) fn pid(self) -> u8 {
        self.signal.request().pid()
    }

    pub(crate) fn data_len(self) -> usize {
        self.signal.request().data_len()
    }

    pub fn metadata(self) -> &'static vehicle::SignalMetadata {
        self.signal.metadata()
    }

    pub(crate) fn semantic(self) -> &'static str {
        self.metadata().semantic
    }

    pub(crate) fn unit(self) -> &'static str {
        self.metadata().unit
    }

    pub(crate) fn profile(self) -> &'static str {
        self.metadata().profile
    }

    pub(crate) fn value(self, response: &[u8]) -> Result<f64, String> {
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
    pub(crate) timestamp_ms: u128,
    pub(crate) source: String,
    pub(crate) profile: &'static str,
    pub(crate) semantic: &'static str,
    pub(crate) request: Vec<u8>,
    pub(crate) response: Vec<u8>,
    pub(crate) value: f64,
    pub(crate) unit: &'static str,
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn read_request_accessors_agree_with_operation_for_every_catalog_signal() {
        for signal in supported_signals() {
            let request = prepare_read(signal.metadata().semantic).unwrap();
            let protocol::ReadOperation::Mode01(mode) = request.operation() else {
                panic!("semantic ReadRequest must use Mode01");
            };
            assert_eq!(request.bytes(), mode.bytes());
            assert_eq!(request.pid(), mode.pid());
            assert_eq!(request.data_len(), mode.data_len());
        }
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
            (
                "engine.short_term_fuel_trim_bank1",
                [0x01, 0x06],
                "%",
                -100.0,
                99.21875,
                "powertrain",
            ),
            (
                "engine.long_term_fuel_trim_bank1",
                [0x01, 0x07],
                "%",
                -100.0,
                99.21875,
                "powertrain",
            ),
            (
                "engine.short_term_fuel_trim_bank2",
                [0x01, 0x08],
                "%",
                -100.0,
                99.21875,
                "powertrain",
            ),
            (
                "engine.long_term_fuel_trim_bank2",
                [0x01, 0x09],
                "%",
                -100.0,
                99.21875,
                "powertrain",
            ),
            (
                "engine.fuel_rail_gauge_pressure_low",
                [0x01, 0x0a],
                "kPa",
                0.0,
                765.0,
                "powertrain",
            ),
            (
                "engine.timing_advance",
                [0x01, 0x0e],
                "°",
                -64.0,
                63.5,
                "powertrain",
            ),
            (
                "engine.fuel_rail_pressure_relative",
                [0x01, 0x22],
                "kPa",
                0.0,
                5177.265,
                "powertrain",
            ),
            (
                "engine.evap_purge_commanded",
                [0x01, 0x2e],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "vehicle.fuel_tank_level",
                [0x01, 0x2f],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.catalyst_temperature_b1s1",
                [0x01, 0x3c],
                "°C",
                -40.0,
                6513.5,
                "powertrain",
            ),
            (
                "engine.catalyst_temperature_b2s1",
                [0x01, 0x3d],
                "°C",
                -40.0,
                6513.5,
                "powertrain",
            ),
            (
                "engine.catalyst_temperature_b1s2",
                [0x01, 0x3e],
                "°C",
                -40.0,
                6513.5,
                "powertrain",
            ),
            (
                "engine.catalyst_temperature_b2s2",
                [0x01, 0x3f],
                "°C",
                -40.0,
                6513.5,
                "powertrain",
            ),
            (
                "engine.absolute_load",
                [0x01, 0x43],
                "%",
                0.0,
                25700.0,
                "powertrain",
            ),
            (
                "engine.commanded_equivalence_ratio",
                [0x01, 0x44],
                "ratio",
                0.0,
                2.0,
                "powertrain",
            ),
            (
                "engine.throttle_position_b",
                [0x01, 0x47],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.throttle_position_c",
                [0x01, 0x48],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "vehicle.accelerator_pedal_f",
                [0x01, 0x4b],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.ethanol_fuel_percentage",
                [0x01, 0x52],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.fuel_rail_pressure_absolute",
                [0x01, 0x59],
                "kPa",
                0.0,
                655350.0,
                "powertrain",
            ),
            (
                "vehicle.relative_accelerator_pedal_position",
                [0x01, 0x5a],
                "%",
                0.0,
                100.0,
                "powertrain",
            ),
            (
                "engine.oil_temperature",
                [0x01, 0x5c],
                "°C",
                -40.0,
                215.0,
                "powertrain",
            ),
            (
                "engine.fuel_injection_timing",
                [0x01, 0x5d],
                "°",
                -210.0,
                301.9921875,
                "powertrain",
            ),
            (
                "engine.fuel_rate",
                [0x01, 0x5e],
                "L/h",
                0.0,
                3276.75,
                "powertrain",
            ),
            (
                "engine.driver_demand_torque_percent",
                [0x01, 0x61],
                "%",
                -125.0,
                130.0,
                "powertrain",
            ),
            (
                "engine.actual_torque_percent",
                [0x01, 0x62],
                "%",
                -125.0,
                130.0,
                "powertrain",
            ),
            (
                "engine.reference_torque",
                [0x01, 0x63],
                "N·m",
                0.0,
                65535.0,
                "powertrain",
            ),
            (
                "vehicle.odometer",
                [0x01, 0xa6],
                "km",
                0.0,
                429496729.5,
                "diagnostics",
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
    fn profile_catalog_holds_only_the_standard_profile() {
        assert_eq!(
            crate::supported_profiles()
                .iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>(),
            ["obd2-v1"]
        );
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
}
