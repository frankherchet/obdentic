# Android workbench

OBDentic's Android app runs the shared Rust diagnostic core on the phone. Android
owns BLE/GATT and application lifetime; the Rust facade produces the same closed
Carly initialization, semantic reads and diagnostic requests as the desktop core.
The UI never accepts raw commands, addresses or mutation capabilities.

## First release

- Carly BLE scan, explicit connect/disconnect and recoverable permission errors.
- Supported semantic measurements, live trends and sample age.
- VIN where readable and a **partial** functional responder inventory; this is not
  a full manufacturer installation list.
- Stored Mode 03 DTCs, scoped by responder.
- Seven experimental EA189 DPF probes, gated on validated engine targeting.
- The existing engine baseline, drive and OBD expansion capture profiles, plus
  repeated experimental DPF capture.
- Private recording, foreground connected-device service, screen-off operation,
  explicit export, offline replay and timestamped raw TX/RX.
- Demo responses pass through the real Rust parser; the mode is always visible.

DPF interpretations remain experimental. Pending/permanent DTCs, freeze frames,
full OEM fault-memory scans and DTC clearing are not implemented by this app.

## Build and install

Prerequisites: JDK 17+, Rust 1.98.0, `rtk`, Android SDK platform 36, build tools
35.0.0, NDK 27.2.12479018 and the `aarch64-linux-android` Rust target. The initial
build supports ARM64 phones on Android 8/API 26 and newer. Android Studio can
install the SDK/NDK packages; the Gradle wrapper pins Gradle 8.13 with its checksum.

```sh
rtk env PATH="$HOME/.cargo/bin:/opt/local/bin:/usr/bin:/bin" \
  rustup target add aarch64-linux-android --toolchain 1.98.0
rtk env JAVA_HOME="/path/to/jdk/Contents/Home" \
  ANDROID_HOME="$HOME/Library/Android/sdk" \
  ./android/gradlew -p android :app:assembleDebug :app:lintDebug
rtk proxy adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

Gradle builds and packages the Rust JNI library automatically. `ANDROID_NDK_HOME`
can select a different compatible NDK installation. The build adds no Java/Kotlin
runtime dependencies beyond Android itself. APKs, JNI binaries, local SDK paths
and Gradle caches remain untracked. The native crate has its own lockfile.

Start OBDentic, open **Adapter**, and choose **Demo starten** for desk testing,
or **Adapter suchen** for the real Carly. Android requests Nearby devices and
notification permissions as needed. No USB connection to the Mac is required for
live use. A connected session remains owned by the service when switching tabs,
recreating the Activity or locking the screen. Reopening after process death
requires a new connection; existing files remain available as partial recordings.

## Recording, privacy and desktop inspection

Recordings live in the application's private `recordings/` directory. Cloud
backup and device transfer are disabled. Export grants read-only access to the
one chosen file through Android's share sheet. Uninstalling the application
removes its private files, so export recordings you need to keep first.

The mobile JSONL envelope is `OBDENTIC-MOBILE`, version 1. It preserves session
mode, profile/catalog context, wire fragments, operation results, wallclock and
monotonic offsets, and a terminal marker. It is distinct from the desktop
`OBDENTIC-CAPTURE` event format: do not pass it to `capture inspect`.

After explicitly exporting and copying a file to a private local path:

```sh
rtk env PATH="$HOME/.cargo/bin:/opt/local/bin:/usr/bin:/bin" \
  cargo +1.98.0 run --locked --bin obdentic-mobile-report -- evidence/mobile.jsonl
```

The report uses the shared offline decoders to recompute measurements and
diagnostic results rather than trusting saved values. Original TX/RX remains
in the source file. Interrupted files remain explicitly partial. Demo recordings
stay labelled demo, including when opened as replay. Phone replay reconstructs
recorded sample histories; it does not contact an adapter.

Recordings and debug logs may contain VINs, ECU identifiers and other private
vehicle evidence. Keep them under untracked `evidence/` or `captures/`; never
commit or attach them to an issue/PR. Debug copy and file export are explicit
local user actions.

## Acceptance

Build and install the instrumentation APK, then run it with the phone unlocked:

```sh
rtk env JAVA_HOME="/path/to/jdk/Contents/Home" \
  ANDROID_HOME="$HOME/Library/Android/sdk" \
  ./android/gradlew -p android :app:assembleDebugAndroidTest
rtk proxy adb install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
rtk proxy adb shell am instrument -w de.obdentic.app.test/de.obdentic.app.AcceptanceRunner
```

Grant Bluetooth and notification permissions before running acceptance. The
runner exercises the real JNI/core with a simulated adapter, profile recording,
screen off, export provider, offline re-decoding, reconnect and Activity
recreation. It emits synthetic screenshots and a demo recording as instrumentation
artifacts (also stored in the app-specific acceptance directory). Passing this test
does **not** establish live BLE or vehicle acceptance.

Real vehicle acceptance remains a separate required gate:

Tracked separately in [acceptance issue #178](https://github.com/frankherchet/obdentic/issues/178).

1. Search for Carly, connect, inspect initialization TX/RX and supported signals.
2. Read real measurements, inventory and stored DTCs; verify responder attribution.
3. Run the EA189 DPF job only on the intended vehicle; retain unsupported/negative
   answers explicitly rather than treating them as zero values.
4. Record the drive profile while locked for at least five minutes. Stop it,
   export locally, replay on the phone and inspect the same file on the Mac.
5. Disconnect, reconnect and test adapter power loss. The UI must not retain old
   live values under a new connection or silently continue a failed recording.

Retain raw acceptance evidence locally. Publish only a sanitized pass/fail summary.
