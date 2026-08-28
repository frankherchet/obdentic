use crate::{
    subscription_policy::{ObservationRequest, PollingPlan, SubscriptionPolicy},
    tui::{DashboardLayout, View},
};
use std::time::Duration;

const LAYOUT_SOURCE_PREFIX: &str = "layout:";

/// Presentation freshness intent kept outside the persisted layout format.
///
/// These intervals are requests, not guarantees. `SubscriptionPolicy` remains
/// the sole authority for the effective polling plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutFreshnessPolicy {
    value: Duration,
    sparkline: Duration,
    time_series: Duration,
    compare: Duration,
}

impl LayoutFreshnessPolicy {
    pub fn new(
        value: Duration,
        sparkline: Duration,
        time_series: Duration,
        compare: Duration,
    ) -> Result<Self, String> {
        if [value, sparkline, time_series, compare]
            .into_iter()
            .any(|interval| interval.is_zero())
        {
            return Err("layout freshness intervals must be greater than zero".into());
        }
        Ok(Self {
            value,
            sparkline,
            time_series,
            compare,
        })
    }

    pub const fn desired_interval(self, view: View) -> Duration {
        match view {
            View::Value => self.value,
            View::Sparkline => self.sparkline,
            View::TimeSeries => self.time_series,
            View::Compare => self.compare,
        }
    }
}

impl Default for LayoutFreshnessPolicy {
    fn default() -> Self {
        Self {
            value: Duration::from_secs(1),
            sparkline: Duration::from_millis(200),
            time_series: Duration::from_millis(500),
            compare: Duration::from_millis(500),
        }
    }
}

