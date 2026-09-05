# Declarative capture profiles

Capture profiles describe observation intent. They select known semantic facts and requested intervals; they do not define vehicle protocol knowledge.

The intended dependency direction remains:

```text
observed inventory + canonical knowledge
  -> effective Vehicle Knowledge
  -> capture profile
  -> SubscriptionPolicy
  -> SafetyPolicy / closed read-only core
  -> DiagnosticSession
```

## Schema v1

The first YAML profile schema is intentionally small and closed:

```yaml
version: 1
id: engine-drive
description: Conservative drive-test profile.
observations:
  - semantic: engine.rpm
    interval: 1s
  - semantic: engine.maf
    interval: 2s
```

Allowed observation fields are only:

- `semantic`: one exact semantic identifier
- `interval`: a positive integer duration using `ms` or `s`

The top-level fields are `version`, `id`, optional `description`, and `observations`.

Unknown fields fail parsing. Profiles therefore cannot introduce PID/DID values, CAN addresses, UDS payloads/services, ELM commands, decoder formulas, session control, SecurityAccess, coding, adaptation, actuator operations, or DTC-clear operations.

Unversioned semantic wildcards such as `engine.*` or `dpf.*` are rejected. Adding knowledge must not silently expand an existing profile's wire traffic.

## Safety boundary

YAML is configuration, not an execution capability. After parsing, every semantic is resolved through the existing closed semantic-to-`ReadRequest` path before any adapter connection. Unknown semantics fail closed.

`SubscriptionPolicy` remains the polling-rate authority. The profile only requests intervals; it cannot bypass rate admission or create transport operations directly.

The diagnostic core and `SafetyPolicy` remain authoritative for which read-only operations can reach the physical session. The profile loader exposes no raw-send API.

## Built-in profile migration

All three generic built-in capture profiles are now versioned YAML files under `profiles/`:

- `engine-baseline.yaml`
- `engine-drive.yaml`
- `obd2-expansion-validation.yaml`

The YAML files are embedded in the binary at build time so named profile loading does not depend on a source checkout being present next to an installed executable. Each migrated profile preserves its previous ordered semantic set and requested intervals exactly.

EA189 DPF/longitudinal orchestration remains a separate temporary bridge tracked by #14/#88 and PR #105.

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

Issue #86, the effective Vehicle Knowledge resolver, is still open. Until that resolver is available, this migration slice validates profile semantics against OBDentic's existing closed semantic catalog and routes them through the existing Vehicle Knowledge path.

This is deliberately a migration seam, not a claim that #88's final effective-catalog resolution is complete. Required/optional availability and ambiguity handling should be added only on top of the #86 resolver contract rather than invented in the profile loader.

## Hardware acceptance

Offline tests prove schema validation, deterministic order, behavior equivalence, rate admission, and fail-closed handling. Hardware equivalence remains a separate acceptance step: the YAML-backed `engine-drive` profile should produce the same semantic intent, sequential one-owner session behavior, requested/effective rates, and evidence preservation as the previous Rust-backed profile.
