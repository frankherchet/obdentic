//! Evidence-gated VW EA189 vehicle knowledge.
//!
//! The profile contains no guessed ECU address and no manufacturer-specific
//! request.  It can only bind an existing generic, read-only semantic to
//! target evidence supplied by discovery or a validated cache.

use std::collections::BTreeMap;

use crate::{
    topology::{AddressingContext, Confidence, EcuRole, Protocol, Provenance},
    vehicle_knowledge::EcuTargetMapping,
    ReadRequest,
};

/// Stable identifier for the first VW vehicle profile.
pub const PROFILE_ID: &str = "vw-ea189-v1";

/// Human-readable platform identity.  This is intentionally not a VIN or a
/// vehicle-specific identity.
pub const PLATFORM: &str = "VW EA189";

/// Identity of the platform knowledge, independent of any one vehicle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ea189ProfileIdentity {
    id: &'static str,
    platform: &'static str,
    engine_family: &'static str,
}

impl Ea189ProfileIdentity {
    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn platform(self) -> &'static str {
        self.platform
    }

    pub const fn engine_family(self) -> &'static str {
        self.engine_family
    }
}

/// The EA189 identity contains only platform-level knowledge.
pub const fn identity() -> Ea189ProfileIdentity {
    Ea189ProfileIdentity {
        id: PROFILE_ID,
        platform: PLATFORM,
        engine_family: "EA189",
    }
}

/// Evidence attached to a profile binding.  Target provenance remains on the
/// supplied [`EcuTargetMapping`]; this provenance describes why the generic
/// semantic is admitted to this profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEvidence {
    provenance: Provenance,
    hardware_validation: String,
}

impl ProfileEvidence {
    pub fn new(
        provenance: Provenance,
        hardware_validation: impl Into<String>,
    ) -> Result<Self, Ea189ProfileError> {
        let hardware_validation = hardware_validation.into();
        if hardware_validation.trim().is_empty() {
            return Err(Ea189ProfileError::MissingHardwareValidation);
        }
        Ok(Self {
            provenance,
            hardware_validation,
        })
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub const fn confidence(&self) -> Confidence {
        self.provenance.confidence()
    }

    pub fn hardware_validation(&self) -> &str {
        &self.hardware_validation
    }
}

/// One valid semantic binding exposed by an [`Ea189Profile`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ea189SignalBinding {
    request: ReadRequest,
    mapping: EcuTargetMapping,
    evidence: ProfileEvidence,
}

impl Ea189SignalBinding {
    pub fn semantic(&self) -> &'static str {
        self.request.metadata().semantic
    }

    pub const fn request(&self) -> ReadRequest {
        self.request
    }

    pub fn metadata(&self) -> &'static crate::SignalMetadata {
        self.request.metadata()
    }

    pub fn target_mapping(&self) -> &EcuTargetMapping {
        &self.mapping
    }

    pub fn evidence(&self) -> &ProfileEvidence {
        &self.evidence
    }
}

/// Small, empty-by-default EA189 profile.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ea189Profile {
    bindings: BTreeMap<String, Ea189SignalBinding>,
}

impl Ea189Profile {
    pub const fn new() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    pub const fn identity(&self) -> Ea189ProfileIdentity {
        identity()
    }

    /// Bind an existing generic semantic to independently supplied,
    /// validated physical target evidence.
    pub fn bind_generic_signal(
        &mut self,
        semantic: &str,
        mapping: EcuTargetMapping,
        evidence: ProfileEvidence,
    ) -> Result<(), Ea189ProfileError> {
        let request = crate::prepare_read(semantic)
            .map_err(|_| Ea189ProfileError::UnsupportedSemantic(semantic.to_owned()))?;
        if request.metadata().profile != "obd2-v1" {
            return Err(Ea189ProfileError::NotGenericSignal(semantic.to_owned()));
        }
        validate_mapping(&mapping)?;
        if self.bindings.contains_key(semantic) {
            return Err(Ea189ProfileError::DuplicateSemantic(semantic.to_owned()));
        }
        self.bindings.insert(
            semantic.to_owned(),
            Ea189SignalBinding {
                request,
                mapping,
                evidence,
            },
        );
        Ok(())
    }

    pub fn binding(&self, semantic: &str) -> Option<&Ea189SignalBinding> {
        self.bindings.get(semantic)
    }

    /// Only externally evidenced bindings are visible as EA189 signals.
    pub fn signals(&self) -> impl Iterator<Item = &Ea189SignalBinding> {
        self.bindings.values()
    }
}

