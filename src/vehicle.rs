use crate::protocol::Mode01;

#[derive(Debug, PartialEq)]
pub struct SignalMetadata {
    pub semantic: &'static str,
    pub profile: &'static str,
    pub protocol: &'static str,
    pub request: [u8; 2],
    pub decoder: &'static str,
    pub minimum: f64,
    pub maximum: f64,
    pub description: &'static str,
    pub subsystem: &'static str,
    pub unit: &'static str,
    pub provenance: &'static str,
    pub confidence: &'static str,
    pub hardware_validation: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProfileMetadata {
    pub id: &'static str,
    pub description: &'static str,
    pub confidence: &'static str,
    pub hardware_validation: &'static str,
}

#[derive(Debug)]
pub struct SignalDefinition {
    metadata: SignalMetadata,
    request: Mode01,
    decode: fn(&[u8]) -> f64,
}

const PROFILES: [ProfileMetadata; 2] = [
    ProfileMetadata {
        id: "obd2-v1",
        description: "Generic SAE J1979 Mode 01 signals.",
        confidence: "standards-derived/offline-tested",
        hardware_validation: "rust-hardware-pending",
    },
    ProfileMetadata {
        id: "vw-ea189-v1",
        description: "EA189 profile skeleton; no manufacturer-specific signals yet.",
        confidence: "experimental",
        hardware_validation: "hardware-evidence-required",
    },
];

const SIGNALS: [SignalDefinition; 22] = [
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.rpm",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x0c],
            decoder: "((A * 256) + B) / 4",
            minimum: 0.0,
            maximum: 16383.75,
            description: "Engine speed.",
            subsystem: "powertrain",
            unit: "rpm",
            provenance: "SAE J1979 Mode 01 PID 0C",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x0c, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64 / 4.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.coolant_temperature",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x05],
            decoder: "A - 40",
            minimum: -40.0,
            maximum: 215.0,
            description: "Engine coolant temperature.",
            subsystem: "powertrain",
            unit: "°C",
            provenance: "SAE J1979 Mode 01 PID 05",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x05, 1),
        |data| data[0] as f64 - 40.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.speed",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x0d],
            decoder: "A",
            minimum: 0.0,
            maximum: 255.0,
            description: "Vehicle speed.",
            subsystem: "powertrain",
            unit: "km/h",
            provenance: "SAE J1979 Mode 01 PID 0D",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x0d, 1),
        |data| data[0] as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.maf",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x10],
            decoder: "((A * 256) + B) / 100",
            minimum: 0.0,
            maximum: 655.35,
            description: "Mass air flow rate.",
            subsystem: "powertrain",
            unit: "g/s",
            provenance: "SAE J1979 Mode 01 PID 10",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x10, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64 / 100.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.load",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x04],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Calculated engine load.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 04",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x04, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.intake_manifold_pressure",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x0b],
            decoder: "A",
            minimum: 0.0,
            maximum: 255.0,
            description: "Intake manifold absolute pressure.",
            subsystem: "powertrain",
            unit: "kPa",
            provenance: "SAE J1979 Mode 01 PID 0B",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x0b, 1),
        |data| data[0] as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.intake_air_temperature",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x0f],
            decoder: "A - 40",
            minimum: -40.0,
            maximum: 215.0,
            description: "Intake air temperature.",
            subsystem: "powertrain",
            unit: "°C",
            provenance: "SAE J1979 Mode 01 PID 0F",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x0f, 1),
        |data| data[0] as f64 - 40.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.egr.commanded",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x2c],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Commanded exhaust gas recirculation.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 2C",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x2c, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.egr.error",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x2d],
            decoder: "(A - 128) * 100 / 128",
            minimum: -100.0,
            maximum: 99.21875,
            description: "Exhaust gas recirculation error.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 2D",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x2d, 1),
        |data| (data[0] as f64 - 128.0) * 100.0 / 128.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.runtime",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x1f],
            decoder: "(A * 256) + B",
            minimum: 0.0,
            maximum: 65535.0,
            description: "Engine run time since start.",
            subsystem: "powertrain",
            unit: "s",
            provenance: "SAE J1979 Mode 01 PID 1F",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x1f, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.accelerator_pedal_d",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x49],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Accelerator pedal position D.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 49",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x49, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.accelerator_pedal_e",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x4a],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Accelerator pedal position E.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 4A",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x4a, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.relative_throttle",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x45],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Relative throttle position.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 45",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x45, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.barometric_pressure",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x33],
            decoder: "A",
            minimum: 0.0,
            maximum: 255.0,
            description: "Barometric pressure.",
            subsystem: "powertrain",
            unit: "kPa",
            provenance: "SAE J1979 Mode 01 PID 33",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x33, 1),
        |data| data[0] as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.control_module_voltage",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x42],
            decoder: "((A * 256) + B) / 1000",
            minimum: 0.0,
            maximum: 65.535,
            description: "Control module voltage.",
            subsystem: "powertrain",
            unit: "V",
            provenance: "SAE J1979 Mode 01 PID 42",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-pending",
        },
        Mode01::new(0x42, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64 / 1000.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.throttle_position",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x11],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Throttle position.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 11",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x11, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.distance_with_mil_on",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x21],
            decoder: "(A * 256) + B",
            minimum: 0.0,
            maximum: 65535.0,
            description: "Distance traveled while the malfunction indicator lamp is on.",
            subsystem: "diagnostics",
            unit: "km",
            provenance: "SAE J1979 Mode 01 PID 21",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x21, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.fuel_rail_gauge_pressure",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x23],
            decoder: "((A * 256) + B) * 10",
            minimum: 0.0,
            maximum: 655350.0,
            description: "Fuel rail gauge pressure for diesel/direct-injection systems.",
            subsystem: "powertrain",
            unit: "kPa",
            provenance: "SAE J1979 Mode 01 PID 23",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x23, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64 * 10.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.warmups_since_dtc_clear",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x30],
            decoder: "A",
            minimum: 0.0,
            maximum: 255.0,
            description: "Warm-up cycles since diagnostic trouble codes were cleared.",
            subsystem: "diagnostics",
            unit: "count",
            provenance: "SAE J1979 Mode 01 PID 30",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x30, 1),
        |data| data[0] as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.distance_since_dtc_clear",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x31],
            decoder: "(A * 256) + B",
            minimum: 0.0,
            maximum: 65535.0,
            description: "Distance traveled since diagnostic trouble codes were cleared.",
            subsystem: "diagnostics",
            unit: "km",
            provenance: "SAE J1979 Mode 01 PID 31",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x31, 2),
        |data| u16::from_be_bytes([data[0], data[1]]) as f64,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "vehicle.ambient_air_temperature",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x46],
            decoder: "A - 40",
            minimum: -40.0,
            maximum: 215.0,
            description: "Ambient air temperature.",
            subsystem: "environment",
            unit: "°C",
            provenance: "SAE J1979 Mode 01 PID 46",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x46, 1),
        |data| data[0] as f64 - 40.0,
    ),
    SignalDefinition::new(
        SignalMetadata {
            semantic: "engine.throttle_actuator.commanded",
            profile: "obd2-v1",
            protocol: "OBD-II Mode 01",
            request: [0x01, 0x4c],
            decoder: "A * 100 / 255",
            minimum: 0.0,
            maximum: 100.0,
            description: "Commanded throttle actuator position.",
            subsystem: "powertrain",
            unit: "%",
            provenance: "SAE J1979 Mode 01 PID 4C",
            confidence: "standards-derived/offline-tested",
            hardware_validation: "rust-hardware-observed",
        },
        Mode01::new(0x4c, 1),
        |data| data[0] as f64 * 100.0 / 255.0,
    ),
];

