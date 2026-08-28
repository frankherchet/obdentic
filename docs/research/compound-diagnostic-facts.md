# Compound diagnostic facts

Status: domain-shape decision (2026-08-28). No compound PID decoder is enabled by
this note.

The existing `Transaction` and `TelemetryState` intentionally remain a
one-request-to-one-scalar path. They are not a safe place to force a bitfield or
a multi-value response into an arbitrary number.

When independently evidenced Vehicle Knowledge decodes a compound response, it
uses one `CompoundObservation`:

```text
one source-scoped raw-evidence identity
  -> one timestamp
  -> zero or more uniquely named SemanticFact values
```

`ObservationIdentity` carries the source and an opaque `evidence_id`; the
capture/audit stream remains the sole owner of raw TX/RX bytes. This avoids
duplicating sensitive evidence while letting every derived fact point to the
same responder/request/response observation. A fact is either a finite scalar
with an explicit unit or a named bit status.

The model is transport-neutral: it contains no PID, service, CAN target, BLE,
or scheduler API. It performs no reads. It also permits zero facts for a valid
response whose deterministic decoder finds no applicable facts.

Concrete Mode 01 PIDs such as `01 13`, `01 24`, and `01 4F` remain deferred.
Each needs an independently verified response layout, synthetic fixture, and
owned-hardware evidence before it can create these facts. The clean-room rule
in `obdium-standard-obd-gap-analysis.md` continues to apply.
