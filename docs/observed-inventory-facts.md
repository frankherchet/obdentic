# Private observed inventory -> normalized ECU facts

OBDentic keeps three concepts separate:

```text
private observed inventory / evidence
  -> justified normalized ECU facts
  + pinned canonical Knowledge
  -> effective Vehicle Knowledge
```

The local vehicle cache is evidence about one concrete vehicle and its observed ECUs. It is not the canonical Knowledge database and it does not become one by decoding values opportunistically.

## Vehicle and ECU instances

`VehicleCache` is one `VehicleInstance`: a private local key (never the raw VIN), `first_seen_ms`/`last_seen_ms`, and a `VehicleCacheSnapshot` holding this vehicle's current typed evidence.

The snapshot stores topology observations, Mode 01 capabilities, target/role mappings, and standard UDS identification results as independent vectors, each keyed by the same typed `ResponderIdentity`. `VehicleCacheSnapshot::ecu_instances()` groups them by that identity into a read-only `EcuInstance` view — `Vehicle -> n EcuInstances` is therefore directly queryable rather than left implicit in four separately enumerated vectors. This is a projection, not a second persistence format: the on-disk TSV layout and its version header are unchanged, and grouping is deterministic (sorted by `ResponderIdentity`) regardless of insertion order.

An `EcuInstance` never fabricates a field it has no evidence for: a responder observed only through a target mapping (for example a validated engine target before its standard identification set has run) still yields an instance, with `capabilities()`/`identification()` legitimately absent rather than borrowed from another ECU.

History is currently the vehicle-level `history: Vec<String>` evidence log plus each stored evidence value's own provenance; there is no per-ECU multi-version timeline yet distinguishing an old fingerprint from a newer one on the same responder. A fresh discovery run replaces this vehicle's entire snapshot rather than merging into the previous one, so a software-version change on an existing ECU is not currently distinguishable from its prior evidence. Representing an explicit per-field history remains a follow-up slice.

## Raw ECU-identification evidence is not automatically a fingerprint fact

The bounded standard UDS ECU-identification path persists normalized response payload bytes together with target/responder identity, status, Knowledge definition identity and errors/NRCs.

The current generic standard definitions deliberately use `opaque_bytes` for identity payloads whose textual encoding is not independently justified. Therefore the inventory projection does **not** perform any of the following conversions:

- UTF-8/ASCII guessing
- trimming guessed padding
- hexadecimal rendering treated as semantic identity
- case folding
- numeric/plausibility selection

A response such as a successful F189 payload remains authoritative raw evidence until a reviewed normalizer can justify a canonical `ecu.manufacturer_software_version` string. Missing normalization stays missing; the effective-Knowledge resolver may consequently report `insufficient_identity` for a specific definition.

Unsupported, negative-response, unavailable, malformed, timeout, transport-error and not-probed observations likewise create no fingerprint value.

## Explicit normalized facts

`NormalizedInventoryFact` is the narrow handoff for an upstream reviewed normalizer. It contains:

- an already-known typed `ResponderIdentity`
- one closed `FingerprintField`
- one non-empty normalized string value
- provenance for that normalization

The projection rejects a normalized fact whose responder is absent from the private observed snapshot. It never creates a new ECU merely because external text claims one exists.

Equal duplicate facts merge provenance. Conflicting values for the same responder/field fail explicitly; there is no first-wins or plausibility rule.

## Logical role

A known standard logical role is already a typed vehicle fact, so it can be projected without interpreting response bytes:

- `Engine` -> `engine`
- `Transmission` -> `transmission`
- `Gateway` -> `gateway`

Only an explicit `RoleAssignment` for the exact responder contributes this fact. Numeric responder/address resemblance and observation order do not infer a role.

`Unknown` and `VendorSpecific` roles remain unresolved until a separately reviewed normalization contract defines their canonical string semantics.

## Local ECU identity

The projection sorts typed responder identities and assigns deterministic local projection IDs (`inventory-ecu-0000`, ...). These IDs exist only to keep resolver input for separate ECUs distinct.

They are not:

- VINs
- CAN IDs promoted into canonical Knowledge keys
- request targets
- manufacturer logical addresses

The original typed responder remains attached to the projected result for private inventory/audit correlation.

## VIN boundary

VIN is absent from the canonical fingerprint-field vocabulary and from `ObservedEcuFacts`. Vehicle identity may select the local private cache record, but it cannot select a canonical ECU decoder definition.

## Safety

The projection performs no adapter, session, transport, Git or network I/O. It cannot construct a diagnostic request and cannot expand the existing executable operation vocabulary.

Downstream flow remains:

```text
projected ObservedEcuFacts
  + pinned canonical Knowledge
  -> EffectiveVehicleKnowledge
  -> closed typed operation
  -> SafetyPolicy
  -> single-owner DiagnosticSession
```

A normalized fact changes applicability only; it never grants protocol or mutation authority.
