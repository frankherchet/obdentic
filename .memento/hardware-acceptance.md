---
title: Finish bundled hardware acceptance
type: todo
created: 2026-10-23
updated: 2026-11-13
---

Real-vehicle acceptance stays open in issue #178. Do not assume the adapter and vehicle are available; ask before any live session.

Confirmed on the vehicle by 2026-11-12: VIN record, PID 0xA6 not advertised (expected for EA189), export and pull, auto-reconnect, Firebase updates, automatic `…-live-auto.jsonl` with handshake, the availability check and default-session learning, Knowledge oil temperature and DPF values in Live, sealed and exported app logs, BLE abort with stack trace and GATT status, and F187 identification on the engine ECU (#85 comment). Removed experimental jobs (#230) stay removed.

**Next step:** the user drives the test plan posted on #178 on 2026-11-13 (comment "Testplan nächste Fahrt (v0.5.5)": cold start + DTC scan, ≥30 min warm-up/thermostat, DPF, turbo/EGR, warm idle rail, reference-tool comparison). Then pull with `tools/pull-recordings.sh`, rerun the coverage analysis (all Knowledge definitions vs. reads/availability per semantic), and report back on #283 (F192 from 74F), #290, #292, #293, thermostat and regeneration. Keep evidence private. Delete this memo when #178 and #85 hardware acceptance are complete.
