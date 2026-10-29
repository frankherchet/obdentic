# Android workbench

OBDentic's Android app runs the shared Rust diagnostic core on the phone. Android
owns BLE/GATT and application lifetime; the Rust facade produces the same closed
Carly initialization, semantic reads and diagnostic requests as the desktop core.
The UI never accepts raw commands, addresses or mutation capabilities.

## First release

- Carly BLE scan, explicit connect/disconnect and recoverable permission errors.
- 50 standard semantic measurements, vehicle-advertised support, live trends and sample age.
- The Live tab shows signals as compact tiles (2 columns portrait, 3 landscape) with short name, value, sparkline and session min/max; a stale age ("vor N s") appears only after 10 s.
- VIN where readable and a **partial** functional responder inventory; this is not
  a full manufacturer installation list.
- Stored Mode 03 DTCs, scoped by responder.
- The engine baseline, drive and OBD expansion capture profiles.
- The `obd2-extended-validation` profile records additional standard scalars
  conservatively; unavailable optional values are not assumed supported.
- Private recording, foreground connected-device service, screen-off operation,
  explicit export, offline replay and timestamped raw TX/RX.
- Confirmed deletion of completed internal recordings; active recordings and loaded
  replays are protected. Exported copies remain separate.
- The display stays awake while the app is visible; background operation and manual
  locking remain available.
- Demo responses pass through the real Rust parser; the mode is always visible.

Pending/permanent DTCs, freeze frames, full OEM fault-memory scans and DTC clearing
are not implemented by this app. The old experimental hard-coded manufacturer
jobs were removed (#230). Manufacturer DID signals now come from pinned Knowledge
and require an identified, applicable ECU. Old recordings containing removed
actions still open in replay; those actions are skipped and never sent.

For a targeted read, selection uses the explicitly expected responder, never
the decoded value. Additional responders remain in the evidence. A missing
expected response or conflicting payloads from that same responder still fail;
an untargeted functional read remains ambiguous when responders disagree.

## Vehicle identity

Inventory includes an offline, partial WMI manufacturer hint derived from a
format-checked VIN. No VIN is sent to an external decoder. The initial sourced
snapshot uses NHTSA vPIC records; [source details and KBA limitations](research/vin-identity.md)
explain its incomplete European coverage. Model, engine and ECU compatibility
remain unresolved; the first three VIN characters never select an OEM profile.

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

A changed live signal selection is stored in the selected record (`selection`) and restored
when that record is selected again (and after a process restart), without adapter traffic;
demo and replay sessions never read or write it, and a new record gets the default
selection of supported signals. Without an active session the signal chooser marks each
signal from the record's last observation with its date ("unterstützt (zuletzt gesehen
22.10.2026)"); stored support never skips discovery or decides whether a read is sent, every
live connection still discovers support and the current observation replaces the stored one.

## Automatic connection

The app remembers the last successfully connected adapter (its address, in the app-private
`adapter` preferences; backup is disabled and the address is never exported or recorded) and
connects to exactly that address, directly through GATT and without a scan, when the app starts
and after an unexpected connection loss (REQ-020). A failed attempt is retried after 2, 5 and
10 seconds and then every 30 seconds; the status line shows when the next attempt follows. The
foreground service stays up while waiting, so retries also run with the screen off. A manual
"Verbindung trennen", the "Automatische Verbindung stoppen" button, starting the demo or opening
a replay ends the attempts; none follow until the user connects or the app is started again.
"Automatisch verbinden" on the Adapter tab (default on, persisted) switches the feature.
With Bluetooth off or a permission missing no attempt is made, the status line says why, and
attempts resume when Bluetooth turns on (or when the app returns to the foreground).

After an automatic connection only the existing initialization and inventory run, with the normal
vehicle-record handling; no earlier job is repeated. A recording interrupted by the loss stays
saved as a partial recording (`session_end` reason `error`); after the reconnection a new recording
with the same profile starts.

## Pending feedback

Buttons whose action finishes asynchronously (recording start/stop, search, connect,
disconnect, Diagnose jobs, export all, automatic export and connection, vehicle select/rename/delete)
react at once (REQ-021): the button is disabled, shows a pending text ("Aufzeichnung wird gestartet …")
with a progress indicator, and repeated taps start nothing. The state ends when the service snapshot
shows the target state or a message for the action arrives; without either after the limit (connect 30 s,
search 15 s, Diagnose jobs and export 60 s, everything else 10 s) the status line says "Keine Rückmeldung · <Aktion>"
and the button is usable again; a late result still updates the display. The pending entries
(`PendingActions`) live in `ObdService`, so they survive tab switches and Activity recreation; the pending
text is the button's content description and is announced, as is the final state. The mapping from action to
target state is the comment above the specs in `MainActivity`.

