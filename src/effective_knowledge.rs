//! Pure Effective Vehicle Knowledge resolution.
//!
//! This module performs no adapter, session, transport or network I/O. It
//! combines already-normalized observed ECU identity facts with pinned
//! canonical Knowledge and preserves ambiguity instead of guessing.

use crate::knowledge_db::{
    FingerprintField, HardwareValidation, KnowledgeApplicability, KnowledgeCatalog,
    KnowledgeDefinition, KnowledgeProvenance,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedEcuFacts {
    ecu_id: String,
    facts: BTreeMap<FingerprintField, String>,
}

impl ObservedEcuFacts {
    pub fn new(ecu_id: impl Into<String>) -> Result<Self, String> {
        let ecu_id = ecu_id.into();
        if ecu_id.trim().is_empty() {
            return Err("observed ECU identity requires a non-empty local ECU id".into());
        }
        Ok(Self {
            ecu_id,
            facts: BTreeMap::new(),
        })
    }

    pub fn insert(
        &mut self,
        field: FingerprintField,
        value: impl Into<String>,
    ) -> Result<(), String> {
        let value = value.into();
        if value.is_empty() {
            return Err(format!(
                "normalized ECU fingerprint fact {} must not be empty",
                field.as_str()
            ));
        }
        self.facts.insert(field, value);
        Ok(())
    }

    pub fn with_fact(
        mut self,
        field: FingerprintField,
        value: impl Into<String>,
    ) -> Result<Self, String> {
        self.insert(field, value)?;
        Ok(self)
    }

    pub fn ecu_id(&self) -> &str {
        &self.ecu_id
    }

    pub fn fact(&self, field: FingerprintField) -> Option<&str> {
        self.facts.get(&field).map(String::as_str)
    }

    pub fn facts(&self) -> &BTreeMap<FingerprintField, String> {
        &self.facts
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicabilityMatch {
    Generic,
    ExactMatch,
    PartialCandidate,
    NoMatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticResolutionState {
    ResolvedGeneric,
    ResolvedSpecific,
    InsufficientIdentity,
    Ambiguous,
    NoMatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateResolution {
    definition_id: String,
    definition_version: u32,
    applicability_match: ApplicabilityMatch,
    specificity: usize,
    applicability_provenance: KnowledgeProvenance,
    definition_provenance: KnowledgeProvenance,
    hardware_validation: HardwareValidation,
}

impl CandidateResolution {
    pub fn definition_id(&self) -> &str {
        &self.definition_id
    }

    pub const fn definition_version(&self) -> u32 {
        self.definition_version
    }

    pub const fn applicability_match(&self) -> ApplicabilityMatch {
        self.applicability_match
    }

    pub const fn specificity(&self) -> usize {
        self.specificity
    }

    pub fn applicability_provenance(&self) -> &KnowledgeProvenance {
        &self.applicability_provenance
    }

    pub fn definition_provenance(&self) -> &KnowledgeProvenance {
        &self.definition_provenance
    }

    pub fn hardware_validation(&self) -> &HardwareValidation {
        &self.hardware_validation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticResolution {
    semantic: String,
    state: SemanticResolutionState,
    selected_definition_id: Option<String>,
    candidates: Vec<CandidateResolution>,
}

impl SemanticResolution {
    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn state(&self) -> SemanticResolutionState {
        self.state
    }

    pub fn selected_definition_id(&self) -> Option<&str> {
        self.selected_definition_id.as_deref()
    }

    pub fn selected_definition<'a>(
        &self,
        catalog: &'a KnowledgeCatalog,
    ) -> Option<&'a KnowledgeDefinition> {
        self.selected_definition_id()
            .and_then(|id| catalog.definition(id))
    }

    pub fn candidates(&self) -> &[CandidateResolution] {
        &self.candidates
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveEcuKnowledge {
    ecu_id: String,
    resolutions: BTreeMap<String, SemanticResolution>,
}

impl EffectiveEcuKnowledge {
    pub fn ecu_id(&self) -> &str {
        &self.ecu_id
    }

    pub fn semantic(&self, semantic: &str) -> Option<&SemanticResolution> {
        self.resolutions.get(semantic)
    }

    pub fn semantics(&self) -> impl ExactSizeIterator<Item = &SemanticResolution> {
        self.resolutions.values()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveVehicleKnowledge {
    knowledge_repository: String,
    knowledge_revision: String,
    knowledge_schema_version: u32,
    ecus: BTreeMap<String, EffectiveEcuKnowledge>,
}

impl EffectiveVehicleKnowledge {
    pub fn resolve(
        catalog: &KnowledgeCatalog,
        ecus: impl IntoIterator<Item = ObservedEcuFacts>,
    ) -> Result<Self, String> {
        let mut resolved_ecus = BTreeMap::new();
        for ecu in ecus {
            if resolved_ecus.contains_key(ecu.ecu_id()) {
                return Err(format!("duplicate observed ECU id {:?}", ecu.ecu_id()));
            }
            let mut resolutions = BTreeMap::new();
            for semantic in catalog.semantic_ids() {
                let definitions = catalog.definitions_for_semantic(semantic);
                resolutions.insert(
                    semantic.to_owned(),
                    resolve_semantic(semantic, definitions, &ecu),
                );
            }
            resolved_ecus.insert(
                ecu.ecu_id().to_owned(),
                EffectiveEcuKnowledge {
                    ecu_id: ecu.ecu_id().to_owned(),
                    resolutions,
                },
            );
        }
        Ok(Self {
            knowledge_repository: catalog.pin().repository().to_owned(),
            knowledge_revision: catalog.pin().revision().to_owned(),
            knowledge_schema_version: catalog.pin().schema_version(),
            ecus: resolved_ecus,
        })
    }

    pub fn knowledge_repository(&self) -> &str {
        &self.knowledge_repository
    }

    pub fn knowledge_revision(&self) -> &str {
        &self.knowledge_revision
    }

    pub const fn knowledge_schema_version(&self) -> u32 {
        self.knowledge_schema_version
    }

    pub fn ecu(&self, ecu_id: &str) -> Option<&EffectiveEcuKnowledge> {
        self.ecus.get(ecu_id)
    }

    pub fn ecus(&self) -> impl ExactSizeIterator<Item = &EffectiveEcuKnowledge> {
        self.ecus.values()
    }
}

fn resolve_semantic(
    semantic: &str,
    mut definitions: Vec<&KnowledgeDefinition>,
    ecu: &ObservedEcuFacts,
) -> SemanticResolution {
    definitions.sort_by_key(|definition| definition.id());
    let candidates = definitions
        .into_iter()
        .map(|definition| evaluate_candidate(definition, ecu))
        .collect::<Vec<_>>();

    let exact = candidates
        .iter()
        .filter(|candidate| candidate.applicability_match == ApplicabilityMatch::ExactMatch)
        .collect::<Vec<_>>();

    let (state, selected_definition_id) = if !exact.is_empty() {
        let maximum = exact
            .iter()
            .map(|candidate| candidate.specificity)
            .max()
            .expect("non-empty exact candidates have maximum specificity");
        let finalists = exact
            .into_iter()
            .filter(|candidate| candidate.specificity == maximum)
            .collect::<Vec<_>>();
        if finalists.len() == 1 {
            (
                SemanticResolutionState::ResolvedSpecific,
                Some(finalists[0].definition_id.clone()),
            )
        } else {
            (SemanticResolutionState::Ambiguous, None)
        }
    } else if candidates
        .iter()
        .any(|candidate| candidate.applicability_match == ApplicabilityMatch::PartialCandidate)
    {
        (SemanticResolutionState::InsufficientIdentity, None)
    } else {
        let generic = candidates
            .iter()
            .filter(|candidate| candidate.applicability_match == ApplicabilityMatch::Generic)
            .collect::<Vec<_>>();
        match generic.as_slice() {
            [candidate] => (
                SemanticResolutionState::ResolvedGeneric,
                Some(candidate.definition_id.clone()),
            ),
            [] => (SemanticResolutionState::NoMatch, None),
            _ => (SemanticResolutionState::Ambiguous, None),
        }
    };

    SemanticResolution {
        semantic: semantic.to_owned(),
        state,
        selected_definition_id,
        candidates,
    }
}

fn evaluate_candidate(
    definition: &KnowledgeDefinition,
    ecu: &ObservedEcuFacts,
) -> CandidateResolution {
    let (applicability_match, specificity) = match definition.applicability() {
        KnowledgeApplicability::Generic { .. } => (ApplicabilityMatch::Generic, 0),
        KnowledgeApplicability::EcuFingerprint { predicates, .. } => {
            let mut missing = false;
            let mut conflict = false;
            for predicate in predicates {
                match ecu.fact(predicate.field()) {
                    Some(value) if value == predicate.equals() => {}
                    Some(_) => conflict = true,
                    None => missing = true,
                }
            }
            let applicability_match = if conflict {
                ApplicabilityMatch::NoMatch
            } else if missing {
                ApplicabilityMatch::PartialCandidate
            } else {
                ApplicabilityMatch::ExactMatch
            };
            (applicability_match, predicates.len())
        }
    };

    CandidateResolution {
        definition_id: definition.id().to_owned(),
        definition_version: definition.version(),
        applicability_match,
        specificity,
        applicability_provenance: definition.applicability().provenance().clone(),
        definition_provenance: definition.provenance().clone(),
        hardware_validation: definition.hardware_validation().clone(),
    }
}
