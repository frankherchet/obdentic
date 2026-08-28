//! Deterministic, transport-free replay of successful JSONL capture reads.
//!
//! The replay path deliberately re-decodes persisted raw response bytes with the
//! current semantic decoder instead of trusting the decoded value stored by the
//! recording version. Original capture evidence remains untouched and replay
//! differences are reported as issues rather than rewritten.

use crate::{
    capture_events::{CaptureEvent, CaptureValue},
    jsonl_capture::ParsedCapture,
    prepare_read,
    telemetry::TelemetryState,
    Transaction,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayIssueKind {
    UnsupportedSemantic,
    RequestChanged,
    DecodeFailed,
    RecordedDecodeChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayIssue {
    at_us: u64,
    semantic: String,
    kind: ReplayIssueKind,
    detail: String,
}

impl ReplayIssue {
    pub const fn at_us(&self) -> u64 {
        self.at_us
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn kind(&self) -> ReplayIssueKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

pub struct CaptureReplay {
    duration_us: u64,
    offsets_us: Vec<u64>,
    transactions: Vec<Transaction>,
    issues: Vec<ReplayIssue>,
}

impl CaptureReplay {
    /// Re-decode every successful semantic read in a parsed capture.
    ///
    /// No transport/session/scheduler object is constructed here. Reads that the
    /// current catalog can no longer decode are preserved as replay issues and do
    /// not make the rest of the capture unusable.
    pub fn from_capture(capture: &ParsedCapture) -> Self {
        let mut duration_us = 0_u64;
        let mut decoded = Vec::<(u64, Transaction)>::new();
        let mut issues = Vec::new();

        for event in &capture.events {
            match event {
                CaptureEvent::ReadSucceeded {
                    semantic,
                    finished_us,
                    request_payload,
                    response_payload,
                    value,
                    unit,
                    source,
                    ..
                } => {
                    duration_us = duration_us.max(*finished_us);
                    let request = match prepare_read(semantic) {
                        Ok(request) => request,
                        Err(error) => {
                            issues.push(ReplayIssue {
                                at_us: *finished_us,
                                semantic: semantic.clone(),
                                kind: ReplayIssueKind::UnsupportedSemantic,
                                detail: error,
                            });
                            continue;
                        }
                    };
                    if request_payload.as_slice() != request.bytes().as_slice() {
                        issues.push(ReplayIssue {
                            at_us: *finished_us,
                            semantic: semantic.clone(),
                            kind: ReplayIssueKind::RequestChanged,
                            detail: format!(
                                "recorded request {:?} differs from current semantic request {:?}",
                                request_payload,
                                request.bytes()
                            ),
                        });
                        continue;
                    }
                    let transaction = match request.complete(source, response_payload.clone()) {
                        Ok(transaction) => {
                            transaction.with_timestamp_ms(u128::from(*finished_us / 1_000))
                        }
                        Err(error) => {
                            issues.push(ReplayIssue {
                                at_us: *finished_us,
                                semantic: semantic.clone(),
                                kind: ReplayIssueKind::DecodeFailed,
                                detail: error,
                            });
                            continue;
                        }
                    };
                    if recorded_decode_changed(value, unit, &transaction) {
                        issues.push(ReplayIssue {
                            at_us: *finished_us,
                            semantic: semantic.clone(),
                            kind: ReplayIssueKind::RecordedDecodeChanged,
                            detail: format!(
                                "recorded={} {} current={:.6} {}",
                                capture_value_text(value),
                                unit,
                                transaction.value(),
                                transaction.unit()
                            ),
                        });
                    }
                    decoded.push((*finished_us, transaction));
                }
                CaptureEvent::ReadFailed {
                    timing: Some(timing),
                    ..
                } => {
                    duration_us = duration_us.max(timing.finished_us);
                }
                CaptureEvent::ReadFailed { timing: None, .. } => {}
                CaptureEvent::SlotsSkipped { last_due_us, .. } => {
                    duration_us = duration_us.max(*last_due_us);
                }
                CaptureEvent::SessionStopped { offset_us } => {
                    duration_us = duration_us.max(*offset_us);
                }
                _ => {}
            }
        }

        decoded.sort_by_key(|(at_us, _)| *at_us);
        let (offsets_us, transactions): (Vec<_>, Vec<_>) = decoded.into_iter().unzip();
        issues.sort_by_key(ReplayIssue::at_us);
        Self {
            duration_us,
            offsets_us,
            transactions,
            issues,
        }
    }

    pub const fn duration_us(&self) -> u64 {
        self.duration_us
    }

    /// Exact monotonic JSONL timestamps aligned one-to-one with [`Self::transactions`].
    pub fn offsets_us(&self) -> &[u64] {
        &self.offsets_us
    }

    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    pub fn issues(&self) -> &[ReplayIssue] {
        &self.issues
    }

    pub fn telemetry_at(&self, cursor_us: u64, capacity: usize) -> Result<TelemetryState, String> {
        let mut state = TelemetryState::new(capacity)?;
        let end = self.offsets_us.partition_point(|at_us| *at_us <= cursor_us);
        for (timestamp_us, transaction) in self.offsets_us[..end]
            .iter()
            .copied()
            .zip(&self.transactions[..end])
        {
            state.ingest_at_us(transaction, u128::from(timestamp_us));
        }
        Ok(state)
    }

    pub fn telemetry_full(&self, capacity: usize) -> Result<TelemetryState, String> {
        self.telemetry_at(self.duration_us, capacity)
    }
}

fn recorded_decode_changed(value: &CaptureValue, unit: &str, transaction: &Transaction) -> bool {
    unit != transaction.unit()
        || match value {
            CaptureValue::Number(recorded) => *recorded != transaction.value(),
            _ => true,
        }
}

fn capture_value_text(value: &CaptureValue) -> String {
    match value {
        CaptureValue::Number(value) => format!("{value:.6}"),
        CaptureValue::Boolean(value) => value.to_string(),
        CaptureValue::Enum(value) | CaptureValue::Text(value) => value.clone(),
        CaptureValue::Unavailable { reason } => format!("unavailable({reason})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capture_events::ReadTiming, jsonl_capture::CaptureStatus};

    fn successful_read(
        semantic: &str,
        at_us: u64,
        request: Vec<u8>,
        response: Vec<u8>,
        recorded_value: f64,
        unit: &str,
    ) -> CaptureEvent {
        CaptureEvent::ReadSucceeded {
            semantic: semantic.into(),
            requested_interval_us: 1_000_000,
            due_us: at_us.saturating_sub(100_000),
            started_us: at_us.saturating_sub(50_000),
            finished_us: at_us,
            request_payload: request,
            response_payload: response,
            value: CaptureValue::Number(recorded_value),
            unit: unit.into(),
            source: "user".into(),
            profile: "obd2-v1".into(),
            decoder: "recorded-decoder".into(),
            provenance: "recorded-provenance".into(),
        }
    }

    fn capture(events: Vec<CaptureEvent>) -> ParsedCapture {
        ParsedCapture {
            events,
            status: CaptureStatus::Complete,
        }
    }

    #[test]
    fn raw_response_is_redecoded_instead_of_trusting_recorded_value() {
        let replay = CaptureReplay::from_capture(&capture(vec![
            successful_read(
                "engine.rpm",
                1_234_567,
                vec![0x01, 0x0c],
                vec![0x41, 0x0c, 0x0c, 0x80],
                9_999.0,
                "rpm",
            ),
            CaptureEvent::SessionStopped {
                offset_us: 2_000_000,
            },
        ]));

        assert_eq!(replay.transactions().len(), 1);
        assert_eq!(replay.offsets_us(), [1_234_567]);
        assert_eq!(replay.transactions()[0].value(), 800.0);
        assert_eq!(replay.transactions()[0].timestamp_ms(), 1_234);
        assert_eq!(replay.duration_us(), 2_000_000);
        assert_eq!(replay.issues().len(), 1);
        assert_eq!(
            replay.issues()[0].kind(),
            ReplayIssueKind::RecordedDecodeChanged
        );
    }

    #[test]
    fn telemetry_cursor_never_ingests_future_reads() {
        let replay = CaptureReplay::from_capture(&capture(vec![
            successful_read(
                "engine.rpm",
                1_000_000,
                vec![0x01, 0x0c],
                vec![0x41, 0x0c, 0x0c, 0x80],
                800.0,
                "rpm",
            ),
            successful_read(
                "engine.rpm",
                3_000_000,
                vec![0x01, 0x0c],
                vec![0x41, 0x0c, 0x12, 0xc0],
                1_200.0,
                "rpm",
            ),
        ]));

        let before = replay.telemetry_at(2_000_000, 8).unwrap();
        assert_eq!(before.current("engine.rpm").unwrap().value, 800.0);
        let after = replay.telemetry_at(3_000_000, 8).unwrap();
        assert_eq!(after.current("engine.rpm").unwrap().value, 1_200.0);
    }

    #[test]
    fn replay_preserves_distinct_submillisecond_capture_offsets_in_telemetry() {
        let replay = CaptureReplay::from_capture(&capture(vec![
            successful_read(
                "engine.rpm",
                1_000_001,
                vec![0x01, 0x0c],
                vec![0x41, 0x0c, 0x00, 0x04],
                1.0,
                "rpm",
            ),
            successful_read(
                "engine.rpm",
                1_000_999,
                vec![0x01, 0x0c],
                vec![0x41, 0x0c, 0x00, 0x08],
                2.0,
                "rpm",
            ),
        ]));

        let state = replay.telemetry_full(8).unwrap();
        let exact = state
            .timed_history("engine.rpm")
            .unwrap()
            .map(|(timestamp_us, sample)| (timestamp_us, sample.value))
            .collect::<Vec<_>>();
        assert_eq!(exact, [(1_000_001, 1.0), (1_000_999, 2.0)]);
        assert_eq!(
            state
                .history("engine.rpm")
                .unwrap()
                .iter()
                .map(|sample| sample.timestamp_ms)
                .collect::<Vec<_>>(),
            [1_000, 1_000]
        );
    }

    #[test]
    fn request_drift_is_reported_and_not_replayed() {
        let replay = CaptureReplay::from_capture(&capture(vec![successful_read(
            "engine.rpm",
            1_000_000,
            vec![0x01, 0x0d],
            vec![0x41, 0x0c, 0x0c, 0x80],
            800.0,
            "rpm",
        )]));

        assert!(replay.transactions().is_empty());
        assert_eq!(replay.issues().len(), 1);
        assert_eq!(replay.issues()[0].kind(), ReplayIssueKind::RequestChanged);
    }

    #[test]
    fn failed_reads_affect_duration_but_never_create_telemetry() {
        let replay = CaptureReplay::from_capture(&capture(vec![CaptureEvent::ReadFailed {
            semantic: "engine.rpm".into(),
            requested_interval_us: 1_000_000,
            timing: Some(ReadTiming::new(4_000_000, 4_050_000, 4_200_000)),
            request_payload: Some(vec![0x01, 0x0c]),
            error: "conflicting responders".into(),
        }]));

        assert_eq!(replay.duration_us(), 4_200_000);
        assert!(replay.transactions().is_empty());
        assert!(replay.telemetry_full(4).unwrap().signals().next().is_none());
    }
}