fn validate_mapping(mapping: &EcuTargetMapping) -> Result<(), Ea189ProfileError> {
    let target = mapping.target().target();
    if target.context().protocol() != &Protocol::Obd2
        || target.context().addressing() != &AddressingContext::Physical
    {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require physical OBD-II target evidence".into(),
        ));
    }
    if target.address().is_none() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require a concrete request target".into(),
        ));
    }
    if mapping.expected_responder().context() != target.context() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "target and responder contexts differ".into(),
        ));
    }
    if mapping.expected_responder().value().is_none() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require an expected responder".into(),
        ));
    }
    if mapping.role().role() != &EcuRole::Engine {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "initial EA189 bindings are limited to the engine role".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ea189ProfileError {
    UnsupportedSemantic(String),
    NotGenericSignal(String),
    InvalidTargetEvidence(String),
    MissingHardwareValidation,
    DuplicateSemantic(String),
}

impl std::fmt::Display for Ea189ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSemantic(semantic) => {
                write!(
                    formatter,
                    "EA189 profile rejected unsupported semantic: {semantic}"
                )
            }
            Self::NotGenericSignal(semantic) => {
                write!(
                    formatter,
                    "EA189 profile requires a generic signal: {semantic}"
                )
            }
            Self::InvalidTargetEvidence(reason) => {
                write!(
                    formatter,
                    "EA189 profile rejected target evidence: {reason}"
                )
            }
            Self::MissingHardwareValidation => {
                formatter.write_str("EA189 profile evidence requires hardware validation")
            }
            Self::DuplicateSemantic(semantic) => {
                write!(
                    formatter,
                    "EA189 profile already binds semantic: {semantic}"
                )
            }
        }
    }
}

impl std::error::Error for Ea189ProfileError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{
        ProtocolContext, RequestAddress, RequestTarget, RequestTargetEvidence, ResponderIdentity,
        RoleAssignment,
    };

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn mapping() -> EcuTargetMapping {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        EcuTargetMapping::new(
            RoleAssignment::new(EcuRole::Engine, provenance("validated topology")),
            RequestTargetEvidence::new(
                RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0")),
                provenance("validated topology"),
            ),
            ResponderIdentity::address(context, "7E8"),
        )
    }

    fn evidence() -> ProfileEvidence {
        ProfileEvidence::new(provenance("sanitized hardware fixture"), "fixture-01").unwrap()
    }

    #[test]
    fn identity_is_distinct_from_generic_profile_without_vehicle_identity() {
        let profile = Ea189Profile::new();
        assert_eq!(profile.identity().id(), PROFILE_ID);
        assert_eq!(profile.identity().engine_family(), "EA189");
        assert_ne!(profile.identity().id(), "obd2-v1");
        assert!(profile.signals().next().is_none());
        assert!(!format!("{profile:?}").contains("VIN"));
    }

    #[test]
    fn binding_preserves_generic_request_and_exposes_evidence() {
        let mut profile = Ea189Profile::new();
        profile
            .bind_generic_signal("engine.rpm", mapping(), evidence())
            .unwrap();
        let binding = profile.binding("engine.rpm").unwrap();

        assert_eq!(binding.request().bytes(), [0x01, 0x0c]);
        assert_eq!(binding.metadata().profile, "obd2-v1");
        assert_eq!(
            binding
                .target_mapping()
                .target()
                .target()
                .address()
                .unwrap()
                .value(),
            "7E0"
        );
        assert_eq!(binding.evidence().confidence(), Confidence::High);
        assert_eq!(binding.evidence().hardware_validation(), "fixture-01");
    }

    #[test]
    fn no_evidence_means_no_profile_signal_and_closed_operations_stay_rejected() {
        let mut profile = Ea189Profile::new();
        assert!(matches!(
            profile.bind_generic_signal("dtc.clear", mapping(), evidence(),),
            Err(Ea189ProfileError::UnsupportedSemantic(_))
        ));
        assert!(profile.binding("dtc.clear").is_none());
        assert!(crate::prepare_read("dtc.clear").is_err());
    }

    #[test]
    fn speculative_or_wrong_target_facts_are_not_admitted() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let invalid = EcuTargetMapping::new(
            RoleAssignment::new(EcuRole::Engine, provenance("hypothesis")),
            RequestTargetEvidence::new(
                RequestTarget::functional(context.clone()),
                provenance("hypothesis"),
            ),
            ResponderIdentity::opaque(context, "7E8"),
        );
        let mut profile = Ea189Profile::new();
        assert!(matches!(
            profile.bind_generic_signal("engine.rpm", invalid, evidence()),
            Err(Ea189ProfileError::InvalidTargetEvidence(_))
        ));
        assert!(profile.signals().next().is_none());
    }
}
