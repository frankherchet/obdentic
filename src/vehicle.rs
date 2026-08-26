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

const SIGNALS: [SignalDefinition; 4] = [
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
