//! Pure, read-only decoding of standardized OBD-II Mode 03 responses.
//!
//! Transport framing, retries, job execution, and interpretation of a DTC
//! are deliberately outside this module.  A responder value is retained as
//! opaque adapter metadata; it is never promoted to a CAN or ECU identity.

use std::fmt;

/// The only request represented by this decoder's protocol vocabulary.
pub const MODE03_REQUEST: [u8; 1] = [0x03];
/// Positive response service byte for OBD-II Mode 03.
pub const MODE03_POSITIVE_RESPONSE: u8 = 0x43;

/// The first letter of a standardized five-character OBD-II DTC.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DtcSystem {
    Powertrain,
    Chassis,
    Body,
    Network,
}

impl DtcSystem {
    const fn prefix(self) -> char {
        match self {
            Self::Powertrain => 'P',
            Self::Chassis => 'C',
            Self::Body => 'B',
            Self::Network => 'U',
        }
    }
}

/// A validated standardized OBD-II DTC.
///
/// `number` contains the four hexadecimal characters following the system
/// letter.  It is private so a caller cannot construct a code with a missing
/// or non-standard prefix.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DtcCode {
    system: DtcSystem,
    number: u16,
}

impl DtcCode {
    pub const fn system(self) -> DtcSystem {
        self.system
    }

    /// Return the four hexadecimal digits after the system letter.
    pub const fn number(self) -> u16 {
        self.number
    }
}

impl fmt::Display for DtcCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{:04X}", self.system.prefix(), self.number)
    }
}

/// An opaque, privacy-safe label for the adapter metadata that identified a
/// responder.  Control characters are rejected to prevent log/output
/// injection; the value is not interpreted as a CAN or UDS address.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResponderIdentity(String);

impl ResponderIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, ResponderIdentityError> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() {
            return Err(ResponderIdentityError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(ResponderIdentityError::ControlCharacter);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ResponderIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ResponderIdentity")
            .field(&"[redacted]")
            .finish()
    }
}

impl fmt::Display for ResponderIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for ResponderIdentity {
    type Error = ResponderIdentityError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for ResponderIdentity {
    type Error = ResponderIdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ResponderIdentityError {
    Empty,
    ControlCharacter,
}

impl fmt::Display for ResponderIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "responder identity must not be empty",
            Self::ControlCharacter => "responder identity must not contain control characters",
        })
    }
}

impl std::error::Error for ResponderIdentityError {}

/// One normalized Mode 03 response and its optional adapter-level source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseEvidence {
    pub responder: Option<ResponderIdentity>,
    pub payload: Vec<u8>,
}

impl ResponseEvidence {
    pub fn new(responder: Option<ResponderIdentity>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            responder,
            payload: payload.into(),
        }
    }

    pub fn unknown(payload: impl Into<Vec<u8>>) -> Self {
        Self::new(None, payload)
    }
}

/// Source identity attached to one decoded response.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DtcSource {
    Unknown,
    Responder(ResponderIdentity),
}

impl DtcSource {
    pub fn responder(&self) -> Option<&ResponderIdentity> {
        match self {
            Self::Unknown => None,
            Self::Responder(identity) => Some(identity),
        }
    }
}

/// Errors from a normalized Mode 03 payload.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DtcDecodeError {
    /// The payload has the Mode 03 service byte but not complete DTC pairs.
    MalformedResponse,
    /// The payload is not a recognized positive Mode 03 response.
    UnknownResponse,
}

impl fmt::Display for DtcDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MalformedResponse => "malformed OBD-II Mode 03 response",
            Self::UnknownResponse => "unknown OBD-II Mode 03 response",
        })
    }
}

impl std::error::Error for DtcDecodeError {}

/// The factual result of decoding one responder's Mode 03 answer.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DtcResponse {
    NoDtcs,
    Stored(Vec<DtcCode>),
    Error(DtcDecodeError),
}

impl DtcResponse {
    pub fn dtcs(&self) -> &[DtcCode] {
        match self {
            Self::Stored(dtcs) => dtcs,
            Self::NoDtcs | Self::Error(_) => &[],
        }
    }

    pub const fn is_no_dtcs(&self) -> bool {
        matches!(self, Self::NoDtcs)
    }
}

/// One response remains one result, even when another responder has the same
/// DTC.  This prevents a vehicle-wide merge from hiding responder evidence.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DtcObservation {
    source: DtcSource,
    response: DtcResponse,
}

impl DtcObservation {
    pub fn source(&self) -> &DtcSource {
        &self.source
    }

    pub fn response(&self) -> &DtcResponse {
        &self.response
    }
}

/// Deterministic Mode 03 results, ordered by source and then response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DtcScanResult {
    observations: Vec<DtcObservation>,
}

impl DtcScanResult {
    pub fn observations(&self) -> &[DtcObservation] {
        &self.observations
    }
}

/// Decode normalized Mode 03 response evidence.
///
/// Every input response produces one observation.  A malformed or unknown
/// response is recoverable and stays attached to its responder; it does not
/// discard successful results from other responders.
pub fn decode_mode03(evidence: &[ResponseEvidence]) -> DtcScanResult {
    let mut observations = evidence
        .iter()
        .map(|evidence| DtcObservation {
            source: evidence
                .responder
                .clone()
                .map_or(DtcSource::Unknown, DtcSource::Responder),
            response: decode_mode03_payload(&evidence.payload).unwrap_or_else(DtcResponse::Error),
        })
        .collect::<Vec<_>>();
    observations.sort_unstable();
    DtcScanResult { observations }
}