## Pending feedback

Buttons whose action finishes asynchronously (recording start/stop, test run, search, connect,
disconnect, Diagnose jobs, export all, automatic export and connection, vehicle select/rename/delete)
react at once (REQ-021): the button is disabled, shows a pending text ("Aufzeichnung wird gestartet …")
with a progress indicator, and repeated taps start nothing. The state ends when the service snapshot
shows the target state or a message for the action arrives; without either after the limit (connect 30 s,
search 15 s, Diagnose jobs and export 60 s, everything else 10 s) the status line says "Keine Rückmeldung · <Aktion>"
and the button is usable again; a late result still updates the display. The pending entries
(`PendingActions`) live in `ObdService`, so they survive tab switches and Activity recreation; the pending
text is the button's content description and is announced, as is the final state. The mapping from action to
target state is the comment above the specs in `MainActivity`.

## Knowledge ECU discovery and live signals

"Steuergeräte erkennen" probes the bounded manufacturer target list from pinned
Knowledge once with F187, then reads the standard identity plan of responding
targets. Responses from the expected identifier establish presence; a timeout
means "nicht beobachtet", never absent. The list and identity evidence stay in
the private vehicle record. The discovery has its own raw recording and offline
replay/report path. It does not change diagnostic sessions.

Applicable normal-session Knowledge values become selectable alongside standard
OBD2 measurements. The full-screen selector groups by ECU and signal group, searches
names/groups/units, limits selection to 32 and saves named vehicle-local presets.
Unconfirmed values show "nicht abgeglichen". Replay re-decodes raw DID responses
using the pinned Knowledge decoder and rejects mismatched Knowledge provenance.

"Sitzungs-Kandidaten lernen" reads each applicable session candidate once in the
default session, bounded to 120 seconds. Answered, NRC-rejected and unanswered
results stay distinct and are stored locally. Stopping a running native job
disconnects the adapter to interrupt the pending exchange and keeps a partial
recording; reconnect before starting another job.

"UDS: erweiterte Sitzung" explicitly activates the closed engine-ECU session
described in ADR 0003. The persistent indicator shows when it is active; OBD2
polling pauses while Knowledge DID reads remain available. Session candidates
are marked UDS and disappear when it ends. The core sends tester present while
active and ends the session after ten minutes, before ordinary functional
operations or on errors. On normal disconnect it attempts `10 01`; link loss
stops keepalive. An unconfirmed return blocks operations until reconnect. First
vehicle use is stationary, with engine off or idling, under the separate #178 gate.

## Adapter throughput (Diagnose)

"Adapter-Leistung messen" in the Diagnose tab runs a closed, read-only job (`adapter.throughput`, REQ-022). It
needs a live, ready session with the validated engine target and no running recording; in demo, replay or
without a connection the button is disabled and the reason is shown. Phase 1 reads up to 6 advertised catalog
PIDs 10 times each as single requests (median, 90th percentile, reads per second); phase 2 sends one Mode 01
request with 2, 3 and 6 of those PIDs (`01 0C 05 ...`, built only from advertised catalog PIDs) and reports each as
vollständig, teilweise or abgelehnt with its time. The reply is decoded per PID with the catalog decoders; a
missing or malformed PID is reported and never filled in. The job writes its own recording (profile
`adapter-throughput`, all TX/RX plus the result), ends after at most 60 s on its own (`truncated`), and the Rust core
has no cancellation: stop or disconnect closes the link, the pending read fails and the recording stays partial.
Opening the recording shows the same card, re-derived with `replay.adapter.throughput`; `obdentic-mobile-report`
prints the same summary. The demo answers 2 and 3 PIDs and rejects 6; real vehicle answers are covered in #178.

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

The debug build installs as `de.obdentic.app.debug`; the Firebase release build is
`de.obdentic.app`. Both coexist on the same phone, each with its own data (vehicle
records, settings), and the release build is the daily app. The debug build is for
development and desk acceptance only. The recording provider authority follows the
installed app id (`<app id>.files`); the shared `Download/OBDentic` export folder works
for both.

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

