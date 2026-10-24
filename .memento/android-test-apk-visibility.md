---
title: Acceptance APK cannot see package-private app members
type: gotcha
created: 2026-10-23
updated: 2026-10-23
---

The Android acceptance tests (`android/app/src/androidTest/...`) run from a separate test APK. Package-private classes, fields and static helpers of the app compile fine but fail at runtime with `IllegalAccessError`; anything the acceptance code calls must be `public` (e.g. `VehicleStore`, `ObdService.vehicles`, `MainActivity.supportLabel`).

**Why:** hit three times (#189, #190, #198), even when the delegation brief said "make them public"; Gradle build and lint stay green, only the phone run fails.
**How to apply:** before installing on the phone, check mechanically: for every `MainActivity.`/`ObdService.`/`VehicleStore.` call in `android/app/src/androidTest`, grep its declaration in `android/app/src/main` and make sure it is `public`. Always run the phone `AcceptanceRunner` (default, no screen-off) before merging Android changes.
