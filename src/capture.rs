use crate::{
    capability::HardwareCapability,
    scheduler::Subscription,
    subscription_policy::{ObservationRequest, PlanStatus, SubscriptionPolicy},
};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};

const PROFILE_SCHEMA_VERSION: u32 = 1;
const ENGINE_BASELINE_YAML: &str = include_str!("../profiles/engine-baseline.yaml");
const ENGINE_DRIVE_YAML: &str = include_str!("../profiles/engine-drive.yaml");
const OBD2_EXPANSION_VALIDATION_YAML: &str =
    include_str!("../profiles/obd2-expansion-validation.yaml");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureProfile {
    name: String,
    description: Option<String>,
    subscriptions: Vec<ProfileSubscription>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProfileSubscription {
    semantic: String,
    interval: Duration,
}

impl CaptureProfile {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn subscriptions(&self) -> Result<Vec<Subscription>, String> {
        self.subscriptions
            .iter()
            .map(|subscription| Subscription::new(&subscription.semantic, subscription.interval))
            .collect()
    }

    /// Reject a profile before connecting when its offered load exceeds the
    /// session's conservative sequential-command budget.
    pub fn admit(&self, capability: HardwareCapability) -> Result<(), String> {
        let requests = self
            .subscriptions
            .iter()
            .map(|subscription| {
                ObservationRequest::new(
                    "capture",
                    subscription.semantic.clone(),
                    subscription.interval,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let plan = SubscriptionPolicy::new(capability).plan(
            &requests,
            self.subscriptions
                .iter()
                .map(|subscription| subscription.semantic.as_str()),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    version: u32,
    id: String,
    #[serde(default)]
    description: Option<String>,
    observations: Vec<ProfileObservationDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileObservationDocument {
    semantic: String,
    interval: String,
}

pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => parse_profile_yaml(ENGINE_BASELINE_YAML),
        "engine-drive" => parse_profile_yaml(ENGINE_DRIVE_YAML),
        "obd2-expansion-validation" => parse_profile_yaml(OBD2_EXPANSION_VALIDATION_YAML),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}

fn parse_profile_yaml(input: &str) -> Result<CaptureProfile, String> {
    let document: ProfileDocument = serde_yaml_ng::from_str(input)
        .map_err(|error| format!("invalid capture profile YAML: {error}"))?;
    if document.version != PROFILE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported capture profile schema version {}; expected {}",
            document.version, PROFILE_SCHEMA_VERSION
        ));
    }
    if !valid_profile_id(&document.id) {
        return Err(format!("invalid capture profile id: {}", document.id));
    }
    if document.observations.is_empty() {
        return Err(format!(
            "capture profile {} must contain at least one observation",
            document.id
        ));
    }

    let mut seen = HashSet::new();
    let mut subscriptions = Vec::with_capacity(document.observations.len());
    for observation in document.observations {
        if observation.semantic.trim() != observation.semantic || observation.semantic.is_empty() {
            return Err("capture profile semantic must be a non-empty exact identifier".into());
        }
        if observation.semantic.contains('*') || observation.semantic.contains('?') {
            return Err(format!(
                "capture profile {} uses forbidden semantic wildcard {}",
                document.id, observation.semantic
            ));
        }
        if !seen.insert(observation.semantic.clone()) {
            return Err(format!(
                "capture profile {} contains duplicate semantic {}",
                document.id, observation.semantic
            ));
        }
        let interval = parse_interval(&observation.interval)?;

        // Profiles select only semantics. Constructing a Subscription performs
        // the existing closed semantic -> ReadRequest resolution, without I/O,
        // so an unknown semantic fails before any adapter is contacted.
        Subscription::new(&observation.semantic, interval).map_err(|error| {
            format!(
                "capture profile {} references unavailable semantic {}: {error}",
                document.id, observation.semantic
            )
        })?;
        subscriptions.push(ProfileSubscription {
            semantic: observation.semantic,
            interval,
        });
    }

    Ok(CaptureProfile {
        name: document.id,
        description: document.description,
        subscriptions,
    })
}

fn valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn parse_interval(value: &str) -> Result<Duration, String> {
    if value.trim() != value || value.is_empty() {
        return Err(format!("invalid capture profile interval: {value:?}"));
    }
    let (number, multiplier_ms) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000_u64)
    } else {
        return Err(format!(
            "invalid capture profile interval {value:?}; expected positive integer ms or s"
        ));
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "invalid capture profile interval {value:?}; expected positive integer ms or s"
        ));
    }
    let amount = number
        .parse::<u64>()
        .map_err(|_| format!("capture profile interval is too large: {value}"))?;
    if amount == 0 {
        return Err("capture profile interval must be greater than zero".into());
    }
    let milliseconds = amount
        .checked_mul(multiplier_ms)
        .ok_or_else(|| format!("capture profile interval is too large: {value}"))?;
    Ok(Duration::from_millis(milliseconds))
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
    fn engine_drive_is_loaded_from_yaml_with_exact_legacy_behavior() {
        let profile = profile("engine-drive").unwrap();
        assert_eq!(profile.name(), "engine-drive");
        assert_eq!(
            profile.description(),
            Some("Conservative drive-test profile derived from real Carly/ELM hardware evidence.")
        );
        let subscriptions = profile.subscriptions().unwrap();
        let actual = subscriptions
            .iter()
            .map(|subscription| (subscription.semantic(), subscription.interval()))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                ("engine.rpm", Duration::from_secs(1)),
                ("engine.maf", Duration::from_secs(2)),
                ("engine.load", Duration::from_secs(4)),
                ("engine.intake_manifold_pressure", Duration::from_secs(2)),
                ("vehicle.speed", Duration::from_secs(4)),
                ("engine.egr.commanded", Duration::from_secs(4)),
                ("engine.egr.error", Duration::from_secs(4)),
                ("vehicle.accelerator_pedal_e", Duration::from_secs(4)),
                ("engine.coolant_temperature", Duration::from_secs(8)),
                ("engine.intake_air_temperature", Duration::from_secs(8)),
                ("engine.runtime", Duration::from_secs(8)),
                ("engine.barometric_pressure", Duration::from_secs(8)),
            ]
        );
    }

    #[test]
    fn engine_drive_stays_below_observed_request_budget_and_avoids_conflict_pids() {
        let profile = profile("engine-drive").unwrap();
        let subscriptions = profile.subscriptions().unwrap();
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
            profile.admit(HardwareCapability::conservative_default()),
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

    #[test]
    fn rejects_malformed_yaml_and_unsupported_schema_versions() {
        assert!(parse_profile_yaml("version: [").is_err());
        assert_eq!(
            parse_profile_yaml(
                "version: 2\nid: future\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n"
            )
            .unwrap_err(),
            "unsupported capture profile schema version 2; expected 1"
        );
    }

    #[test]
    fn rejects_duplicate_unknown_and_wildcard_semantics() {
        assert!(parse_profile_yaml(
            "version: 1\nid: duplicate\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n  - semantic: engine.rpm\n    interval: 2s\n"
        )
        .unwrap_err()
        .contains("duplicate semantic engine.rpm"));
        assert!(parse_profile_yaml(
            "version: 1\nid: unknown\nobservations:\n  - semantic: vehicle.not_real\n    interval: 1s\n"
        )
        .is_err());
        assert!(parse_profile_yaml(
            "version: 1\nid: wildcard\nobservations:\n  - semantic: engine.*\n    interval: 1s\n"
        )
        .unwrap_err()
        .contains("forbidden semantic wildcard"));
    }

    #[test]
    fn rejects_zero_invalid_and_overflowing_intervals() {
        for interval in ["0s", "0ms", "1.5s", "1m", " 1s", "1s "] {
            assert!(parse_interval(interval).is_err(), "accepted {interval}");
        }
        assert!(parse_interval("18446744073709551615s").is_err());
        assert_eq!(parse_interval("250ms"), Ok(Duration::from_millis(250)));
        assert_eq!(parse_interval("2s"), Ok(Duration::from_secs(2)));
    }

    #[test]
    fn rejects_protocol_and_unknown_fields_fail_closed() {
        for yaml in [
            "version: 1\nid: raw\nrequest: 010C\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n",
            "version: 1\nid: raw\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n    pid: 0x0C\n",
            "version: 1\nid: raw\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n    elm_command: 010C\n",
        ] {
            assert!(parse_profile_yaml(yaml).is_err());
        }
    }

    #[test]
    fn yaml_observation_order_is_deterministic() {
        let yaml = "version: 1\nid: ordered\nobservations:\n  - semantic: vehicle.speed\n    interval: 4s\n  - semantic: engine.rpm\n    interval: 1s\n";
        let first = parse_profile_yaml(yaml).unwrap().subscriptions().unwrap();
        let second = parse_profile_yaml(yaml).unwrap().subscriptions().unwrap();
        assert_eq!(
            first
                .iter()
                .map(|subscription| subscription.semantic())
                .collect::<Vec<_>>(),
            ["vehicle.speed", "engine.rpm"]
        );
        assert_eq!(first, second);
    }
}
