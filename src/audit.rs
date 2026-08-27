use crate::Transaction;
use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    pub timestamp_ms: u128,
    pub source: String,
    pub semantic: &'static str,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseAuditEntry {
    pub timestamp_ms: u128,
    pub semantic: String,
    pub request: Vec<u8>,
    pub responses: Vec<crate::capture_events::ResponderEvidence>,
    pub selected_responder: Option<String>,
    pub selection_error: Option<String>,
}

pub struct AuditState {
    capacity: usize,
    entries: VecDeque<AuditEntry>,
    response_entries: VecDeque<ResponseAuditEntry>,
}

impl AuditState {
    pub fn new(capacity: usize) -> Result<Self, String> {
        (capacity > 0)
            .then_some(Self {
                capacity,
                entries: VecDeque::with_capacity(capacity),
                response_entries: VecDeque::with_capacity(capacity),
            })
            .ok_or_else(|| "audit capacity must be greater than zero".into())
    }

    pub fn ingest(&mut self, transaction: &Transaction) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(AuditEntry {
            timestamp_ms: transaction.timestamp_ms(),
            source: transaction.source().into(),
            semantic: transaction.semantic(),
            request: transaction.request().to_vec(),
            response: transaction.response().to_vec(),
        });
    }

    pub fn record_error(&mut self, error: &str) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(AuditEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            source: format!("error: {error}"),
            semantic: "scheduler.error",
            request: Vec::new(),
            response: Vec::new(),
        });
    }

    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.iter().cloned().collect()
    }

    pub fn ingest_responses(
        &mut self,
        timestamp_ms: u128,
        semantic: impl Into<String>,
        request: Vec<u8>,
        responses: Vec<crate::capture_events::ResponderEvidence>,
        selected_responder: Option<String>,
        selection_error: Option<String>,
    ) -> Result<(), String> {
        if request.is_empty() {
            return Err("response audit request must not be empty".into());
        }
        if responses.is_empty() {
            return Err("response audit list must not be empty".into());
        }
        if self.response_entries.len() == self.capacity {
            self.response_entries.pop_front();
        }
        self.response_entries.push_back(ResponseAuditEntry {
            timestamp_ms,
            semantic: semantic.into(),
            request,
            responses,
            selected_responder,
            selection_error,
        });
        Ok(())
    }

    pub fn response_snapshot(&self) -> Vec<ResponseAuditEntry> {
        self.response_entries.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;

    fn transaction(timestamp_ms: u128, response: Vec<u8>) -> Transaction {
        prepare_read("engine.rpm")
            .unwrap()
            .complete("user", response)
            .unwrap()
            .with_timestamp_ms(timestamp_ms)
    }

    #[test]
    fn preserves_chronological_entries_and_evicts_oldest() {
        let mut state = AuditState::new(2).unwrap();
        state.ingest(&transaction(1, vec![0x41, 0x0c, 0x00, 0x04]));
        state.ingest(&transaction(2, vec![0x41, 0x0c, 0x00, 0x08]));
        state.ingest(&transaction(3, vec![0x41, 0x0c, 0x00, 0x0c]));

        let entries = state.snapshot();
        assert_eq!(state.len(), 2);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.timestamp_ms)
                .collect::<Vec<_>>(),
            [2, 3]
        );
    }

    #[test]
    fn snapshots_source_semantic_and_raw_bytes() {
        let mut state = AuditState::new(1).unwrap();
        state.ingest(&transaction(42, vec![0x41, 0x0c, 0x1a, 0xf8]));

        assert_eq!(
            state.snapshot(),
            vec![AuditEntry {
                timestamp_ms: 42,
                source: "user".into(),
                semantic: "engine.rpm",
                request: vec![0x01, 0x0c],
                response: vec![0x41, 0x0c, 0x1a, 0xf8],
            }]
        );
    }

    #[test]
    fn rejects_zero_capacity() {
        assert!(AuditState::new(0).is_err());
    }

    #[test]
    fn preserves_all_responder_payloads_for_offline_inspection() {
        let mut state = AuditState::new(1).unwrap();
        state
            .ingest_responses(
                42,
                "engine.rpm",
                vec![0x01, 0x0c],
                vec![
                    crate::capture_events::ResponderEvidence::new(
                        Some("7E8".into()),
                        vec![0x41, 0x0c, 0x1a, 0xf8],
                    )
                    .unwrap(),
                    crate::capture_events::ResponderEvidence::new(
                        Some("7E9".into()),
                        vec![0x41, 0x0c, 0x1a, 0xf4],
                    )
                    .unwrap(),
                ],
                None,
                Some("ambiguous responders".into()),
            )
            .unwrap();

        let entries = state.response_snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].responses.len(), 2);
        assert_eq!(entries[0].selected_responder, None);
        assert_eq!(
            entries[0].selection_error.as_deref(),
            Some("ambiguous responders")
        );
    }
}
