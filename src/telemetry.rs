use crate::Transaction;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub timestamp_ms: u128,
    pub value: f64,
    pub unit: &'static str,
}

pub struct TelemetryState {
    capacity: usize,
    samples: BTreeMap<&'static str, VecDeque<Sample>>,
}

impl TelemetryState {
    pub fn new(capacity: usize) -> Result<Self, String> {
        (capacity > 0)
            .then_some(Self {
                capacity,
                samples: BTreeMap::new(),
            })
            .ok_or_else(|| "telemetry capacity must be greater than zero".into())
    }

    pub fn ingest(&mut self, transaction: &Transaction) {
        let samples = self.samples.entry(transaction.semantic()).or_default();
        let sample = Sample {
            timestamp_ms: transaction.timestamp_ms(),
            value: transaction.value(),
            unit: transaction.unit(),
        };
        // ponytail: histories cap at 600 samples; use a time-indexed store if much larger out-of-order streams arrive.
        let position = samples
            .iter()
            .position(|current| current.timestamp_ms > sample.timestamp_ms)
            .unwrap_or(samples.len());
        samples.insert(position, sample);
        if samples.len() > self.capacity {
            samples.pop_front();
        }
    }

    pub fn current(&self, semantic: &str) -> Option<&Sample> {
        self.samples.get(semantic)?.back()
    }

    pub fn history(&self, semantic: &str) -> Option<&VecDeque<Sample>> {
        self.samples.get(semantic)
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
}
