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
    "use serde::Deserialize;\nuse std::{collections::HashSet, time::Duration};\n",
    "use serde::Deserialize;\nuse std::{\n    collections::HashSet,\n    ffi::OsStr,\n    fs,\n    path::Path,\n    time::Duration,\n};\n",
)

replace_once(
    "src/capture.rs",
    '''pub fn profile(name: &str) -> Result<CaptureProfile, String> {
    match name {
        "engine-baseline" => parse_profile_yaml(ENGINE_BASELINE_YAML),
        "engine-drive" => parse_profile_yaml(ENGINE_DRIVE_YAML),
        "obd2-expansion-validation" => parse_profile_yaml(OBD2_EXPANSION_VALIDATION_YAML),
        _ => Err(format!("unknown capture profile: {name}")),
    }
}

fn parse_profile_yaml(input: &str) -> Result<CaptureProfile, String> {
''',
    '''pub fn profile(spec: &str) -> Result<CaptureProfile, String> {
    if is_explicit_profile_path(spec) {
        return load_profile_path(Path::new(spec));
    }
    match spec {
        "engine-baseline" => parse_profile_yaml(ENGINE_BASELINE_YAML),
        "engine-drive" => parse_profile_yaml(ENGINE_DRIVE_YAML),
        "obd2-expansion-validation" => parse_profile_yaml(OBD2_EXPANSION_VALIDATION_YAML),
        _ => Err(format!("unknown capture profile: {spec}")),
    }
}

fn is_explicit_profile_path(spec: &str) -> bool {
    let path = Path::new(spec);
    path.is_absolute()
        || path.components().count() > 1
        || matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yaml") | Some("yml")
        )
}

fn load_profile_path(path: &Path) -> Result<CaptureProfile, String> {
    let input = fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read capture profile {}: {error}",
            path.display()
        )
    })?;
    parse_profile_yaml(&input)
}

fn parse_profile_yaml(input: &str) -> Result<CaptureProfile, String> {
''',
)

replace_once(
    "src/capture.rs",
    '''mod tests {
    use super::*;

    #[test]
''',
    '''mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_profile(contents: &str) -> std::path::PathBuf {
        let sequence = TEMP_PROFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "obdentic-capture-profile-{}-{sequence}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
''',
)

replace_once(
    "src/capture.rs",
    '''    #[test]
    fn rejects_unknown_profiles() {
        assert_eq!(
            profile("not-a-profile"),
            Err("unknown capture profile: not-a-profile".into())
        );
    }

    #[test]
    fn engine_baseline_is_admitted_by_the_conservative_session_budget() {
''',
    '''    #[test]
    fn rejects_unknown_profiles() {
        assert_eq!(
            profile("not-a-profile"),
            Err("unknown capture profile: not-a-profile".into())
        );
    }

    #[test]
    fn explicit_profile_path_classification_is_deterministic() {
        assert!(is_explicit_profile_path("/tmp/custom-profile"));
        assert!(is_explicit_profile_path("./custom-profile"));
        assert!(is_explicit_profile_path("../custom-profile"));
        assert!(is_explicit_profile_path("profiles/custom-profile"));
        assert!(is_explicit_profile_path("custom-profile.yaml"));
        assert!(is_explicit_profile_path("custom-profile.yml"));
        assert!(!is_explicit_profile_path("engine-drive"));
        assert!(!is_explicit_profile_path("not-a-profile"));
    }

    #[test]
    fn loads_explicit_yaml_path_and_uses_document_id_as_profile_name() {
        let path = temp_profile(
            "version: 1\nid: local-drive\ndescription: Local explicit profile.\nobservations:\n  - semantic: vehicle.speed\n    interval: 4s\n  - semantic: engine.rpm\n    interval: 1s\n",
        );
        let loaded = profile(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(loaded.name(), "local-drive");
        assert_eq!(loaded.description(), Some("Local explicit profile."));
        assert_eq!(
            loaded
                .subscriptions()
                .unwrap()
                .iter()
                .map(|subscription| (subscription.semantic(), subscription.interval()))
                .collect::<Vec<_>>(),
            [
                ("vehicle.speed", Duration::from_secs(4)),
                ("engine.rpm", Duration::from_secs(1)),
            ]
        );
    }

    #[test]
    fn missing_explicit_profile_fails_as_local_file_read() {
        let sequence = TEMP_PROFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "obdentic-missing-profile-{}-{sequence}.yaml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let error = profile(path.to_str().unwrap()).unwrap_err();
        assert!(error.starts_with("failed to read capture profile "));
    }

    #[test]
    fn explicit_profile_uses_the_same_fail_closed_schema() {
        let path = temp_profile(
            "version: 1\nid: unsafe-local\nobservations:\n  - semantic: engine.rpm\n    interval: 1s\n    pid: 0x0C\n",
        );
        let error = profile(path.to_str().unwrap()).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("invalid capture profile YAML"));
    }

    #[test]
    fn engine_baseline_is_admitted_by_the_conservative_session_budget() {
''',
)

replace_once(
    "docs/capture-profiles.md",
    '''EA189 DPF/longitudinal orchestration remains a separate temporary bridge tracked by #14/#88 and PR #105.

## Relationship to effective Vehicle Knowledge
''',
    '''EA189 DPF/longitudinal orchestration remains a separate temporary bridge tracked by #14/#88 and PR #105.

## Profile file resolution

`--profile` accepts either an embedded built-in name or an explicit local YAML path:

```text
--profile engine-drive
--profile ./my-profile.yaml
--profile ../profiles/my-profile.yml
--profile /absolute/path/my-profile.yaml
```

Resolution is deterministic:

1. a syntactically explicit path is read exactly from the local filesystem;
2. otherwise the value must match an embedded built-in profile name;
3. otherwise loading fails as an unknown profile.

Absolute paths, multi-component paths (`./`, `../`, or nested paths), and plain `.yaml`/`.yml` filenames are treated as explicit paths. Extensionless files remain available when written explicitly, for example `./my-profile`.

There is no implicit directory scan, globbing, URL/network loading, profile inheritance, or user-config-directory fallback in this slice. A future user-config search path can be added only with separately documented deterministic precedence.

Explicit files pass through exactly the same closed schema-v1 parser as embedded profiles. File read and parse errors occur before any adapter connection. The semantic profile identity is the YAML `id`; the local filesystem path is not stored as the profile name used by capture metadata.

## Relationship to effective Vehicle Knowledge
''',
)
