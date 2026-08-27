use crate::capability::HardwareCapability;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::Duration,
};

/// A consumer's desired freshness for one semantic observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationRequest {
    source: String,
    semantic: String,
    desired_interval: Duration,
}

impl ObservationRequest {
    pub fn new(
        source: impl Into<String>,
        semantic: impl Into<String>,
        desired_interval: Duration,
    ) -> Result<Self, PolicyError> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(PolicyError::SourceMustNotBeEmpty);
        }
        let semantic = semantic.into();
        if semantic.trim().is_empty() {
            return Err(PolicyError::SemanticMustNotBeEmpty);
        }
        if desired_interval.is_zero() {
            return Err(PolicyError::DesiredIntervalMustBePositive);
        }
        Ok(Self {
            source,
            semantic,
            desired_interval,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub const fn desired_interval(&self) -> Duration {
        self.desired_interval
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Accepted,
    RateReduced,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReason {
    WithinBudget,
    SessionRequestBudget,
    SignalUnsupported,
}

/// One merged semantic subscription in a polling plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    semantic: String,
    sources: Vec<String>,
    requested_interval: Duration,
    effective_interval: Option<Duration>,
    status: PlanStatus,
    reason: PlanReason,
}

impl PlanEntry {
    pub fn semantic(&self) -> &str {
        &self.semantic
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    pub const fn requested_interval(&self) -> Duration {
        self.requested_interval
    }

    pub const fn effective_interval(&self) -> Option<Duration> {
        self.effective_interval
    }

    pub const fn status(&self) -> PlanStatus {
        self.status
    }

    pub const fn reason(&self) -> PlanReason {
        self.reason
    }

    pub const fn is_scheduled(&self) -> bool {
        !matches!(self.status, PlanStatus::Unsupported)
    }
}

/// The deterministic output consumed by a concrete scheduler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PollingPlan {
    entries: Vec<PlanEntry>,
}

impl PollingPlan {
    pub fn entries(&self) -> &[PlanEntry] {
        &self.entries
    }

    pub fn scheduled_entries(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries.iter().filter(|entry| entry.is_scheduled())
    }

    pub fn effective_request_rate_per_second(&self) -> f64 {
        self.scheduled_entries()
            .filter_map(PlanEntry::effective_interval)
            .map(|interval| 1.0 / interval.as_secs_f64())
            .sum()
    }
}

/// Deterministic policy that translates semantic demand into a bounded plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscriptionPolicy {
    capability: HardwareCapability,
}

const RATE_PRECISION: u128 = 1_000_000;

impl SubscriptionPolicy {
    pub const fn new(capability: HardwareCapability) -> Self {
        Self { capability }
    }

    pub const fn capability(&self) -> HardwareCapability {
        self.capability
    }

    /// Merge duplicate semantics, omit unsupported semantics, and apply a
    /// common fair rate cap when the merged demand exceeds the session budget.
    /// Under overload every supported semantic receives at most an equal
    /// `budget / signal_count` share; slower requested rates remain slower.
    pub fn plan<Requests, RequestItem, Supported, Name>(
        &self,
        requests: Requests,
        supported_semantics: Supported,
    ) -> PollingPlan
    where
        Requests: IntoIterator<Item = RequestItem>,
        RequestItem: Borrow<ObservationRequest>,
        Supported: IntoIterator<Item = Name>,
        Name: AsRef<str>,
    {
        let supported: BTreeSet<String> = supported_semantics
            .into_iter()
            .map(|semantic| semantic.as_ref().to_owned())
            .collect();
        let mut merged = BTreeMap::<String, (BTreeSet<String>, Duration)>::new();
        for request in requests {
            let request = request.borrow();
            let (sources, requested_interval) = merged
                .entry(request.semantic.clone())
                .or_insert_with(|| (BTreeSet::new(), request.desired_interval));
            sources.insert(request.source.clone());
            *requested_interval = (*requested_interval).min(request.desired_interval);
        }

        let mut supported_entries = Vec::new();
        let mut entries = Vec::new();
        for (semantic, (sources, requested_interval)) in merged {
            let sources = sources.into_iter().collect();
            if supported.contains(&semantic) {
                supported_entries.push((semantic, sources, requested_interval));
            } else {
                entries.push(PlanEntry {
                    semantic,
                    sources,
                    requested_interval,
                    effective_interval: None,
                    status: PlanStatus::Unsupported,
                    reason: PlanReason::SignalUnsupported,
                });
            }
        }

        let overloaded = requested_rate_exceeds_budget(
            &supported_entries,
            self.capability.request_budget_per_second(),
        );
        let fair_interval = overloaded.then(|| {
            fair_interval(
                supported_entries.len(),
                self.capability.request_budget_per_second(),
            )
        });
        for (semantic, sources, requested_interval) in supported_entries {
            let effective_interval = fair_interval
                .map(|fair| requested_interval.max(fair))
                .unwrap_or(requested_interval);
            let reduced = effective_interval > requested_interval;
            entries.push(PlanEntry {
                semantic,
                sources,
                requested_interval,
                effective_interval: Some(effective_interval),
                status: if reduced {
                    PlanStatus::RateReduced
                } else {
                    PlanStatus::Accepted
                },
                reason: if reduced {
                    PlanReason::SessionRequestBudget
                } else {
                    PlanReason::WithinBudget
                },
            });
        }

        entries.sort_by(|left, right| left.semantic.cmp(&right.semantic));
        PollingPlan { entries }
    }
}

fn requested_rate_exceeds_budget(
    entries: &[(String, Vec<String>, Duration)],
    request_budget_per_second: u32,
) -> bool {
    let budget = u128::from(request_budget_per_second).saturating_mul(RATE_PRECISION);
    let requested = entries.iter().fold(0_u128, |total, (_, _, interval)| {
        let numerator = 1_000_000_000_u128.saturating_mul(RATE_PRECISION);
        let rounded_up = numerator.div_ceil(interval.as_nanos());
        total.saturating_add(rounded_up)
    });
    requested > budget
}

fn fair_interval(entry_count: usize, request_budget_per_second: u32) -> Duration {
    let numerator = (entry_count as u128) * 1_000_000_000;
    let denominator = u128::from(request_budget_per_second);
    let rounded_up = numerator.div_ceil(denominator);
    Duration::from_nanos(u64::try_from(rounded_up).unwrap_or(u64::MAX))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    SourceMustNotBeEmpty,
    SemanticMustNotBeEmpty,
    DesiredIntervalMustBePositive,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SourceMustNotBeEmpty => "observation source must not be empty",
            Self::SemanticMustNotBeEmpty => "observation semantic must not be empty",
            Self::DesiredIntervalMustBePositive => {
                "observation desired interval must be greater than zero"
            }
        })
    }
}

