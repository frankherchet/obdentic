# 0001. Manufacturer-specific odometer source for VW EA189-era vehicles
Date: 2026-10-23
Status: accepted

## Context
The Android app reads the odometer only through standard Mode 01 PID 0xA6
(REQ-015). EA189-era VW vehicles (about 2008–2016) are unlikely to advertise it,
so their records show "km-Stand nicht verfügbar". Spike #192 asked whether an
evidence-backed, read-only manufacturer-specific source exists that fits the
safety rules: closed typed target selection without address sweeps or DID scans
(REQ-003.4), no raw command path, session change, SecurityAccess or
RoutineControl (REQ-007.3), and visible provenance without plausibility-based
choice (REQ-011).

The investigation was offline only: the pinned Knowledge DB, the repository code
and the locally extracted Car Scanner 2.1.50 profile archive. The archive stays
private and is research input, not executable configuration. No vehicle traffic
was sent.

Findings:
- The Knowledge DB holds no odometer or instrument-cluster knowledge. ISO 14229
  defines no standard odometer identifier.
- In the archive, 67 VAG profiles carry odometer-like reads. The only candidate
  that covers the EA189 era is a `0x22` ReadDataByIdentifier of DID `2203` on the
  instrument cluster (request `714`, response `77E`), present in 32 profiles,
  including the EA189 profile. Its scaling conflicts between platforms: 2 bytes ×10
  km on PQ35-era profiles, 3 bytes ×1 or ×10 on MQB/MLB. Eight profiles prepend an
  extended diagnostic session (`10 03`); the dedicated PQ35/PQ46 dashboard profile
  does not. No profile uses SecurityAccess or RoutineControl for it.
- Other candidates are copies or later platforms: engine `16A9` requires `10 03`
  and is an ECU-internal copy; engine `10E0`/`295A`/`108C`, gateway `2A0A` and
  transmission values carry inconsistent scaling or are MQB/MLB-only. Many
  "odometer"-named entries are adaptation or event stamps (brake, TPMS, DPF, oil,
  clutch, ECU-installed mileage), so a name-based choice would be wrong.
- The code supports one closed targeted request type per job, all hard-bound to
  the validated engine target `7E0`/`7E8` (`src/elm.rs` `TargetedReadRequest` and
  the EA189 request types). Generic header validation and `ATSH`/`ATCRA` targeting
  exist, but there is no cluster role, no flow-control configuration and no
  evidence that the Carly path reaches `714`.
- The confidence of all manufacturer candidates is COMMUNITY. None is
  hardware-validated.

## Options considered
1. **PID 0xA6 only (status quo).** No new traffic or target. EA189-era records
   stay without an odometer.
2. **One closed, explicitly selected VW cluster job.** Read only DID `2203` once
   from one validated instrument-cluster target `714`/`77E`, in the default
   session, without `10 03`, fallback or retries on other targets. The decoder is
   keyed on the observed payload length, labelled EXPERIMENTAL/COMMUNITY and
   hardware-pending. Negative responses are kept as evidence and end the job.
   Gating mirrors the EA189 jobs. The value is stored with source and ECU, never
   merged with PID 0xA6 or corrected.
3. **Several manufacturer sources** (cluster, gateway, engine) with selection
   between them. Rejected: this requires plausibility-based choice and session
   changes, and adds MQB/MLB-only identifiers with contradictory scaling.
4. **A generic "read DID from header" capability.** Rejected: it is a raw command
   path and violates REQ-007.3.

## Decision
Accepted by the maintainer on 2026-10-23:
- Ship option 1 now: PID 0xA6 stays the only automatic odometer source.
- Allow option 2 only as a separately refined follow-up feature, with these
  conditions:
  - it is user-selected, not part of automatic connection traffic;
  - it reads exactly one DID from one target, without session or security
    services;
  - its result is stored as a separate experimental source;
  - its scaling and default-session behaviour stay unconfirmed until a bundled
    real-vehicle check reads `2203` once on the intended EA189 vehicle and compares
    the value with the instrument display. That check joins the existing hardware
    gate (#178), not a separate trip.

## Consequences
- No new vehicle traffic until the follow-up feature is refined and accepted.
- The follow-up needs:
  - Knowledge first: an instrument-cluster target and DID definition with
    provenance and applicability in the Knowledge DB (schema v2).
  - Core work: a typed, closed cluster target mapping and job kind in the safety
    policy, mirroring the EA189 DPF job.
  - Replay: raw evidence and offline re-decode.
- If the vehicle check shows that the cluster refuses `2203` in the default
  session, the decision falls back to option 1. Escalating to `10 03` or
  SecurityAccess requires a new human decision.
- Newer VW platforms (MQB/MLB) and gateway-protected vehicles stay out of scope.
- Mileage remains private vehicle evidence; captures and the profile archive are
  never committed or published.
