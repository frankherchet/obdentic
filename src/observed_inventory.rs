//! Validated, read-only target evidence assembled from functional discovery.
//!
//! This module keeps the fixed physical OBD-II validation rules together. It
//! accepts only the responder evidence already observed by functional
//! discovery and returns typed mappings for later cache and routing consumers.

use crate::{
    ble::{self, SessionClient, TargetedReadRequest},
    functional_discovery::{CapabilityStatus, FunctionalResponderDiscovery},
    topology::{
        AddressingContext, Confidence, EcuRole, Protocol, ProtocolContext, Provenance,
        RequestAddress, RequestTarget, ResponderIdentity, RoleAssignment,
    },
    vehicle_cache::TargetMappingSnapshot,
    Transaction,
};

/// Validate the known physical OBD-II targets exposed by functional discovery.
///
/// Engine target validation is fatal, matching vehicle discovery's existing
/// behavior. Secondary target validation is best-effort: an unavailable or
/// malformed secondary target is omitted while the validated engine mapping
/// remains usable.
pub async fn discover_target_mappings(
    session: &SessionClient,
    discovery: &FunctionalResponderDiscovery,
) -> Result<Vec<TargetMappingSnapshot>, String> {
    if !engine_responder_observed(discovery) {
        return Ok(Vec::new());
    }

    let engine = validate_engine_target(session, discovery).await?;
    let secondary = match validate_secondary_target(session, discovery).await {
        Ok(mapping) => mapping,
        Err(error) => {
            eprintln!("secondary target unavailable; continuing discovery: {error}");
            None
        }
    };
    Ok(merge_target_mappings(engine, secondary))
}

async fn validate_engine_target(
    session: &SessionClient,
    discovery: &FunctionalResponderDiscovery,
) -> Result<TargetMappingSnapshot, String> {
    if !engine_responder_observed(discovery) {
        return Err("engine target validation requires observed responder 7E8".into());
    }

    let transaction = session.read_targeted(engine_target_request()?).await?;
    validate_engine_target_transaction(&transaction)?;
    confirmed_engine_target()
}

async fn validate_secondary_target(
    session: &SessionClient,
    discovery: &FunctionalResponderDiscovery,
) -> Result<Option<TargetMappingSnapshot>, String> {
    if !secondary_target_allowed(discovery) {
        return Ok(None);
    }

    let transaction = session.read_targeted(secondary_target_request()?).await?;
    validate_vehicle_speed_target_transaction(&transaction)?;
    Ok(Some(confirmed_secondary_target()?))
}

fn secondary_target_allowed(discovery: &FunctionalResponderDiscovery) -> bool {
    discovery.capabilities().iter().any(|capability| {
        capability
            .responder()
            .value()
            .is_some_and(|value| value.eq_ignore_ascii_case("7E9"))
            && matches!(
                capability.status("vehicle.speed"),
                Ok(CapabilityStatus::Supported)
            )
    })
}

fn validate_vehicle_speed_target_transaction(transaction: &Transaction) -> Result<(), String> {
    if transaction.semantic() != "vehicle.speed"
        || transaction.request() != [0x01, 0x0D]
        || transaction.response().len() != 3
        || transaction.response().first() != Some(&0x41)
        || transaction.response().get(1) != Some(&0x0D)
    {
        return Err("targeted secondary validation returned an invalid 010D response".into());
    }
    Ok(())
}

fn secondary_target_request() -> Result<TargetedReadRequest, String> {
    let context = physical_obd2_context();
    TargetedReadRequest::new(
        crate::prepare_read("vehicle.speed")?,
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E1")),
        ble::ResponderIdentity::ElmHeader("7E9".into()),
    )
}

fn confirmed_secondary_target() -> Result<TargetMappingSnapshot, String> {
    let context = physical_obd2_context();
    let provenance = Provenance::new(
        "targeted vehicle.speed Mode 01 validation",
        Confidence::Verified,
    )
    .map_err(|error| error.to_string())?;
    Ok(TargetMappingSnapshot::new(
        None,
        Some(ResponderIdentity::address(context.clone(), "7E9")),
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E1")),
        provenance,
    ))
}

fn merge_target_mappings(
    engine: TargetMappingSnapshot,
    secondary: Option<TargetMappingSnapshot>,
) -> Vec<TargetMappingSnapshot> {
    let mut mappings = vec![engine];
    if let Some(secondary) = secondary {
        mappings.push(secondary);
    }
    mappings.sort();
    mappings
}

fn engine_responder_observed(discovery: &FunctionalResponderDiscovery) -> bool {
    discovery.responders().iter().any(|responder| {
        responder
            .value()
            .is_some_and(|value| value.eq_ignore_ascii_case("7E8"))
    })
}

