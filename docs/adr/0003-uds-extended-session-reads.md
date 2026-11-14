# 0003. Read-only UDS extended-session reads on explicit user action
Date: 2026-10-28
Status: accepted; amended 2026-11-14 (mixed OBD2/UDS polling; activation by explicitly applied component presets)

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
  - The original decision adds only `0x22` reads of that ECU's session-flagged Knowledge DIDs. They are marked **UDS** in the selector and on live tiles, distinct from **OBD2**. The 2026-11-14 amendment adds selected Mode 01 reads (see Mixed selections).
- **Return to the default session** (`10 01`) happens:
  - when the user switches it off;
  - on disconnect, demo or replay;
  - 10 minutes after activation;
  - before any other functional operation, before Knowledge reads of another ECU, and when the validated engine target is missing or mismatched (selected Mode 01 polling of the validated engine target is not a functional operation, see Mixed selections);
  - on any error, including a rejected, unanswered or malformed read of either kind (see Mixed selections).

  The return is attempted and must be acknowledged before further ordinary vehicle operations.
  If its outcome is uncertain, operations stop until reconnection. With a lost link,
  tester present stops and the ECU uses its own session timeout; its exact duration
  needs vehicle acceptance.
- **Never:** SecurityAccess (`27`), RoutineControl (`31`), InputOutputControl (`2F`), WriteDataByIdentifier (`2E`), ECUReset (`11`), Communication/DTC control, or any caller-provided service, address or DID. The safety policy models the extended-session read as its own closed operation kind; no profile, layout, MCP call or prompt can enable it.
- **Visibility:** while the extended session is active, the app shows a persistent indicator, and recordings mark the session state for every read. The indicator and marker are the app's view of the session, not proof from the ECU: the keepalive `3E 80` suppresses its response, so a still-active ECU session is not confirmed.
- **Mixed selections (amended 2026-11-14, replaces the earlier OBD2 pause):** while the extended session is active,
  the user's selected standard Mode 01 PIDs and the selected UDS DIDs of the active ECU are polled together by one
  serialized core read loop. The OBD2 selection is neither paused nor re-armed; it keeps polling in the extended
  session and after it ends, and the UDS signals disappear when the session ends.
  - Mode 01 reads keep their existing engine-target validation, timeouts and fail-closed handling. The amendment adds `01 xx` reads of the user's selected standard PIDs as the only new service; nothing else is added to the closed set. If the validated engine target is missing or mismatched, the session is returned to default (`10 01`, when possible) and no Mode 01 read is sent.
  - Tester present `3E 80` is still sent at most every 2 s; due maintenance is attempted between reads. A single read can still block longer than 2 s, so no hard keepalive bound is claimed.
  - A successful standard Mode 01 read keeps the session. A rejected, unanswered or malformed read of either kind (Mode 01 or UDS DID) ends it, returning to default when possible, as for any error. Each read keeps and shows its own outcome (decoded, NRC-rejected, unanswered, malformed); missing values are never filled in or carried over from an earlier cycle. Recordings keep the raw request/response bytes and mark the session state for every read, Mode 01 reads included, so replay re-derives the mix offline.
  - Ordinary (default-session) Knowledge DID reads of the same engine ECU may continue. Knowledge reads of any other ECU leave the session first, like other functional operations.
  - **Component-preset activation (amended 2026-11-14; #291, REQ-035):** besides the explicit switch, the explicit apply of one of the five OBDentic-owned component presets (`dpf`, `egr`, `cooling`, `turbo`, `rail`) in a ready live session may activate the session automatically when an entry exists solely as a session-flagged DID of the one ECU for which the session is offered. It is the same closed operation: exactly `10 03` to that ECU's validated target, the persistent indicator, tester present at most every 2 s, the ten-minute limit, the return and error rules above and no service or read beyond this closed set. Entries of other ECUs stay unavailable.
    - It happens once per apply. It is never retried and never repeated after an end or expiry; only a new explicit apply may activate again. No job may be running.
    - It never happens at app start, connection, reconnect, selection restore, when applying a vehicle preset or recording profile, in demo or replay, or from smart analysis (#297, REQ-036), which stays in the default session.
    - Component presets are OBDentic-owned and closed. Profiles, layouts, stored vehicle presets, MCP calls, AI prompts and Knowledge gain no privilege, cannot define or change a component preset, and cannot activate the session.
    - If the activation is refused or fails, the preset still selects its default-session entries and marks the UDS entries unavailable with the reason. When the session ends, those entries become unavailable with that reason. Metadata of session-flagged definitions never becomes an ordinary selectable catalog entry before the session is active (REQ-024.5).
    - Recordings mark the preset id, the resolution result per entry and that the activation was requested by a preset, in addition to the session state of every read.
  - Other functional operations (for example DTC reads, adapter throughput, the session-candidate learning run
    and full sweeps) still leave the session first, as before. Polling selected Mode 01 PIDs is not a functional
    operation in this sense.
- **Vehicle:** the user decides when to use it. It should be used with the engine off or idling until behaviour while driving is known. The first use, including preset activation and smart analysis, is part of the bundled vehicle session (#178), which remains a separate hardware gate.
- **Vehicle acceptance is separate:** the amendment is a design and core-policy decision. Whether the engine ECU keeps
  answering Mode 01 reads while in `10 03`, how the interleaved rate affects tester present and response timing, and
  the ECU's own session timeout are unverified until the vehicle session (#178). No hardware test is claimed here.

## Consequences
- `docs/project-goals.md` and AGENTS.md read-only wording gain this narrow, explicitly user-initiated exception: the session change is not a write, and mutating services stay forbidden.
- New requirement REQ-027; spike #248 is resolved by this ADR.
- If an ECU rejects `10 03` (NRC) or misbehaves, the feature stays off for it, and the result is recorded.
- The amendment supersedes the Mixed-selections pause. REQ-027.3/.4 are amended and REQ-027.6 added for the observable mixed behaviour.
- Preset activation (2026-11-14) is covered by REQ-035.3/.4 and REQ-027.7. Smart analysis (REQ-036) never activates the session. Whether the engine ECU answers the preset signals in the session on the vehicle is not claimed here; it is hardware evidence of #178.
