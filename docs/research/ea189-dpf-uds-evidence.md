# EA189 DPF UDS evidence research

This note supports issue #14. It separates normative UDS protocol facts from VW/EA189-specific Vehicle Knowledge and records candidate DPF data identifiers that still require owned-hardware validation before promotion into production profile knowledge.

## Research rule

OBDentic keeps these evidence classes separate:

```text
Protocol Knowledge
  ISO 14229 / ISO 15765
  -> what a UDS request/response means structurally

Vehicle Knowledge
  VW ECU-specific DID + type + scale + unit + meaning
  -> must be independently evidenced and hardware-gated

Diagnostic Knowledge
  relationships, plausibility, regeneration interpretation
  -> must not be silently promoted into a deterministic decoder
```

A standards-compliant `ReadDataByIdentifier` transaction proves neither the OEM meaning nor the scaling of a DID. A plausible decoded value is also not sufficient evidence by itself.

## Normative protocol references

Current published standards relevant to the first EA189 UDS-on-CAN read slice:

- ISO 14229-1:2026, *Road vehicles — Unified diagnostic services (UDS) — Part 1: Application layer*, Edition 4, published 2026-06: https://www.iso.org/standard/87962.html
- ISO 14229-2:2021, *Part 2: Session layer services*, Edition 2: https://www.iso.org/standard/77322.html
- ISO 14229-3:2022, *Part 3: Unified diagnostic services on CAN implementation (UDSonCAN)*, Edition 2: https://www.iso.org/standard/77323.html
- ISO 15765-2:2024, *Diagnostic communication over Controller Area Network (DoCAN) — Part 2: Transport protocol and network layer services*, Edition 4: https://www.iso.org/standard/84211.html

Useful public explanatory material supplied during research:

- UDS overview with the OSI split between ISO 14229-1/-2, UDSonCAN, and ISO 15765-2: https://nvdungx.github.io/unified-diagnostic-protocol-overview/
- Public preview of the historical ISO 14229-1:2013 edition: https://cdn.standards.iteh.ai/samples/55283/fe6f5aa0c13f45048501fd86060853e3/ISO-14229-1-2013.pdf

The 2013 preview is useful historical/explanatory material but is not the current normative edition.

For #14 the relevant protocol-level shape is conceptually:

```text
request:  22 <DID_hi> <DID_lo>
positive: 62 <DID_hi> <DID_lo> <data...>
negative: 7F 22 <NRC>
```

Transport segmentation/reassembly on CAN is a different concern from the UDS application message and belongs to DoCAN/ISO-TP (ISO 15765-2).

OBDentic already has a closed `UdsReadDataByIdentifier` response validator that checks positive service `0x62`, echoed DID, malformed/truncated replies, and `0x7f 0x22 <NRC>`. It is intentionally not a generic arbitrary live-send API.

## External implementation sources

These sources are research/corroboration inputs only. Do not copy their source code, comments, tables, request databases, or fixtures into OBDentic. Re-derive OBDentic definitions independently and validate them against owned hardware.

### OBDium

Repository: https://github.com/provrb/obdium

Pinned research revision previously reviewed by OBDentic: `9dc97f36567cdbc3abb4e331882121d071976550`.

For #14, OBDium is mainly a negative/reference source. Its generic exhaust module contains standard EGR/catalyst functions, while exhaust-gas temperature remains unimplemented there; searches at the pinned revision did not yield the candidate VW DPF DIDs below. OBDium therefore remains useful for ELM/protocol behavior, not as a primary EA189 DPF Vehicle Knowledge donor.

License note: GPL-3.0. Treat as reference/test-shape material only.

### v-cu/dpf-load-monitor-wide

Repository: https://github.com/v-cu/dpf-load-monitor-wide

The project describes itself as a VAG UDS-over-CAN DPF monitor and states that DID availability can vary with ECU/software. Its current firmware sends physical UDS `0x22` requests to CAN ID `0x7E0` and contains DPF-oriented identifiers including `114E`, `114F`, `1156`, `115E`, `11B2`, `10F9`, `14F5`, and `1153`.

Important for OBDentic: this repository is licensed CC BY-NC-SA 4.0. It must not be used as a code/data donor. It is suitable only as a corroborating research source; all semantics, scaling and fixtures must be independently derived and hardware-validated.

### blizniukp/WIFI_kit_32_dpf

Repository: https://github.com/blizniukp/WIFI_kit_32_dpf

MIT-licensed project built around an ELM-style adapter. Its documented measurement set independently repeats several candidate DIDs: `114E`, `114F`, `1156`, `115E`, `11B2`, `10F9`, plus one ash candidate `178C`. The README says the complete set was tested on an Audi A4 B8 2.0 TDI CAGA; other listed vehicle rows are explicitly marked as untested/expected where appropriate.

