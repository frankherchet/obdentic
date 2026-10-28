# 0002. ECU request targets in the Knowledge DB (VAG first)
Date: 2026-10-28
Status: accepted

## Context
Spike #238 asks how to model manufacturer ECU request targets (request and
response CAN id, addressing, logical role, applicability, provenance,
confidence, hardware validation) so OBDentic can run a bounded, evidence-backed
identification over known candidates instead of blind address sweeps
(REQ-003.4). Related: epic #22, #85 (bounded standard identification per known
target), REQ-002 (canonical knowledge vs private observations), REQ-003.1
(installed, reachable, role and request target stay distinct) and REQ-011
(provenance, no plausibility guessing).

State of the code and Knowledge DB (schema v2, pinned):
- The Knowledge DB holds standard identification DIDs only
  (`uds.standard.ecu_identification`, 14 members, F190 excluded). It has no target,
  role or manufacturer section.
- The core already has `RequestTarget`/`RequestTargetEvidence` (`topology.rs`),
  `EcuRole` (`Engine | Transmission | Gateway | Unknown | VendorSpecific`),
  `TargetMappingSnapshot` and `discover_known_ecus` (`ecu_identification_discovery.rs`),
  which runs the canonical plan per mapping, but only for mappings with
  `Confidence::High|Verified`, a concrete address and a responder. Targets come
  from evidence, never from knowledge. `configure_target` (`elm.rs`) issues `ATSH`
  and `ATCRA` only, and `validate_request_target` accepts 3-hex-digit headers in
  physical OBD-II context only.
- `FunctionalEcuSerialProbe` shows the pattern for one closed probe request.

Input: the private local Car Scanner 2.1.50 archive (never published; only
counts and CAN identifiers are cited). Of 1304 profiles, 68 carry a VAG-group
brand (65 with sensor entries; the issue text and the archive README give 90
and 67 under other selection rules, see open questions). Findings, all offline:
- 31 distinct request headers appear in these profiles. 22 are 11-bit
  `7xx` ids; 6 are 29-bit `FC00xx` (request `17FC00xx`, filter `17FE00xx`, tester
  `DB33F1`), plus one empty header, one `000` (KWP-style service, no 22) and one
  `8210F1` (KWP2000).
- Response ids: 19 of the 22 11-bit targets carry an explicit `ATCRA` filter in
  the data. Each is request +0x6A (VAG convention) except `7E6`->`7EE` (+8). `7E0`,
  `7E1` and `7E5` carry none and follow ISO 15765 +8 by convention. The
  derivation was checked per target; no target contradicts the two rules.
  Profiles also set flow control explicitly to the request id (`ATFCSH`, `ATFCSD 300000`).
- No profile uses SecurityAccess (`27`) at all. Extended session (`10 03`) is
  prepended for some OEM DIDs on 9 of the 22 11-bit targets; 13 targets (including
  `7E1`, `7E6`, `70B`, `715`, `712`) are read by plain `22 xxxx` in every profile
  and `71E` is never read without it.
- No profile reads a standard F18x/F19x identification DID. The archive therefore
  proves request/response addressing for OEM DIDs only; it says nothing about
  standard identification support on those ECUs.
- Roles come only from name tags and group labels on sensor entries.

## Options considered
1. **Keep targets in code or only in private inventory.** Hard-codes VAG
   knowledge in the core and gives no reviewed provenance. Rejected (REQ-002).
2. **Extend v2 definitions with a target field.** v2 definitions are
   operation/decoder records; a target is not an operation, and v2 is a pinned
   contract. Rejected.
3. **New document kind `ecu_targets` (schema v3), consumed as probe candidates
   only.** v1/v2 stay immutable, the file carries no operation, payload or
   session vocabulary and cannot widen capability. Chosen.
4. **Address-range probing for responders.** Violates REQ-003.4. Rejected.

## Decision (proposed)
**Schema.** `knowledge-v3` documents hold `ecu_targets`. Each entry has: stable
`id`; `addressing` (`family: can_11bit`, `request_id`, `response_id`,
`response_id_rule: iso15765_plus_8 | vag_plus_0x6a`, `response_id_stated_in_source`);
`transport: iso_tp_can`; `logical_role` (closed enum with its own confidence);
`applicability` (`vehicle_manufacturer`, optional `platforms`; VIN never a key);
`provenance` (`COMMUNITY`/`INFERRED`/... , confidence, sources with
`archive_version`, `profile_count`, `profile_total`, `plain_read_profile_count`);
`hardware_validation`. Proposal files:
`docs/adr/0002-knowledge-proposal/knowledge-v3.schema.json` and
`manufacturers/vag/ecu-targets.yaml`; `check_proposal.py` validates them.

**First VAG set.** A 11-bit target qualifies if: it appears in at least 2
profiles; its response id is derived by one of the two rules above and does not
collide with another entry; its role has a name tag; and at least one profile
reads it with plain `22 xxxx`. 18 targets qualify (table below). Excluded:
`71E` (every profile prepends `10 03`), `71D` and `752` (1 profile), `765` (2
profiles, no role tag), all 29-bit `FC00xx`, `000`, `8210F1` and the empty header.
Entries cite counts only; no archive text, DIDs, formulas or commands are copied.
Confidence is `medium` at 10 or more profiles and `low` below; nothing is `high`
and nothing is `validated` before hardware evidence.

