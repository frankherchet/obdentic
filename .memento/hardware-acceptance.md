---
title: Finish bundled hardware acceptance
type: todo
created: 2026-10-23
updated: 2026-10-23
---

Real-vehicle acceptance is still open in issue #178 after the Android signal expansion. The previous vehicle session ended; do not assume the adapter and vehicle remain available.

The user explicitly requested a dedicated ash-candidate learning run, tracked in [#182](https://github.com/frankherchet/obdentic/issues/182). Prepare it alongside #178 to minimize vehicle visits; follow its separate session-design gate before considering any `10 03` experiment.

**Next step:** Review both issue checklists and `docs/android.md`, prepare the missing bounded candidate test support, then schedule the prepared runs when the user confirms availability. Keep software/demo success separate from hardware evidence. Delete this memo when #178 and #182 are completed, or narrow it to the remaining gate.
