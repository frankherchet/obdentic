# OBDium communication reference: clean-room evidence review

Status: research/reference only (2026-08-28). No OBDium source code, data tables,
fixtures, databases, or other GPL-covered implementation material are copied into
OBDentic as a result of this document.

## Purpose

Use the public OBDium implementation as an external behavioral reference to reduce
research time around ELM327 communication, standard OBD diagnostic jobs, responder
handling, and ISO-TP-shaped responses, while preserving OBDentic's independent
architecture, deterministic behavior, provenance model, and strict read-only safety
boundary.

Source reviewed:

- repository: https://github.com/provrb/obdium
- pinned revision: `9dc97f36567cdbc3abb4e331882121d071976550`
- license: GPL-3.0

The source is treated as `COMMUNITY / EXTERNAL_PROJECT` evidence. It can suggest
questions, expected wire behavior, edge cases, and tests. It is not sufficient by
itself to mark proprietary knowledge as verified.

## Clean-room rule

Allowed use:

- observe which documented OBD/ELM operations another implementation uses
- identify protocol edge cases worth testing
- compare request/response shapes against standards and OBDentic hardware evidence
- derive independent test requirements
- independently implement behavior from protocol specifications and our own captures

Not allowed:

- copy implementation code
- copy GPL data tables, SQLite databases, request recordings, or PID datasets
- copy proprietary Mode 22 definitions as OBDentic Vehicle Knowledge
- expose OBDium's arbitrary-command capability through OBDentic

For a useful external observation, prefer this promotion path:

```text
OBDium observation
  -> normative/public protocol verification where available
  -> independent OBDentic fixture or owned-hardware evidence
  -> independent implementation
  -> hardware validation
  -> provenance/confidence promotion
```

## Findings

### 1. ELM initialization keeps response headers visible

OBDium's serial ELM initialization sends, in order:

```text
ATZ
ATE0
ATL0
ATH1
```

and then configures the selected protocol with `ATSP0` through `ATSP9`.

This is useful behavioral corroboration for OBDentic's decision to retain ELM
headers while collecting responder-aware evidence. OBDium also treats `>` as the
command terminator and handles textual adapter states such as `SEARCHING...`,
`NO DATA`, and `UNABLE TO CONNECT`.

OBDentic implication: keep header visibility as transport evidence, but continue to
normalize it into typed responder identities rather than exposing ELM text to
Vehicle Knowledge or semantic consumers.

### 2. OBDium explicitly encounters multiple OBD responders

OBDium extracts three-character header tokens from response lines and stores a list
of responding ECUs. Its supported-PID discovery returns a map keyed by ECU header,
with a separate supported-PID list for each responder.

This independently supports OBDentic's existing model:

```text
functional OBD request
  -> responder A capability evidence
  -> responder B capability evidence

not

functional OBD request
  -> one merged vehicle capability bitmap
```

Important limitation: OBDium's extraction assumes a three-character header and its
response association is positional. OBDentic must retain its more explicit,
transport-neutral responder model and must not generalize `3 hex chars == ECU` to
all transports/addressing modes.

### 3. Supported-PID continuation pages are a useful reference, but not a parser to copy

For Mode 01 OBDium requests support pages:

```text
01 00
01 20
01 40
01 60
01 80
01 A0
01 C0
```

and calculates supported PIDs independently for each observed responder.

That page progression is useful as a comparison point for OBDentic's bounded support
discovery. However, OBDium's generic `get_service_supported_pids(service)` parser
constructs a `41 <page>` positive-response split even when the caller selects a
service other than Mode 01. Therefore the implementation should be treated as
Mode-01-specific behavioral evidence, not as a generic service-support algorithm.

OBDentic implication: keep service-specific positive-response validation typed and
explicit. Do not generalize a `41` parser across Mode 05/09/etc.

### 4. The VIN path provides an ISO-TP-shaped real-world reference

OBDium reads VIN with standard Mode 09 PID 02 (`0902`) and documents/handles an
example shaped like:

```text
7E8 10 14 49 02 01 ...
7E8 21 ...
7E8 22 ...
```

It distinguishes ISO-TP single, first, and consecutive frame PCI nibbles before
building the VIN payload.

This is useful confirmation that ELM header-visible Mode 09 VIN traffic may arrive
as ordinary ISO-TP first/consecutive frames.

Important limitations for OBDentic:

- OBDium strips responder headers before its reassembly helper.
- Frames are not grouped/reassembled independently per responder.
- The helper does not provide a general responder-aware sequence/declared-length
  validation model.
- Its extraction is tailored to the calling VIN/DTC routines rather than producing
  one canonical normalized diagnostic message abstraction.

OBDentic implication: use the observed frame shapes as research/test inspiration,
but keep ISO-TP normalization responder-aware and deterministic. Never concatenate
frames from different responders into one semantic payload.

### 5. Standard diagnostic operations suggest a useful DiagnosticJob sequence

OBDium implements or references these standard read-side operations:

```text
01 01  monitor status / MIL / DTC count / readiness bits
01 02  DTC that caused the freeze frame
03     stored emission-related DTCs
06     monitor/test results (currently incomplete in OBDium)
0A     permanent DTCs
```

