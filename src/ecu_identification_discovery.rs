//! Bounded per-ECU identification discovery.
//!
//! This module connects already-evidenced physical ECU targets to the
//! canonical identification plan. It never discovers targets, never accepts a
//! caller-provided DID, and never escalates the diagnostic session.

use crate::{
    ble::{DiagnosticResponses, SessionClient, TargetedEcuIdentificationRequest},
    ecu_identification::{
        EcuIdentificationPlan, IdentificationCandidate, IdentificationObservation,
        IdentificationResponseEvidence, IdentificationResultStatus,
    },
    protocol::ReadResponseError,
    runtime_state::Activity,
    safety::{Operation, OperationRequest, SafetyPolicy},
    scheduler::is_fatal_runtime_error,
    topology::{Confidence, RequestTargetEvidence, ResponderIdentity},
    vehicle_cache::TargetMappingSnapshot,
};

const SESSION_ABORT_REASON: &str =
    "not probed because the diagnostic session became unavailable after a fatal transport failure";

/// Execute the canonical bounded identification plan against already-evidenced
/// physical targets only. Mapping order is normalized before execution and
/// candidate order is inherited unchanged from canonical Knowledge.
pub async fn discover_known_ecus(
    session: &SessionClient,
    plan: &EcuIdentificationPlan,
    mappings: &[TargetMappingSnapshot],
) -> Result<Vec<IdentificationObservation>, String> {
    let mut observations = Vec::new();
    let mut session_unavailable = false;

    for mapping in eligible_mappings(mappings) {
        let expected_responder = mapping.responder().ok_or_else(|| {
            "eligible ECU identification mapping lost responder evidence".to_string()
        })?;
        let target =
            RequestTargetEvidence::new(mapping.target().clone(), mapping.provenance().clone());

        for candidate in plan.candidates() {
            if session_unavailable {
                observations.push(not_probed(plan, candidate, mapping, SESSION_ABORT_REASON)?);
                continue;
            }

            let authorized = SafetyPolicy::read_only()
                .authorize_activity(
                    Activity::Diagnose,
                    OperationRequest::EcuIdentification(candidate.clone()),
                )
                .map_err(|error| error.to_string())?;
            let Operation::EcuIdentification(candidate) = authorized else {
                return Err(
                    "ECU identification safety authorization returned a different operation".into(),
                );
            };
            let request = TargetedEcuIdentificationRequest::from_evidence(
                &candidate,
                &target,
                expected_responder,
            )?;

            match session.read_ecu_identification(request).await {
                Ok(responses) => observations.push(classify_diagnostic_responses(
                    plan, &candidate, mapping, &responses,
                )?),
                Err(error) => {
                    let fatal = is_fatal_runtime_error(&error);
                    observations.push(transport_failure(plan, &candidate, mapping, error)?);
                    session_unavailable |= fatal;
                }
            }
        }
    }

    Ok(observations)
}

