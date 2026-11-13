//! One module that routes a signal key and decodes its evidence (REQ-024.6).
//!
//! A signal key is either a standard Mode 01 semantic (`engine.rpm`) or a Knowledge signal addressed as
//! `<semantic>@<target id>` (#256). [`SignalRoute::of`] is the only place that decides which, and
//! [`decode`] is the only place that turns request/response evidence into a [`SignalOutcome`]. Live
//! reads, replay, the availability sweep and CLI capture replay all hand it plain evidence and render the
//! outcome through one of its views. It has no transport, no session and no authorisation: those stay with
//! the callers that produce the evidence.

use crate::{
    knowledge_db::{
        AsciiTrim, Endian, HardwareValidationStatus, KnowledgeDecoder, KnowledgeDefinition,
        KnowledgePin,
    },
    prepare_read, ReadRequest, Transaction,
};
use serde_json::{json, Value};

/// #256: the identity of a live signal. A Knowledge semantic can coincide with a standard PID or with
/// the same semantic of another ECU, so a Knowledge signal is addressed as `<semantic>@<target id>`.
pub(crate) fn signal_key(semantic: &str, target_id: &str) -> String {
    format!("{semantic}@{target_id}")
}

#[derive(Debug)]
pub(crate) enum SignalRoute {
    Standard(ReadRequest),
    Knowledge {
        semantic: String,
        target_id: Option<String>,
    },
}

impl SignalRoute {
    /// `x@t` is Knowledge signal `x` of target `t`; a known PID semantic is standard; anything else is a
    /// bare Knowledge semantic that the context resolves against its identified ECUs.
    pub(crate) fn of(key: &str) -> Self {
        if let Some((semantic, target)) = key.rsplit_once('@') {
            return Self::Knowledge {
                semantic: semantic.to_owned(),
                target_id: Some(target.to_owned()),
            };
        }
        match prepare_read(key) {
            Ok(request) => Self::Standard(request),
            Err(_) => Self::Knowledge {
                semantic: key.to_owned(),
                target_id: None,
            },
        }
    }
}

/// The key of a recorded read: its `key` field, else derived from `definition_id`/`target_id`. A recorded
/// `key` that contradicts the record's own semantic, definition or target is rejected.
pub(crate) fn key_of_recording(recorded: &Value) -> Result<String, String> {
    let text = |field: &str| recorded.get(field).and_then(Value::as_str);
    let semantic = text("semantic").ok_or("recording semantic missing")?;
    let derived = match text("definition_id") {
        Some(_) => signal_key(
            semantic,
            text("target_id").ok_or("Knowledge recording target_id missing")?,
        ),
        None => semantic.to_owned(),
    };
    let Some(key) = text("key") else {
        return Ok(derived);
    };
    let same = if text("definition_id").is_some() {
        key == derived
    } else {
        key.rsplit_once('@').map_or(key, |(semantic, _)| semantic) == semantic
    };
    if same {
        Ok(key.to_owned())
    } else {
        Err(format!(
            "recorded key {key} contradicts its semantic, definition or target"
        ))
    }
}

pub(crate) struct Evidence {
    pub(crate) request: Vec<u8>,
    pub(crate) response: Option<Vec<u8>>,
    pub(crate) error: Option<String>,
}

pub(crate) enum Decoder<'a> {
    Standard {
        request: ReadRequest,
        source: &'a str,
    },
    Knowledge {
        definition: &'a KnowledgeDefinition,
        target_id: &'a str,
        pin: &'a KnowledgePin,
    },
}

#[derive(Debug)]
pub(crate) enum SignalReadError {
    /// The evidence's request is not the one the decoder's definition issues.
    RequestChanged(String),
}

#[derive(Debug)]
pub(crate) enum OutcomeState {
    Value {
        value: Value,
        /// The plausible range, when the value lies outside it.
        implausible: Option<(f64, f64)>,
    },
    Rejected {
        nrc: u8,
    },
    /// No usable answer; the reason is the decode failure, else the evidence's own error.
    NotAnswered {
        reason: Option<String>,
    },
}

#[derive(Debug)]
struct DefinitionFacts {
    id: String,
    did: u16,
    validation: &'static str,
    repository: String,
    revision: String,
}

#[derive(Debug)]
pub(crate) struct SignalOutcome {
    pub(crate) key: String,
    pub(crate) semantic: String,
    pub(crate) target_id: Option<String>,
    pub(crate) response: Option<Vec<u8>>,
    pub(crate) unit: String,
    pub(crate) state: OutcomeState,
    /// The completed standard read, for callers that keep telemetry.
    pub(crate) transaction: Option<Transaction>,
    request: Vec<u8>,
    definition: Option<DefinitionFacts>,
}

