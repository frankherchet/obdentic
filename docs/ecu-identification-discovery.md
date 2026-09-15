# Bounded ECU identification discovery

OBDentic treats vehicle identity, ECU identity and canonical Knowledge as separate layers.

```text
VIN / local vehicle identity
        ↓
private vehicle inventory
        ↓
known, evidenced ECU targets
        +
pinned canonical Knowledge
        ↓
bounded standard ECU-identification plan
        ↓
SafetyPolicy
        ↓
single-owner DiagnosticSession
        ↓
per-ECU identification evidence
```

## Vehicle identity versus ECU identity

A VIN identifies one concrete vehicle instance and is handled by the separate vehicle-identity/privacy path. ECU identification describes a controller observed inside that vehicle. One VIN may therefore contain many ECU instances, and the same ECU hardware/software fingerprint may later be observed under multiple VINs.

`F190` (VIN) is not part of the default ECU-identification loop.

## Bounded standard DID policy

The core does not contain a second hand-maintained DID scan list. Discovery consumes the versioned `uds.standard.ecu_identification` set from the pinned `obdentic-knowledge` repository.

Only typed `ReadDataByIdentifier` candidates from that canonical set can reach the safety boundary. Discovery has no API for a caller-provided DID, range scan, raw UDS payload, raw CAN frame or ELM command.

Targets are not discovered by this operation. Only existing concrete target mappings with an evidenced responder and High/Verified provenance are eligible.

## Result states

Support is stored independently for every ECU/candidate pair. The result model intentionally distinguishes:

- `Supported`: a valid positive response for the requested DID was observed.
- `Unsupported`: a bounded NRC such as `serviceNotSupported` or `requestOutOfRange` explicitly indicates that the standard read is unsupported.
- `NegativeResponse`: another valid UDS negative response was observed and its NRC is preserved.
- `Unavailable`: the read is unavailable in the current session/context. OBDentic does not escalate the session or request security access.
- `Malformed`: framing, responder selection or DID echo is inconsistent with the requested read.
- `Timeout`: the transport timed out.
- `TransportError`: another transport/session failure occurred.
- `NotProbed`: a candidate was intentionally not sent, for example after the single-owner session became unhealthy.

A timeout is never rewritten as `Unsupported`.

## Evidence and persistence

The private vehicle cache retains per observation:

- exact request target and expected responder
- semantic and canonical Knowledge definition/version
- pinned Knowledge repository/revision
- normalized request bytes
- normalized responder payload evidence
- result state
- NRC when present
- deterministic value bytes when a positive response is valid
- explicit parser/transport errors

The cache format is private and versioned. ECU-identification observations are inventory evidence and do not change the existing bounded Mode-01 cache validation signature.

ECU serial numbers and other unique identification values remain private observed evidence. They are not promoted automatically into public captures or canonical Knowledge.

## Safety invariants

Discovery never performs or falls back to:

- DiagnosticSessionControl
- SecurityAccess
- coding or adaptation
- basic settings or actuator tests
- DTC clear
- RoutineControl / forced regeneration
- raw CAN / UDS / ELM injection
- ECU-address or DID-range scanning

Every candidate is re-authorized as `Activity::Diagnose` through `SafetyPolicy` before the existing one-owner `DiagnosticSession` actor executes it.

## Hardware acceptance

Offline acceptance and hardware acceptance are separate.

On owned hardware, acceptance should verify at least one safely targeted ECU in the default session, record which canonical standard IDs respond, preserve normalized evidence and NRCs, confirm that no session/security/write operation is transmitted, and verify that a normal read/capture still works immediately afterwards.
