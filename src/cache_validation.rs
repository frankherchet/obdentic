//! Pure validation of a typed cache snapshot against one observation.

use crate::vehicle_cache::{ValidationSignature, VehicleCache, VehicleCacheSnapshot};
use std::collections::BTreeSet;

/// The outcome of validating the current cache snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheValidation {
    /// The observed snapshot exactly matches the cache.
    Validated,
    /// A current snapshot item was not observed.
    StaleMissingExpected(Vec<String>),
    /// A current snapshot item was observed that is not in the cache.
    StaleUnexpected(Vec<String>),
    /// Observation failed before the snapshot could be compared.
    TransportError(String),
}

/// Input accepted by [`validate_cache`]. The `Vec<String>` implementation is
/// retained only so old callers fail closed; historical text is not a
/// validation source.
pub trait ValidationInput {
    fn into_validation_signature(self) -> Result<ValidationSignature, String>;
}

impl ValidationInput for ValidationSignature {
    fn into_validation_signature(self) -> Result<ValidationSignature, String> {
        Ok(self)
    }
}

impl ValidationInput for VehicleCacheSnapshot {
    fn into_validation_signature(self) -> Result<ValidationSignature, String> {
        Ok(self.validation_signature())
    }
}

impl ValidationInput for &VehicleCacheSnapshot {
    fn into_validation_signature(self) -> Result<ValidationSignature, String> {
        Ok(self.validation_signature())
    }
}

impl ValidationInput for Vec<String> {
    fn into_validation_signature(self) -> Result<ValidationSignature, String> {
        Err("historical textual evidence is not a cache validation signature".into())
    }
}

/// Compare only the current typed snapshot. Timestamps, local identity and
/// historical evidence are intentionally excluded from this comparison.
pub fn validate_cache<T: ValidationInput>(
    cache: &VehicleCache,
    observed_result: Result<T, String>,
) -> CacheValidation {
    let observed = match observed_result.and_then(ValidationInput::into_validation_signature) {
        Ok(observed) => observed,
        Err(error) => return CacheValidation::TransportError(error),
    };
    compare_signatures(&cache.snapshot().validation_signature(), &observed)
}

pub fn validate_snapshot(
    cache: &VehicleCache,
    observed_result: Result<VehicleCacheSnapshot, String>,
) -> CacheValidation {
    validate_cache(cache, observed_result)
}

pub fn validate_signature(
    cache: &VehicleCache,
    observed_result: Result<ValidationSignature, String>,
) -> CacheValidation {
    validate_cache(cache, observed_result)
}

fn compare_signatures(
    expected: &ValidationSignature,
    observed: &ValidationSignature,
) -> CacheValidation {
    let expected = expected.entries().into_iter().collect::<BTreeSet<_>>();
    let observed = observed.entries().into_iter().collect::<BTreeSet<_>>();
    let missing = expected.difference(&observed).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        return CacheValidation::StaleMissingExpected(missing);
    }
    let unexpected = observed.difference(&expected).cloned().collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return CacheValidation::StaleUnexpected(unexpected);
    }
    CacheValidation::Validated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{AddressingContext, Confidence, Protocol, ProtocolContext, Provenance};
    use crate::vehicle_cache::TopologyObservation;

    fn provenance() -> Provenance {
        Provenance::new("capture", Confidence::High).unwrap()
    }

    fn snapshot(bitmap: u8) -> VehicleCacheSnapshot {
        VehicleCacheSnapshot::new(
            [TopologyObservation::new(
                ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional),
                crate::topology::ResponderIdentity::opaque(
                    ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional),
                    "7E8",
                ),
                Some(vec![0x41, 0x00, bitmap, 0, 0, 0]),
                None,
                provenance(),
            )],
            [],
            [],
        )
    }

    fn cache(snapshot: VehicleCacheSnapshot, history: &[&str]) -> VehicleCache {
        VehicleCache::with_snapshot(
            "vehicle-key",
            1,
            2,
            snapshot,
            history.iter().map(|value| (*value).into()).collect(),
        )
    }

    #[test]
    fn validates_matching_current_snapshot() {
        let snapshot = snapshot(1);
        assert_eq!(
            validate_snapshot(&cache(snapshot.clone(), &["old"]), Ok(snapshot)),
            CacheValidation::Validated
        );
    }

    #[test]
    fn history_is_not_canonical_validation_input() {
        assert_eq!(
            validate_cache(
                &cache(snapshot(1), &["ecu-a", "ecu-b"]),
                Ok(vec!["ecu-a".into(), "ecu-b".into()]),
            ),
            CacheValidation::TransportError(
                "historical textual evidence is not a cache validation signature".into()
            )
        );
    }

    #[test]
    fn reports_current_snapshot_mismatch_deterministically() {
        let result = validate_snapshot(&cache(snapshot(1), &[]), Ok(snapshot(2)));
        assert!(matches!(result, CacheValidation::StaleMissingExpected(_)));
    }

    #[test]
    fn preserves_transport_errors_as_transport_errors() {
        assert_eq!(
            validate_snapshot(&cache(snapshot(1), &[]), Err("timeout".into())),
            CacheValidation::TransportError("timeout".into())
        );
    }

    #[test]
    fn empty_legacy_snapshot_fails_closed_against_real_0100_snapshot() {
        assert!(matches!(
            validate_snapshot(
                &cache(VehicleCacheSnapshot::default(), &["old"]),
                Ok(snapshot(1))
            ),
            CacheValidation::StaleUnexpected(_)
        ));
    }
}
