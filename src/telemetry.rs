use crate::Transaction;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub timestamp_ms: u128,
    pub value: f64,
    pub unit: &'static str,
}

#[derive(Clone)]
pub struct TelemetryState {
    capacity: usize,
    samples: BTreeMap<&'static str, VecDeque<Sample>>,
    timestamps_us: BTreeMap<&'static str, VecDeque<u128>>,
}

impl TelemetryState {
    pub fn new(capacity: usize) -> Result<Self, String> {
        (capacity > 0)
            .then_some(Self {
                capacity,
                samples: BTreeMap::new(),
                timestamps_us: BTreeMap::new(),
            })
            .ok_or_else(|| "telemetry capacity must be greater than zero".into())
    }

    pub fn ingest(&mut self, transaction: &Transaction) {
        self.ingest_at_us(
            transaction,
            transaction.timestamp_ms().saturating_mul(1_000),
        );
    }

    /// Ingest a decoded transaction while preserving an exact monotonic timestamp.
    ///
    /// Live callers normally use [`Self::ingest`]. Offline capture replay uses this
    /// method so JSONL microsecond offsets remain canonical even though the legacy
    /// [`Sample::timestamp_ms`] compatibility field is only millisecond precision.
    pub fn ingest_at_us(&mut self, transaction: &Transaction, timestamp_us: u128) {
        let semantic = transaction.semantic();
        let samples = self.samples.entry(semantic).or_default();
        let timestamps = self.timestamps_us.entry(semantic).or_default();
        debug_assert_eq!(samples.len(), timestamps.len());

        let sample = Sample {
            timestamp_ms: timestamp_us / 1_000,
            value: transaction.value(),
            unit: transaction.unit(),
        };
        let position = timestamps
            .iter()
            .position(|current| *current > timestamp_us)
            .unwrap_or(timestamps.len());
        timestamps.insert(position, timestamp_us);
        samples.insert(position, sample);
        if samples.len() > self.capacity {
            samples.pop_front();
            timestamps.pop_front();
        }
        debug_assert_eq!(samples.len(), timestamps.len());
    }

    pub fn current(&self, semantic: &str) -> Option<&Sample> {
        self.samples.get(semantic)?.back()
    }

    pub fn history(&self, semantic: &str) -> Option<&VecDeque<Sample>> {
        self.samples.get(semantic)
    }

    /// Iterate a signal history with its exact timestamp in microseconds.
    ///
    /// The timestamps are kept in lock-step with [`Self::history`]. This is the
    /// canonical time axis for offline JSONL replay and avoids collapsing distinct
    /// samples that happen within the same millisecond.
    pub fn timed_history(
        &self,
        semantic: &str,
    ) -> Option<impl Iterator<Item = (u128, &Sample)> + '_> {
        let samples = self.samples.get(semantic)?;
        let timestamps = self.timestamps_us.get(semantic)?;
        debug_assert_eq!(samples.len(), timestamps.len());
        Some(timestamps.iter().copied().zip(samples.iter()))
    }

    pub fn signals(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.samples.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;

    fn transaction(timestamp_ms: u128, response: Vec<u8>) -> Transaction {
        let mut transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("user", response)
            .unwrap();
        transaction.timestamp_ms = timestamp_ms;
        transaction
    }

    #[test]
    fn ingests_current_history_and_evicts_oldest_samples() {
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&transaction(1, vec![0x41, 0x0c, 0x00, 0x04]));
        state.ingest(&transaction(2, vec![0x41, 0x0c, 0x00, 0x08]));
        state.ingest(&transaction(3, vec![0x41, 0x0c, 0x00, 0x0c]));

        assert_eq!(state.current("engine.rpm").unwrap().value, 3.0);
        assert_eq!(
            state
                .history("engine.rpm")
                .unwrap()
                .iter()
                .map(|sample| sample.timestamp_ms)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(
            state
                .timed_history("engine.rpm")
                .unwrap()
                .map(|(timestamp_us, _)| timestamp_us)
                .collect::<Vec<_>>(),
            [2_000, 3_000]
        );
        assert_eq!(state.signals().collect::<Vec<_>>(), ["engine.rpm"]);
    }

    #[test]
    fn keeps_signals_independent_and_handles_unknown_signals() {
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&transaction(1, vec![0x41, 0x0c, 0x00, 0x04]));
        let speed = prepare_read("vehicle.speed")
            .unwrap()
            .complete("user", vec![0x41, 0x0d, 0x32])
            .unwrap();
        state.ingest(&speed);

        assert_eq!(state.current("vehicle.speed").unwrap().value, 50.0);
        assert!(state.current("dpf.diff_pressure").is_none());
        assert!(state.history("dpf.diff_pressure").is_none());
        assert!(state.timed_history("dpf.diff_pressure").is_none());
        assert!(TelemetryState::new(0).is_err());
    }

    #[test]
    fn keeps_history_chronological_when_samples_arrive_out_of_order() {
        let mut state = TelemetryState::new(2).unwrap();
        state.ingest(&transaction(3, vec![0x41, 0x0c, 0x00, 0x0c]));
        state.ingest(&transaction(1, vec![0x41, 0x0c, 0x00, 0x04]));
        state.ingest(&transaction(2, vec![0x41, 0x0c, 0x00, 0x08]));

        assert_eq!(
            state
                .history("engine.rpm")
                .unwrap()
                .iter()
                .map(|sample| sample.timestamp_ms)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(state.current("engine.rpm").unwrap().value, 3.0);
    }

    #[test]
    fn exact_microsecond_timeline_distinguishes_samples_within_one_millisecond() {
        let mut state = TelemetryState::new(4).unwrap();
        let first = transaction(0, vec![0x41, 0x0c, 0x00, 0x04]);
        let second = transaction(0, vec![0x41, 0x0c, 0x00, 0x08]);
        state.ingest_at_us(&second, 1_000_999);
        state.ingest_at_us(&first, 1_000_001);

        assert_eq!(
            state
                .history("engine.rpm")
                .unwrap()
                .iter()
                .map(|sample| sample.timestamp_ms)
                .collect::<Vec<_>>(),
            [1_000, 1_000]
        );
        let exact = state
            .timed_history("engine.rpm")
            .unwrap()
            .map(|(timestamp_us, sample)| (timestamp_us, sample.value))
            .collect::<Vec<_>>();
        assert_eq!(exact, [(1_000_001, 1.0), (1_000_999, 2.0)]);
        assert_eq!(state.current("engine.rpm").unwrap().value, 2.0);
    }
}
