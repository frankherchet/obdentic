//! Vehicle-knowledge routing for already validated ECU target evidence.
//!
//! This module owns semantic-to-role rules.  It does not infer a role from a
//! responder header and it never creates a diagnostic request from raw bytes.

use crate::{
    ble::{ResponderIdentity as ElmResponderIdentity, TargetedReadRequest},
    prepare_read,
    topology::{AddressingContext, EcuRole, Protocol, RequestTargetEvidence, ResponderIdentity},
    ReadRequest,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum FallbackPolicy {
    Functional,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct SemanticRoleRule {
    semantic: String,
    role: EcuRole,
}

impl SemanticRoleRule {
    pub fn new(semantic: impl Into<String>, role: EcuRole) -> Self {
        Self {
            semantic: semantic.into(),
            role,
        }
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub fn role(&self) -> &EcuRole {
        &self.role
    }
}

/// The explicit Vehicle-Knowledge semantic-to-role table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VehicleKnowledge {
    rules: BTreeMap<String, SemanticRoleRule>,
}

impl VehicleKnowledge {
    pub fn new(rules: impl IntoIterator<Item = SemanticRoleRule>) -> Self {
        let mut knowledge = Self::default();
        for rule in rules {
            knowledge.rules.insert(rule.semantic.clone(), rule);
        }
        knowledge
    }

    /// Generic OBD-II knowledge names the semantic role, but supplies no
    /// target.  A target still requires explicit per-vehicle evidence.
    pub fn generic_obd2() -> Self {
        Self::new([SemanticRoleRule::new("engine.rpm", EcuRole::Engine)])
    }

    pub fn rule(&self, semantic: &str) -> Option<&SemanticRoleRule> {
        self.rules.get(semantic)
    }

    pub fn route(
        &self,
        semantic: &str,
        mapping: Option<&EcuTargetMapping>,
        cache_valid: bool,
        fallback: FallbackPolicy,
    ) -> Result<RoutingDecision, RoutingError> {
        let request = prepare_read(semantic)
            .map_err(|_| RoutingError::UnsupportedSemantic(semantic.to_owned()))?;
        let role = self.rule(semantic).map(|rule| rule.role().clone());

        let Some(mapping) = mapping else {
            return Ok(fallback_decision(
                request,
                None,
                RoutingReason::NoTargetMapping,
                fallback,
            ));
        };
        let Some(role) = role else {
            return Ok(fallback_decision(
                request,
                Some(mapping),
                RoutingReason::NoRoleMapping,
                fallback,
            ));
        };
        if mapping.role().role() != &role {
            return Ok(fallback_decision(
                request,
                Some(mapping),
                RoutingReason::RoleMismatch,
                fallback,
            ));
        }
        if !cache_valid {
            return Ok(fallback_decision(
                request,
                Some(mapping),
                RoutingReason::StaleCache,
                fallback,
            ));
        }

        let targeted = mapping.targeted_request(request)?;
        Ok(RoutingDecision::Targeted {
            request: targeted,
            mapping: mapping.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EcuTargetMapping {
    role: crate::topology::RoleAssignment,
    target: RequestTargetEvidence,
    expected_responder: ResponderIdentity,
}

impl EcuTargetMapping {
    pub fn new(
        role: crate::topology::RoleAssignment,
        target: RequestTargetEvidence,
        expected_responder: ResponderIdentity,
    ) -> Self {
        Self {
            role,
            target,
            expected_responder,
        }
    }

    pub fn role(&self) -> &crate::topology::RoleAssignment {
        &self.role
    }

    pub fn target(&self) -> &RequestTargetEvidence {
        &self.target
    }

    pub fn expected_responder(&self) -> &ResponderIdentity {
        &self.expected_responder
    }

    fn targeted_request(&self, request: ReadRequest) -> Result<TargetedReadRequest, RoutingError> {
        let target_context = self.target.target().context();
        if target_context.protocol() != &Protocol::Obd2
            || target_context.addressing() != &AddressingContext::Physical
        {
            return Err(RoutingError::InvalidTargetEvidence(
                "target mapping must use physical OBD-II addressing".into(),
            ));
        }
        if self.expected_responder.context() != target_context {
            return Err(RoutingError::InvalidTargetEvidence(
                "target and expected responder contexts differ".into(),
            ));
        }
        let value = self.expected_responder.value().ok_or_else(|| {
            RoutingError::InvalidTargetEvidence(
                "target mapping must include an expected responder".into(),
            )
        })?;
        TargetedReadRequest::new(
            request,
            self.target.target().clone(),
            ElmResponderIdentity::ElmHeader(value.to_owned()),
        )
        .map_err(RoutingError::InvalidTargetEvidence)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum RoutingReason {
    NoRoleMapping,
    NoTargetMapping,
    RoleMismatch,
    StaleCache,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingDecision {
    Targeted {
        request: TargetedReadRequest,
        mapping: EcuTargetMapping,
    },
    FunctionalFallback {
        request: ReadRequest,
        mapping: Option<EcuTargetMapping>,
        reason: RoutingReason,
    },
    Ambiguous {
        request: ReadRequest,
        mapping: Option<EcuTargetMapping>,
        reason: RoutingReason,
    },
}

/// The closed request shape a polling plan may execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadRouting {
    Functional(ReadRequest),
    Targeted(TargetedReadRequest),
}

impl ReadRouting {
    pub fn request(&self) -> ReadRequest {
        match self {
            Self::Functional(request) => *request,
            Self::Targeted(request) => request.request(),
        }
    }

    pub fn from_decision(decision: RoutingDecision) -> Result<Self, RoutingError> {
        match decision {
            RoutingDecision::Targeted { request, .. } => Ok(Self::Targeted(request)),
            RoutingDecision::FunctionalFallback { request, .. } => Ok(Self::Functional(request)),
            RoutingDecision::Ambiguous { .. } => Err(RoutingError::AmbiguousFallback),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingError {
    UnsupportedSemantic(String),
    InvalidTargetEvidence(String),
    AmbiguousFallback,
}

impl std::fmt::Display for RoutingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSemantic(semantic) => {
                write!(
                    formatter,
                    "vehicle knowledge rejected unsupported semantic: {semantic}"
                )
            }
            Self::InvalidTargetEvidence(reason) => {
                write!(formatter, "invalid ECU target evidence: {reason}")
            }
            Self::AmbiguousFallback => {
                formatter.write_str("polling routing requires an explicit functional fallback")
            }
        }
    }
}

impl std::error::Error for RoutingError {}

fn fallback_decision(
    request: ReadRequest,
    mapping: Option<&EcuTargetMapping>,
    reason: RoutingReason,
    fallback: FallbackPolicy,
) -> RoutingDecision {
    let mapping = mapping.cloned();
    match fallback {
        FallbackPolicy::Functional => RoutingDecision::FunctionalFallback {
            request,
            mapping,
            reason,
        },
        FallbackPolicy::Ambiguous => RoutingDecision::Ambiguous {
            request,
            mapping,
            reason,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{
        Confidence, ProtocolContext, Provenance, RequestAddress, RequestTarget, RoleAssignment,
    };

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical)
    }

    fn mapping(role: EcuRole, target_header: &str, responder_header: &str) -> EcuTargetMapping {
        let context = context();
        EcuTargetMapping::new(
            RoleAssignment::new(role, provenance("vehicle profile")),
            RequestTargetEvidence::new(
                RequestTarget::concrete(
                    context.clone(),
                    RequestAddress::new("elm-header", target_header),
                ),
                provenance("validated topology cache"),
            ),
            ResponderIdentity::address(context, responder_header),
        )
    }

    #[test]
    fn evidenced_role_routes_to_target_and_retains_provenance() {
        let knowledge = VehicleKnowledge::generic_obd2();
        let mapping = mapping(EcuRole::Engine, "7E0", "7E8");
        let decision = knowledge
            .route(
                "engine.rpm",
                Some(&mapping),
                true,
                FallbackPolicy::Functional,
            )
            .unwrap();

        let RoutingDecision::Targeted { request, mapping } = decision else {
            panic!("evidenced engine mapping should target");
        };
        assert_eq!(
            request.request(),
            crate::prepare_read("engine.rpm").unwrap()
        );
        assert_eq!(request.target().address().unwrap().value(), "7E0");
        assert_eq!(request.expected_responder().as_str(), "7E8");
        assert_eq!(mapping.role().provenance().source(), "vehicle profile");
        assert_eq!(
            mapping.target().provenance().source(),
            "validated topology cache"
        );
    }

    #[test]
    fn missing_mapping_uses_the_declared_functional_fallback() {
        let decision = VehicleKnowledge::generic_obd2()
            .route("engine.rpm", None, true, FallbackPolicy::Functional)
            .unwrap();
        assert!(matches!(
            decision,
            RoutingDecision::FunctionalFallback {
                reason: RoutingReason::NoTargetMapping,
                ..
            }
        ));
    }

    #[test]
    fn stale_mapping_is_never_targeted_and_can_be_explicitly_ambiguous() {
        let mapping = mapping(EcuRole::Engine, "7E0", "7E8");
        let decision = VehicleKnowledge::generic_obd2()
            .route(
                "engine.rpm",
                Some(&mapping),
                false,
                FallbackPolicy::Ambiguous,
            )
            .unwrap();
        assert!(matches!(
            decision,
            RoutingDecision::Ambiguous {
                reason: RoutingReason::StaleCache,
                ..
            }
        ));
    }

    #[test]
    fn wrong_role_and_responder_are_rejected_without_guessing() {
        let wrong_role = mapping(EcuRole::Transmission, "7E0", "7E8");
        assert!(matches!(
            VehicleKnowledge::generic_obd2()
                .route(
                    "engine.rpm",
                    Some(&wrong_role),
                    true,
                    FallbackPolicy::Functional
                )
                .unwrap(),
            RoutingDecision::FunctionalFallback {
                reason: RoutingReason::RoleMismatch,
                ..
            }
        ));

        let unknown = mapping(EcuRole::Engine, "7E0", "7E8");
        let unknown = EcuTargetMapping::new(
            unknown.role().clone(),
            unknown.target().clone(),
            ResponderIdentity::unknown(context()),
        );
        assert!(matches!(
            VehicleKnowledge::generic_obd2().route(
                "engine.rpm",
                Some(&unknown),
                true,
                FallbackPolicy::Functional,
            ),
            Err(RoutingError::InvalidTargetEvidence(_))
        ));
    }

    #[test]
    fn routing_is_order_independent_and_cannot_add_mutating_requests() {
        let knowledge = VehicleKnowledge::new([
            SemanticRoleRule::new("engine.rpm", EcuRole::Engine),
            SemanticRoleRule::new("engine.coolant_temperature", EcuRole::Engine),
        ]);
        let first = mapping(EcuRole::Engine, "7E0", "7E8");
        let second = mapping(EcuRole::Engine, "7E0", "7E8");
        assert_eq!(
            knowledge.route("engine.rpm", Some(&first), true, FallbackPolicy::Functional),
            knowledge.route(
                "engine.rpm",
                Some(&second),
                true,
                FallbackPolicy::Functional
            )
        );
        assert!(knowledge
            .route("dtc.clear", Some(&first), true, FallbackPolicy::Functional)
            .is_err());
    }

    #[test]
    fn polling_routing_is_closed_and_requires_functional_fallback() {
        let mapping = mapping(EcuRole::Engine, "7E0", "7E8");
        let targeted = VehicleKnowledge::generic_obd2()
            .route(
                "engine.rpm",
                Some(&mapping),
                true,
                FallbackPolicy::Functional,
            )
            .unwrap();
        assert!(matches!(
            ReadRouting::from_decision(targeted),
            Ok(ReadRouting::Targeted(_))
        ));

        let fallback = VehicleKnowledge::generic_obd2()
            .route("engine.rpm", None, true, FallbackPolicy::Functional)
            .unwrap();
        assert!(matches!(
            ReadRouting::from_decision(fallback),
            Ok(ReadRouting::Functional(_))
        ));

        let ambiguous = VehicleKnowledge::generic_obd2()
            .route("engine.rpm", None, true, FallbackPolicy::Ambiguous)
            .unwrap();
        assert_eq!(
            ReadRouting::from_decision(ambiguous),
            Err(RoutingError::AmbiguousFallback)
        );
    }
}
