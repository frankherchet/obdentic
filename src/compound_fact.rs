//! Transport-neutral compound diagnostic observations.
//!
//! One response may deterministically produce several semantic facts.  The
//! facts share one source, timestamp, and evidence identity so consumers do
//! not mistake derived values for additional vehicle requests.

use std::{collections::BTreeSet, fmt};

/// The identity shared by every fact decoded from one observation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ObservationIdentity {
    source: String,
    evidence_id: String,
    timestamp_us: u128,
}

impl ObservationIdentity {
    /// Create an identity for one source-scoped observation.
    pub fn new(
        source: impl Into<String>,
        evidence_id: impl Into<String>,
        timestamp_us: u128,
    ) -> Result<Self, ObservationError> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(ObservationError::EmptySource);
        }
        let evidence_id = evidence_id.into();
        if evidence_id.trim().is_empty() {
            return Err(ObservationError::EmptyEvidenceId);
        }
        Ok(Self {
            source,
            evidence_id,
            timestamp_us,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn evidence_id(&self) -> &str {
        &self.evidence_id
    }

    pub const fn timestamp_us(&self) -> u128 {
        self.timestamp_us
    }
}

/// A typed, deterministic value associated with one semantic identifier.
#[derive(Clone, Debug, PartialEq)]
pub enum FactValue {
    Scalar { value: f64, unit: String },
    BitStatus { bit: u8, asserted: bool },
}

/// One semantic fact decoded from an observation.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticFact {
    semantic: String,
    value: FactValue,
}

impl SemanticFact {
    pub fn scalar(
        semantic: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
    ) -> Result<Self, ObservationError> {
        let semantic = validated_semantic(semantic.into())?;
        if !value.is_finite() {
            return Err(ObservationError::NonFiniteScalar { semantic });
        }
        let unit = unit.into();
        if unit.trim().is_empty() {
            return Err(ObservationError::EmptyUnit { semantic });
        }
        Ok(Self {
            semantic,
            value: FactValue::Scalar { value, unit },
        })
    }

    pub fn bit_status(
        semantic: impl Into<String>,
        bit: u8,
        asserted: bool,
    ) -> Result<Self, ObservationError> {
        Ok(Self {
            semantic: validated_semantic(semantic.into())?,
            value: FactValue::BitStatus { bit, asserted },
        })
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn value(&self) -> &FactValue {
        &self.value
    }
}

/// An immutable set of facts sharing one observation identity.
#[derive(Clone, Debug, PartialEq)]
pub struct CompoundObservation {
    identity: ObservationIdentity,
    facts: Vec<SemanticFact>,
}

impl CompoundObservation {
    /// Construct an observation without issuing or describing a transport
    /// request. Empty fact sets are valid for decodes with no applicable facts.
    pub fn new(
        identity: ObservationIdentity,
        facts: impl IntoIterator<Item = SemanticFact>,
    ) -> Result<Self, ObservationError> {
        let facts = facts.into_iter().collect::<Vec<_>>();
        let mut semantics = BTreeSet::new();
        for fact in &facts {
            if !semantics.insert(fact.semantic.as_str()) {
                return Err(ObservationError::DuplicateSemantic {
                    semantic: fact.semantic.clone(),
                });
            }
        }
        Ok(Self { identity, facts })
    }

    pub fn identity(&self) -> &ObservationIdentity {
        &self.identity
    }

    pub fn source(&self) -> &str {
        self.identity.source()
    }

    pub fn evidence_id(&self) -> &str {
        self.identity.evidence_id()
    }

    pub const fn timestamp_us(&self) -> u128 {
        self.identity.timestamp_us()
    }

    pub fn facts(&self) -> &[SemanticFact] {
        &self.facts
    }

    pub fn fact(&self, semantic: &str) -> Option<&SemanticFact> {
        self.facts.iter().find(|fact| fact.semantic == semantic)
    }
}

fn validated_semantic(semantic: String) -> Result<String, ObservationError> {
    if semantic.trim().is_empty() {
        Err(ObservationError::EmptySemantic)
    } else {
        Ok(semantic)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationError {
    EmptySource,
    EmptyEvidenceId,
    EmptySemantic,
    EmptyUnit { semantic: String },
    DuplicateSemantic { semantic: String },
    NonFiniteScalar { semantic: String },
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySource => formatter.write_str("observation source must not be empty"),
            Self::EmptyEvidenceId => {
                formatter.write_str("observation evidence identity must not be empty")
            }
            Self::EmptySemantic => {
                formatter.write_str("fact semantic identifier must not be empty")
            }
            Self::EmptyUnit { semantic } => {
                write!(formatter, "scalar fact {semantic} must have a unit")
            }
            Self::DuplicateSemantic { semantic } => {
                write!(formatter, "duplicate fact semantic identifier: {semantic}")
            }
            Self::NonFiniteScalar { semantic } => {
                write!(formatter, "scalar fact {semantic} must be finite")
            }
        }
    }
}

impl std::error::Error for ObservationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ObservationIdentity {
        ObservationIdentity::new("capture", "evidence-42", 1_234_567).unwrap()
    }

    #[test]
    fn compound_facts_share_one_observation_identity() {
        let observation = CompoundObservation::new(
            identity(),
            [
                SemanticFact::scalar("sensor.voltage", 1.2, "V").unwrap(),
                SemanticFact::bit_status("sensor.present", 3, true).unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(observation.source(), "capture");
        assert_eq!(observation.evidence_id(), "evidence-42");
        assert_eq!(observation.timestamp_us(), 1_234_567);
        assert_eq!(observation.facts().len(), 2);
        assert_eq!(
            observation.fact("sensor.voltage"),
            Some(&observation.facts()[0])
        );
        assert_eq!(
            observation.fact("sensor.present"),
            Some(&observation.facts()[1])
        );
    }

    #[test]
    fn facts_do_not_duplicate_shared_traffic_identity() {
        let observation = CompoundObservation::new(
            identity(),
            [
                SemanticFact::scalar("value.a", 10.0, "unit").unwrap(),
                SemanticFact::scalar("value.b", 20.0, "unit").unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(observation.facts().len(), 2);
        assert!(observation
            .facts()
            .iter()
            .all(|fact| fact.semantic() != observation.evidence_id()));
        assert_eq!(observation.identity(), &identity());
    }

    #[test]
    fn rejects_invalid_and_duplicate_data_but_allows_no_applicable_facts() {
        assert_eq!(
            ObservationIdentity::new(" ", "evidence", 0),
            Err(ObservationError::EmptySource)
        );
        assert_eq!(
            ObservationIdentity::new("capture", "", 0),
            Err(ObservationError::EmptyEvidenceId)
        );
        assert_eq!(
            SemanticFact::scalar(" ", 1.0, "unit"),
            Err(ObservationError::EmptySemantic)
        );
        assert_eq!(
            SemanticFact::scalar("value", f64::NAN, "unit"),
            Err(ObservationError::NonFiniteScalar {
                semantic: "value".into()
            })
        );
        assert_eq!(
            SemanticFact::scalar("value", 1.0, " "),
            Err(ObservationError::EmptyUnit {
                semantic: "value".into()
            })
        );
        let duplicate = SemanticFact::scalar("value", 1.0, "unit").unwrap();
        assert_eq!(
            CompoundObservation::new(identity(), [duplicate.clone(), duplicate]),
            Err(ObservationError::DuplicateSemantic {
                semantic: "value".into()
            })
        );
        assert!(CompoundObservation::new(identity(), [])
            .unwrap()
            .facts()
            .is_empty());
    }
}
