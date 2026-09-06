# ECU identification normalization

OBDentic treats ECU-identification bytes as private observed evidence first. A successful UDS ReadDataByIdentifier response does not become an ECU fingerprint merely because its payload looks textual or plausible.

The normalization boundary is:

```text
IdentificationObservation
  + exact pinned Knowledge definition
  -> fail-closed provenance binding
  -> canonical decoder contract
  -> optional NormalizedInventoryFact
  -> project_observed_ecu_facts
  -> EffectiveVehicleKnowledge
```

## Exact Knowledge binding

Before any decoder is considered, normalization requires exact agreement between the persisted observation and the currently supplied pinned `KnowledgeCatalog` for:

- Knowledge repository
- Knowledge revision
- definition ID
- definition version
- semantic ID
- typed request bytes

A mismatch is an error, not a reason to guess or silently reinterpret the evidence.

For a supported observation, the canonical response contract is re-applied offline by reconstructing the normalized positive UDS response from the persisted request and value bytes. This validates the definition's response-length contract before decoding while leaving the original observation unchanged.

## Decoder behavior

The Core executes only decoder primitives already declared by the exact canonical definition.

### `opaque_bytes`

Produces no normalized fingerprint fact. The bytes remain evidence.

This is the behavior of the currently pinned generic standard F18x/F19x ECU-identification definitions. Their standardized DID meaning does not justify a text encoding.

### `ascii`

ASCII decoding follows the canonical contract documented in the pinned `obdentic-knowledge/docs/decoders.md` revision.

`trim` is trailing-only:

- `none`: remove nothing
- `space`: remove trailing `0x20`
- `nul`: remove trailing `0x00`
- `space_and_nul`: remove a trailing run of `0x20` and/or `0x00`

Leading and interior bytes are not trimmed. Remaining bytes must be printable 7-bit ASCII (`0x20..0x7e`), and the normalized result must be non-empty.

The Core does not case-fold, parse numbers, strip punctuation, collapse whitespace, select substrings or use plausibility to choose a value.

### `linear_integer`

The decoder is part of the closed Knowledge vocabulary, but this slice does not format numeric results into ECU fingerprint strings. A canonical textual representation would need its own reviewed contract.

## Fingerprint eligibility

Only identification semantics already present in the closed ECU fingerprint vocabulary can produce `NormalizedInventoryFact` values. Standard ECU serial number and manufacturing date remain private observed evidence and are intentionally not applicability keys. VIN/F190 remains outside ECU identification entirely.

## Provenance

A normalized fact records a stable source reference containing the exact canonical repository, revision, definition ID and definition version. Knowledge confidence is mapped to the existing topology confidence vocabulary without promoting provenance classification.

The richer canonical Knowledge provenance remains in the pinned catalog. Decoder success does not turn `EXPERIMENTAL`, `INFERRED` or `COMMUNITY` knowledge into `VERIFIED`, and it does not change hardware-validation state.

## Safety

Normalization is pure and transport-free. It cannot create a diagnostic request, discover a DID, open a session, perform SecurityAccess, mutate ECU state, clear DTCs, or widen `SafetyPolicy`.

A normalized fact affects applicability only. Executable reads still require the existing closed typed operation path and SafetyPolicy authorization.
