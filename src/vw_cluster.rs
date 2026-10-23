//! Closed, experimental VW instrument-cluster odometer read (ADR 0001
//! amendment, REQ-016).
//!
//! The definition lives in the core next to the EA189 candidates instead of the
//! Knowledge DB. This module has no transport access: it holds the fixed
//! definition and the offline interpretation of the recorded response.

use crate::elm::DiagnosticResponses;

pub(crate) const TARGET: &str = "714";
pub(crate) const RESPONDER: &str = "77E";
pub(crate) const DID: u16 = 0x2203;
pub(crate) const SEMANTIC: &str = "vehicle.odometer.cluster";
/// Stored source label (REQ-016.1).
pub(crate) const SOURCE: &str = "VW Kombiinstrument DID 2203 (experimentell)";
/// Where the definition comes from (REQ-016.8). The archive stays private.
pub(crate) const PROVENANCE: &str =
    "Car Scanner 2.1.50, locally extracted VAG dashboard and EA189 profiles (PQ35/PQ46)";
pub(crate) const CLASSIFICATION: &str = "COMMUNITY";
pub(crate) const HARDWARE_VALIDATION: &str = "pending";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Value,
    NegativeResponse,
    NoResponse,
    OtherResponder,
    UnparseableLength,
    Malformed,
}

impl Outcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::NegativeResponse => "negative_response",
            Self::NoResponse => "no_response",
            Self::OtherResponder => "other_responder",
            Self::UnparseableLength => "unparseable_length",
            Self::Malformed => "malformed_response",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) outcome: Outcome,
    pub(crate) value_km: Option<u32>,
    pub(crate) nrc: Option<u8>,
    pub(crate) data: Option<Vec<u8>>,
    pub(crate) error: Option<String>,
}

impl Reading {
    fn failed(outcome: Outcome, error: impl Into<String>) -> Self {
        Self {
            outcome,
            value_km: None,
            nrc: None,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// Interpret one normalized `22 2203` exchange. Only a positive response from
/// `77E` with exactly two data bytes yields a value: `(A*256 + B) * 10` km.
pub(crate) fn interpret(responses: &DiagnosticResponses) -> Reading {
    let matching: Vec<_> = responses
        .as_slice()
        .iter()
        .filter(|response| {
            response
                .responder
                .as_ref()
                .is_some_and(|responder| responder.as_str().eq_ignore_ascii_case(RESPONDER))
        })
        .collect();
    let Some(first) = matching.first() else {
        return match responses.as_slice().first() {
            Some(other) => Reading::failed(
                Outcome::OtherResponder,
                format!(
                    "response only from {}; expected {RESPONDER}",
                    other
                        .responder
                        .as_ref()
                        .map_or("unknown responder", |responder| responder.as_str())
                ),
            ),
            None => Reading::failed(
                Outcome::NoResponse,
                responses.errors().first().map_or_else(
                    || format!("no response from {RESPONDER}"),
                    |e| e.error.clone(),
                ),
            ),
        };
    };
    if matching
        .iter()
        .any(|response| response.payload != first.payload)
    {
        return Reading::failed(
            Outcome::Malformed,
            format!("conflicting responses from {RESPONDER}"),
        );
    }
    if let Some(error) = responses.errors().first() {
        return Reading::failed(
            Outcome::Malformed,
            format!("incomplete UDS response: {}", error.error),
        );
    }
    let payload = first.payload.as_slice();
    if let [0x7f, 0x22, nrc] = payload {
        return Reading {
            outcome: Outcome::NegativeResponse,
            value_km: None,
            nrc: Some(*nrc),
            data: None,
            error: Some(format!("negative UDS response (NRC 0x{nrc:02X})")),
        };
    }
    let [0x62, 0x22, 0x03, data @ ..] = payload else {
        return Reading::failed(
            Outcome::Malformed,
            format!(
                "unexpected response from {RESPONDER}: {}",
                crate::hex(payload)
            ),
        );
    };
    match data {
        [high, low] => Reading {
            outcome: Outcome::Value,
            value_km: Some((u32::from(*high) * 256 + u32::from(*low)) * 10),
            nrc: None,
            data: Some(data.to_vec()),
            error: None,
        },
        _ => Reading {
            outcome: Outcome::UnparseableLength,
            value_km: None,
            nrc: None,
            data: Some(data.to_vec()),
            error: Some(format!(
                "nicht auswertbar: {} data bytes, expected 2",
                data.len()
            )),
        },
    }
}