fn validate_engine_target_transaction(transaction: &Transaction) -> Result<(), String> {
    if transaction.semantic() != "engine.rpm"
        || transaction.request() != [0x01, 0x0C]
        || transaction.response().len() != 4
        || transaction.response().first() != Some(&0x41)
        || transaction.response().get(1) != Some(&0x0C)
    {
        return Err("targeted engine validation returned an invalid 010C response".into());
    }
    Ok(())
}

fn engine_target_request() -> Result<TargetedReadRequest, String> {
    let context = physical_obd2_context();
    TargetedReadRequest::new(
        crate::prepare_read("engine.rpm")?,
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E0")),
        ble::ResponderIdentity::ElmHeader("7E8".into()),
    )
}

fn confirmed_engine_target() -> Result<TargetMappingSnapshot, String> {
    let context = physical_obd2_context();
    let provenance = Provenance::new(
        "targeted engine.rpm Mode 01 validation",
        Confidence::Verified,
    )
    .map_err(|error| error.to_string())?;
    Ok(TargetMappingSnapshot::new(
        Some(RoleAssignment::new(EcuRole::Engine, provenance.clone())),
        Some(ResponderIdentity::address(context.clone(), "7E8")),
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E0")),
        provenance,
    ))
}

fn physical_obd2_context() -> ProtocolContext {
    ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{functional_discovery::FunctionalPageObservation, prepare_read};

    fn provenance() -> Provenance {
        Provenance::new("test", Confidence::High).unwrap()
    }

    fn discovery(responders: &[(&str, [u8; 4])]) -> FunctionalResponderDiscovery {
        FunctionalResponderDiscovery::new(responders.iter().map(|(responder, bitmap)| {
            FunctionalPageObservation::new(
                [0x01, 0x00],
                ResponderIdentity::opaque(
                    ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional),
                    *responder,
                ),
                [0x41, 0x00, bitmap[0], bitmap[1], bitmap[2], bitmap[3]].to_vec(),
                provenance(),
            )
            .unwrap()
        }))
    }

    #[test]
    fn target_requests_are_closed_and_physical() {
        let engine = engine_target_request().unwrap();
        assert_eq!(engine.request().bytes(), [0x01, 0x0C]);
        assert_eq!(engine.target().address().unwrap().value(), "7E0");
        assert_eq!(engine.expected_responder().as_str(), "7E8");

        let secondary = secondary_target_request().unwrap();
        assert_eq!(secondary.request().bytes(), [0x01, 0x0D]);
        assert_eq!(secondary.target().address().unwrap().value(), "7E1");
        assert_eq!(secondary.expected_responder().as_str(), "7E9");
    }

    #[test]
    fn engine_mapping_preserves_role_target_responder_and_provenance() {
        let mapping = confirmed_engine_target().unwrap();
        assert_eq!(mapping.role().unwrap().role(), &EcuRole::Engine);
        assert_eq!(mapping.target().address().unwrap().value(), "7E0");
        assert_eq!(mapping.responder().unwrap().value(), Some("7E8"));
        assert_ne!(
            mapping.target().address().unwrap().value(),
            mapping.responder().unwrap().value().unwrap()
        );
        assert_eq!(mapping.confidence(), Confidence::Verified);
        assert_eq!(
            mapping.provenance().source(),
            "targeted engine.rpm Mode 01 validation"
        );
    }

    #[test]
    fn target_validation_requires_the_expected_closed_reads() {
        let valid_engine = prepare_read("engine.rpm")
            .unwrap()
            .complete("test", vec![0x41, 0x0C, 0x00, 0x00])
            .unwrap();
        assert!(validate_engine_target_transaction(&valid_engine).is_ok());

        let wrong_signal = prepare_read("vehicle.speed")
            .unwrap()
            .complete("test", vec![0x41, 0x0D, 0x00])
            .unwrap();
        assert!(validate_engine_target_transaction(&wrong_signal).is_err());

        let valid_secondary = prepare_read("vehicle.speed")
            .unwrap()
            .complete("test", vec![0x41, 0x0D, 0x00])
            .unwrap();
        assert!(validate_vehicle_speed_target_transaction(&valid_secondary).is_ok());
    }

    #[test]
    fn secondary_target_requires_7e9_functional_support() {
        let engine_discovery = discovery(&[("7E8", [0, 8, 0, 0])]);
        assert!(!secondary_target_allowed(&engine_discovery));
        let secondary_discovery = discovery(&[("7E9", [0, 8, 0, 0])]);
        assert!(secondary_target_allowed(&secondary_discovery));
    }

    #[test]
    fn engine_target_requires_7e8_functional_evidence() {
        assert!(!engine_responder_observed(&discovery(&[])));
        let secondary_discovery = discovery(&[("7E9", [0, 0, 0, 0])]);
        assert!(!engine_responder_observed(&secondary_discovery));
        let engine_discovery = discovery(&[("7E8", [0, 0, 0, 0])]);
        assert!(engine_responder_observed(&engine_discovery));
    }
}
