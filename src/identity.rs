use std::fmt;

/// The only diagnostic request used by this identity layer.
pub const MODE09_PID02_REQUEST: [u8; 2] = [0x09, 0x02];

/// A validated 17-character vehicle identification number.
///
/// The bytes are kept private so callers cannot construct an unvalidated VIN.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Vin([u8; 17]);

impl Vin {
    pub const LENGTH: usize = 17;

    pub fn parse(value: &str) -> Result<Self, VinError> {
        Self::from_bytes(value.as_bytes())
    }

    pub fn from_bytes(value: &[u8]) -> Result<Self, VinError> {
        if value.len() != Self::LENGTH {
            return Err(VinError::InvalidLength);
        }

        let mut vin = [0; Self::LENGTH];
        for (index, &byte) in value.iter().enumerate() {
            if !is_vin_character(byte) {
                return Err(VinError::InvalidCharacter);
            }
            vin[index] = byte;
        }
        Ok(Self(vin))
    }

    pub fn as_str(&self) -> &str {
        // Validation in `from_bytes` guarantees ASCII/UTF-8 for the lifetime of the VIN.
        std::str::from_utf8(&self.0).expect("validated VIN must be UTF-8")
    }
}

impl fmt::Debug for Vin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Vin([redacted])")
    }
}

impl fmt::Display for Vin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for Vin {
    type Error = VinError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&[u8]> for Vin {
    type Error = VinError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VinError {
    InvalidLength,
    InvalidCharacter,
}

impl fmt::Display for VinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLength => "VIN must contain exactly 17 characters",
            Self::InvalidCharacter => "VIN contains an invalid character",
        })
    }
}

impl std::error::Error for VinError {}

/// The standards-based source from which an identity was obtained.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IdentitySource {
    ObdMode09Pid02,
}

impl IdentitySource {
    pub const fn request(self) -> [u8; 2] {
        match self {
            Self::ObdMode09Pid02 => MODE09_PID02_REQUEST,
        }
    }
}

/// Provenance is explicit so a future profiled UDS fallback cannot be confused
/// with the standards-based Mode 09 source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Provenance {
    SaeJ1979Mode09Pid02,
}

/// A validated vehicle identity with its read-only source and provenance.
#[derive(Clone, Eq, PartialEq)]
pub struct VehicleIdentity {
    vin: Vin,
    source: IdentitySource,
    provenance: Provenance,
}

impl VehicleIdentity {
    pub fn vin(&self) -> &Vin {
        &self.vin
    }

    pub const fn source(&self) -> IdentitySource {
        self.source
    }

    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }
}

impl fmt::Debug for VehicleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VehicleIdentity")
            .field("vin", &"[redacted]")
            .field("source", &self.source)
            .field("provenance", &self.provenance)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityDecodeError {
    EmptyResponse,
    MalformedResponse,
    InvalidVinLength,
    InvalidVinCharacter,
}

impl fmt::Display for IdentityDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyResponse => "Mode 09 PID 02 response is empty",
            Self::MalformedResponse => "malformed Mode 09 PID 02 response",
            Self::InvalidVinLength => "Mode 09 PID 02 response contains an invalid VIN length",
            Self::InvalidVinCharacter => {
                "Mode 09 PID 02 response contains an invalid VIN character"
            }
        })
    }
}

impl std::error::Error for IdentityDecodeError {}

/// Decode normalized Mode 09 PID 02 response segments.
///
/// The first segment contains `49 02 01`; subsequent segments contain only
/// their normalized payload. A repeated `49 02 01` prefix is also accepted for
/// adapters that retain the application header on every segment. Transport and
/// ISO-TP framing are intentionally outside this function.
pub fn decode_mode09_pid02<T: AsRef<[u8]>>(
    segments: &[T],
) -> Result<VehicleIdentity, IdentityDecodeError> {
    let first = segments
        .first()
        .map(AsRef::as_ref)
        .ok_or(IdentityDecodeError::EmptyResponse)?;
    if first.len() < 3 || first[..2] != [0x49, 0x02] || first[2] != 0x01 {
        return Err(IdentityDecodeError::MalformedResponse);
    }

    let mut bytes = first[3..].to_vec();
    for segment in segments.iter().skip(1) {
        let segment = segment.as_ref();
        if segment.is_empty() {
            return Err(IdentityDecodeError::MalformedResponse);
        }
        if segment.starts_with(&[0x49, 0x02, 0x01]) {
            bytes.extend_from_slice(&segment[3..]);
        } else if segment.starts_with(&[0x49, 0x02]) {
            return Err(IdentityDecodeError::MalformedResponse);
        } else {
            bytes.extend_from_slice(segment);
        }
    }

    let vin_end = Vin::LENGTH;
    if bytes.len() < vin_end {
        return Err(IdentityDecodeError::InvalidVinLength);
    }
    if bytes[vin_end..].iter().any(|&byte| byte != 0) {
        return Err(IdentityDecodeError::InvalidVinLength);
    }

    Vin::from_bytes(&bytes[..vin_end])
        .map(|vin| VehicleIdentity {
            vin,
            source: IdentitySource::ObdMode09Pid02,
            provenance: Provenance::SaeJ1979Mode09Pid02,
        })
        .map_err(|error| match error {
            VinError::InvalidLength => IdentityDecodeError::InvalidVinLength,
            VinError::InvalidCharacter => IdentityDecodeError::InvalidVinCharacter,
        })
}

