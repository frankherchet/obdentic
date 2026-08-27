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

pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => Ok(ENGINE_BASELINE),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_unknown_profiles() {
        assert_eq!(
            profile("not-a-profile"),
            Err("unknown capture profile: not-a-profile".into())
        );
    }
}
