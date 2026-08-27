//! Pure validation of cached opaque evidence against one observation.

use crate::vehicle_cache::VehicleCache;
use std::collections::BTreeSet;

/// The outcome of validating cached evidence against a read-only observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheValidation {
    /// The observed evidence exactly matches the cache.
    Validated,
    /// Cached evidence was not observed.
    StaleMissingExpected(Vec<String>),
    /// Evidence was observed that is not present in the cache.
    StaleUnexpected(Vec<String>),
    /// Observation failed before topology could be compared.
    TransportError(String),
}

/// Compare opaque evidence strings without interpreting their contents.
///
/// A transport failure is kept distinct from topology drift. If both kinds of
/// topology difference occur, missing expected evidence takes precedence so a
/// caller cannot accidentally trust a partially matching cache.
pub fn validate_cache(
    cache: &VehicleCache,
    observed_result: Result<Vec<String>, String>,
) -> CacheValidation {
    let observed = match observed_result {
        Ok(observed) => observed,
        Err(error) => return CacheValidation::TransportError(error),
    };

    let expected = cache.evidence().iter().collect::<BTreeSet<_>>();
    let observed = observed.iter().collect::<BTreeSet<_>>();
    let missing = expected
        .difference(&observed)
        .map(|evidence| (*evidence).clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return CacheValidation::StaleMissingExpected(missing);
    }

    let unexpected = observed
        .difference(&expected)
        .map(|evidence| (*evidence).clone())
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return CacheValidation::StaleUnexpected(unexpected);
    }

    CacheValidation::Validated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(evidence: &[&str]) -> VehicleCache {
        VehicleCache::new(
            "vehicle-key",
            1,
            2,
            evidence.iter().map(|value| (*value).into()).collect(),
        )
    }

    #[test]
    fn validates_matching_evidence() {
        assert_eq!(
            validate_cache(
                &cache(&["ecu-a", "ecu-b"]),
                Ok(vec!["ecu-b".into(), "ecu-a".into()])
            ),
            CacheValidation::Validated
        );
    }

    #[test]
    fn reports_missing_expected_evidence_deterministically() {
        assert_eq!(
            validate_cache(&cache(&["ecu-b", "ecu-a"]), Ok(vec!["ecu-a".into()])),
            CacheValidation::StaleMissingExpected(vec!["ecu-b".into()])
        );
    }

    #[test]
    fn reports_unexpected_evidence_deterministically() {
        assert_eq!(
            validate_cache(&cache(&["ecu-a"]), Ok(vec!["ecu-c".into(), "ecu-a".into()])),
            CacheValidation::StaleUnexpected(vec!["ecu-c".into()])
        );
    }

    #[test]
    fn preserves_transport_errors_as_transport_errors() {
        assert_eq!(
            validate_cache(&cache(&["ecu-a"]), Err("timeout".into())),
            CacheValidation::TransportError("timeout".into())
        );
    }
}