**Dauer-Aufzeichnung (REQ-028):** every live connection attempt is recorded
automatically, from before the first adapter byte, so initialisation, support
discovery and inventory TX/RX are in the file. The file `…-live-auto.jsonl` has the
normal `OBDENTIC-MOBILE` v1 format with profile `session-auto`; it ends on
disconnect, error or when the switch is turned off, and a failed attempt still leaves
a file with its error marker. A new file starts every 30 minutes or 10 MB
(`session_end` reason `rotated`). A named recording or test sequence closes the
automatic file and the automatic recording continues after it ends. The switch and
**Automatische Aufnahmen löschen …** are on the recordings tab. The app never deletes
a recording on its own; demo and replay are never recorded automatically. Replay
and `obdentic-mobile-report` re-derive the vehicle and signal support from the
recorded handshake.

Each saved recording has **Aufnahme löschen …**. Confirmation permanently deletes
only the phone's internal file; exported copies remain. Stop an active recording
first, or leave a loaded replay before deleting its file.

Recordings live in the application's private `recordings/` directory. Cloud
backup and device transfer are disabled. Recordings can contain VINs, ECU
identifiers and mileage, so nothing leaves the app without an explicit choice.
The **EXPORT** card on the recordings tab offers:

- **Datei exportieren …** (per recording): Android's system file picker saves one copy.
- **Export-Ordner wählen …**: choose a folder once with Android's system folder
  picker, after a privacy notice. The choice is remembered across restarts and shown
  on the card. Recommended: `Download/OBDentic`, the default source of
  `tools/pull-recordings.sh` (added by a separate change), so USB or wireless adb,
  Syncthing or similar can collect the files.
- **Alle exportieren**: copies every completed recording that is not yet in the
  folder with the same name and size, skips present ones and reports copied, skipped
  and failed counts. A same-named file of a different size is reported as failed and
  never overwritten. The recording being written is never exported.
- **Automatischer Export: aus/an**: off by default. When on, each recording is
  copied right after it ends (stop, completion, disconnect or error).
- **Aufnahmen teilen …**: hands all completed recordings to the Android share sheet
  as read-only content URIs.

If the folder was deleted or its permission revoked, export stops with "Export-Ordner
nicht verfügbar · bitte neu wählen", the stored choice is cleared and nothing
internal changes. A copy that fails midway removes the incomplete target file.
Copies are byte-identical; export never deletes, moves or modifies the internal
recordings. It needs no storage permission and no network: the application's
permissions are unchanged (no `INTERNET`, no storage permission), using the Storage
Access Framework folder grant and the internal read-only content provider.
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

## Pull recordings to the computer

Choose `Download/OBDentic` as the app's export folder; that is the script's
default phone folder. Then pull everything that was exported into the untracked
`evidence/phone/` directory:

```sh
rtk tools/pull-recordings.sh            # copy new recordings, skip existing ones
rtk tools/pull-recordings.sh --report   # also run obdentic-mobile-report on new files
```

The script reads from the phone only (`adb devices`, `shell ls/stat/test`, `pull`)
and refuses destinations that git does not ignore. It needs exactly one adb device,
skips files that already exist locally with the same name and size, and labels each
copied file `complete` (it ends with `session_end`) or `partial`. Partial files are
kept. `--source` and `--dest` override the phone folder and the git-ignored local
directory. `tools/test-pull-recordings.sh` tests the script against a stubbed `adb`.
It works with debug and release builds because it never uses `run-as`.

**Without a cable (wireless debugging, Android 11+):** In the developer options,
enable *Wireless debugging* and choose *Pair device with pairing code*. Then pair
once and connect:

```sh
rtk proxy adb pair <phone-ip>:<pairing-port>    # enter the shown code
rtk proxy adb connect <phone-ip>:<debug-port>
rtk tools/pull-recordings.sh
```

The phone and the computer must be on the same network. Disconnect any USB cable
first, otherwise the script sees two devices and refuses.

**Automatic sync (optional):** With automatic export enabled in the app, Syncthing
can mirror `Download/OBDentic` to a folder on the computer under `evidence/`. The
transfer is end-to-end encrypted, peer to peer, with no cloud storage. Keep the
target inside `evidence/` so recordings stay untracked.

## Distribute test builds (Firebase App Distribution)

Pushing a tag `vX.Y.Z` on a commit of `main` runs
`.github/workflows/android-release.yml`. It only runs when the existing CI check
already succeeded for that commit (otherwise it stops and distributes nothing),
builds a release APK signed with the project key (versionName `X.Y.Z`,
versionCode = workflow run number), verifies it with `tools/verify-release-apk.sh`
(not debug-signed, not debuggable, not the test APK, no captures, exactly the
permissions of `AndroidManifest.xml`) and uploads it with the Firebase CLI to the
tester group. Pull requests, branch pushes and untagged commits never sign or
upload. The app contains no Firebase SDK and no internet permission; the App
Tester app on the phone does the notifying and installing. Local debug builds are
unchanged and need no keystore; `:app:assembleRelease` without the signing
environment fails on purpose.