Safety warning: its initialization includes a `10031` command after selecting header `7E0`. OBDentic must not copy this behavior. `0x10 DiagnosticSessionControl` remains outside the read-only safety boundary; candidate #14 reads must first be tested in the existing/default diagnostic context only.

### yangosoft/dpf

Repository: https://github.com/yangosoft/dpf

Older ELM-based project. It independently uses `22114F` for calculated soot and `221156` for distance since regeneration with physical header `7E0`. This is useful additional corroboration for those two identifiers but not sufficient by itself for promotion to VERIFIED knowledge.

## Candidate DID matrix

Status below is research confidence, not OBDentic hardware-validation status.

| Candidate semantic | DID | Observed independent source pattern | Research assessment | Required before production |
| --- | --- | --- | --- | --- |
| measured soot mass | `114E` | v-cu + blizniukp | strong candidate | raw owned-vehicle response; signedness/scale confirmation across states |
| calculated soot mass | `114F` | v-cu + blizniukp + yangosoft | strongest first candidate | raw owned-vehicle response; scale/unit confirmation |
| distance since regeneration | `1156` | v-cu + blizniukp + yangosoft | strong DID candidate | exact response width and `/1000` interpretation from raw evidence |
| time since regeneration | `115E` | v-cu + blizniukp | strong DID candidate | exact response width, canonical unit and `/60` interpretation from raw evidence |
| DPF-related inlet temperature candidate | `11B2` | v-cu + blizniukp | good candidate | raw response and physical sensor-position mapping |
| DPF-related outlet temperature candidate | `10F9` | v-cu + blizniukp | good candidate | raw response and physical sensor-position mapping |
| DPF differential pressure candidate | `14F5` | v-cu | promising but weakly corroborated | raw response, signedness, scale and engineering unit before decoder admission |
| oil ash candidate | `1153` | v-cu | conflicting family | identify ECU/software applicability and unit/scale |
| oil ash candidate | `178C` | blizniukp | conflicting family | identify ECU/software applicability and unit/scale |

Do not globally alias the two ash candidates. They may reflect different ECU/software definitions.

## Physical semantics corroborated by VCDS logs

Public Ross-Tech/VCDS logs are useful for the *meaning and engineering units* of DPF facts, but they do not reveal the UDS DID mapping by themselves.

Examples:

- https://forums.ross-tech.com/index.php?threads/6802/ records `DIP_PF` in hPa, `DIST_RGN`, calculated soot in g and measured soot in g. The measured soot value is negative in that real log (`-2.76 g`).
- https://forums.ross-tech.com/index.php?threads/18864/ records `DIP_PF` in hPa, `T_WOUT_RGN` in seconds, `MASS_ASH_PF` in g, calculated/measured soot in g and distance since regeneration in km for a `03L` engine controller.

This matters for decoder design:

- negative measured-soot values are physically/ECU-semantically observable and must not be silently clamped to zero;
- differential pressure is exposed by VCDS as hPa in these examples, but that does **not** prove that raw DID `14F5` is an unscaled hPa `u16`;
- time-since-regeneration appears as seconds in VCDS, while community implementations often divide an underlying counter by 60 to display minutes. OBDentic must decide its canonical unit only after raw response layout is confirmed.

## Candidate decoder hypotheses

These are hypotheses to test, not yet VERIFIED profile definitions.

### `114F` calculated soot

Multiple implementations interpret a big-endian 16-bit quantity with `/100` scaling and present grams.

Hardware gate:

1. capture raw `62 11 4F ...` response;
2. independently decode BE16 `/100`;
3. compare against expected range and, where possible, a trusted diagnostic-tool reading near the same time;
4. repeat across more than one operating state.

### `114E` measured soot

Evidence strongly suggests a signed big-endian 16-bit quantity with `/100` scaling in grams. The signed interpretation is important because VCDS logs demonstrate legitimate negative measured soot values.

Do not clamp negative values. A sentinel, if any, must be separately evidenced rather than inferred from sign.

### `1156` distance since regeneration

Community implementations agree on `/1000`, but disagree in code shape about the number of source bytes consumed. OBDentic should not choose 24-bit vs 32-bit from implementation folklore. The raw response length from the owned ECU must decide the concrete decoder.

### `115E` time since regeneration

Community implementations commonly divide an underlying counter by 60 to display minutes. VCDS exposes the semantic time value in seconds. Preserve raw timing evidence and choose one explicit canonical OBDentic unit only after the owned response width/scaling is confirmed.

