# 0003. Read-only UDS extended-session reads on explicit user action
Date: 2026-10-28
Status: accepted

## Context
Some EA189 engine DIDs are read by community tools only after `10 03`, the UDS extended diagnostic session (`manufacturers/vag/ea189-engine-session-candidates.yaml` in the Knowledge DB, about 18 DIDs). OBDentic has so far excluded session changes, because the extended session is also where mutating services (actuator tests, coding, adaptation, routines) become reachable. The maintainer decided on 2026-10-28 to offer these values: "unter Diagnose die UDS-Umschaltung umsetzen; dann sollen die erweiterten IDs verfügbar sein, markiert als UDS". A default-session learning run (#249) shows first which of them answer without a switch.

## Options considered
1. Keep excluding session changes. This is safe, but the values stay unavailable.
2. A process-start capability flag, like the DTC-clear design (#126). This is robust, but inconvenient for a phone app that is started from the launcher.
3. **An explicit, visible user action in the Diagnose tab**, scoped to one identified ECU and to read-only `0x22` reads of Knowledge-listed session DIDs, with a guaranteed return to the default session.

## Decision
Option 3:
- **Scope:**
  - The Diagnose tab offers "UDS: erweiterte Sitzung" per identified ECU that has session-flagged Knowledge DIDs; initially the engine ECU.
  - Activation sends exactly `10 03` to that ECU's validated target.
  - While it is active, the app sends `3E 80` (tester present, suppress response) at most every 2 s.
  - Only `0x22` reads of that ECU's session-flagged Knowledge DIDs are added. They are marked **UDS** in the selector and on live tiles, distinct from **OBD2**.
- **Return to the default session** (`10 01`) happens:
  - when the user switches it off;
  - on disconnect, demo or replay;
  - after 10 minutes of activity;
  - before any functional operation;
  - on any error.

  The return is attempted and must be acknowledged before further ordinary vehicle operations.
  If its outcome is uncertain, operations stop until reconnection. With a lost link,
  tester present stops and the ECU uses its own session timeout; its exact duration
  needs vehicle acceptance.
- **Never:** SecurityAccess (`27`), RoutineControl (`31`), InputOutputControl (`2F`), WriteDataByIdentifier (`2E`), ECUReset (`11`), Communication/DTC control, or any caller-provided service, address or DID. The safety policy models the extended-session read as its own closed operation kind; no profile, layout, MCP call or prompt can enable it.
- **Visibility:** while the extended session is active, the app shows a persistent indicator, and recordings mark the session state for every read.
- **Mixed selections:** ordinary OBD2 polling pauses while the extended session is active;
  the selection remains for resumption afterwards. Knowledge-listed DID reads to the
  active ECU may continue. An explicit functional/Mode-01 operation leaves the session first.
- **Vehicle:** the user decides when to use it. It should be used with the engine off or idling until behaviour while driving is known. The first use is part of the bundled vehicle session (#178).

## Consequences
- `docs/project-goals.md` and AGENTS.md read-only wording gain this narrow, explicitly user-initiated exception: the session change is not a write, and mutating services stay forbidden.
- New requirement REQ-027; spike #248 is resolved by this ADR.
- If an ECU rejects `10 03` (NRC) or misbehaves, the feature stays off for it, and the result is recorded.