pub(crate) fn decode(
    key: &str,
    decoder: Decoder<'_>,
    evidence: Evidence,
) -> Result<SignalOutcome, SignalReadError> {
    let Evidence {
        request,
        response,
        error,
    } = evidence;
    match decoder {
        Decoder::Standard {
            request: read,
            source,
        } => {
            if request.as_slice() != read.bytes() {
                return Err(SignalReadError::RequestChanged(format!(
                    "recorded request {request:?} differs from current semantic request {:?}",
                    read.bytes()
                )));
            }
            let (transaction, state) = match response
                .as_ref()
                .map(|response| read.complete(source, response.clone()))
            {
                Some(Ok(transaction)) => {
                    let state = OutcomeState::Value {
                        value: json!(transaction.value()),
                        implausible: None,
                    };
                    (Some(transaction), state)
                }
                Some(Err(reason)) => (
                    None,
                    OutcomeState::NotAnswered {
                        reason: Some(reason),
                    },
                ),
                None => (None, OutcomeState::NotAnswered { reason: error }),
            };
            Ok(SignalOutcome {
                key: key.to_owned(),
                semantic: read.semantic().to_owned(),
                target_id: None,
                response,
                unit: read.unit().to_owned(),
                state,
                transaction,
                request,
                definition: None,
            })
        }
        Decoder::Knowledge {
            definition,
            target_id,
            pin,
        } => {
            let expected = definition.operation().request_bytes();
            if request.as_slice() != expected {
                return Err(SignalReadError::RequestChanged(format!(
                    "recorded request {request:?} differs from definition {} request {expected:?}",
                    definition.id()
                )));
            }
            let state = match decode_definition(definition, response.as_deref()) {
                Ok(Some(value)) => {
                    let implausible = match (value.as_f64(), definition.plausible_range()) {
                        (Some(number), Some(range))
                            if number < range.minimum() || number > range.maximum() =>
                        {
                            Some((range.minimum(), range.maximum()))
                        }
                        _ => None,
                    };
                    OutcomeState::Value { value, implausible }
                }
                Ok(None) => OutcomeState::NotAnswered { reason: error },
                Err(DecodeFailure::NegativeResponse(nrc)) => OutcomeState::Rejected { nrc },
                Err(DecodeFailure::Invalid(reason)) => OutcomeState::NotAnswered {
                    reason: Some(reason),
                },
            };
            Ok(SignalOutcome {
                key: key.to_owned(),
                semantic: definition.semantic().to_owned(),
                target_id: Some(target_id.to_owned()),
                response,
                unit: definition.unit().unwrap_or("").to_owned(),
                state,
                transaction: None,
                request,
                definition: Some(DefinitionFacts {
                    id: definition.id().to_owned(),
                    did: definition.operation().did(),
                    validation: hardware_validation(definition),
                    repository: pin.repository().to_owned(),
                    revision: pin.revision().to_owned(),
                }),
            })
        }
    }
}

impl SignalOutcome {
    /// The read result of a live read or of `replay.read`. A standard read that did not decode is an error.
    pub(crate) fn read_json(&self, replay: bool) -> Result<Value, String> {
        let Some(definition) = &self.definition else {
            let (OutcomeState::Value { value, .. }, Some(transaction)) =
                (&self.state, &self.transaction)
            else {
                return Err(self.reason().unwrap_or("no usable answer").to_owned());
            };
            let mut result = json!({"key":self.key,"semantic":self.semantic,"value":value,
                "unit":self.unit,"timestamp_ms":transaction.timestamp_ms(),"response":self.response});
            if replay {
                result["replay"] = json!(true);
            }
            return Ok(result);
        };
        let (status, nrc) = match &self.state {
            OutcomeState::Value { .. } => ("available", None),
            OutcomeState::Rejected { nrc } => ("unavailable", Some(*nrc)),
            OutcomeState::NotAnswered { .. } => ("unavailable", None),
        };
        let mut result = self.knowledge_base(definition);
        result["status"] = json!(status);
        result["nrc"] = json!(nrc);
        result["validation"] = json!(definition.validation);
        result["validation_label"] = json!(validation_label(definition.validation));
        result["replay"] = json!(replay);
        match &self.state {
            OutcomeState::Value { value, .. } => result["value"] = value.clone(),
            OutcomeState::Rejected { .. } => result["error"] = json!("negative response"),
            OutcomeState::NotAnswered { reason } => {
                result["error"] = json!(reason.as_deref().unwrap_or("Knowledge DID unavailable"));
            }
        }
        Ok(result)
    }

