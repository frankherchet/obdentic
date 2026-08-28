use crate::{
    capability::HardwareCapability,
    scheduler::Subscription,
    subscription_policy::{ObservationRequest, PlanStatus, SubscriptionPolicy},
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureProfile {
    name: &'static str,
    subscriptions: &'static [(&'static str, u64)],
}

impl CaptureProfile {
    pub fn name(self) -> &'static str {
        self.name
    }

    pub fn subscriptions(self) -> Result<Vec<Subscription>, String> {
        self.subscriptions
            .iter()
            .map(|(semantic, interval_ms)| {
                Subscription::new(semantic, Duration::from_millis(*interval_ms))
            })
            .collect()
    }

    /// Reject a profile before connecting when its offered load exceeds the
    /// session's conservative sequential-command budget.
    pub fn admit(self, capability: HardwareCapability) -> Result<(), String> {
        let requests = self
            .subscriptions
            .iter()
            .map(|(semantic, interval_ms)| {
                ObservationRequest::new("capture", *semantic, Duration::from_millis(*interval_ms))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let plan = SubscriptionPolicy::new(capability).plan(
            &requests,
            self.subscriptions.iter().map(|(semantic, _)| *semantic),
        );
        if plan
            .entries()
            .iter()
            .any(|entry| entry.status() == PlanStatus::RateReduced)
        {
            return Err(format!(
                "capture profile {} exceeds the {} reads/s session budget",
                self.name,
                capability.request_budget_per_second()
            ));
        }
        Ok(())
    }
}

const ENGINE_BASELINE: CaptureProfile = CaptureProfile {
    name: "engine-baseline",
    subscriptions: &[
        ("engine.rpm", 1_000),
        ("engine.maf", 2_000),
        ("engine.load", 2_000),
        ("engine.intake_manifold_pressure", 2_000),
        ("vehicle.speed", 8_000),
        ("engine.egr.commanded", 8_000),
        ("engine.egr.error", 8_000),
        ("vehicle.accelerator_pedal_e", 8_000),
        ("engine.relative_throttle", 8_000),
        ("engine.coolant_temperature", 8_000),
        ("engine.intake_air_temperature", 8_000),
        ("engine.runtime", 8_000),
        ("engine.barometric_pressure", 8_000),
    ],
};

/// Conservative drive-test profile derived from real Carly/ELM hardware evidence.
///
/// `10-idle.jsonl` sustained roughly 5.4 completed logical reads/s while the
/// denser baseline continuously overbooked the single sequential ELM path.
/// This profile offers 3.75 reads/s, leaving headroom for response-time
/// jitter.  It also omits PID 42 and PID 49, which repeatedly produced
/// responder conflicts and therefore expensive retries in that capture.
const ENGINE_DRIVE: CaptureProfile = CaptureProfile {
    name: "engine-drive",
    subscriptions: &[
        ("engine.rpm", 1_000),
        ("engine.maf", 2_000),
        ("engine.load", 4_000),
        ("engine.intake_manifold_pressure", 2_000),
        ("vehicle.speed", 4_000),
        ("engine.egr.commanded", 4_000),
        ("engine.egr.error", 4_000),
        ("vehicle.accelerator_pedal_e", 4_000),
        ("engine.coolant_temperature", 8_000),
        ("engine.intake_air_temperature", 8_000),
        ("engine.runtime", 8_000),
        ("engine.barometric_pressure", 8_000),
    ],
};

const OBD2_EXPANSION_VALIDATION: CaptureProfile = CaptureProfile {
    name: "obd2-expansion-validation",
    subscriptions: &[
        ("engine.throttle_position", 2_000),
        ("vehicle.distance_with_mil_on", 2_000),
        ("engine.fuel_rail_gauge_pressure", 2_000),
        ("vehicle.warmups_since_dtc_clear", 2_000),
        ("vehicle.distance_since_dtc_clear", 2_000),
        ("vehicle.ambient_air_temperature", 2_000),
        ("engine.throttle_actuator.commanded", 2_000),
    ],
};

pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => Ok(ENGINE_BASELINE),
        "engine-drive" => Ok(ENGINE_DRIVE),
        "obd2-expansion-validation" => Ok(OBD2_EXPANSION_VALIDATION),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn exposes_engine_baseline_name_and_semantics() {
        let profile = profile("engine-baseline").unwrap();
        assert_eq!(profile.name(), "engine-baseline");
        let semantics = profile
            .subscriptions()
            .unwrap()
            .into_iter()
            .map(Subscription::semantic)
            .collect::<Vec<_>>();
        assert_eq!(
            semantics,
            [
                "engine.rpm",
                "engine.maf",
                "engine.load",
                "engine.intake_manifold_pressure",
                "vehicle.speed",
                "engine.egr.commanded",
                "engine.egr.error",
                "vehicle.accelerator_pedal_e",
                "engine.relative_throttle",
                "engine.coolant_temperature",
                "engine.intake_air_temperature",
                "engine.runtime",
                "engine.barometric_pressure",
            ]
        );
    }

    #[test]
    fn engine_baseline_keeps_its_declared_sampling_intervals() {
        let subscriptions = profile("engine-baseline").unwrap().subscriptions().unwrap();
        assert_eq!(subscriptions[0].interval(), Duration::from_secs(1));
        assert!(subscriptions[1..4]
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_secs(2)));
        assert!(subscriptions[4..]
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_secs(8)));
    }

    #[test]
    fn engine_drive_stays_below_observed_request_budget_and_avoids_conflict_pids() {
        let subscriptions = profile("engine-drive").unwrap().subscriptions().unwrap();
        let semantics = subscriptions
            .iter()
            .map(|subscription| subscription.semantic())
            .collect::<Vec<_>>();

        assert_eq!(subscriptions.len(), 12);
        assert!(!semantics.contains(&"engine.control_module_voltage"));
        assert!(!semantics.contains(&"vehicle.accelerator_pedal_d"));
        assert!(!semantics.contains(&"engine.relative_throttle"));
        assert!(semantics.contains(&"vehicle.accelerator_pedal_e"));

        let offered_reads_per_second = subscriptions
            .iter()
            .map(|subscription| 1.0 / subscription.interval().as_secs_f64())
            .sum::<f64>();
        assert!(offered_reads_per_second < 4.0);
        assert_eq!(
            profile("engine-drive")
                .unwrap()
                .admit(HardwareCapability::conservative_default()),
            Ok(())
        );
    }

    #[test]
    fn engine_drive_preserves_dynamic_priority_over_background_signals() {
        let subscriptions = profile("engine-drive").unwrap().subscriptions().unwrap();
        let interval = |semantic| {
            subscriptions
                .iter()
                .find(|subscription| subscription.semantic() == semantic)
                .unwrap()
                .interval()
        };

        assert_eq!(interval("engine.rpm"), Duration::from_secs(1));
        assert!(interval("engine.rpm") < interval("engine.coolant_temperature"));
        assert!(interval("engine.intake_manifold_pressure") < interval("engine.runtime"));
        assert_eq!(
            interval("engine.coolant_temperature"),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn obd2_expansion_validation_has_exact_catalogued_semantics_and_intervals() {
        let profile = profile("obd2-expansion-validation").unwrap();
        assert_eq!(profile.name(), "obd2-expansion-validation");
        let subscriptions = profile.subscriptions().unwrap();
        assert_eq!(
            subscriptions
                .iter()
                .map(|subscription| subscription.semantic())
                .collect::<Vec<_>>(),
            [
                "engine.throttle_position",
                "vehicle.distance_with_mil_on",
                "engine.fuel_rail_gauge_pressure",
                "vehicle.warmups_since_dtc_clear",
                "vehicle.distance_since_dtc_clear",
                "vehicle.ambient_air_temperature",
                "engine.throttle_actuator.commanded",
            ]
        );
        assert!(subscriptions
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_secs(2)));
    }

    #[test]
    fn obd2_expansion_validation_is_admitted_by_default_budget() {
        let profile = profile("obd2-expansion-validation").unwrap();
        assert_eq!(
            profile.admit(HardwareCapability::conservative_default()),
            Ok(())
        );
    }

    #[test]
    fn profiles_have_unique_catalogued_semantics_and_positive_intervals() {
        for name in [
            "engine-baseline",
            "engine-drive",
            "obd2-expansion-validation",
        ] {
            let subscriptions = profile(name).unwrap().subscriptions().unwrap();
            let semantics = subscriptions
                .iter()
                .map(|subscription| subscription.semantic())
                .collect::<Vec<_>>();
            let unique = semantics.iter().copied().collect::<HashSet<_>>();

            assert_eq!(unique.len(), semantics.len());
            assert!(!semantics.is_empty());
            for subscription in subscriptions {
                assert!(subscription.interval() > Duration::ZERO);
                assert!(crate::vehicle::signal(subscription.semantic()).is_some());
            }
        }
    }

    #[test]
    fn rejects_unknown_profiles() {
        assert_eq!(
            profile("not-a-profile"),
            Err("unknown capture profile: not-a-profile".into())
        );
    }

    #[test]
    fn engine_baseline_is_admitted_by_the_conservative_session_budget() {
        let profile = profile("engine-baseline").unwrap();
        assert_eq!(
            profile.admit(HardwareCapability::conservative_default()),
            Ok(())
        );
        assert!(profile
            .admit(
                HardwareCapability::new(
                    3,
                    Duration::from_millis(250),
                    crate::capability::CapabilityProvenance::MeasuredFromCapture,
                )
                .unwrap(),
            )
            .is_err());
    }
}
