---
title: Finish bundled hardware acceptance
type: todo
created: 2026-10-23
updated: 2026-10-28
---

Real-vehicle acceptance stays open in issue #178. Do not assume the adapter and vehicle are available; ask before any live session.

Verified on the vehicle so far (2026-10-24/28): the VIN vehicle record, PID 0xA6 not advertised (expected for EA189), recording export and pull, auto-reconnect after a BLE drop, and the Firebase update path. All experimental features (EA189 DPF/extended/ash jobs, ash test #182, VW cluster odometer) were removed in #230, so they need no vehicle check anymore.

**Next step:** at the next vehicle visit, check the manual inventory after live polling (#215/#221 fix), reconnect after an app restart, faster live polling (#231) and the live view (#232). Pull recordings with `tools/pull-recordings.sh`. Delete this memo when #178 is completed.