    /// REQ-030.8/9: one availability entry; no value is kept for a non-answer.
    pub(crate) fn sweep_entry(&self) -> Value {
        let mut entry = json!({"key":self.key,"target":self.target_id});
        if let Some(definition) = &self.definition {
            entry["definition_id"] = json!(definition.id);
            entry["response"] = json!(self.response);
        }
        match &self.state {
            OutcomeState::Value { value, implausible } => {
                entry["status"] = json!("answered");
                entry["response"] = json!(self.response);
                entry["value"] = value.clone();
                entry["unit"] = json!(self.unit);
                if let Some((minimum, maximum)) = implausible {
                    entry["implausible"] = json!(true);
                    entry["plausible_range"] = json!({"minimum":minimum,"maximum":maximum});
                }
            }
            OutcomeState::Rejected { nrc } => {
                entry["status"] = json!("rejected");
                entry["nrc"] = json!(nrc);
            }
            OutcomeState::NotAnswered { .. } => {
                entry["status"] = json!("not_answered");
                entry["reason"] = json!(self.reason().unwrap_or("no usable answer"));
            }
        }
        entry
    }

    /// REQ-026: the result of one learning probe of a session-flagged Knowledge definition.
    pub(crate) fn learning_json(&self) -> Value {
        let definition = self
            .definition
            .as_ref()
            .expect("learning decodes Knowledge definitions only");
        let mut result = self.knowledge_base(definition);
        let (status, nrc) = match &self.state {
            OutcomeState::Value { .. } => ("answered", None),
            OutcomeState::Rejected { nrc } => ("rejected", Some(*nrc)),
            OutcomeState::NotAnswered { .. } => ("not_answered", None),
        };
        result["status"] = json!(status);
        result["nrc"] = json!(nrc);
        result["validation"] = json!("unconfirmed");
        result["validation_state"] = json!(definition.validation);
        result["validation_label"] = json!(validation_label(definition.validation));
        result["observations"] = json!([]);
        match &self.state {
            OutcomeState::Value { value, .. } => result["value"] = value.clone(),
            OutcomeState::NotAnswered {
                reason: Some(reason),
            } => result["error"] = json!(reason),
            _ => {}
        }
        result
    }

    fn reason(&self) -> Option<&str> {
        match &self.state {
            OutcomeState::NotAnswered { reason } => reason.as_deref(),
            _ => None,
        }
    }

    fn knowledge_base(&self, definition: &DefinitionFacts) -> Value {
        json!({
            "key":self.key,
            "knowledge_repository":definition.repository,
            "knowledge_revision":definition.revision,
            "target_id":self.target_id,
            "semantic":self.semantic,
            "definition_id":definition.id,
            "did":definition.did,
            "request":self.request,
            "response":self.response,
            "unit":self.unit
        })
    }
}

pub(crate) fn hardware_validation(definition: &KnowledgeDefinition) -> &'static str {
    match definition.hardware_validation().status() {
        HardwareValidationStatus::NotValidated => "not_validated",
        HardwareValidationStatus::Validated => "validated",
        HardwareValidationStatus::NotApplicable => "not_applicable",
    }
}

pub(crate) fn validation_label(validation: &str) -> &'static str {
    match validation {
        "not_validated" => "nicht abgeglichen",
        "validated" => "validiert",
        "not_applicable" => "nicht anwendbar",
        _ => "unbekannt",
    }
}

enum DecodeFailure {
    NegativeResponse(u8),
    Invalid(String),
}

fn decode_definition(
    definition: &KnowledgeDefinition,
    response: Option<&[u8]>,
) -> Result<Option<Value>, DecodeFailure> {
    let Some(response) = response else {
        return Ok(None);
    };
    if response.starts_with(&[0x7f, 0x22]) {
        return if response.len() == 3 {
            Err(DecodeFailure::NegativeResponse(response[2]))
        } else {
            Err(DecodeFailure::Invalid(
                "malformed UDS negative response".into(),
            ))
        };
    }
    let payload = definition
        .validate_response(response)
        .map_err(DecodeFailure::Invalid)?;
    decode_value(definition.decoder(), payload)
        .map(Some)
        .map_err(DecodeFailure::Invalid)
}