| Target | Response id | Role | Profiles (plain 22) | Confidence | Note |
|---|---|---|---|---|---|
| 7E0 | 7E8 | engine | 53 (51 plain) | medium | convention +8; ISO +8 by convention; engine already validated in the private inventory |
| 714 | 77E | instrument_cluster | 43 (30 plain) | medium | stated +0x6A; also the ADR 0001 odometer target |
| 7E1 | 7E9 | transmission | 42 (42 plain) | medium | convention +8; ISO +8 by convention |
| 710 | 77A | gateway | 38 (37 plain) | medium | stated +0x6A; gateway; no installation-list claim |
| 713 | 77D | abs_esp | 37 (36 plain) | medium | stated +0x6A |
| 746 | 7B0 | climate | 27 (24 plain) | medium | stated +0x6A |
| 70B | 775 | tpms | 25 (25 plain) | medium | stated +0x6A |
| 715 | 77F | airbag | 23 (23 plain) | medium | stated +0x6A |
| 70E | 778 | body_control | 19 (10 plain) | medium | stated +0x6A; 9 profiles use an extended session for other DIDs |
| 70F | 779 | all_wheel_drive | 18 (18 plain) | medium | stated +0x6A |
| 712 | 77C | steering | 14 (14 plain) | medium | stated +0x6A |
| 74F | 7B9 | driver_assistance | 12 (12 plain) | medium | stated +0x6A |
| 767 | 7D1 | emergency_call | 11 (11 plain) | medium | stated +0x6A |
| 7E5 | 7ED | battery_management | 8 (7 plain) | low | convention +8; ISO +8 by convention |
| 773 | 7DD | infotainment | 6 (6 plain) | low | stated +0x6A; source also issues a reset service here, never imported |
| 7E6 | 7EE | electric_drive | 4 (4 plain) | low | stated +8; role tag on a minority of entries |
| 744 | 7AE | hv_charger | 3 (1 plain) | low | stated +0x6A; 1 of 3 profiles reads without extended session |
| 742 | 7AC | ptc_heater | 2 (2 plain) | low | stated +0x6A |

**Core consumption.**
1. Knowledge targets are candidates, never mappings. A candidate selects no
   action by itself; the vehicle manufacturer must be resolved from the already
   evidenced identity (WMI or an observed ECU fact), not from the VIN as key.
2. Presence probe: for each applicable candidate, in table order, exactly one
   standard request, `22 F187` (spare part number; from the existing standard set,
   not private, never F190 or F18C), sent through `SafetyPolicy`, `ATSH` request id,
   `ATCRA` response id, default session, no `10 03`, no `27`, no retry on other
   ids. One owner session, sequential (REQ-012). The list is bounded by the pinned
   file (18 entries), not by a range.
3. A positive `62 F187` or a negative `7F 22 xx` from the expected response id
   proves a live responder: the target is recorded as **present** with the raw
   exchange as evidence. No answer, timeout or `NO DATA` is **not observed**, never
   absent or not installed (REQ-003.2). A reply from another id is kept as separate
   responder evidence and does not satisfy the candidate (REQ-011.1).
4. Only a present target becomes an `EcuTargetMapping`: the request-target fact
   gets `High` from two matching facts (candidate pair plus an answer on the
   expected id), provenance naming the knowledge revision and entry id. The role
   keeps the entry's own, lower confidence as a separate fact (REQ-003.1). Existing
   `discover_known_ecus` then runs the full bounded 14-DID plan per mapping
   unchanged (#85). F18C and the other private identifiers are read only there
   and stay private evidence.
5. Multi-frame answers need ISO-TP flow control addressed to the request id. The
   archive sets `ATFCSH` explicitly; `configure_target` does not. An
   implementation slice must add `ATFCSH` (and the existing flow-control settings)
   from the entry, or fail closed for multi-frame replies. Not decided here.
6. `validate_request_target` already restricts to 3 hex digits and physical
   OBD-II context; the v3 pattern `^7[0-9A-F]{2}$` matches. `EcuRole` keeps its
   variants; other roles map to `VendorSpecific(role)`.

**Gateway behaviour.** A gateway (`710`) is just another candidate. Behind a
gateway, targets may be filtered or unreachable (the EA189/PQ35 research in #35
stays negative; later gateways may enforce protection). Silence is therefore
"not observed". No gateway routing, installation list or unlock is implied, and
none is attempted.

**Privacy.** The file holds CAN ids, role names, counts and the archive version.
Probe results, ECU serials and VINs stay in the private inventory and are never
written back (REQ-002.3). The archive stays unpublished and is cited only as
"Car Scanner 2.1.50, local archive".

**Out of scope.** 29-bit/`FC00xx`/VWTP 2.0 (different addressing context, the
core accepts only 11-bit physical headers); KWP2000 and `8210F1`; any session
change, `27`, reset, coding, adaptation, actuator, DTC clear or OEM DID
(including everything else the archive contains); platform applicability
(`platforms` is empty until separately evidenced); promotion beyond
`medium`/`not_validated`.

## Consequences
- Knowledge repo: new schema file, `schema_version` 3 dispatch in `validate.py`,
  `manufacturers/vag/ecu-targets.yaml`, tests for id uniqueness, offsets and
  closed enums. Core: v3 loader, probe plan (one request per candidate), mapping
  creation on a present answer, `ATFCSH`, `EcuRole` mapping, offline replay tests.
- A new typed read-only job is needed; it needs REQ criteria (suggest a new
  REQ for "bounded target probe", criteria for present / not observed / foreign
  responder, zero extra frames, no 29-bit) before implementation.
- Bounded cost: up to 18 requests per vehicle. Unknown ECUs that exist but are not
  in the list stay undiscovered, which is accepted.
- Wrong or missing responses cannot cause writes; the worst case is bus
  traffic from 18 default-session reads. Real acceptance needs a separate
  hardware gate with explicit vehicle availability.
- Standard identification support on these targets is unproven (no archive
  evidence); a probe may find many `not observed`, in particular on older KWP-era ECUs.
