---
title: Finish bundled hardware acceptance
type: todo
created: 2026-10-23
updated: 2026-10-24
---

Real-vehicle acceptance is still open in issue #178 after the Android signal expansion. The previous vehicle session ended; do not assume the adapter and vehicle remain available.

The user explicitly requested a dedicated ash-candidate learning run, tracked in [#182](https://github.com/frankherchet/obdentic/issues/182). Prepare it alongside #178 to minimize vehicle visits; follow its separate session-design gate before considering any `10 03` experiment.

**Next step:** Review both issue checklists and the built-in finite ash test sequence documented in `docs/android.md`, then schedule the prepared runs when the user confirms availability. ECU/software applicability and reference-tool comparison still need real evidence. Keep software/demo success separate from hardware evidence. Delete this memo when #178 and #182 are completed, or narrow it to the remaining gate.

**2026-10-24 update:** The first bundled vehicle run happened (5 live recordings, UDS DPF/extended reads all positive). It exposed that every targeted RPM read after session start failed because `restore_functional` sent `ATSP0` (fixed in #208, issue #207). Recordings stay local in `evidence/`; export them via the app's "Datei exportieren" (adb `run-as` is blocked on the Xiaomi). The run did not cover the ash test (#182), VW cluster odometer (#198), PID 0xA6 (#191), VIN vehicle record (#189), DTC inventory or reconnect/adapter-loss. Next vehicle visit must re-verify corrected live RPM plus those open items.
