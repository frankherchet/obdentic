from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "src/capture.rs",
    'const PROFILE_SCHEMA_VERSION: u32 = 1;\nconst ENGINE_DRIVE_YAML: &str = include_str!("../profiles/engine-drive.yaml");\n',
    'const PROFILE_SCHEMA_VERSION: u32 = 1;\nconst ENGINE_BASELINE_YAML: &str = include_str!("../profiles/engine-baseline.yaml");\nconst ENGINE_DRIVE_YAML: &str = include_str!("../profiles/engine-drive.yaml");\nconst OBD2_EXPANSION_VALIDATION_YAML: &str =\n    include_str!("../profiles/obd2-expansion-validation.yaml");\n',
)

old_catalog = '''const ENGINE_BASELINE_SUBSCRIPTIONS: &[(&str, u64)] = &[
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
];

const OBD2_EXPANSION_VALIDATION_SUBSCRIPTIONS: &[(&str, u64)] = &[
    ("engine.throttle_position", 2_000),
    ("vehicle.distance_with_mil_on", 2_000),
    ("engine.fuel_rail_gauge_pressure", 2_000),
    ("vehicle.warmups_since_dtc_clear", 2_000),
    ("vehicle.distance_since_dtc_clear", 2_000),
    ("vehicle.ambient_air_temperature", 2_000),
    ("engine.throttle_actuator.commanded", 2_000),
];

pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => Ok(legacy_profile(
            "engine-baseline",
            ENGINE_BASELINE_SUBSCRIPTIONS,
        )),
        "engine-drive" => parse_profile_yaml(ENGINE_DRIVE_YAML),
        "obd2-expansion-validation" => Ok(legacy_profile(
            "obd2-expansion-validation",
            OBD2_EXPANSION_VALIDATION_SUBSCRIPTIONS,
        )),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}

fn legacy_profile(name: &str, subscriptions: &[(&str, u64)]) -> CaptureProfile {
    CaptureProfile {
        name: name.to_owned(),
        description: None,
        subscriptions: subscriptions
            .iter()
            .map(|(semantic, interval_ms)| ProfileSubscription {
                semantic: (*semantic).to_owned(),
                interval: Duration::from_millis(*interval_ms),
            })
            .collect(),
    }
}
'''

new_catalog = '''pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => parse_profile_yaml(ENGINE_BASELINE_YAML),
        "engine-drive" => parse_profile_yaml(ENGINE_DRIVE_YAML),
        "obd2-expansion-validation" => parse_profile_yaml(OBD2_EXPANSION_VALIDATION_YAML),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}
'''
replace_once("src/capture.rs", old_catalog, new_catalog)

replace_once(
    "docs/capture-profiles.md",
    '''This first #88 migration slice moves only `engine-drive` to `profiles/engine-drive.yaml`. The YAML is embedded in the binary at build time so named profile loading does not depend on a source checkout being present next to an installed executable.

`engine-baseline` and `obd2-expansion-validation` remain legacy Rust definitions in this slice. EA189 DPF/longitudinal orchestration also remains separate and is tracked by #14/#88 and PR #105.

The `engine-drive` YAML preserves the previous ordered semantic set and requested intervals exactly.
''',
    '''All three generic built-in capture profiles are now versioned YAML files under `profiles/`:

- `engine-baseline.yaml`
- `engine-drive.yaml`
- `obd2-expansion-validation.yaml`

The YAML files are embedded in the binary at build time so named profile loading does not depend on a source checkout being present next to an installed executable. Each migrated profile preserves its previous ordered semantic set and requested intervals exactly.

EA189 DPF/longitudinal orchestration remains a separate temporary bridge tracked by #14/#88 and PR #105.
''',
)