It also implements Mode 04 DTC clearing. Mode 04 is a useful negative reference for
OBDentic: it must remain impossible through the current read-only core.

For Milestone 9, the most useful first slice remains:

```text
dtc.scan
  -> Mode 03 only
```

with permanent/pending/freeze-frame/readiness work added only as separately modeled,
closed read-only jobs or job steps.

### 6. Do not gate a vehicle-wide DTC scan on one Mode 01 PID 01 count

OBDium's stored-DTC helper first reads PID `0101`, extracts the DTC count, and skips
Mode 03 entirely when that count is zero.

That shortcut is unsuitable for OBDentic's responder-aware model. A functional
request may involve multiple responders, and a count interpreted from one responder
must not suppress a Mode 03 observation that could return evidence from another
responder.

OBDentic rule:

```text
dtc.scan request
  -> execute the bounded Mode 03 read
  -> preserve every responder and payload
  -> decode each responder independently
```

`0101` may later be captured as a separate readiness/monitor-status fact, but it
should not be a prerequisite that decides whether `dtc.scan` reaches the vehicle.

### 7. DTC decoding must remain responder-scoped

OBDium's DTC decoder extracts ECU names, removes those names from the combined text,
and then decodes the remaining Mode 03/0A response content into DTCs.

For OBDentic, this is an example of information that must not be discarded. The
canonical result should retain at least:

```text
DTC observation
  responder/source
  request/job step
  normalized response payload
  decoded standardized DTC code(s)
  decode error if malformed
```

Two responders returning the same DTC are two source observations, not one anonymous
code. Two different responder payloads must never be concatenated before decoding.

### 8. Replay is useful prior art but intentionally non-deterministic

OBDium records request text, request type, and raw response into `requests.json` and
can replay those requests in demo mode. When multiple recorded responses match a
request, its replay implementation chooses one randomly.

That behavior is appropriate for simulation variety but is a negative reference for
OBDentic's evidence/replay contract.

OBDentic invariant remains:

```text
same persisted evidence
+ same knowledge/reducer version
+ same explicit event sequence
= same normalized/replayed result
```

No random response selection is permitted in deterministic replay or diagnostic-job
tests.

### 9. Arbitrary commands and dynamic Mode 22 formulas are explicitly not an OBDentic pattern

OBDium exposes an `Arbitrary` command type, an arbitrary-message constructor, and an
interactive path capable of sending user-entered ELM/diagnostic text. It also uses
arbitrary messages for Mode 22 values and supports database-provided dynamic
formulas.

OBDentic must not adopt this seam. Its safe direction remains:

```text
semantic operation/job
  -> closed protocol/Vehicle Knowledge definition
  -> typed target/routing decision
  -> safety policy
  -> DiagnosticSession
```

never:

```text
CLI / TUI / MCP / data file
  -> arbitrary bytes or ELM text
  -> vehicle
```

Any proprietary Mode 22 observation found in OBDium is only a research lead until
independently sourced, profiled, provenance-tagged, and validated on owned hardware.

## Actionable OBDentic guidance

### Milestone 9 / `dtc.scan`

Use OBDium only to accelerate the list of cases worth covering. The independent
OBDentic implementation should specifically test:

- Mode 03 with zero DTCs
- one and multiple two-byte DTC entries
- two functional responders with independent DTC sets
- identical DTC from two responders remains two source observations
- malformed/truncated payload for one responder does not erase another responder's
  valid result
- `NO DATA` is represented as an observation/result state, not a transport death
- fatal transport/session error stops remaining job work deterministically
- Mode 04 cannot be constructed through the public job/safety API
- `0101` count is not required before Mode 03
- runtime activity remains `ready.diagnose` for every internal job step

### Transport / ELM regression cases

Maintain independent tests for:

- header-on response lines such as `7E8 <len> <payload...>`
- more than one responder in one functional exchange
- `SEARCHING...` prefix
- `NO DATA`
- `UNABLE TO CONNECT`
- prompt termination
- header/parser behavior that does not assume every responder is a three-character
  11-bit CAN identifier

### ISO-TP regression cases

Maintain or add independent fixtures for:

- single frame
- first frame + consecutive frames
- sequence mismatch
- truncated declared length
- interleaved frames from two responders
- duplicate responder frame
- responder disappears mid-message

Only complete, correctly reassembled responder-scoped messages may enter semantic
VIN/DTC decoding.

## Things worth revisiting later

OBDium can remain a useful external comparison source for:

- standard Mode 01 coverage not yet present in OBDentic
- readiness status decoding
- permanent DTC read (`0A`)
- freeze-frame-related standard reads
- Mode 06 monitor-result research
- serial ELM backend behavior when OBDentic adds non-BLE transports

Each item still requires standards verification and independent OBDentic tests before
implementation.

## Decision

OBDium is approved as a behavioral/research reference only. It is particularly
useful for identifying standard OBD communication paths and regression cases, but
its arbitrary-command seam, responder-flattening helpers, random replay, and
write-capable DTC clearing are deliberately not architecture patterns for OBDentic.

The strongest immediate acceleration is for Milestone 9 `dtc.scan`: implement a
bounded Mode 03 job directly, preserve responder identity throughout, decode per
responder, and do not gate the scan on a prior `0101` DTC count.