One-time setup (by you, in the Firebase console and GitHub):

1. Create a Firebase project, add an Android app with package `de.obdentic.app`,
   and under App Distribution create a tester group (default name `testers`) with
   your e-mail.
2. Create a service account with the role "Firebase App Distribution Admin" and
   download its JSON key.
3. Create the release keystore outside the repository and back it up (a lost
   keystore means no further updates over the installed app):

   ```sh
   keytool -genkeypair -v -keystore obdentic-release.jks -keyalg RSA -keysize 4096 \
     -validity 10000 -alias obdentic
   base64 -i obdentic-release.jks | pbcopy
   ```

4. Add repository secrets: `ANDROID_KEYSTORE_BASE64` (the base64 above),
   `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` (`obdentic`),
   `ANDROID_KEY_PASSWORD`, `FIREBASE_SERVICE_ACCOUNT_JSON` (the JSON key's
   content) and `FIREBASE_APP_ID` (like `1:123456789:android:abc123`, from the
   Firebase project settings). Optional repository variable
   `FIREBASE_TESTER_GROUP` (default `testers`).
5. On the phone install Firebase App Tester and accept the invitation. The first
   distributed build has a different signature than the debug build you have now,
   so Android needs one uninstall: export your recordings first (uninstalling
   deletes all app data). Later versions install as updates and keep their data.

Release a version:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

If the upload fails (network, quota), re-run the workflow; it uploads the same
version again. Missing secrets are listed by name in the failing step.

## Acceptance

Live cards and the signal chooser show short German names from `SignalNames` (no semantic ids, at most two decimals); ids stay in the Debug tab.

Build and install the instrumentation APK, then run it with the phone unlocked:

```sh
rtk env JAVA_HOME="/path/to/jdk/Contents/Home" \
  ANDROID_HOME="$HOME/Library/Android/sdk" \
  ./android/gradlew -p android :app:assembleDebugAndroidTest
rtk proxy adb install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
rtk proxy adb shell am instrument -w de.obdentic.app.debug.test/de.obdentic.app.AcceptanceRunner
```

Grant Bluetooth and notification permissions before running acceptance. The
runner exercises the real JNI/core with a simulated adapter, profile recording
(including background operation after pressing Home), export provider, offline
re-decoding, reconnect, Activity recreation and automatic connection (a fake adapter link and fake
Bluetooth/permission state; it switches automatic connection off for the run and restores the
user's known adapter and setting afterwards). The default run does not turn
the screen off or lock the phone. The real screen-off check is opt-in: add
`-e screenOff true` to the `am instrument` command (the phone is left locked
afterwards). It emits synthetic screenshots and a demo recording as instrumentation
artifacts (also stored in the app-specific acceptance directory). Passing this test
does **not** establish live BLE or vehicle acceptance.

Real vehicle acceptance remains a separate required gate:

Tracked separately in [acceptance issue #178](https://github.com/frankherchet/obdentic/issues/178).

The test APK is `de.obdentic.app.debug.test` and instruments the debug app
`de.obdentic.app.debug` (the release app is never instrumented). The optional
`HardwareAcceptanceRunner` in the test APK (`am instrument -w
de.obdentic.app.debug.test/de.obdentic.app.HardwareAcceptanceRunner`) automates the live gate
(including five minutes of screen-off recording) only when explicitly invoked
with the intended powered adapter and vehicle available. It must find exactly
one Carly candidate and uses no hard-coded device identity. `RecordingAuditRunner`
(`de.obdentic.app.debug.test/de.obdentic.app.RecordingAuditRunner`) audits existing phone recordings completely offline, without starting a service.
Both transfer evidence only over the local instrumentation connection; keep those
outputs private and publish only sanitized acceptance summaries.

1. Search for Carly, connect, inspect initialization TX/RX and supported signals.
2. Read real measurements, inventory and stored DTCs; verify responder attribution.
3. Retain unsupported/negative answers explicitly rather than treating them as zero values.
4. Record the drive profile while locked for at least five minutes. Stop it,
   export locally, replay on the phone and inspect the same file on the Mac.
5. Disconnect, reconnect and test adapter power loss. The UI must not retain old
   live values under a new connection or silently continue a failed recording.

Retain raw acceptance evidence locally. Publish only a sanitized pass/fail summary.