impl std::error::Error for PolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(source: &str, semantic: &str, milliseconds: u64) -> ObservationRequest {
        ObservationRequest::new(source, semantic, Duration::from_millis(milliseconds)).unwrap()
    }

    fn policy(requests_per_second: u32) -> SubscriptionPolicy {
        SubscriptionPolicy::new(
            HardwareCapability::new(
                requests_per_second,
                Duration::from_millis(250),
                crate::capability::CapabilityProvenance::BuiltInDefault,
            )
            .unwrap(),
        )
    }

    #[test]
    fn request_under_budget_is_accepted_unchanged() {
        let plan = policy(4).plan([request("tui", "engine.rpm", 500)], ["engine.rpm"]);

        assert_eq!(plan.entries().len(), 1);
        let entry = &plan.entries()[0];
        assert_eq!(entry.semantic(), "engine.rpm");
        assert_eq!(entry.requested_interval(), Duration::from_millis(500));
        assert_eq!(entry.effective_interval(), Some(Duration::from_millis(500)));
        assert_eq!(entry.status(), PlanStatus::Accepted);
        assert_eq!(entry.reason(), PlanReason::WithinBudget);
    }

    #[test]
    fn duplicate_semantics_merge_to_the_strictest_interval() {
        let plan = policy(20).plan(
            [
                request("slow-consumer", "engine.rpm", 1_000),
                request("fast-consumer", "engine.rpm", 250),
            ],
            ["engine.rpm"],
        );

        assert_eq!(plan.entries().len(), 1);
        assert_eq!(
            plan.entries()[0].requested_interval(),
            Duration::from_millis(250)
        );
        assert_eq!(
            plan.entries()[0].sources(),
            &["fast-consumer".to_owned(), "slow-consumer".to_owned()]
        );
    }

    #[test]
    fn unsupported_semantics_are_visible_but_not_scheduled() {
        let plan = policy(4).plan([request("tui", "vehicle.unknown", 500)], ["engine.rpm"]);

        let entry = &plan.entries()[0];
        assert_eq!(entry.status(), PlanStatus::Unsupported);
        assert_eq!(entry.reason(), PlanReason::SignalUnsupported);
        assert_eq!(entry.effective_interval(), None);
        assert_eq!(plan.scheduled_entries().count(), 0);
    }

    #[test]
    fn overload_uses_a_common_fair_rate_cap() {
        let plan = policy(4).plan(
            [
                request("tui", "engine.rpm", 100),
                request("tui", "engine.maf", 100),
                request("tui", "engine.load", 100),
            ],
            ["engine.load", "engine.maf", "engine.rpm"],
        );

        for entry in plan.entries() {
            assert_eq!(entry.effective_interval(), Some(Duration::from_millis(750)));
            assert_eq!(entry.status(), PlanStatus::RateReduced);
            assert_eq!(entry.reason(), PlanReason::SessionRequestBudget);
        }
        assert!(plan.effective_request_rate_per_second() <= 4.0);
    }

    #[test]
    fn input_order_does_not_change_the_plan() {
        let first = policy(4).plan(
            [
                request("b", "engine.maf", 100),
                request("a", "engine.rpm", 100),
            ],
            ["engine.rpm", "engine.maf"],
        );
        let second = policy(4).plan(
            [
                request("a", "engine.rpm", 100),
                request("b", "engine.maf", 100),
            ],
            ["engine.maf", "engine.rpm"],
        );

        assert_eq!(first, second);
    }

    #[test]
    fn rejects_invalid_observation_requests() {
        assert_eq!(
            ObservationRequest::new("", "engine.rpm", Duration::from_millis(100)),
            Err(PolicyError::SourceMustNotBeEmpty)
        );
        assert_eq!(
            ObservationRequest::new("tui", "", Duration::from_millis(100)),
            Err(PolicyError::SemanticMustNotBeEmpty)
        );
        assert_eq!(
            ObservationRequest::new("tui", "engine.rpm", Duration::ZERO),
            Err(PolicyError::DesiredIntervalMustBePositive)
        );
    }

    #[test]
    fn effective_rate_respects_budget_with_mixed_intervals() {
        let plan = policy(5).plan(
            [
                request("tui", "engine.rpm", 50),
                request("tui", "engine.maf", 500),
                request("tui", "engine.load", 2_000),
                request("tui", "vehicle.speed", 10_000),
            ],
            ["engine.load", "engine.maf", "engine.rpm", "vehicle.speed"],
        );

        assert!(plan.effective_request_rate_per_second() <= 5.0);
    }
}