### `11B2` and `10F9` temperatures

Two independent implementations use a Kelvin-like integer transform equivalent to `(raw - 2731) / 10` °C. This is a strong hypothesis, but the mapping of each DID to exact physical sensor position (for example pre-DPF vs post-DPF) still requires vehicle-specific corroboration.

### `14F5` differential pressure

Do not implement a production decoder yet. The DID is a useful experimental probe candidate, but current research does not independently establish raw type, sign, scale and unit with sufficient confidence.

### Ash

Do not implement a single global EA189 ash semantic from current research. `1153` and `178C` conflict across working projects, and VCDS/ASAM naming/units vary across ECU software. Bind any future ash definition to concrete ECU/software evidence.

## Regeneration state

No sufficiently evidenced direct DID for a deterministic `dpf.regeneration_active` fact was found in this research.

Some external monitors infer regeneration from post-injection, temperature, soot trend or last-regeneration counters. That is Diagnostic Knowledge, not a deterministic vehicle fact, unless a direct ECU state signal is separately identified and evidenced.

Therefore the initial #14 slice should omit a boolean `regeneration_active` rather than encode a heuristic as VERIFIED Vehicle Knowledge.

## Safety and session policy

The hardware research gate must remain strictly read-only:

```text
validated physical engine target
  -> default/current diagnostic context
  -> exact allowlisted 0x22 DID reads only
  -> preserve positive response or NRC as evidence
  -> no fallback session change
```

Explicitly forbidden during this research:

- `0x10 DiagnosticSessionControl`, including `10 03`
- `0x27 SecurityAccess`
- `0x2E WriteDataByIdentifier`
- `0x31 RoutineControl`
- DTC clear
- forced regeneration
- adaptation/reset/coding
- DID sweeps or caller-supplied arbitrary UDS bytes

If a candidate returns `7F 22 xx` in the default context, capture the NRC and stop for that DID. Do not automatically change session to make the read succeed.

## Proposed owned-hardware evidence gate

First tranche, in this order:

```text
22 114F  calculated soot candidate
22 114E  measured soot candidate
22 1156  distance since regeneration candidate
22 115E  time since regeneration candidate
22 11B2  inlet-temperature candidate
22 10F9  outlet-temperature candidate
22 14F5  differential-pressure experimental candidate
```

Ash is intentionally deferred until the ECU/software-specific mapping is better constrained.

Capture every transaction as raw evidence before decoding. Suggested operating states:

1. ignition on / engine off;
2. warm idle;
3. controlled elevated RPM or normal driving load;
4. if naturally encountered, a later capture spanning a normal regeneration, without requesting or forcing one.

Expected validation properties rather than hard-coded verdicts:

- differential pressure should be near its offset/low-flow behavior engine-off and change with exhaust flow;
- inlet/outlet temperatures should move physically coherently but must not be used to guess DID identity by plausibility alone;
- distance/time counters should be monotonic between regenerations and reset/change only when the ECU reports such a transition;
- calculated and measured soot remain distinct facts and may differ significantly;
- a negative measured-soot value remains a valid decoded candidate unless a separately evidenced sentinel rule applies.

## Promotion rule

A candidate becomes production Vehicle Knowledge only after OBDentic can record:

```text
semantic
+ exact physical ECU target
+ 0x22 DID
+ positive-response layout
+ byte width/endian/signedness
+ scale/offset
+ engineering unit
+ sentinel/error behavior
+ provenance
+ owned-hardware fixture
+ hardware-validation state
```

Non-selected/raw evidence must remain preserved so a later decoder can re-evaluate the same capture.

## Current OBDentic implementation seam

At the time of this note:

- the EA189 profile is evidence-gated and empty-by-default for manufacturer-specific requests;
- its current binding helper admits only existing generic OBD-II semantics with validated physical engine target evidence;
- the protocol core already models/validates constrained UDS `ReadDataByIdentifier` responses but deliberately does not expose arbitrary UDS sending.

Therefore #14 should extend the closed Vehicle Knowledge representation with explicitly defined, profiled UDS DID reads rather than add any raw `uds.send`/byte-array escape hatch.

## Research conclusion

The best first hardware candidates are `114F` and `114E`, followed by `1156`/`115E` and the two temperature candidates. `14F5` should remain EXPERIMENTAL until scaling is proven. Ash is ECU/software-sensitive and should be deferred. A direct regeneration-state DID is still unresolved.

The ISO material is normative for service framing and errors; it does not establish VW DID meanings. Community/open-source sources are corroboration only. Owned raw captures are the final promotion gate.