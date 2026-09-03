//! Small, privacy-safe summaries of experimentally decoded DPF replay data.

use std::collections::BTreeMap;

use crate::capture_replay::CaptureReplay;

/// One per-semantic summary of successful DPF replay readings.
#[derive(Clone, Debug, PartialEq)]
pub struct DpfSummary {
    semantic: &'static str,
    count: usize,
    first_value: f64,
    last_value: f64,
    min: f64,
    max: f64,
    delta: f64,
    unit: &'static str,
}

impl DpfSummary {
    pub fn semantic(&self) -> &'static str {
        self.semantic
    }

    pub const fn count(&self) -> usize {
        self.count
    }

    pub const fn first_value(&self) -> f64 {
        self.first_value
    }

    pub const fn last_value(&self) -> f64 {
        self.last_value
    }

    pub const fn min(&self) -> f64 {
        self.min
    }

    pub const fn max(&self) -> f64 {
        self.max
    }

    pub const fn delta(&self) -> f64 {
        self.delta
    }

    pub const fn unit(&self) -> &'static str {
        self.unit
    }
}

/// The privacy-safe, offline view of a capture's successful DPF decodes.
#[derive(Clone, Debug, PartialEq)]
pub struct DpfReport {
    duration_us: u64,
    summaries: Vec<DpfSummary>,
}

impl DpfReport {
    /// Summarize only successful DPF readings from a deterministic replay.
    pub fn from_replay(replay: &CaptureReplay) -> Self {
        Self::from_replays([replay])
    }

    /// Summarize captures in the caller-provided order.  Captures are
    /// snapshots, not one continuous timestamped DPF stream.
    pub fn from_replays<'a>(replays: impl IntoIterator<Item = &'a CaptureReplay>) -> Self {
        let mut accumulators = BTreeMap::new();
        let mut duration_us = 0_u64;
        for replay in replays {
            duration_us = duration_us.saturating_add(replay.duration_us());
            for replay_reading in replay.dpf_readings() {
                let reading = replay_reading.reading();
                accumulators
                    .entry(reading.semantic())
                    .and_modify(|summary: &mut Accumulator| summary.push(reading.value()))
                    .or_insert_with(|| Accumulator::new(reading.value(), reading.unit()));
            }
        }

        Self {
            duration_us,
            summaries: accumulators
                .into_iter()
                .map(|(semantic, accumulator)| accumulator.finish(semantic))
                .collect(),
        }
    }

    pub const fn duration_us(&self) -> u64 {
        self.duration_us
    }

    pub fn summaries(&self) -> &[DpfSummary] {
        &self.summaries
    }

    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }
}

struct Accumulator {
    count: usize,
    first_value: f64,
    last_value: f64,
    min: f64,
    max: f64,
    unit: &'static str,
}

impl Accumulator {
    fn new(value: f64, unit: &'static str) -> Self {
        Self {
            count: 1,
            first_value: value,
            last_value: value,
            min: value,
            max: value,
            unit,
        }
    }

    fn push(&mut self, value: f64) {
        self.count += 1;
        self.last_value = value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    fn finish(self, semantic: &'static str) -> DpfSummary {
        DpfSummary {
            semantic,
            count: self.count,
            first_value: self.first_value,
            last_value: self.last_value,
            min: self.min,
            max: self.max,
            delta: self.last_value - self.first_value,
            unit: self.unit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capture_events::{CaptureEvent, ResponderEvidence},
        jsonl_capture::{CaptureStatus, ParsedCapture},
    };

    fn dpf_response(value: [u8; 2]) -> CaptureEvent {
        CaptureEvent::ResponsesObserved {
            semantic: "dpf.soot_mass_calculated".into(),
            request_payload: vec![0x22, 0x11, 0x4f],
            responses: vec![ResponderEvidence {
                responder: Some("7E8".into()),
                payload: vec![0x62, 0x11, 0x4f, value[0], value[1]],
            }],
            selected_responder: Some("7E8".into()),
            selection_error: None,
        }
    }

    fn replay(events: Vec<CaptureEvent>) -> CaptureReplay {
        CaptureReplay::from_capture(&ParsedCapture {
            events,
            status: CaptureStatus::Complete,
        })
    }

    #[test]
    fn summarizes_values_and_keeps_capture_duration() {
        let report = DpfReport::from_replay(&replay(vec![
            dpf_response([0x04, 0xe2]),
            dpf_response([0x09, 0xc4]),
            CaptureEvent::SessionStopped {
                offset_us: 5_000_000,
            },
        ]));

        assert_eq!(report.duration_us(), 5_000_000);
        assert_eq!(report.summaries().len(), 1);
        let summary = &report.summaries()[0];
        assert_eq!(summary.semantic(), "dpf.soot_mass_calculated");
        assert_eq!(summary.count(), 2);
        assert_eq!(summary.first_value(), 12.5);
        assert_eq!(summary.last_value(), 25.0);
        assert_eq!(summary.min(), 12.5);
        assert_eq!(summary.max(), 25.0);
        assert_eq!(summary.delta(), 12.5);
        assert_eq!(summary.unit(), "g");
    }

    #[test]
    fn empty_replay_has_no_summaries() {
        let report = DpfReport::from_replay(&replay(Vec::new()));

        assert!(report.is_empty());
        assert!(report.summaries().is_empty());
        assert_eq!(report.duration_us(), 0);
    }

    #[test]
    fn multiple_snapshots_keep_input_order_for_first_last_and_delta() {
        let first = replay(vec![dpf_response([0x01, 0x90])]);
        let last = replay(vec![dpf_response([0x01, 0xf4])]);

        let report = DpfReport::from_replays([&first, &last]);

        let summary = &report.summaries()[0];
        assert_eq!(summary.first_value(), 4.0);
        assert_eq!(summary.last_value(), 5.0);
        assert_eq!(summary.delta(), 1.0);
    }
}
