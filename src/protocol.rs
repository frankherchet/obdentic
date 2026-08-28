#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mode01 {
    pid: u8,
    data_len: usize,
}

impl Mode01 {
    pub(crate) const fn new(pid: u8, data_len: usize) -> Self {
        Self { pid, data_len }
    }

    pub(crate) const fn bytes(self) -> [u8; 2] {
        [0x01, self.pid]
    }

    pub(crate) const fn pid(self) -> u8 {
        self.pid
    }

    pub(crate) const fn data_len(self) -> usize {
        self.data_len
    }

    pub(crate) fn data<'a>(self, response: &'a [u8], semantic: &str) -> Result<&'a [u8], String> {
        let expected = self.data_len + 2;
        if response.len() != expected || response[..2] != [0x41, self.pid] {
            return Err(format!(
                "invalid OBD-II {semantic} response: {}",
                crate::hex(response)
            ));
        }
        Ok(&response[2..])
    }
}

/// The closed set of read services understood by the diagnostic core.
///
/// This stays crate-private on purpose: callers select semantic signals, not
/// protocol services or caller-supplied bytes.  The UDS variant is prepared
/// for profiled, offline decoders; it is not a live transport API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Offline UDS validation is intentionally not live-dispatched yet.
pub(crate) enum ReadOperation {
    Mode01(Mode01),
    UdsReadDataByIdentifier { did: u16 },
}

impl ReadOperation {
    #[allow(dead_code)] // Used by profiled offline decoders when they are added.
    pub(crate) const fn uds_read_data_by_identifier(did: u16) -> Self {
        Self::UdsReadDataByIdentifier { did }
    }

    pub(crate) fn validate_response<'a>(
        self,
        response: &'a [u8],
        semantic: &str,
    ) -> Result<&'a [u8], ReadResponseError> {
        match self {
            Self::Mode01(mode) => mode
                .data(response, semantic)
                .map_err(ReadResponseError::Mode01),
            Self::UdsReadDataByIdentifier { did } => {
                if response.starts_with(&[0x7f, 0x22]) {
                    if response.len() != 3 {
                        return Err(ReadResponseError::MalformedUds);
                    }
                    return Err(ReadResponseError::UdsNegative { nrc: response[2] });
                }
                if response.len() < 3 || response[0] != 0x62 {
                    return Err(ReadResponseError::MalformedUds);
                }
                let response_did = u16::from_be_bytes([response[1], response[2]]);
                if response_did != did {
                    return Err(ReadResponseError::WrongDid {
                        expected: did,
                        actual: response_did,
                    });
                }
                if response.len() == 3 {
                    return Err(ReadResponseError::MalformedUds);
                }
                Ok(&response[3..])
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadResponseError {
    Mode01(String),
    MalformedUds,
    WrongDid { expected: u16, actual: u16 },
    UdsNegative { nrc: u8 },
}

impl std::fmt::Display for ReadResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mode01(error) => formatter.write_str(error),
            Self::MalformedUds => {
                formatter.write_str("malformed UDS ReadDataByIdentifier response")
            }
            Self::WrongDid { expected, actual } => write!(
                formatter,
                "UDS response DID {:04X} does not match requested DID {:04X}",
                actual, expected
            ),
            Self::UdsNegative { nrc } => {
                write!(formatter, "UDS negative response for 22, NRC {:02X}", nrc)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReadOperation, ReadResponseError};
    use crate::prepare_read;

    #[test]
    fn uds_read_data_by_identifier_accepts_variable_payloads() {
        let operation = ReadOperation::uds_read_data_by_identifier(0x1234);

        assert_eq!(
            operation
                .validate_response(&[0x62, 0x12, 0x34, 0x57], "test.did")
                .unwrap(),
            &[0x57]
        );
        assert_eq!(
            operation
                .validate_response(
                    &[0x62, 0x12, 0x34, 0x57, 0x56, 0x47, 0x5a, 0x5a],
                    "test.did"
                )
                .unwrap(),
            &[0x57, 0x56, 0x47, 0x5a, 0x5a]
        );
    }

    #[test]
    fn uds_read_data_by_identifier_rejects_truncated_and_wrong_responses() {
        let operation = ReadOperation::uds_read_data_by_identifier(0x1234);

        for response in [
            vec![],
            vec![0x62],
            vec![0x62, 0x12],
            vec![0x62, 0x12, 0x34],
            vec![0x61, 0x12, 0x34, 0x01],
        ] {
            assert_eq!(
                operation.validate_response(&response, "test.did"),
                Err(ReadResponseError::MalformedUds)
            );
        }
        assert_eq!(
            operation.validate_response(&[0x62, 0x12, 0x35, 0x01], "test.did"),
            Err(ReadResponseError::WrongDid {
                expected: 0x1234,
                actual: 0x1235,
            })
        );
    }

    #[test]
    fn uds_negative_response_preserves_nrc() {
        assert_eq!(
            ReadOperation::uds_read_data_by_identifier(0x1234)
                .validate_response(&[0x7f, 0x22, 0x31], "test.did"),
            Err(ReadResponseError::UdsNegative { nrc: 0x31 })
        );
    }

    #[test]
    fn existing_mode01_read_request_remains_unchanged() {
        let request = prepare_read("engine.rpm").unwrap();
        assert_eq!(request.bytes(), [0x01, 0x0c]);
        assert_eq!(
            request
                .complete("user", vec![0x41, 0x0c, 0x1a, 0xf8])
                .unwrap()
                .value(),
            1726.0
        );
    }
}
