# OBDium vs OBDentic: clean-room standard OBD gap analysis

Status: research/reference only (2026-08-28).

No OBDium source code, databases, request recordings, fixtures, PID tables, or other
GPL-covered implementation material are copied into OBDentic as a result of this
review.

## Sources and evidence levels

External implementation reference:

- OBDium repository: https://github.com/provrb/obdium
- pinned revision reviewed: `9dc97f36567cdbc3abb4e331882121d071976550`
- license: GPL-3.0
- evidence class in OBDentic: `COMMUNITY / EXTERNAL_PROJECT`

Independent public standard cross-checks used for naming/scaling direction:

- CSS Electronics OBD2/J1979 PID overview:
  https://www.csselectronics.com/pages/obd2-pid-table-on-board-diagnostics-j1979
- Wikipedia OBD-II PID overview, used only as a secondary public reference:
  https://en.wikipedia.org/wiki/OBD-II_PIDs

Owned-hardware support evidence used for prioritization comes from the previously
reviewed Touran Mode 01 support-page observations. The raw vehicle captures are not
committed to this repository, so this evidence is useful for local prioritization
but must not be treated as a public normative source.

The existing communication review remains in:

- `docs/research/obdium-communication-reference.md`

## Clean-room rule

OBDium may tell us which questions are worth asking and which protocol shapes another
implementation encountered. OBDentic still derives every production definition from
independent protocol references and its own evidence.

```text
OBDium observation
  -> independent standards/public verification
  -> OBDentic-owned fixture or hardware evidence
  -> independent implementation
  -> hardware validation
  -> provenance/confidence promotion
```

Do not copy OBDium formulas merely because a decoded value looks plausible.

## Current OBDentic generic scalar catalog

At OBDentic revision `c164d76f43bdc3f2c9b6555418b64017a0f4a2c9`, `obd2-v1`
contains 15 scalar Mode 01 definitions:

```text
04  calculated engine load
05  coolant temperature
0B  intake manifold absolute pressure
0C  engine RPM
0D  vehicle speed
0F  intake air temperature
10  mass air flow
1F  engine runtime
2C  commanded EGR
2D  EGR error
33  barometric pressure
42  control module voltage
45  relative throttle position
49  accelerator pedal position D
4A  accelerator pedal position E
```

This set deliberately models only one semantic scalar per request.

## What OBDium adds as useful research leads

The reviewed OBDium PID modules cover a materially larger standard Mode 01 surface.
The useful observation is not that every item should be added, but that the following
families deserve explicit OBDentic decisions.

### Simple scalar/status candidates below PID 0x60

OBDium contains implementations or direct reads for examples including:

```text
03  fuel system status
06-09 fuel trims
0A  fuel pressure
0E  timing advance
11  throttle position
12  commanded secondary air status
1C  OBD standard
1E  auxiliary input status
21  distance traveled with MIL on
22  fuel rail pressure relative to manifold vacuum
23  fuel rail gauge pressure
2E  commanded EVAP purge
2F  fuel tank level
30  warm-ups since DTCs cleared
31  distance since DTCs cleared
32  EVAP vapor pressure
3C-3F catalyst temperatures
46  ambient air temperature
47-48 absolute throttle positions B/C
4B  accelerator pedal position F
4D  time with MIL on
4E  time since DTCs cleared
50  maximum MAF rate
51  fuel type
52  ethanol percentage
5C  engine oil temperature
5D  fuel injection timing
```

Each remains only a research lead until OBDentic independently verifies request,
length, signedness, scaling and unit.

### Compound / multi-fact Mode 01 responses

OBDium also exposes requests that return multiple facts or bitfields:

```text
01  monitor status since DTCs cleared
13  oxygen sensors present
14-1B narrow-band oxygen sensor voltage + fuel trim
24-2B wide-range oxygen sensor equivalence ratio + voltage
41  monitor status this drive cycle (standard reference; not implemented in reviewed OBDium code)
4F  maximum equivalence ratio / O2 voltage / O2 current / MAP
```

These are architecturally important because the current OBDentic `Transaction`
shape assumes one request -> one scalar value. They should not be flattened into an
arbitrary scalar simply to fit the current type.

Preferred direction:

```text
one diagnostic observation
  -> one request/responder/raw evidence identity
  -> zero or more deterministic decoded facts
```

See #53.

### Later standard diesel/torque PIDs

OBDium references later standard PIDs such as:

```text
61-64 torque data
66    multiple MAF sensors
67    multiple coolant sensors
74    turbocharger RPM
7F    engine runtime family
9D    engine fuel rate
A2    cylinder fuel rate
A6    odometer
```

The public PID table also lists standardized diesel-oriented families in the
`69-7C` range, including EGR/intake-air control, boost/VGT/exhaust pressure,
turbocharger data, EGT and DPF data.

These are interesting for the long-term EA189/generic-diesel roadmap, but support
must be discovered page-by-page. Do not probe them merely because they are standard.

## Target-vehicle support cross-check

Previously reviewed owned-hardware support discovery produced:

```text
01 00 -> 98 3B A0 13
01 20 -> B0 19 A0 01
01 40 -> CC D2 00 00
```

Decoded as support evidence, these advertise:

```text
01-20:
01 04 05 0B 0C 0D 0F 10 11 13 1C 1F 20

21-40:
21 23 24 2C 2D 30 31 33 40

41-60:
41 42 45 46 49 4A 4C 4F
```

The final bit for PID 60 is not set, so this evidence does not advertise a `61-80`
continuation page. Therefore, for this vehicle, generic PIDs above `0x60` should not
be queried unless new support evidence changes that conclusion.

A useful result of this comparison is that all 15 current OBDentic scalar signals are
within the advertised set. The missing advertised PIDs are:

```text
01 11 13 1C 21 23 24 30 31 41 46 4C 4F
```

They divide cleanly into three classes.

### Class A: straightforward missing scalar reads

```text
11  throttle position
21  distance traveled with MIL on
23  fuel rail gauge pressure
30  warm-ups since DTCs cleared
31  distance since DTCs cleared
46  ambient air temperature
4C  commanded throttle actuator
```

These are the first implementation tranche in #49.

### Class B: diagnostic/status facts

```text
01  monitor status since DTCs cleared
1C  OBD standard / conformance identifier
41  monitor status this drive cycle
```

PID 01/41 belong naturally in bounded diagnostic/readiness jobs rather than high-rate
scalar telemetry. See #50.

PID 1C is vehicle/ECU information. It may be exposed as a stable read-only fact, but
it does not need engine-baseline polling.

### Class C: compound capability/sensor facts

```text
13  oxygen sensors present
24  oxygen sensor 1 equivalence ratio + voltage
4F  maximum equivalence ratio / O2 voltage / O2 current / MAP
```

These need a reviewed multi-fact result model before implementation. See #53.

## Diagnostic-service gap

OBDium also provides useful behavioral leads outside Mode 01 telemetry:

```text
03     stored emission-related DTCs
0A     permanent DTCs
01 02  freeze-frame DTC-related observation in OBDium's implementation
06     monitor test results (OBDium only has an incomplete/raw stub)
04     DTC clear (mutating; negative reference only)
```

OBDentic decisions:

- `03` is the Milestone 9 `dtc.scan` proof job (#46).
- `0A` and freeze-frame work are follow-ups (#51).
- Mode 06 requires a research gate before implementation (#52).
- Mode 04 remains forbidden and must never reach transport.

## Important negative findings from the OBDium review

The value of an external implementation is partly in showing shortcuts OBDentic
should not inherit.

### Do not merge responder payloads before decode

OBDium often extracts responder labels and later works on combined/stripped payload
text. OBDentic must instead keep responder -> payload association structurally all
the way through decoding and capture.

### Do not use PID 01 DTC count to suppress Mode 03

OBDium's stored-DTC helper can skip Mode 03 if its PID 01-derived count is zero. In a
multi-responder system, one responder's count must not suppress evidence from another
responder. OBDentic `dtc.scan` should execute its bounded Mode 03 step directly.

### Do not adopt OBDium's compression-engine runtime shortcut

The reviewed OBDium code selects a different PID (`7F`) for its compression-engine
runtime helper, while standard PID `1F` already represents runtime since engine
start and PID `7F` belongs to a later, more complex standard family. OBDentic should
not replace `1F` based on engine type without independent protocol evidence.

### Verify signed quantities independently

At least one OBDium EVAP vapor-pressure implementation treats the raw pair as an
unsigned value, while public standard PID references describe signed semantics for
that PID. This is exactly why OBDium formulas are research leads, not source data.

### Do not inherit arbitrary command seams

OBDium supports arbitrary ELM/diagnostic messages. OBDentic must retain the closed
semantic request/job -> safety policy -> session path.

## Prioritized backlog

The resulting small backlog is intentionally separated from Milestone 9 so the
runtime-state/DiagnosticJob milestone does not expand uncontrollably.

```text
P0  #46  Finish first responder-aware read-only dtc.scan job (Mode 03)

P1  #49  Add simple target-supported generic Mode 01 scalar facts
P1  #50  Add readiness/monitor-status diagnostic jobs (PID 01/41)

P2  #51  Add permanent-DTC and freeze-frame jobs
P2  #53  Define compound/multi-fact standard read result model

P3  #52  Research Mode 06 before any implementation
```

The order is deliberate: add cheap scalar facts first; prove structured diagnostic
jobs; then expand into compound responses and less trivial standard diagnostics.

## Acceptance rule for any item promoted from this research

A candidate becomes OBDentic Vehicle/Protocol Knowledge only when all of the following
are true:

- exact read operation is independently documented
- response service/PID/length validation is known
- signedness/endian/scaling/unit are independently documented
- semantic name and subsystem are stable
- provenance/confidence are explicit
- support discovery constrains live use where applicable
- responder evidence is preserved
- independent fixtures cover malformed/boundary cases
- no mutating/raw-command path is introduced
- hardware validation is recorded separately from specification confidence

Until then, it remains a research lead, even if OBDium displays a plausible value.