fn eligible_mappings(mappings: &[TargetMappingSnapshot]) -> Vec<&TargetMappingSnapshot> {
    let mut eligible = mappings
        .iter()
        .filter(|mapping| {
            matches!(
                mapping.confidence(),
                Confidence::High | Confidence::Verified
            ) && mapping.target().address().is_some()
                && mapping
                    .responder()
                    .and_then(ResponderIdentity::value)
                    .is_some()
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| (*left).cmp(*right));
    eligible
}

fn classify_diagnostic_responses(
    plan: &EcuIdentificationPlan,
    candidate: &IdentificationCandidate,
    mapping: &TargetMappingSnapshot,
    responses: &DiagnosticResponses,
) -> Result<IdentificationObservation, String> {
    let context = mapping.target().context().clone();
    let expected = mapping
        .responder()
        .ok_or_else(|| "ECU identification classification requires a responder".to_string())?;
    let expected_value = expected.value().ok_or_else(|| {
        "ECU identification classification requires a responder value".to_string()
    })?;

    let evidence = responses
        .as_slice()
        .iter()
        .map(|response| {
            IdentificationResponseEvidence::new(
                response.responder.as_ref().map(|responder| {
                    ResponderIdentity::address(context.clone(), responder.as_str())
                }),
                response.payload.clone(),
            )
        })
        .collect::<Vec<_>>();
    let errors = responses
        .errors()
        .iter()
        .map(|error| match error.responder.as_ref() {
            Some(responder) => format!("responder {}: {}", responder.as_str(), error.error),
            None => error.error.clone(),
        })
        .collect::<Vec<_>>();
    let has_expected_error = responses.errors().iter().any(|error| {
        error
            .responder
            .as_ref()
            .is_some_and(|responder| responder.as_str().eq_ignore_ascii_case(expected_value))
    });
    let adapter_unavailable = responses.as_slice().is_empty()
        && responses.errors().iter().any(|error| {
            error.responder.is_none()
                && error
                    .error
                    .to_ascii_uppercase()
                    .contains("ELM327 REJECTED UDS 22 RESPONSE")
        });

    classify_normalized(
        plan,
        candidate,
        mapping,
        evidence,
        errors,
        has_expected_error,
        adapter_unavailable,
    )
}

fn classify_normalized(
    plan: &EcuIdentificationPlan,
    candidate: &IdentificationCandidate,
    mapping: &TargetMappingSnapshot,
    responses: Vec<IdentificationResponseEvidence>,
    mut errors: Vec<String>,
    has_expected_error: bool,
    adapter_unavailable: bool,
) -> Result<IdentificationObservation, String> {
    let expected = mapping
        .responder()
        .ok_or_else(|| "ECU identification classification requires a responder".to_string())?;
    let expected_value = expected.value().ok_or_else(|| {
        "ECU identification classification requires a responder value".to_string()
    })?;
    let matching = responses
        .iter()
        .filter(|response| {
            response
                .responder()
                .and_then(ResponderIdentity::value)
                .is_some_and(|value| value.eq_ignore_ascii_case(expected_value))
        })
        .collect::<Vec<_>>();

    let (status, nrc, value) = if matching.is_empty() {
        errors.push(format!(
            "expected responder {expected_value} did not provide a normalized ECU identification payload"
        ));
        let status = if adapter_unavailable {
            IdentificationResultStatus::Unavailable
        } else if has_expected_error || !responses.is_empty() {
            IdentificationResultStatus::Malformed
        } else {
            IdentificationResultStatus::Unavailable
        };
        (status, None, None)
    } else {
        let payload = matching[0].payload();
        if matching
            .iter()
            .skip(1)
            .any(|response| response.payload() != payload)
        {
            errors.push(format!(
                "conflicting ECU identification payloads from responder {expected_value}"
            ));
            (IdentificationResultStatus::Malformed, None, None)
        } else if has_expected_error {
            errors.push(format!(
                "responder {expected_value} produced malformed ECU identification framing"
            ));
            (IdentificationResultStatus::Malformed, None, None)
        } else {
            match candidate
                .operation()
                .validate_response(payload, candidate.semantic())
            {
                Ok(value) => (
                    IdentificationResultStatus::Supported,
                    None,
                    Some(value.to_vec()),
                ),
                Err(ReadResponseError::UdsNegative { nrc }) => {
                    (status_for_nrc(nrc), Some(nrc), None)
                }
                Err(error) => {
                    errors.push(error.to_string());
                    (IdentificationResultStatus::Malformed, None, None)
                }
            }
        }
    };

    build_observation(
        plan, candidate, mapping, status, responses, nrc, value, errors,
    )
}

fn status_for_nrc(nrc: u8) -> IdentificationResultStatus {
    match nrc {
        // serviceNotSupported and requestOutOfRange are the bounded evidence
        // used here for an explicitly unsupported standard identification read.
        0x11 | 0x31 => IdentificationResultStatus::Unsupported,
        // conditionsNotCorrect, securityAccessDenied and active-session NRCs
        // are unavailable in the current context. Discovery never escalates.
        0x22 | 0x33 | 0x7e | 0x7f => IdentificationResultStatus::Unavailable,
        _ => IdentificationResultStatus::NegativeResponse,
    }
}

fn transport_failure(
    plan: &EcuIdentificationPlan,
    candidate: &IdentificationCandidate,
    mapping: &TargetMappingSnapshot,
    error: String,
) -> Result<IdentificationObservation, String> {
    let lowercase = error.to_ascii_lowercase();
    let status = if lowercase.contains("timed out") || lowercase.contains("timeout") {
        IdentificationResultStatus::Timeout
    } else {
        IdentificationResultStatus::TransportError
    };
    build_observation(
        plan,
        candidate,
        mapping,
        status,
        Vec::new(),
        None,
        None,
        vec![error],
    )
}

fn not_probed(
    plan: &EcuIdentificationPlan,
    candidate: &IdentificationCandidate,
    mapping: &TargetMappingSnapshot,
    reason: &str,
) -> Result<IdentificationObservation, String> {
    build_observation(
        plan,
        candidate,
        mapping,
        IdentificationResultStatus::NotProbed,
        Vec::new(),
        None,
        None,
        vec![reason.to_owned()],
    )
}

#[allow(clippy::too_many_arguments)]
fn build_observation(
    plan: &EcuIdentificationPlan,
    candidate: &IdentificationCandidate,
    mapping: &TargetMappingSnapshot,
    status: IdentificationResultStatus,
    responses: Vec<IdentificationResponseEvidence>,
    nrc: Option<u8>,
    value: Option<Vec<u8>>,
    errors: Vec<String>,
) -> Result<IdentificationObservation, String> {
    let expected_responder = mapping
        .responder()
        .cloned()
        .ok_or_else(|| "ECU identification observation requires responder evidence".to_string())?;
    IdentificationObservation::new(
        mapping.target().clone(),
        expected_responder,
        candidate.semantic(),
        candidate.definition_id(),
        candidate.definition_version(),
        plan.knowledge_repository(),
        plan.knowledge_revision(),
        candidate.request_bytes(),
        status,
        responses,
        nrc,
        value,
        errors,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        knowledge_db::KnowledgeCatalog,
        topology::{
            AddressingContext, Protocol, ProtocolContext, Provenance, RequestAddress, RequestTarget,
        },
    };

    fn plan() -> EcuIdentificationPlan {
        EcuIdentificationPlan::from_catalog(
            &KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap(),
        )
        .unwrap()
    }

    fn mapping(target: &str, responder: &str, confidence: Confidence) -> TargetMappingSnapshot {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        TargetMappingSnapshot::new(
            None,
            Some(ResponderIdentity::address(context.clone(), responder)),
            RequestTarget::concrete(context, RequestAddress::new("elm-header", target)),
            Provenance::new("test mapping", confidence).unwrap(),
        )
    }

    fn candidate(plan: &EcuIdentificationPlan, did: u16) -> IdentificationCandidate {
        plan.candidates()
            .iter()
            .find(|candidate| candidate.did() == did)
            .unwrap()
            .clone()
    }

    fn response(mapping: &TargetMappingSnapshot, payload: &[u8]) -> IdentificationResponseEvidence {
        IdentificationResponseEvidence::new(mapping.responder().cloned(), payload.to_vec())
    }

    #[test]
    fn heterogeneous_ecus_keep_support_independent() {
        let plan = plan();
        let f188 = candidate(&plan, 0xf188);
        let f189 = candidate(&plan, 0xf189);
        let ecu_a = mapping("7E0", "7E8", Confidence::Verified);
        let ecu_b = mapping("7E1", "7E9", Confidence::Verified);

        let a_f188 = classify_normalized(
            &plan,
            &f188,
            &ecu_a,
            vec![response(&ecu_a, &[0x62, 0xf1, 0x88, b'A'])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();
        let a_f189 = classify_normalized(
            &plan,
            &f189,
            &ecu_a,
            vec![response(&ecu_a, &[0x7f, 0x22, 0x31])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();
        let b_f188 = classify_normalized(
            &plan,
            &f188,
            &ecu_b,
            vec![response(&ecu_b, &[0x7f, 0x22, 0x31])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();
        let b_f189 = classify_normalized(
            &plan,
            &f189,
            &ecu_b,
            vec![response(&ecu_b, &[0x62, 0xf1, 0x89, b'B'])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(a_f188.status(), IdentificationResultStatus::Supported);
        assert_eq!(a_f189.status(), IdentificationResultStatus::Unsupported);
        assert_eq!(b_f188.status(), IdentificationResultStatus::Unsupported);
        assert_eq!(b_f189.status(), IdentificationResultStatus::Supported);
        assert_ne!(a_f188.expected_responder(), b_f188.expected_responder());
    }

    #[test]
    fn wrong_did_echo_is_malformed_and_nrc_context_is_explicit() {
        let plan = plan();
        let f189 = candidate(&plan, 0xf189);
        let ecu = mapping("7E0", "7E8", Confidence::High);

        let wrong_did = classify_normalized(
            &plan,
            &f189,
            &ecu,
            vec![response(&ecu, &[0x62, 0xf1, 0x88, 0x01])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();
        let unavailable = classify_normalized(
            &plan,
            &f189,
            &ecu,
            vec![response(&ecu, &[0x7f, 0x22, 0x7f])],
            Vec::new(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(wrong_did.status(), IdentificationResultStatus::Malformed);
        assert_eq!(
            unavailable.status(),
            IdentificationResultStatus::Unavailable
        );
        assert_eq!(unavailable.nrc(), Some(0x7f));
    }

    #[test]
    fn adapter_unavailable_without_nrc_remains_explicit() {
        let plan = plan();
        let f189 = candidate(&plan, 0xf189);
        let ecu = mapping("7E0", "7E8", Confidence::Verified);

        let unavailable = classify_normalized(
            &plan,
            &f189,
            &ecu,
            Vec::new(),
            vec!["ELM327 rejected UDS 22 response: NO DATA".into()],
            false,
            true,
        )
        .unwrap();

        assert_eq!(
            unavailable.status(),
            IdentificationResultStatus::Unavailable
        );
        assert_eq!(unavailable.nrc(), None);
        assert!(!unavailable.errors().is_empty());
    }

    #[test]
    fn timeout_transport_error_and_not_probed_never_collapse() {
        let plan = plan();
        let f189 = candidate(&plan, 0xf189);
        let ecu = mapping("7E0", "7E8", Confidence::Verified);

        let timeout = transport_failure(
            &plan,
            &f189,
            &ecu,
            "Carly response timed out: 22F189".into(),
        )
        .unwrap();
        let transport = transport_failure(
            &plan,
            &f189,
            &ecu,
            "Bluetooth notification stream failed".into(),
        )
        .unwrap();
        let skipped = not_probed(&plan, &f189, &ecu, SESSION_ABORT_REASON).unwrap();

        assert_eq!(timeout.status(), IdentificationResultStatus::Timeout);
        assert_eq!(
            transport.status(),
            IdentificationResultStatus::TransportError
        );
        assert_eq!(skipped.status(), IdentificationResultStatus::NotProbed);
    }

    #[test]
    fn only_high_confidence_concrete_responder_mappings_are_eligible_and_sorted() {
        let low = mapping("7E2", "7EA", Confidence::Low);
        let second = mapping("7E1", "7E9", Confidence::Verified);
        let first = mapping("7E0", "7E8", Confidence::High);
        let mappings = vec![low, second, first];

        let eligible = eligible_mappings(&mappings);
        assert_eq!(eligible.len(), 2);
        assert_eq!(eligible[0].target().address().unwrap().value(), "7E0");
        assert_eq!(eligible[1].target().address().unwrap().value(), "7E1");
    }
}
