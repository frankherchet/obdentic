---
title: Android acceptance on obdentic-acc — setup, order, pitfalls
type: gotcha
created: 2026-11-22
updated: 2026-11-22
---

The dedicated AVD `obdentic-acc` (1080×2220, 440 dpi, API 33, `~/.android/avd/obdentic-acc.avd`) runs as `emulator -avd obdentic-acc -port 5584 -no-window -no-snapshot -no-audio` → `adb -s emulator-5584`. A boot needs about 8 GB free disk (the emulator refuses below 7.4 GB). Run `:app:lintDebug` together with the APK builds **before** `AcceptanceRunner`: one full run takes about 5 minutes.

Runner pitfalls: the runner presses HOME mid-run (after "Explicit file export"), so later UI checks must bring `MainActivity` back to the front; `setDemo()` with an empty selection applies a default selection and starts polling, so clear it before comparing `ObdService.wireEventCount()`; send the back key only once the target window has focus.

**Why:** #370/PR #371 needed six full runs. Lint errors (`GestureBackNavigation`, `NewApi`) surfaced after acceptance, and each pitfall above failed one run. A full disk blocked creating the AVD.
**How to apply:** check `df -h /` before starting; build plus lint first; apply the pitfalls when adding UI checks to `AcceptanceRunner`.
