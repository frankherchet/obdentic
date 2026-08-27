use crate::Transaction;

/// Relative capture time in integer microseconds.
pub type CaptureTimeUs = u64;

/// The monotonic offsets associated with one diagnostic read.
///
/// A duration is intentionally not stored: `finished_us - started_us` can be
/// derived by a consumer without creating another value that can disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadTiming {
    pub due_us: CaptureTimeUs,
    pub started_us: CaptureTimeUs,
    pub finished_us: CaptureTimeUs,
}

impl ReadTiming {
    pub const fn new(
        due_us: CaptureTimeUs,
        started_us: CaptureTimeUs,
        finished_us: CaptureTimeUs,
    ) -> Self {
        Self {
            due_us,
            started_us,
            finished_us,
        }
    }

    pub const fn is_monotonic(self) -> bool {
        self.due_us <= self.started_us && self.started_us <= self.finished_us
    }
}

/// Values are extensible beyond the numeric values currently exposed by the
/// vehicle catalog.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureValue {
    Number(f64),
    Boolean(bool),
    Enum(String),
    Text(String),
    Unavailable { reason: String },
}

/// A closed vocabulary of passive observations from one capture session.
///
/// These values contain no transport handles and no operation that can issue
/// a diagnostic request. A writer can consume them in the order received.
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureEvent {
    CaptureStarted {
        wallclock_ms: Option<u64>,
        profile: Option<String>,
    },
    SessionInitialized,
    SubscriptionConfigured {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
    },
    SupportDiscovery {
        request_payload: Vec<u8>,
        response_payload: Vec<u8>,
    },
    ReadSucceeded {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
        due_us: CaptureTimeUs,
        started_us: CaptureTimeUs,
        finished_us: CaptureTimeUs,
        request_payload: Vec<u8>,
        response_payload: Vec<u8>,
        value: CaptureValue,
        unit: String,
        source: String,
        profile: String,
        decoder: String,
        provenance: String,
    },
    ReadFailed {
        semantic: String,
        requested_interval_us: CaptureTimeUs,
        timing: Option<ReadTiming>,
        request_payload: Option<Vec<u8>>,
        error: String,
    },
    SlotsSkipped {
        semantic: String,
        count: u64,
        first_due_us: CaptureTimeUs,
        last_due_us: CaptureTimeUs,
    },
    SessionError {
        error: String,
    },
    ShutdownRequested,
    SessionStopped {
        offset_us: CaptureTimeUs,
    },
}

impl CaptureEvent {
    pub fn capture_started(wallclock_ms: Option<u64>, profile: Option<String>) -> Self {
        Self::CaptureStarted {
            wallclock_ms,
            profile,
        }
    }

    pub fn subscription_configured(
        semantic: impl Into<String>,
        requested_interval_us: CaptureTimeUs,
    ) -> Self {
        Self::SubscriptionConfigured {
            semantic: semantic.into(),
            requested_interval_us,
        }
    }

    pub fn support_discovery(request_payload: Vec<u8>, response_payload: Vec<u8>) -> Self {
        Self::SupportDiscovery {
            request_payload,
            response_payload,
        }
    }

    /// Copy a completed read into an owned event without rebuilding its
    /// diagnostic request. The raw request and response bytes are preserved
    /// exactly as observed by the diagnostic layer.
    pub fn read_succeeded_from_transaction(
        transaction: &Transaction,
        requested_interval_us: CaptureTimeUs,
        timing: ReadTiming,
    ) -> Result<Self, String> {
        let metadata = crate::supported_signals()
            .iter()
            .find(|signal| signal.metadata().semantic == transaction.semantic())
            .map(|signal| signal.metadata())
            .ok_or_else(|| format!("unknown signal in transaction: {}", transaction.semantic()))?;

        Ok(Self::ReadSucceeded {
            semantic: transaction.semantic().into(),
            requested_interval_us,
            due_us: timing.due_us,
            started_us: timing.started_us,
            finished_us: timing.finished_us,
            request_payload: transaction.request().to_vec(),
            response_payload: transaction.response().to_vec(),
            value: CaptureValue::Number(transaction.value()),
            unit: transaction.unit().into(),
            source: transaction.source().into(),
            profile: transaction.profile().into(),
            decoder: metadata.decoder.into(),
            provenance: metadata.provenance.into(),
        })
    }

