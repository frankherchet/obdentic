---
title: Never lock the test phone
type: preference
created: 2026-10-23
updated: 2026-10-23
---

Automated phone runs must not turn the screen off or lock the user's phone. Before this was changed, `AcceptanceRunner` sent `input keyevent 223` (sleep) and `224` (wakeup), and the phone stayed locked afterwards.

**Why:** the user said explicitly on 2026-10-23 that this must not happen ("mach das NICHT!").
**How to apply:** run the default `AcceptanceRunner` only, without screen-off; the screen-off check is opt-in (`-e screenOff true`) and must be run only when the user asks for it. Never send sleep/power key events via adb unasked.
