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

## Relationship to effective Vehicle Knowledge

Issue #86, the effective Vehicle Knowledge resolver, is still open. Until that resolver is available, this migration slice validates profile semantics against OBDentic's existing closed semantic catalog and routes them through the existing Vehicle Knowledge path.

This is deliberately a migration seam, not a claim that #88's final effective-catalog resolution is complete. Required/optional availability and ambiguity handling should be added only on top of the #86 resolver contract rather than invented in the profile loader.

## Hardware acceptance

Offline tests prove schema validation, deterministic order, behavior equivalence, rate admission, and fail-closed handling. Hardware equivalence remains a separate acceptance step: the YAML-backed `engine-drive` profile should produce the same semantic intent, sequential one-owner session behavior, requested/effective rates, and evidence preservation as the previous Rust-backed profile.
