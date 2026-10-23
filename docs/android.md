# Android workbench

OBDentic's Android app runs the shared Rust diagnostic core on the phone. Android
owns BLE/GATT and application lifetime; the Rust facade produces the same closed
Carly initialization, semantic reads and diagnostic requests as the desktop core.
The UI never accepts raw commands, addresses or mutation capabilities.

## First release

- Carly BLE scan, explicit connect/disconnect and recoverable permission errors.
- 50 standard semantic measurements, vehicle-advertised support, live trends and sample age.
- VIN where readable and a **partial** functional responder inventory; this is not
  a full manufacturer installation list.
- Stored Mode 03 DTCs, scoped by responder.
- Seven experimental EA189 DPF probes, gated on validated engine targeting.
- A separate, explicitly selected 15-value experimental EA189 additional-read job
  and recording profile, including oil/boost/fuel data and uninterpreted regeneration
  status numbers. These new interpretations are not hardware-validated.
- The existing engine baseline, drive and OBD expansion capture profiles, plus
  repeated experimental DPF capture.
- The `obd2-extended-validation` profile records additional standard scalars
  conservatively; unavailable optional values are not assumed supported.
- Private recording, foreground connected-device service, screen-off operation,
  explicit export, offline replay and timestamped raw TX/RX.
- A finite EA189 ash learning sequence under **Aufnahmeprofil / Testsequenz**, with
  operator-reported operating context, visible progress and automatic recording stop.
- Confirmed deletion of completed internal recordings; active recordings and loaded
  replays are protected. Exported copies remain separate.
- The display stays awake while the app is visible; background operation and manual
  locking remain available.
- Demo responses pass through the real Rust parser; the mode is always visible.

DPF interpretations remain experimental. Pending/permanent DTCs, freeze frames,
full OEM fault-memory scans and DTC clearing are not implemented by this app.

For a targeted read, selection uses the explicitly expected responder, never
the decoded value. Additional responders remain in the evidence. A missing
expected response or conflicting payloads from that same responder still fail;
an untargeted functional read remains ambiguous when responders disagree.

## Vehicle identity and additional reads

Inventory includes an offline, partial WMI manufacturer hint derived from a
format-checked VIN. No VIN is sent to an external decoder. The initial sourced
snapshot uses NHTSA vPIC records; [source details and KBA limitations](research/vin-identity.md)
explain its incomplete European coverage. Model, engine and ECU compatibility
remain unresolved; the first three VIN characters never select an OEM profile.

`ea189.extended.probe` is a closed set of 15 fixed, engine-targeted `0x22` reads.
The user must explicitly choose the EA189 job or its recording profile. It reuses
validated `7E0`/`7E8` routing, preserves raw responder evidence and negative answers,
and redecodes recordings offline. The candidates were checked against the locally
extracted Car Scanner 2.1.50 EA189 profile and remain experimental with source
metadata; their original upstream source is unknown. No executable vendor database,
arbitrary formula, address or preparation sequence is imported.

The `178C` ash-mass candidate is omitted from the 15-value extended job: the source
includes `10 03` preparation (UDS extended diagnostic session). A separate closed
`ea189.ash.probe` learning job reads only `1153` and `178C`, once each, without that
preparation or any fallback. It requires validated engine targeting and retains
uninterpreted responses/NRCs rather than manufacturing ash telemetry. No session-start
traffic is added. The existing seven-value DPF job and its replay remain separate.
Support discovery follows advertised continuation bits only, bounded by the highest page
the catalog needs (currently `0xA0`, for the odometer PID `A6`).

## Known vehicles

Each live inventory with a format-valid VIN creates or refreshes a record in
`vehicles.json` (app-private files directory, written atomically; excluded from
backup and device transfer, never exported). A record keeps the VIN, a
manufacturer-based default name (renamable, 1-40 characters), responders, signal
support, engine-target validation and first/last connection time; the last
observation replaces the previous one, no history is kept. The selected vehicle
persists in the same file and is shown as a header chip on every tab; the chip
opens the "Fahrzeuge" page (masked VIN, details, rename, delete with
confirmation). Demo and replay sessions never touch the store, an unreadable VIN
saves nothing, and selecting a record sends no adapter traffic. A corrupt file is
reported and never overwritten implicitly; only a confirmed reset replaces it. A
failed write leaves the previous file and the in-memory state unchanged. After
the inventory of a live session for a stored vehicle, the odometer is read once through
standard Mode 01 PID `A6` (`(A·2^24+B·2^16+C·2^8+D)/10` km) when the vehicle advertises
it, and stored with read date and source "OBD PID A6"; when it is not advertised no
request is sent. A failed read keeps the stored value and date and is shown in the
details, as is a value lower than the stored one (the new value is stored, neither is
corrected). Without a stored value the page shows "km-Stand nicht verfügbar". Covered
by `VehicleRecordsAcceptance` (REQ-013, REQ-014, REQ-015) and the demo read in `AcceptanceRunner`.