fn decode_value(decoder: &KnowledgeDecoder, payload: &[u8]) -> Result<Value, String> {
    match decoder {
        KnowledgeDecoder::OpaqueBytes => Ok(json!(payload)),
        KnowledgeDecoder::Ascii { trim } => {
            if !payload.is_ascii() {
                return Err("Knowledge ASCII decoder received non-ASCII bytes".into());
            }
            let trim_byte = |byte: u8| match trim {
                AsciiTrim::None => false,
                AsciiTrim::Space => byte == b' ',
                AsciiTrim::Nul => byte == 0,
                AsciiTrim::SpaceAndNul => matches!(byte, 0 | b' '),
            };
            let end = payload
                .iter()
                .rposition(|byte| !trim_byte(*byte))
                .map_or(0, |index| index + 1);
            let payload = &payload[..end];
            if payload.is_empty() || payload.iter().any(u8::is_ascii_control) {
                return Err("Knowledge ASCII decoder received empty or control bytes".into());
            }
            Ok(json!(
                std::str::from_utf8(payload).map_err(|error| error.to_string())?
            ))
        }
        KnowledgeDecoder::LinearInteger {
            width_bytes,
            endian,
            signed,
            scale,
            offset,
        } => {
            if payload.len() != usize::from(*width_bytes) {
                return Err(format!(
                    "Knowledge linear decoder expected {width_bytes} bytes, received {}",
                    payload.len()
                ));
            }
            let mut raw = 0u64;
            match endian {
                Endian::Big => {
                    for byte in payload {
                        raw = (raw << 8) | u64::from(*byte);
                    }
                }
                Endian::Little => {
                    for (shift, byte) in payload.iter().enumerate() {
                        raw |= u64::from(*byte) << (shift * 8);
                    }
                }
            }
            let bit_count = u32::from(*width_bytes) * 8;
            let integer = if *signed && raw & (1u64 << (bit_count - 1)) != 0 {
                (raw as i128) - (1i128 << bit_count)
            } else {
                raw as i128
            };
            let value = (integer as f64) * scale + offset;
            if !value.is_finite() {
                return Err("Knowledge linear decoder produced a non-finite value".into());
            }
            Ok(json!(value))
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{knowledge_db::KnowledgeCatalog, mobile::knowledge};

    const TARGET: &str = "vag.ecu_target.7e0";

    fn without_key(mut value: Value) -> Value {
        value.as_object_mut().unwrap().remove("key");
        value
    }

    fn without_timestamp(mut value: Value) -> Value {
        value.as_object_mut().unwrap().remove("timestamp_ms");
        value
    }

    fn knowledge_outcome(
        catalog: &KnowledgeCatalog,
        definition: &KnowledgeDefinition,
        response: Option<&[u8]>,
        error: Option<&str>,
    ) -> SignalOutcome {
        decode(
            "k",
            Decoder::Knowledge {
                definition,
                target_id: TARGET,
                pin: catalog.pin(),
            },
            Evidence {
                request: definition.operation().request_bytes().to_vec(),
                response: response.map(<[u8]>::to_vec),
                error: error.map(str::to_owned),
            },
        )
        .unwrap()
    }

    fn responses(definition: &KnowledgeDefinition) -> Vec<Option<Vec<u8>>> {
        let did = definition.operation().did();
        let head = [0x62, (did >> 8) as u8, did as u8];
        vec![
            Some([&head[..], &[0x04, 0x00]].concat()),
            Some([&head[..], &[0x01]].concat()),
            Some(vec![0x7f, 0x22, 0x31]),
            Some(vec![0x7f, 0x22]),
            None,
        ]
    }

    // REQ-024.6: the views reproduce the former per-context decoders field for field.
    #[test]
    fn req_024_6_views_equal_the_former_knowledge_decoders() {
        let catalog = KnowledgeCatalog::embedded_pinned().unwrap();
        let oil = catalog
            .definitions_for_semantic("engine.oil_temperature")
            .first()
            .unwrap()
            .id()
            .to_owned();
        for id in [
            "vag.ea189_session.air.intercooler_outlet_temperature",
            oil.as_str(),
        ] {
            let definition = catalog.definition(id).unwrap();
            for response in responses(definition) {
                for error in [None, Some("timeout")] {
                    let outcome =
                        knowledge_outcome(&catalog, definition, response.as_deref(), error);
                    let old_read = knowledge::decode_read_result(
                        &catalog,
                        TARGET,
                        definition,
                        response.as_deref(),
                        error,
                        true,
                    );
                    assert_eq!(
                        without_timestamp(without_key(outcome.read_json(true).unwrap())),
                        without_timestamp(old_read),
                        "{id} {response:?} {error:?}"
                    );
                    assert_eq!(
                        without_key(outcome.learning_json()),
                        knowledge::decode_learning_result(
                            &catalog,
                            TARGET,
                            definition,
                            response.as_deref(),
                            error
                        ),
                        "{id} {response:?} {error:?}"
                    );
                    assert_eq!(
                        outcome.sweep_entry(),
                        knowledge::availability_entry(
                            &catalog,
                            "k",
                            TARGET,
                            id,
                            response.as_deref(),
                            error
                        )
                        .unwrap(),
                        "{id} {response:?} {error:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn req_024_6_standard_sweep_entry_equals_the_former_mode_01_entry() {
        let request = prepare_read("engine.rpm").unwrap();
        for response in [
            Some(vec![0x41, 0x0C, 0x1A, 0xF8]),
            Some(vec![0x41, 0x0C]),
            None,
        ] {
            for reason in [None, Some("no answer")] {
                let mut outcome = decode(
                    "engine.rpm",
                    Decoder::Standard {
                        request,
                        source: "test",
                    },
                    Evidence {
                        request: request.bytes().to_vec(),
                        response: response.clone(),
                        error: reason.map(str::to_owned),
                    },
                )
                .unwrap();
                outcome.target_id = Some("t".into());
                assert_eq!(
                    outcome.sweep_entry(),
                    knowledge::mode01_entry("engine.rpm", Some("t"), response.clone(), reason),
                    "{response:?} {reason:?}"
                );
            }
        }
    }

    #[test]
    fn req_024_6_route_rule_is_key_based() {
        assert!(matches!(
            SignalRoute::of("engine.rpm"),
            SignalRoute::Standard(_)
        ));
        assert!(matches!(
            SignalRoute::of("engine.oil_temperature@vag.ecu_target.7e0"),
            SignalRoute::Knowledge { target_id: Some(t), .. } if t == TARGET
        ));
        assert!(matches!(
            SignalRoute::of("air.charge_limit_overcharge"),
            SignalRoute::Knowledge {
                target_id: None,
                ..
            }
        ));
        // A PID semantic with a target is a Knowledge signal.
        assert!(matches!(
            SignalRoute::of("engine.rpm@vag.ecu_target.7e0"),
            SignalRoute::Knowledge { semantic, .. } if semantic == "engine.rpm"
        ));
    }

    #[test]
    fn req_024_6_recording_key_is_derived_or_must_agree() {
        let pid = json!({"semantic":"engine.rpm"});
        assert_eq!(key_of_recording(&pid).unwrap(), "engine.rpm");
        let knowledge = json!({"semantic":"s","definition_id":"d","target_id":"t"});
        assert_eq!(key_of_recording(&knowledge).unwrap(), "s@t");
        let keyed = json!({"semantic":"s","definition_id":"d","target_id":"t","key":"s@t"});
        assert_eq!(key_of_recording(&keyed).unwrap(), "s@t");
        let other = json!({"semantic":"s","definition_id":"d","target_id":"t","key":"s@u"});
        assert!(key_of_recording(&other).is_err());
        let wrong = json!({"semantic":"engine.rpm","key":"engine.speed"});
        assert!(key_of_recording(&wrong).is_err());
    }

    #[test]
    fn req_024_6_changed_request_is_reported_not_decoded() {
        let request = prepare_read("engine.rpm").unwrap();
        let changed = decode(
            "engine.rpm",
            Decoder::Standard {
                request,
                source: "test",
            },
            Evidence {
                request: vec![0x01, 0xFF],
                response: Some(vec![0x41, 0x0C, 0x1A, 0xF8]),
                error: None,
            },
        );
        assert!(matches!(changed, Err(SignalReadError::RequestChanged(_))));
    }

    #[test]
    fn req_024_2_knowledge_decoder_handles_signed_scaled_and_ascii_values() {
        assert_eq!(
            decode_value(
                &KnowledgeDecoder::LinearInteger {
                    width_bytes: 2,
                    endian: Endian::Big,
                    signed: true,
                    scale: 0.01,
                    offset: 0.0,
                },
                &[0xff, 0x9c],
            )
            .unwrap(),
            json!(-1.0)
        );
        assert_eq!(
            decode_value(
                &KnowledgeDecoder::Ascii {
                    trim: AsciiTrim::SpaceAndNul
                },
                b"part  \0",
            )
            .unwrap(),
            json!("part")
        );
        assert!(decode_value(
            &KnowledgeDecoder::LinearInteger {
                width_bytes: 2,
                endian: Endian::Big,
                signed: false,
                scale: 1.0,
                offset: 0.0,
            },
            &[0x01],
        )
        .is_err());
    }
}