fn is_vin_character(byte: u8) -> bool {
    byte.is_ascii_digit()
        || (b'A'..=b'H').contains(&byte)
        || (b'J'..=b'N').contains(&byte)
        || (b'P'..=b'R').contains(&byte)
        || (b'S'..=b'Z').contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIN: &str = "WVWZZZ1JZXW000001";

    fn response_segments() -> Vec<Vec<u8>> {
        VIN.as_bytes()
            .chunks(6)
            .enumerate()
            .map(|(index, chunk)| {
                if index == 0 {
                    [&[0x49, 0x02, 0x01][..], chunk].concat()
                } else {
                    chunk.to_vec()
                }
            })
            .collect()
    }

    #[test]
    fn decodes_valid_segmented_mode09_pid02_response() {
        let identity = decode_mode09_pid02(&response_segments()).unwrap();

        assert_eq!(identity.vin().as_str(), VIN);
        assert_eq!(identity.source(), IdentitySource::ObdMode09Pid02);
        assert_eq!(identity.source().request(), [0x09, 0x02]);
        assert_eq!(identity.provenance(), Provenance::SaeJ1979Mode09Pid02);
    }

    #[test]
    fn accepts_a_single_normalized_response_and_zero_padding() {
        let mut response = vec![0x49, 0x02, 0x01];
        response.extend_from_slice(VIN.as_bytes());
        response.extend_from_slice(&[0, 0]);

        assert_eq!(
            decode_mode09_pid02(&[response]).unwrap().vin().as_str(),
            VIN
        );
    }

    #[test]
    fn accepts_segments_that_repeat_the_application_header() {
        let segments = VIN
            .as_bytes()
            .chunks(6)
            .map(|chunk| [&[0x49, 0x02, 0x01][..], chunk].concat())
            .collect::<Vec<_>>();

        assert_eq!(decode_mode09_pid02(&segments).unwrap().vin().as_str(), VIN);
    }

    #[test]
    fn rejects_invalid_length_and_characters() {
        let mut short = vec![0x49, 0x02, 0x01];
        short.extend_from_slice(b"WVWZZZ1JZXW00000");
        assert_eq!(
            decode_mode09_pid02(&[short]),
            Err(IdentityDecodeError::InvalidVinLength)
        );

        let mut invalid = vec![0x49, 0x02, 0x01];
        invalid.extend_from_slice(b"WVWZZZ1IZXW000001");
        assert_eq!(
            decode_mode09_pid02(&[invalid]),
            Err(IdentityDecodeError::InvalidVinCharacter)
        );
    }

    #[test]
    fn rejects_malformed_headers_segments_and_non_padding_bytes() {
        assert_eq!(
            decode_mode09_pid02(&[vec![0x49, 0x01, 0x01]]),
            Err(IdentityDecodeError::MalformedResponse)
        );
        assert_eq!(
            decode_mode09_pid02(&[vec![0x49, 0x02, 0x01], vec![0x49, 0x02, 0x02],]),
            Err(IdentityDecodeError::MalformedResponse)
        );

        let mut extra = vec![0x49, 0x02, 0x01];
        extra.extend_from_slice(VIN.as_bytes());
        extra.push(b'X');
        assert_eq!(
            decode_mode09_pid02(&[extra]),
            Err(IdentityDecodeError::InvalidVinLength)
        );
    }

    #[test]
    fn vin_debug_and_decode_errors_do_not_include_identity_bytes() {
        let identity = decode_mode09_pid02(&response_segments()).unwrap();
        assert!(!format!("{identity:?}").contains(VIN));

        let mut invalid = vec![0x49, 0x02, 0x01];
        invalid.extend_from_slice(b"WVWZZZ1IZXW000001");
        assert!(!decode_mode09_pid02(&[invalid])
            .unwrap_err()
            .to_string()
            .contains("WVWZZZ1IZXW000001"));
    }
}