/// Side-effect-free semantic demand derived from a presentation layout.
///
/// This function performs no vehicle I/O and cannot construct protocol bytes.
/// Unknown semantic names remain semantic demand only; the downstream
/// `SubscriptionPolicy` marks them unsupported unless the active vehicle
/// catalog explicitly advertises them.
pub fn observation_requests(
    layout: &DashboardLayout,
    freshness: LayoutFreshnessPolicy,
) -> Result<Vec<ObservationRequest>, String> {
    if layout.name.trim().is_empty() {
        return Err("layout observation source requires a non-empty layout name".into());
    }
    let source = format!("{LAYOUT_SOURCE_PREFIX}{}", layout.name);
    let mut requests = Vec::new();
    for panel in &layout.panels {
        let desired_interval = freshness.desired_interval(panel.view);
        for semantic in &panel.signals {
            requests.push(
                ObservationRequest::new(&source, semantic, desired_interval)
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    Ok(requests)
}

/// Derive semantic layout demand and pass it through the existing
/// budget/support policy. The returned plan exposes requested and effective
/// intervals and each signal's status and reason.
pub fn polling_plan<Supported, Name>(
    layout: &DashboardLayout,
    freshness: LayoutFreshnessPolicy,
    policy: SubscriptionPolicy,
    supported_semantics: Supported,
) -> Result<PollingPlan, String>
where
    Supported: IntoIterator<Item = Name>,
    Name: AsRef<str>,
{
    let requests = observation_requests(layout, freshness)?;
    Ok(policy.plan(&requests, supported_semantics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capability::{CapabilityProvenance, HardwareCapability},
        subscription_policy::{PlanReason, PlanStatus},
        tui::{engine_overview, Panel},
    };

    fn policy(requests_per_second: u32) -> SubscriptionPolicy {
        SubscriptionPolicy::new(
            HardwareCapability::new(
                requests_per_second,
                Duration::from_millis(250),
                CapabilityProvenance::BuiltInDefault,
            )
            .unwrap(),
        )
    }

    #[test]
    fn built_in_layout_derives_only_explicit_semantic_demand() {
        let layout = engine_overview();
        let requests = observation_requests(&layout, LayoutFreshnessPolicy::default()).unwrap();
        assert_eq!(requests.len(), 7);
        assert!(requests
            .iter()
            .all(|request| request.source() == "layout:engine-overview"));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.semantic() == "engine.rpm")
                .count(),
            3
        );
        assert_eq!(
            requests
                .iter()
                .map(ObservationRequest::semantic)
                .collect::<Vec<_>>(),
            [
                "engine.rpm",
                "engine.rpm",
                "engine.coolant_temperature",
                "engine.maf",
                "vehicle.speed",
                "engine.rpm",
                "engine.maf",
            ]
        );
        assert_eq!(requests[0].desired_interval(), Duration::from_secs(1));
        assert_eq!(requests[1].desired_interval(), Duration::from_millis(200));
        assert_eq!(requests[5].desired_interval(), Duration::from_millis(500));
    }

    #[test]
    fn compare_derives_exactly_two_requests() {
        let layout = DashboardLayout {
            name: "compare".into(),
            panels: vec![Panel {
                title: "Compare".into(),
                view: View::Compare,
                signals: vec!["engine.rpm".into(), "engine.maf".into()],
            }],
        };
        let requests = observation_requests(&layout, LayoutFreshnessPolicy::default()).unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].semantic(), "engine.rpm");
        assert_eq!(requests[1].semantic(), "engine.maf");
        assert_eq!(requests[0].desired_interval(), Duration::from_millis(500));
    }

    #[test]
    fn duplicate_layout_demand_merges_only_in_subscription_policy() {
        let layout = engine_overview();
        let plan = polling_plan(
            &layout,
            LayoutFreshnessPolicy::default(),
            policy(20),
            [
                "engine.rpm",
                "engine.coolant_temperature",
                "engine.maf",
                "vehicle.speed",
            ],
        )
        .unwrap();
        assert_eq!(plan.entries().len(), 4);
        let rpm = plan
            .entries()
            .iter()
            .find(|entry| entry.semantic() == "engine.rpm")
            .unwrap();
        assert_eq!(rpm.requested_interval(), Duration::from_millis(200));
        assert_eq!(rpm.status(), PlanStatus::Accepted);
    }

    #[test]
    fn unknown_semantic_stays_visible_and_never_becomes_protocol_input() {
        let layout = DashboardLayout {
            name: "unknown".into(),
            panels: vec![Panel {
                title: "Unknown".into(),
                view: View::Value,
                signals: vec!["vehicle.future_fact".into()],
            }],
        };
        let requests = observation_requests(&layout, LayoutFreshnessPolicy::default()).unwrap();
        assert_eq!(requests[0].semantic(), "vehicle.future_fact");
        let plan = polling_plan(
            &layout,
            LayoutFreshnessPolicy::default(),
            policy(4),
            ["engine.rpm"],
        )
        .unwrap();
        let entry = &plan.entries()[0];
        assert_eq!(entry.semantic(), "vehicle.future_fact");
        assert_eq!(entry.status(), PlanStatus::Unsupported);
        assert_eq!(entry.reason(), PlanReason::SignalUnsupported);
        assert_eq!(entry.effective_interval(), None);
        assert_eq!(plan.scheduled_entries().count(), 0);
    }

    #[test]
    fn budget_reduction_is_explicit_in_the_plan() {
        let layout = DashboardLayout {
            name: "busy".into(),
            panels: vec![
                Panel {
                    title: "RPM".into(),
                    view: View::TimeSeries,
                    signals: vec!["engine.rpm".into()],
                },
                Panel {
                    title: "MAF".into(),
                    view: View::TimeSeries,
                    signals: vec!["engine.maf".into()],
                },
            ],
        };
        let freshness = LayoutFreshnessPolicy::new(
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .unwrap();
        let plan =
            polling_plan(&layout, freshness, policy(4), ["engine.rpm", "engine.maf"]).unwrap();
        assert!(plan.entries().iter().all(|entry| {
            entry.status() == PlanStatus::RateReduced
                && entry.reason() == PlanReason::SessionRequestBudget
                && entry.effective_interval() > Some(entry.requested_interval())
        }));
        assert!(plan.effective_request_rate_per_second() <= 4.0);
    }

    #[test]
    fn freshness_is_not_part_of_persisted_layout_data() {
        let layout = engine_overview();
        let debug = format!("{layout:?}");
        assert!(!debug.contains("interval"));
        assert!(!debug.contains("request_payload"));
        assert!(!debug.contains("can"));
        assert!(!debug.contains("uds"));
        assert!(!debug.contains("elm"));
    }

    #[test]
    fn rejects_zero_freshness() {
        assert!(LayoutFreshnessPolicy::new(
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .is_err());
    }
}