/// Decode one normalized positive Mode 03 payload.
pub fn decode_mode03_payload(payload: &[u8]) -> Result<DtcResponse, DtcDecodeError> {
    if payload.first().copied() != Some(MODE03_POSITIVE_RESPONSE) {
        return Err(if !payload.is_empty() {
            DtcDecodeError::UnknownResponse
        } else {
            DtcDecodeError::MalformedResponse
        });
    }

    let bytes = &payload[1..];
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Err(DtcDecodeError::MalformedResponse);
    }

    let mut dtcs = Vec::with_capacity(bytes.len() / 2);
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        if *pair == [0, 0] {
            // 00 00 is the standardized no-code/padding marker.  Padding
            // may only occur at the end; otherwise the answer is ambiguous.
            if bytes[index * 2 + 2..].iter().any(|&byte| byte != 0) {
                return Err(DtcDecodeError::MalformedResponse);
            }
            break;
        }
        dtcs.push(DtcCode {
            system: match pair[0] >> 6 {
                0 => DtcSystem::Powertrain,
                1 => DtcSystem::Chassis,
                2 => DtcSystem::Body,
                _ => DtcSystem::Network,
            },
            number: u16::from_be_bytes([pair[0] & 0x3f, pair[1]]),
        });
    }

    if dtcs.is_empty() {
        Ok(DtcResponse::NoDtcs)
    } else {
        dtcs.sort_unstable();
        dtcs.dedup();
        Ok(DtcResponse::Stored(dtcs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responder(value: &str) -> ResponderIdentity {
        ResponderIdentity::new(value).unwrap()
    }

    fn response(value: Option<&str>, payload: &[u8]) -> ResponseEvidence {
        ResponseEvidence::new(value.map(responder), payload.to_vec())
    }

    #[test]
    fn no_dtcs_is_explicit() {
        let result = decode_mode03(&[response(None, &[0x43, 0x00, 0x00])]);
        assert_eq!(result.observations().len(), 1);
        assert_eq!(result.observations()[0].response(), &DtcResponse::NoDtcs);
    }

    #[test]
    fn decodes_one_stored_dtc() {
        let result = decode_mode03(&[response(Some("7E8"), &[0x43, 0x01, 0x0c])]);
        let observation = &result.observations()[0];
        assert_eq!(observation.source().responder().unwrap().as_str(), "7E8");
        assert_eq!(observation.response().dtcs()[0].to_string(), "P010C");
    }

    #[test]
    fn decodes_and_sorts_multiple_dtcs() {
        let result = decode_mode03(&[response(
            Some("7E8"),
            &[0x43, 0x03, 0x00, 0x01, 0x0c, 0x40, 0x10],
        )]);
        let dtcs = result.observations()[0].response().dtcs();
        assert_eq!(
            dtcs.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["P010C", "P0300", "C0010"]
        );
    }

    #[test]
    fn keeps_two_responders_separate() {
        let result = decode_mode03(&[
            response(Some("7E9"), &[0x43, 0x02, 0x01]),
            response(Some("7E8"), &[0x43, 0x01, 0x0c]),
        ]);
        assert_eq!(result.observations().len(), 2);
        assert_eq!(
            result.observations()[0]
                .source()
                .responder()
                .unwrap()
                .as_str(),
            "7E8"
        );
        assert_eq!(
            result.observations()[1]
                .source()
                .responder()
                .unwrap()
                .as_str(),
            "7E9"
        );
        assert_eq!(
            result.observations()[0].response().dtcs()[0].to_string(),
            "P010C"
        );
        assert_eq!(
            result.observations()[1].response().dtcs()[0].to_string(),
            "P0201"
        );
    }

    #[test]
    fn malformed_and_unknown_stay_per_responder() {
        let result = decode_mode03(&[
            response(Some("7E8"), &[0x43, 0x01]),
            response(Some("7E9"), &[0x7f, 0x03, 0x11]),
        ]);
        assert!(matches!(
            result.observations()[0].response(),
            DtcResponse::Error(DtcDecodeError::MalformedResponse)
        ));
        assert!(matches!(
            result.observations()[1].response(),
            DtcResponse::Error(DtcDecodeError::UnknownResponse)
        ));
    }

    #[test]
    fn ordering_does_not_depend_on_input_order() {
        let first = decode_mode03(&[
            response(Some("7E9"), &[0x43, 0x03, 0x00, 0x01, 0x0c]),
            response(Some("7E8"), &[0x43, 0x03, 0x00, 0x01, 0x0c]),
        ]);
        let second = decode_mode03(&[
            response(Some("7E8"), &[0x43, 0x01, 0x0c, 0x03, 0x00]),
            response(Some("7E9"), &[0x43, 0x01, 0x0c, 0x03, 0x00]),
        ]);
        assert_eq!(first, second);
    }

    #[test]
    fn responder_debug_is_redacted() {
        let identity = responder("vehicle-adapter-7E8");
        assert!(!format!("{identity:?}").contains("vehicle-adapter-7E8"));
    }
}