impl SignalDefinition {
    const fn new(metadata: SignalMetadata, request: Mode01, decode: fn(&[u8]) -> f64) -> Self {
        Self {
            metadata,
            request,
            decode,
        }
    }

    pub fn metadata(&self) -> &SignalMetadata {
        &self.metadata
    }

    pub(crate) fn request(&self) -> Mode01 {
        self.request
    }

    pub(crate) fn decode(&self, response: &[u8]) -> Result<f64, String> {
        let data = self.request.data(response, self.metadata.semantic)?;
        Ok((self.decode)(data))
    }
}

pub(crate) fn signal(semantic: &str) -> Option<&'static SignalDefinition> {
    SIGNALS
        .iter()
        .find(|definition| definition.metadata.semantic == semantic)
}

pub(crate) fn signals() -> &'static [SignalDefinition] {
    &SIGNALS
}

pub fn supported_profiles() -> &'static [ProfileMetadata] {
    &PROFILES
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{signal, signals};
    use crate::prepare_read;

    #[test]
    fn mode01_scalar_signals_decode_normal_and_boundary_values() {
        for (semantic, normal, normal_value, minimum, minimum_value, maximum, maximum_value) in [
            (
                "engine.load",
                vec![0x41, 0x04, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x04, 0x00],
                0.0,
                vec![0x41, 0x04, 0xff],
                100.0,
            ),
            (
                "engine.intake_manifold_pressure",
                vec![0x41, 0x0b, 0x40],
                64.0,
                vec![0x41, 0x0b, 0x00],
                0.0,
                vec![0x41, 0x0b, 0xff],
                255.0,
            ),
            (
                "engine.intake_air_temperature",
                vec![0x41, 0x0f, 0x80],
                88.0,
                vec![0x41, 0x0f, 0x00],
                -40.0,
                vec![0x41, 0x0f, 0xff],
                215.0,
            ),
            (
                "engine.egr.commanded",
                vec![0x41, 0x2c, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x2c, 0x00],
                0.0,
                vec![0x41, 0x2c, 0xff],
                100.0,
            ),
            (
                "engine.egr.error",
                vec![0x41, 0x2d, 0x80],
                0.0,
                vec![0x41, 0x2d, 0x00],
                -100.0,
                vec![0x41, 0x2d, 0xff],
                127.0 * 100.0 / 128.0,
            ),
            (
                "engine.runtime",
                vec![0x41, 0x1f, 0x01, 0x02],
                258.0,
                vec![0x41, 0x1f, 0x00, 0x00],
                0.0,
                vec![0x41, 0x1f, 0xff, 0xff],
                65535.0,
            ),
            (
                "vehicle.accelerator_pedal_d",
                vec![0x41, 0x49, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x49, 0x00],
                0.0,
                vec![0x41, 0x49, 0xff],
                100.0,
            ),
            (
                "vehicle.accelerator_pedal_e",
                vec![0x41, 0x4a, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x4a, 0x00],
                0.0,
                vec![0x41, 0x4a, 0xff],
                100.0,
            ),
            (
                "engine.relative_throttle",
                vec![0x41, 0x45, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x45, 0x00],
                0.0,
                vec![0x41, 0x45, 0xff],
                100.0,
            ),
            (
                "engine.barometric_pressure",
                vec![0x41, 0x33, 0x40],
                64.0,
                vec![0x41, 0x33, 0x00],
                0.0,
                vec![0x41, 0x33, 0xff],
                255.0,
            ),
            (
                "engine.control_module_voltage",
                vec![0x41, 0x42, 0x12, 0x34],
                4.66,
                vec![0x41, 0x42, 0x00, 0x00],
                0.0,
                vec![0x41, 0x42, 0xff, 0xff],
                65.535,
            ),
            (
                "engine.throttle_position",
                vec![0x41, 0x11, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x11, 0x00],
                0.0,
                vec![0x41, 0x11, 0xff],
                100.0,
            ),
            (
                "vehicle.distance_with_mil_on",
                vec![0x41, 0x21, 0x01, 0x02],
                258.0,
                vec![0x41, 0x21, 0x00, 0x00],
                0.0,
                vec![0x41, 0x21, 0xff, 0xff],
                65535.0,
            ),
            (
                "engine.fuel_rail_gauge_pressure",
                vec![0x41, 0x23, 0x01, 0x02],
                2580.0,
                vec![0x41, 0x23, 0x00, 0x00],
                0.0,
                vec![0x41, 0x23, 0xff, 0xff],
                655350.0,
            ),
            (
                "vehicle.warmups_since_dtc_clear",
                vec![0x41, 0x30, 0x2a],
                42.0,
                vec![0x41, 0x30, 0x00],
                0.0,
                vec![0x41, 0x30, 0xff],
                255.0,
            ),
            (
                "vehicle.distance_since_dtc_clear",
                vec![0x41, 0x31, 0x01, 0x02],
                258.0,
                vec![0x41, 0x31, 0x00, 0x00],
                0.0,
                vec![0x41, 0x31, 0xff, 0xff],
                65535.0,
            ),
            (
                "vehicle.ambient_air_temperature",
                vec![0x41, 0x46, 0x80],
                88.0,
                vec![0x41, 0x46, 0x00],
                -40.0,
                vec![0x41, 0x46, 0xff],
                215.0,
            ),
            (
                "engine.throttle_actuator.commanded",
                vec![0x41, 0x4c, 0x80],
                128.0 * 100.0 / 255.0,
                vec![0x41, 0x4c, 0x00],
                0.0,
                vec![0x41, 0x4c, 0xff],
                100.0,
            ),
        ] {
            let definition = signal(semantic).expect("signal is catalogued");
            assert_eq!(
                definition.decode(&normal).unwrap(),
                normal_value,
                "{semantic}"
            );
            assert_eq!(
                definition.decode(&minimum).unwrap(),
                minimum_value,
                "{semantic}"
            );
            assert_eq!(
                definition.decode(&maximum).unwrap(),
                maximum_value,
                "{semantic}"
            );
        }
    }

    #[test]
    fn every_catalog_signal_rejects_truncated_wrong_service_and_wrong_pid_responses() {
        for definition in signals() {
            let metadata = definition.metadata();
            let response_len = definition.request.data_len() + 2;

            let mut truncated = vec![0x41, metadata.request[1]];
            truncated.resize(response_len.saturating_sub(1), 0);
            assert!(
                definition.decode(&truncated).is_err(),
                "truncated response accepted for {}",
                metadata.semantic
            );

            let mut wrong_service = vec![0x42, metadata.request[1]];
            wrong_service.resize(response_len, 0);
            assert!(
                definition.decode(&wrong_service).is_err(),
                "wrong service accepted for {}",
                metadata.semantic
            );

            let mut wrong_pid = vec![0x41, metadata.request[1].wrapping_add(1)];
            wrong_pid.resize(response_len, 0);
            assert!(
                definition.decode(&wrong_pid).is_err(),
                "wrong PID accepted for {}",
                metadata.semantic
            );
        }
    }

    #[test]
    fn catalog_semantics_have_expected_units_and_unique_read_only_requests() {
        let expected = [
            ("engine.rpm", "rpm"),
            ("engine.coolant_temperature", "°C"),
            ("vehicle.speed", "km/h"),
            ("engine.maf", "g/s"),
            ("engine.load", "%"),
            ("engine.intake_manifold_pressure", "kPa"),
            ("engine.intake_air_temperature", "°C"),
            ("engine.egr.commanded", "%"),
            ("engine.egr.error", "%"),
            ("engine.runtime", "s"),
            ("vehicle.accelerator_pedal_d", "%"),
            ("vehicle.accelerator_pedal_e", "%"),
            ("engine.relative_throttle", "%"),
            ("engine.barometric_pressure", "kPa"),
            ("engine.control_module_voltage", "V"),
            ("engine.throttle_position", "%"),
            ("vehicle.distance_with_mil_on", "km"),
            ("engine.fuel_rail_gauge_pressure", "kPa"),
            ("vehicle.warmups_since_dtc_clear", "count"),
            ("vehicle.distance_since_dtc_clear", "km"),
            ("vehicle.ambient_air_temperature", "°C"),
            ("engine.throttle_actuator.commanded", "%"),
        ];

        assert_eq!(signals().len(), expected.len());
        let mut semantics = BTreeSet::new();
        for definition in signals() {
            let metadata = definition.metadata();
            let (_, unit) = expected
                .iter()
                .find(|(semantic, _)| *semantic == metadata.semantic)
                .unwrap_or_else(|| panic!("unexpected catalog semantic: {}", metadata.semantic));
            assert_eq!(metadata.unit, *unit, "{}", metadata.semantic);
            assert!(semantics.insert(metadata.semantic), "duplicate semantic");
            assert_eq!(metadata.request[0], 0x01, "{}", metadata.semantic);
            assert_eq!(metadata.protocol, "OBD-II Mode 01", "{}", metadata.semantic);
        }
    }

    #[test]
    fn issue_49_signals_have_expected_requests_and_provenance() {
        for (semantic, pid) in [
            ("engine.throttle_position", 0x11),
            ("vehicle.distance_with_mil_on", 0x21),
            ("engine.fuel_rail_gauge_pressure", 0x23),
            ("vehicle.warmups_since_dtc_clear", 0x30),
            ("vehicle.distance_since_dtc_clear", 0x31),
            ("vehicle.ambient_air_temperature", 0x46),
            ("engine.throttle_actuator.commanded", 0x4c),
        ] {
            let definition = signal(semantic).expect("issue #49 signal is catalogued");
            let metadata = definition.metadata();
            assert_eq!(metadata.request, [0x01, pid]);
            assert_eq!(prepare_read(semantic).unwrap().bytes(), [0x01, pid]);
            assert_eq!(metadata.profile, "obd2-v1");
            assert_eq!(metadata.confidence, "standards-derived/offline-tested");
            assert_eq!(metadata.hardware_validation, "rust-hardware-observed");
            assert_eq!(
                metadata.provenance,
                match pid {
                    0x11 => "SAE J1979 Mode 01 PID 11",
                    0x21 => "SAE J1979 Mode 01 PID 21",
                    0x23 => "SAE J1979 Mode 01 PID 23",
                    0x30 => "SAE J1979 Mode 01 PID 30",
                    0x31 => "SAE J1979 Mode 01 PID 31",
                    0x46 => "SAE J1979 Mode 01 PID 46",
                    0x4c => "SAE J1979 Mode 01 PID 4C",
                    _ => unreachable!(),
                }
            );
        }
    }
}