    /// Build an explicit failed-read event from an already prepared payload.
    /// Passing `None` represents a failure before a request was prepared.
    pub fn read_failed(
        semantic: impl Into<String>,
        requested_interval_us: CaptureTimeUs,
        timing: Option<ReadTiming>,
        request_payload: Option<Vec<u8>>,
        error: impl Into<String>,
    ) -> Self {
        Self::ReadFailed {
            semantic: semantic.into(),
            requested_interval_us,
            timing,
            request_payload,
            error: error.into(),
        }
    }

    pub fn slots_skipped(
        semantic: impl Into<String>,
        count: u64,
        first_due_us: CaptureTimeUs,
        last_due_us: CaptureTimeUs,
    ) -> Self {
        Self::SlotsSkipped {
            semantic: semantic.into(),
            count,
            first_due_us,
            last_due_us,
        }
    }

    pub fn session_error(error: impl Into<String>) -> Self {
        Self::SessionError {
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;

    #[test]
    fn maps_transaction_without_changing_diagnostic_payloads_or_timing() {
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("user", vec![0x41, 0x0c, 0x1a, 0xf8])
            .unwrap();
        let event = CaptureEvent::read_succeeded_from_transaction(
            &transaction,
            4_294_967_297,
            ReadTiming::new(4_000_000_001, 4_000_000_123, 4_000_001_987),
        )
        .unwrap();

        assert_eq!(
            event,
            CaptureEvent::ReadSucceeded {
                semantic: "engine.rpm".into(),
                requested_interval_us: 4_294_967_297,
                due_us: 4_000_000_001,
                started_us: 4_000_000_123,
                finished_us: 4_000_001_987,
                request_payload: vec![0x01, 0x0c],
                response_payload: vec![0x41, 0x0c, 0x1a, 0xf8],
                value: CaptureValue::Number(1726.0),
                unit: "rpm".into(),
                source: "user".into(),
                profile: "obd2-v1".into(),
                decoder: "((A * 256) + B) / 4".into(),
                provenance: "SAE J1979 Mode 01 PID 0C".into(),
            }
        );
    }

    #[test]
    fn failed_reads_are_explicit_and_keep_known_timing_and_request() {
        let timing = ReadTiming::new(11, 17, 23);
        assert_eq!(
            CaptureEvent::read_failed(
                "engine.rpm",
                250_000,
                Some(timing),
                Some(vec![0x01, 0x0c]),
                "timeout",
            ),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: Some(timing),
                request_payload: Some(vec![0x01, 0x0c]),
                error: "timeout".into(),
            }
        );
        assert_eq!(
            CaptureEvent::read_failed("engine.rpm", 250_000, None, None, "not prepared"),
            CaptureEvent::ReadFailed {
                semantic: "engine.rpm".into(),
                requested_interval_us: 250_000,
                timing: None,
                request_payload: None,
                error: "not prepared".into(),
            }
        );
    }

    #[test]
    fn timing_is_integer_microseconds_and_monotonic() {
        let timing = ReadTiming::new(u64::MAX - 2, u64::MAX - 1, u64::MAX);
        assert!(timing.is_monotonic());
        assert_eq!(timing.due_us, u64::MAX - 2);
        assert_eq!(timing.started_us, u64::MAX - 1);
        assert_eq!(timing.finished_us, u64::MAX);
    }

    #[test]
    fn event_model_only_maps_existing_read_requests() {
        let read = prepare_read("engine.rpm").unwrap();
        assert_eq!(read.bytes(), [0x01, 0x0c]);
        assert!(prepare_read("dtc.clear").is_err());
    }
}
