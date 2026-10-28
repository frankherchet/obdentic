---
title: Finish bundled hardware acceptance
type: todo
created: 2026-10-23
updated: 2026-10-28
---

Real-vehicle acceptance stays open in issue #178. Do not assume the adapter and vehicle are available; ask before any live session.

Verified on the vehicle so far (2026-10-24/28): the VIN vehicle record, PID 0xA6 not advertised (expected for EA189), recording export and pull, auto-reconnect after a BLE drop, and the Firebase update path. All experimental features (EA189 DPF/extended/ash jobs, ash test #182, VW cluster odometer) were removed in #230, so they need no vehicle check anymore.

The replacement software is Knowledge-based ECU discovery (#85 / REQ-023), normal live values (#246), the default-session candidate learning run (#249), and the explicit UDS session switch (#251, ADR 0003). Do not revive the removed jobs.

**Next step:** bundle manual inventory after polling (#215/#221), reconnect after app restart, faster polling (#231), the live view (#232) and discovery, default-session learning, reference-tool comparison of Knowledge values, recording/replay and first stationary UDS activation into #178 after vehicle availability is confirmed. The user wants as much testing as practical on 2026-10-29, without artificial waiting. Applicability remains draft manufacturer/engine-role matching until real identification evidence supports narrowing it. Pull recordings with `tools/pull-recordings.sh`; keep evidence private. Delete this memo when #178 and #85 hardware acceptance are complete.
