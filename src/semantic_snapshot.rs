//! Immutable, transport-free semantic snapshots for generic engine facts.

use crate::{telemetry::TelemetryState, SignalMetadata};
use std::time::Duration;

/// The generic engine signals included in an [`EngineSnapshot`], in stable order.
pub const ENGINE_SIGNALS: [&str; 8] = [
    "engine.rpm",
    "vehicle.speed",
    "engine.coolant_temperature",
    "engine.maf",
    "engine.load",
    "engine.intake_manifold_pressure",
    "engine.intake_air_temperature",
    "engine.control_module_voltage",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotStatus {
    Available,
    NotSampled,
    Stale,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotSample {
    timestamp_ms: u128,
    age_ms: u128,
    value: f64,
    unit: &'static str,
}

impl SnapshotSample {
    pub const fn timestamp_ms(&self) -> u128 {
        self.timestamp_ms
    }

    pub const fn age_ms(&self) -> u128 {
        self.age_ms
    }

    pub const fn value(&self) -> f64 {
        self.value
    }

    pub const fn unit(&self) -> &'static str {
        self.unit
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotField {
    semantic: &'static str,
    status: SnapshotStatus,
    metadata: Option<&'static SignalMetadata>,
    sample: Option<SnapshotSample>,
}

impl SnapshotField {
    pub const fn semantic(&self) -> &'static str {
        self.semantic
    }

    pub const fn status(&self) -> SnapshotStatus {
        self.status
    }

    pub const fn metadata(&self) -> Option<&'static SignalMetadata> {
        self.metadata
    }

    pub const fn sample(&self) -> Option<&SnapshotSample> {
        self.sample.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineSnapshot {
    reference_timestamp_ms: u128,
    stale_after_ms: u128,
    fields: [SnapshotField; ENGINE_SIGNALS.len()],
}

impl EngineSnapshot {
    /// Build a snapshot without reading or modifying the diagnostic state.
    pub fn from_telemetry(
        state: &TelemetryState,
        reference_timestamp_ms: u128,
        stale_after: Duration,
    ) -> Self {
        let stale_after_ms = stale_after.as_millis();
        let fields = ENGINE_SIGNALS.map(|semantic| {
            let metadata = crate::prepare_read(semantic)
                .ok()
                .map(|request| request.metadata());
            let Some(history) = state.history(semantic) else {
                return SnapshotField {
                    semantic,
                    status: if metadata.is_some() {
                        SnapshotStatus::NotSampled
                    } else {
                        SnapshotStatus::Unsupported
                    },
                    metadata,
                    sample: None,
                };
            };
            let Some(sample) = history.back() else {
                return SnapshotField {
                    semantic,
                    status: if metadata.is_some() {
                        SnapshotStatus::NotSampled
                    } else {
                        SnapshotStatus::Unsupported
                    },
                    metadata,
                    sample: None,
                };
            };
            let age_ms = reference_timestamp_ms.saturating_sub(sample.timestamp_ms);
            SnapshotField {
                semantic,
                status: if metadata.is_none() {
                    SnapshotStatus::Unsupported
                } else if age_ms > stale_after_ms {
                    SnapshotStatus::Stale
                } else {
                    SnapshotStatus::Available
                },
                metadata,
                sample: Some(SnapshotSample {
                    timestamp_ms: sample.timestamp_ms,
                    age_ms,
                    value: sample.value,
                    unit: sample.unit,
                }),
            }
        });
        Self {
            reference_timestamp_ms,
            stale_after_ms,
            fields,
        }
    }

    pub const fn reference_timestamp_ms(&self) -> u128 {
        self.reference_timestamp_ms
    }

    pub const fn stale_after_ms(&self) -> u128 {
        self.stale_after_ms
    }

    pub const fn fields(&self) -> &[SnapshotField; ENGINE_SIGNALS.len()] {
        &self.fields
    }

    pub fn field(&self, semantic: &str) -> Option<&SnapshotField> {
        self.fields.iter().find(|field| field.semantic == semantic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;

    fn state_with(semantic: &str, timestamp_ms: u128, response: Vec<u8>) -> TelemetryState {
        let transaction = prepare_read(semantic)
            .unwrap()
            .complete("user", response)
            .unwrap()
            .with_timestamp_ms(timestamp_ms);
        let mut state = TelemetryState::new(4).unwrap();
        state.ingest(&transaction);
        state
    }

    #[test]
    fn complete_field_preserves_metadata_sample_and_age() {
        let state = state_with("engine.rpm", 1_000, vec![0x41, 0x0c, 0x1a, 0xf8]);
        let snapshot = EngineSnapshot::from_telemetry(&state, 1_250, Duration::from_secs(1));
        let field = snapshot.field("engine.rpm").unwrap();
        let sample = field.sample().unwrap();

        assert_eq!(field.status(), SnapshotStatus::Available);
        assert_eq!(field.metadata().unwrap().semantic, "engine.rpm");
        assert_eq!(sample.timestamp_ms(), 1_000);
        assert_eq!(sample.age_ms(), 250);
        assert_eq!(sample.value(), 1726.0);
        assert_eq!(sample.unit(), "rpm");
    }

    #[test]
    fn missing_fields_are_explicitly_not_sampled() {
        let state = TelemetryState::new(4).unwrap();
        let snapshot = EngineSnapshot::from_telemetry(&state, 1_000, Duration::from_secs(1));
        let field = snapshot.field("engine.maf").unwrap();

        assert_eq!(field.status(), SnapshotStatus::NotSampled);
        assert!(field.sample().is_none());
        assert!(field.metadata().is_some());
    }

    #[test]
    fn old_samples_are_explicitly_stale() {
        let state = state_with("vehicle.speed", 100, vec![0x41, 0x0d, 0x32]);
        let snapshot = EngineSnapshot::from_telemetry(&state, 1_000, Duration::from_millis(500));
        let field = snapshot.field("vehicle.speed").unwrap();

        assert_eq!(field.status(), SnapshotStatus::Stale);
        assert_eq!(field.sample().unwrap().age_ms(), 900);
    }

    #[test]
    fn latest_sample_wins_with_uneven_timestamps() {
        let mut state = state_with("engine.rpm", 100, vec![0x41, 0x0c, 0x00, 0x04]);
        let later = prepare_read("engine.rpm")
            .unwrap()
            .complete("user", vec![0x41, 0x0c, 0x00, 0x08])
            .unwrap()
            .with_timestamp_ms(900);
        state.ingest(&later);

        let snapshot = EngineSnapshot::from_telemetry(&state, 1_000, Duration::from_secs(1));
        let sample = snapshot.field("engine.rpm").unwrap().sample().unwrap();
        assert_eq!(sample.timestamp_ms(), 900);
        assert_eq!(sample.age_ms(), 100);
        assert_eq!(sample.value(), 2.0);
    }

    #[test]
    fn snapshot_is_deterministic_and_has_stable_signal_order() {
        let state = state_with("engine.rpm", 100, vec![0x41, 0x0c, 0x00, 0x04]);
        let first = EngineSnapshot::from_telemetry(&state, 1_000, Duration::from_secs(1));
        let second = EngineSnapshot::from_telemetry(&state, 1_000, Duration::from_secs(1));

        assert_eq!(first, second);
        assert_eq!(
            first
                .fields()
                .iter()
                .map(SnapshotField::semantic)
                .collect::<Vec<_>>(),
            ENGINE_SIGNALS
        );
    }
}
