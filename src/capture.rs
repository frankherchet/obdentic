use crate::scheduler::Subscription;
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
}

const ENGINE_BASELINE: CaptureProfile = CaptureProfile {
    name: "engine-baseline",
    subscriptions: &[
        ("engine.rpm", 250),
        ("engine.maf", 500),
        ("engine.load", 500),
        ("engine.intake_manifold_pressure", 500),
        ("vehicle.speed", 1_000),
        ("engine.egr.commanded", 1_000),
        ("engine.egr.error", 1_000),
        ("vehicle.accelerator_pedal_d", 1_000),
        ("vehicle.accelerator_pedal_e", 1_000),
        ("engine.relative_throttle", 1_000),
        ("engine.coolant_temperature", 2_000),
        ("engine.intake_air_temperature", 2_000),
        ("engine.control_module_voltage", 2_000),
        ("engine.runtime", 2_000),
        ("engine.barometric_pressure", 2_000),
    ],
};

/// Conservative drive-test profile derived from real Carly/ELM hardware evidence.
///
/// `10-idle.jsonl` sustained roughly 5.4 completed logical reads/s while the
/// denser baseline continuously overbooked the single sequential ELM path.
/// This profile offers about 4.4 reads/s, leaving headroom for response-time
/// jitter.  It also omits PID 42 and PID 49, which repeatedly produced
/// responder conflicts and therefore expensive retries in that capture.
const ENGINE_DRIVE: CaptureProfile = CaptureProfile {
    name: "engine-drive",
    subscriptions: &[
        ("engine.rpm", 1_000),
        ("engine.maf", 1_500),
        ("engine.load", 3_000),
        ("engine.intake_manifold_pressure", 1_500),
        ("vehicle.speed", 3_000),
        ("engine.egr.commanded", 3_000),
        ("engine.egr.error", 3_000),
        ("vehicle.accelerator_pedal_e", 3_000),
        ("engine.coolant_temperature", 10_000),
        ("engine.intake_air_temperature", 10_000),
        ("engine.runtime", 10_000),
        ("engine.barometric_pressure", 10_000),
    ],
};

pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => Ok(ENGINE_BASELINE),
        "engine-drive" => Ok(ENGINE_DRIVE),
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
                "vehicle.accelerator_pedal_d",
                "vehicle.accelerator_pedal_e",
                "engine.relative_throttle",
                "engine.coolant_temperature",
                "engine.intake_air_temperature",
                "engine.control_module_voltage",
                "engine.runtime",
                "engine.barometric_pressure",
            ]
        );
    }

    #[test]
    fn engine_baseline_keeps_its_declared_sampling_intervals() {
        let subscriptions = profile("engine-baseline").unwrap().subscriptions().unwrap();
        assert_eq!(subscriptions[0].interval(), Duration::from_millis(250));
        assert!(subscriptions[1..4]
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_millis(500)));
        assert!(subscriptions[4..10]
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_secs(1)));
        assert!(subscriptions[10..]
            .iter()
            .all(|subscription| subscription.interval() == Duration::from_secs(2)));
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
        assert!(offered_reads_per_second < 4.5);
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
            Duration::from_secs(10)
        );
    }

    #[test]
    fn profiles_have_unique_catalogued_semantics_and_positive_intervals() {
        for name in ["engine-baseline", "engine-drive"] {
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
}