The Diagnose tab has an experimental, user-started card "EXPERIMENTELL · VW KOMBIINSTRUMENT"
(REQ-016, [ADR 0001](adr/0001-manufacturer-odometer-source.md) amendment, #198). One job
action, `vw.cluster.odometer` (no argument), sends a single `22 2203` to request header
`714` and accepts only a positive answer from `77E`, then restores functional
addressing; there is no session change, SecurityAccess, RoutineControl, retry or other
target, and it is never part of automatic connection traffic. It needs a live ready
session, a stored vehicle selected for the current VIN and a VW Group manufacturer hint;
otherwise the button is disabled with the reason and the core refuses without sending
anything. A 2-byte answer is stored as `cluster_odometer` ((A·256+B)·10 km, with date,
source "VW Kombiinstrument DID 2203 (experimentell)", classification COMMUNITY and
hardware validation "pending"), separate from the PID `A6` value and never merged with it.
The list shows the `A6` value when stored, otherwise the cluster value marked
"Kombiinstrument · experimentell". Failures (no response, other responder, negative
response with its NRC) are stored as `cluster_odometer_error` and keep stored values;
other payload lengths stay raw and are shown as "nicht auswertbar". Recordings re-decode
the action offline through `replay.vw.cluster.odometer`. The scaling and the default-session
behaviour are unconfirmed until the vehicle check bundled into #178.

A changed live signal selection is stored in the selected record (`selection`) and restored
when that record is selected again (and after a process restart), without adapter traffic;
demo and replay sessions never read or write it, and a new record gets the default
selection of supported signals. Without an active session the signal chooser marks each
signal from the record's last observation with its date ("unterstützt (zuletzt gesehen
22.10.2026)"); stored support never skips discovery or decides whether a read is sent, every
live connection still discovers support and the current observation replaces the stored one.

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

Under **Aufnahmeprofil / Testsequenz**, select **Testlauf: EA189 Asche (ohne
Sitzungswechsel)**. Choose either ignition on/engine off or idle and keep that state
throughout the run. This is an operator report, not ECU-confirmed context. The fixed
sequence records validated engine inventory, both ash candidate reads, the seven-value
DPF comparison and a final normal RPM read. It then closes the recording automatically.
Generic polling and manual jobs cannot interleave. Negative candidate replies are
valid learning evidence, not proof that a session change is necessary. Manual stop,
disconnect and errors remain distinct from completed execution; completion does not
mean that the candidates or their units have been validated. Export and replay retain
the context, progress and raw candidate evidence. The live learning gate remains
[#182](https://github.com/frankherchet/obdentic/issues/182).

Each saved recording has **Aufnahme löschen …**. Confirmation permanently deletes
only the phone's internal file; exported copies remain. Stop an active recording
first, or leave a loaded replay before deleting its file.

Recordings live in the application's private `recordings/` directory. Cloud
backup and device transfer are disabled. Export opens Android's system file
picker so the user can save a copy, for example in Downloads for USB transfer.
The internal content provider grants only read access to an individual recording.
Uninstalling the application
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
runner exercises the real JNI/core with a simulated adapter, profile recording
(including background operation after pressing Home), export provider, offline
re-decoding, reconnect and Activity recreation. The default run does not turn
the screen off or lock the phone. The real screen-off check is opt-in: add
`-e screenOff true` to the `am instrument` command (the phone is left locked
afterwards). It emits synthetic screenshots and a demo recording as instrumentation
artifacts (also stored in the app-specific acceptance directory). Passing this test
does **not** establish live BLE or vehicle acceptance.

Real vehicle acceptance remains a separate required gate:

Tracked separately in [acceptance issue #178](https://github.com/frankherchet/obdentic/issues/178).

The optional `HardwareAcceptanceRunner` in the test APK automates the live gate
(including five minutes of screen-off recording) only when explicitly invoked
with the intended powered adapter and vehicle available. It must find exactly
one Carly candidate and uses no hard-coded device identity. `RecordingAuditRunner`
audits existing phone recordings completely offline, without starting a service.
Both transfer evidence only over the local instrumentation connection; keep those
outputs private and publish only sanitized acceptance summaries.

1. Search for Carly, connect, inspect initialization TX/RX and supported signals.
2. Read real measurements, inventory and stored DTCs; verify responder attribution.
3. Run the EA189 DPF job only on the intended vehicle; retain unsupported/negative
   answers explicitly rather than treating them as zero values.
4. Record the drive profile while locked for at least five minutes. Stop it,
   export locally, replay on the phone and inspect the same file on the Mac.
5. Disconnect, reconnect and test adapter power loss. The UI must not retain old
   live values under a new connection or silently continue a failed recording.

Retain raw acceptance evidence locally. Publish only a sanitized pass/fail summary.
