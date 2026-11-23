# Android workbench

OBDentic's Android app runs the shared Rust diagnostic core on the phone. Android
owns BLE/GATT and application lifetime; the Rust facade produces the same closed
Carly initialization, semantic reads and diagnostic requests as the desktop core.
The UI never accepts raw commands, addresses or mutation capabilities.

## First release

- Carly BLE scan, explicit connect/disconnect and recoverable permission errors.
- 50 standard semantic measurements, vehicle-advertised support, live trends and sample age.
- The Live tab shows signals as compact tiles (2 columns portrait, 3 landscape) with short name, value, sparkline and session min/max; a stale age ("vor N s") appears only after 10 s.
  Ordinary OBD2 tiles omit a redundant protocol line; mixed extended-session polling still labels OBD2 beside UDS. Compact
  vertical padding and 20 dp sparklines retain the eight-complete-tile portrait acceptance without shrinking value text.
- VIN where readable and a **partial** functional responder inventory; this is not
  a full manufacturer installation list.
- Stored Mode 03 DTCs, scoped by responder.
- The engine baseline, drive and OBD expansion profiles of the catalog, offered as built-in
  presets ("Vorlage · Motor-Grundwerte", "Fahrt", "OBD-Zusatzwerte", "Weitere OBD-Werte") under
  Live → Messwerte auswählen → Preset anwenden (REQ-025.6); unsupported signals are skipped.
  There is no recording-profile chooser: "Gewählte Messwerte aufzeichnen" below the Live tiles starts a
  recording of the selection; while recording, "Aufzeichnung stoppen" stays above the tiles (REQ-025.7).
- Five OBDentic component presets ("dpf", "egr", "cooling", "turbo", "rail", German names) in the same
  "Preset anwenden" list (REQ-035) and the user-started smart analysis (REQ-036), see below.
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

**Verfügbarkeit (REQ-030):** UDS DIDs are not advertised by the ECU, so the app measures them. The
job `knowledge.availability` (Diagnose tab, "Verfügbarkeit prüfen") sends one targeted `22` read
per default-session Knowledge signal of the identified ECUs (about 25 s, hard limit 90 s, no
session change) and classifies each as answered, rejected (with the NRC) or not answered.
Answered entries also carry the raw response, the decoded value and unit, and an `implausible` flag
with the range when the value lies outside the definition's `plausible_range` (REQ-030.8). The job
also reads each catalog Mode 01 PID the vehicle advertises as supported once from the engine
target (REQ-030.9); `obdentic-mobile-report` prints a per-entry table and status counts (REQ-030.10). It runs automatically after ECU identification for a newly created vehicle record
and after every "Fahrzeug und Steuergeräte erfassen" job, writes into the running automatic recording
(its own recording only when none runs) and is stored
with the vehicle record (never exported or shared). Replay restores the statuses from the recording.
The ECU identification itself (`ecu.discover`, about 18 s) runs after every live connection whose
inventory identified the manufacturer, also for a known vehicle, because the Knowledge signals only
exist in the session that identified the ECUs; a stored selection of Knowledge signals is applied
again afterwards (REQ-030.7). If the multi-frame VIN answer arrives truncated, the inventory reads it
once more and keeps the manufacturer identified earlier in the connection (REQ-032).

The signal selector shows each Knowledge signal's measured availability ("antwortet", "lehnt ab
(NRC 0x31)", "ohne Antwort", "ungeprüft"; the stored result of the vehicle record is dated) and a
button "Nur verfügbare: an" (default) that hides PIDs the ECU does not report and signals measured as
rejected or silent, except already selected ones; every listed signal is selectable. A selected
signal without a value shows "Nicht verfügbar" with its reason on the tile (REQ-031).

A Knowledge signal is identified by `key` (`<semantic>@<target id>`), not by its semantic:
the same semantic can also be a standard PID (for example the engine oil temperature) or
exist on several ECUs. The Android app uses the key as the signal id for selection, polling,
values and recordings and keeps the semantic as `meaning` for display names; standard PIDs
keep their plain semantic.

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
and after an unexpected connection loss (REQ-020). A failed attempt is retried after 2, 5, 10,
30, 60 and 120 seconds and then every 5 minutes; bringing the app to the foreground starts a waiting
attempt at once and the pauses start again at 2 seconds (REQ-020.10); the status line shows when the next attempt follows. The
foreground service stays up while waiting, so retries also run with the screen off. A manual
"Verbindung trennen", the "Automatische Verbindung stoppen" button, starting the demo or opening
a replay ends the attempts; none follow until the user connects or the app is started again.
"Automatisch verbinden" on the Adapter tab (default on, persisted) switches the feature.
With Bluetooth off or a permission missing no attempt is made, the status line says why, and
attempts resume when Bluetooth turns on (or when the app returns to the foreground).

**Fehlerspeicher beim Verbinden lesen (REQ-034):** after every successful inventory of a live
connection (also after an automatic reconnect), once the automatic ECU identification and
availability jobs are done, the app runs the existing read-only `dtc.scan` once: stored (Mode 03)
and pending (Mode 07) codes per responder, or "keine Fehler"; a negative response, NO DATA or
timeout stays an explicit result and never ends the connection. Its TX/RX goes into the running
automatic recording and replay shows both lists without traffic. Switch on the Adapter tab
(default on, persisted). Demo and replay never scan; nothing is ever cleared (no Mode 04).

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
described in ADR 0003 (amended for mixed polling). The explicit switch stays; the
explicit apply of a component preset may also activate it (REQ-035.3, below).
Selecting UDS signals alone does not switch the session.
The persistent indicator shows the app's view of the session; it is not proof
from the ECU, because the keepalive reply is suppressed. While active, the
selected standard Mode 01 PIDs and the selected UDS DIDs are polled together in
one serialized loop; OBD2 polling no longer pauses. Session candidates are
marked UDS and disappear when it ends, while the OBD2 selection keeps polling.
Successful standard reads keep the session; a rejected, unanswered or malformed
read of either type is shown as such, never filled in, and ends the session
(return to default when possible). Ordinary Knowledge DIDs of the same engine
ECU may continue; Knowledge reads of another ECU exit the session first. A
missing or mismatched validated engine target returns the session to default
without a Mode 01 read. Recordings keep raw bytes and mark the session state of
every read, Mode 01 included. The core sends tester present at most every 2 s
while active (due maintenance is attempted between reads) and ends the session
ten minutes after activation, before other functional operations (DTC reads,
adapter throughput, candidate learning, sweeps) or on errors. Selected Mode 01
polling is not such an operation. On normal disconnect it attempts `10 01`; link
loss stops keepalive. An unconfirmed return blocks operations until reconnect.
Whether the ECU keeps answering Mode 01 in the extended session and how the
interleaved timing behaves is not yet vehicle-validated; first vehicle use is
stationary, with engine off or idling, under the separate #178 gate.

**Komponenten-Presets (REQ-035).** "Preset anwenden" also offers five built-in presets
owned by OBDentic - `dpf`, `egr`, `cooling`, `turbo`, `rail`, each with 5 to 12 entries.
Knowledge, profiles, vehicle presets, MCP calls and AI cannot add or change one. An entry
names a standard-PID semantic or the original meaning of a Knowledge signal, never a command
or DID. It is resolved against the effective Knowledge of the connected vehicle: a Knowledge
signal becomes the ordinary catalog entry whose semantic field holds its unique key
(`semantic@target`) and whose meaning field keeps the original semantic; selecting, reading,
recording and replay use that key. An ambiguous entry selects nothing. After applying, every
entry shows its own status and "x von y verfügbar" (not supported by the vehicle, no matching
Knowledge, ECU not identified, ambiguous, rejected, not answered, needs UDS session); an
unavailable entry is never dropped, never shows a value or zero and is not requested.
Session-flagged definitions are not ordinary selectable entries before the session is active.
Applying is refused while a recording runs and in replay.

Only the explicit apply in a ready live session may activate the session automatically,
once per apply, when an entry exists solely as a session-flagged DID of the engine ECU
(entries of other ECUs stay unavailable). It is the closed operation above (exactly `10 03`,
indicator, tester present at most every 2 s, ten minutes, no additional service) and never
runs at start, connection, reconnect, restore, for vehicle presets or profiles, in demo or
replay, while a job runs or from smart analysis; it is never retried. If it is refused or
fails, the default-session entries are still selected and the UDS entries show the reason; when
the session ends or expires the UDS entries become unavailable with that reason, the OBD2
selection keeps polling, and `10 01` is attempted. The recording carries the preset id,
the per-entry resolution, that a preset requested the activation and the session state of
every read; replay shows the same offline. Behaviour on the vehicle (answers, activation,
cycle time of up to 12 signals) is not validated and belongs to the separate #178 gate.

**Smart-Analyse (REQ-036).** The user starts and stops it in the Live tab in a ready live
session; one action starts it. It continues the running automatic recording or, with
"Dauer-Aufzeichnung" off, starts one itself and fails visibly if none can be written; stopping
leaves the setting unchanged. It is never automatic and is refused (with the
reason) in demo, replay, while an extended session, a job, a named recording or a test
sequence runs. It stays in the default session, sends no session change and needs no
interaction while driving. Every set is the available signals of one component preset plus
all available trigger readings (raw `dpf.field_regeneration_request_status`, coolant
temperature, `vehicle.accelerator_pedal_e`, speed, rpm, load), within 32 signals. Missing,
stale (older than 10 s) or unavailable readings are shown as such and never treated as zero,
a crossing, a release or a holding condition; they do not start or extend a window.

| Window | Starts / lasts |
|---|---|
| `dpf` | raw regeneration request status ≠ 0 until a fresh 0; shown as "Regenerationsanforderung (Rohwert)", not a confirmed regeneration |
| `cooling` | upward crossing of 80 °C between consecutive fresh readings; 10 min; re-arms after the condition is false |
| `turbo` | pedal > 80 % and 20 s after the first fresh reading ≤ 80 % |
| `rail` | speed 0, coolant ≥ 80 °C, rpm 500..1100; 3 min; re-arms after the condition is false |
| `egr` | speed 50..80 km/h and load 15..60 % without interruption for 30 s, then 3 min; re-arms after the condition is false |

Priority is dpf > cooling > turbo > rail > egr: a higher window interrupts a lower one, windows
keep their own time while interrupted, a still-valid lower window resumes and an expired one does
not. With no valid window the sets rotate in that order, 3 min each. Each set change (with time,
reason and the trigger readings behind it) is shown in the Live tab, written into the running
automatic recording (across file rotation) and replayed offline. The latency from trigger
reading to changed selection depends on the serial adapter and is reported, not promised.
Results stay in the private recording and the app's memory: no trigger value ever goes into
the app log (only set names and reasons), and nothing is exported, shared or published as
vehicle evidence automatically. Behaviour while driving is not validated and belongs to the
separate #178 gate.

**Messlücken (REQ-037).** In a ready live session the Live tab shows "Keine aktuellen
Messwerte" when selected reads have produced no finite value for at least 10 seconds while
reads are actually being attempted, but only after a selected signal has produced a finite
value at least once in the session. Unsupported, unselected or never-successful signals alone
never start a gap, and individual signal errors next to successful measurements do not. The
notice states only that no values arrive. It does not claim that the ignition is off or that
the adapter or vehicle has disconnected: the cause is not known and the app does not infer it
(no ignition state is asserted anywhere). Missing and stale trigger values stay unknown, never
zero and never proof that an event did not occur.

"Value" here means only a finite decoded result. It is not proof that the reading is plausible:
a numeric sentinel that decodes to a finite number still ends or prevents a gap and can never
establish vehicle health. Detecting malformed or implausible decoding is a separate concern
(#293) and is not part of the gap logic.

The 10 seconds are a minimum, not a promise. Detection also waits until every currently
selected, previously successful, eligible signal has been attempted at least once without a
finite result since the last finite value. This prevents false gaps with slow polling profiles
(intervals of 15 to 30 seconds), so with such profiles a gap can be marked noticeably later
than 10 seconds after the last value.

While a gap is active the app stops polling the full selection. It probes one currently
selected signal that already returned a finite value (the selected known-working signal) at
most once every 5 seconds, using the ordinary read-only signal read: no new vehicle operation,
no background connection and no changed session. The first finite result from any selected OBD
or UDS measurement ends the gap and restores normal polling automatically; no user action is
needed. Selection changes, stop, reconnect, failure, jobs and the extended-session keepalive
and cleanup stay responsive and safe while a gap is open.

Each gap is written into the running automatic recording as the existing action event
`measurement.gap` (empty argument, result `{"ok":true,"data":{"change":…,"reason":…}}`; the
record's own `at` and `offset_us` give the time). `start` with `no_values` opens a gap;
`end` with `recovered` closes it on the first finite value; `end` with `stopped` closes it when
the recording stops and claims no recovery. When the file rotates (REQ-028.3) during an open
gap, the new file begins with `resume` and `no_values`, so every part is understandable on its
own and the true start stays in the earlier file. The `start` timestamp is the time the gap
was detected, not necessarily the exact moment of the last finite value or of the first failed
read. A reported gap duration is therefore a lower bound and can miss the initial 10 seconds or
more; the raw read events in the same recording allow offline correlation of the true last
value and first failure. Offline replay and
`obdentic-mobile-report` show the gaps from the recording without adapter traffic and keep the
raw TX/RX evidence; recordings made before this change contain no such events and open as
before. DPF priority in Smart-Analyse is unchanged and remains covered by its existing
deterministic replay tests. Gap behaviour on a real vehicle (how long values are missing,
whether the first probe recovers) is not validated and belongs to the separate #178 hardware
acceptance; until then the thresholds above are the specified behaviour, not measured results.

## Coverage facts in the recording (REQ-038.9)

A Smart run writes the facts that rebuild its run ledger (`coverage_ledger.rs`, REQ-038) into the running
automatic recording, as three action events in the existing envelope (empty `argument`, result
`{"ok":true,"data":{…}}`; the record's own `at` is the wallclock time and `offset_us` the file offset; the header
format version stays 1):

- `coverage.run`: `action` `start` (`part` 1) or `resume` (`part` = previous + 1, `previous_part_records`), `run`,
  `goal_definition`, `knowledge_revision`, `run_offset_us`. The run id is a random token created at each Smart start;
  it names no vehicle or adapter. `run_offset_us` is the monotonic run clock since the Smart start and is never reset
  by a rotation.
- `coverage.fact`: one record per read attempt, written right after the raw `read` record and classified by the Rust
  core (`coverage.fact`, a closed read-only action without transport access): `signal_key`, `context` (`baseline`),
  `ecu_target`, `session` (`default`/`extended`), `outcome` (`decoded` with `value_kind` and `finite`/`nonempty`,
  `unsupported`, `rejected` with `response_code`, `empty`, `error`/`no_answer`/`session_restricted` with `reason`),
  `implausible`, `static_identity`, `identity_confirmed`, versions, `run_offset_us` and `evidence` (the `at` and
  `offset_us` of the raw read record, plus `in_previous_file` when a rotation fell between the read and the fact).
  A fact never carries the value; it stays in the raw `read` record. Nor does it carry the text of an error or of a
  wire line, which can quote a VIN, an ECU serial number or an adapter identifier: a `reason` is one of a closed set
  (`timeout`, `no_data`, `session_state`, `malformed_response`, `read_failed`, `no_usable_answer`,
  `undecodable_value`, `classification_failed`) chosen by the classifier. Replay rejects any reason outside this vocabulary
  too; it never republishes arbitrary recorded error text. A standard Mode 01 negative response (`7F 01 <nrc>`) is a
  `rejected` fact with its `response_code`, never an `error`: the core adds it to the failed
  read as `negative_response` (`{"service":1,"nrc":…}`, taken from the same observation that stays in the raw
  record) and the classifier reads that structure, not the error text. A read whose classification fails is still
  written, as an `error` fact with the reason `classification_failed`, because a lost failure could let the two
  readings around it count as one continuous segment.
- `coverage.boundary`: `class` `gap` (with the start of a `measurement.gap`), `disconnect` (before the end of a
  session by disconnect or link failure), `run_end` (with the Smart stop; always the run's last record) and
  `reconnect` (accepted on read, not written yet).

A rotation (REQ-028.3) keeps a Smart run going: the new file starts, after the header and the `smart.set` and
`measurement.gap` resume records, with `coverage.run` `resume`, so every file is understandable on its own and a long
run is a chain of parts. `previous_part_records` is the number of `coverage.*` records written into the previous part
(its own `start`/`resume` included). Parts are identified by run id and `part`, never by file name, file time or
order.

Replay and `obdentic-mobile-report` never re-classify raw reads: they fold only the recorded `coverage.*` records
through the counting module (`SmartRun::fold` with the replay session kind, which can never credit a vehicle
ledger) and send nothing. One function, `mobile::coverage::derive`, validates, orders and folds the parts of a run for
both. A join of two consecutive parts is verified when the earlier part ended `rotated`, the record count matches and
the run clock does not go backwards; only then may a segment span the rotation. A run is complete
(`run_complete: true`, `figures: "exact"`) only when part 1 through the part with `run_end` are all supplied and
every join is verified. Otherwise it is marked partial in the data (`partial: true`, `figures:
"supplied_parts_only"`, `partial_reasons` from `missing_start`, `missing_parts` with `missing_parts: [numbers]`,
`unverified_join` and `no_run_end`) and no segment is bridged over a missing or unverified join (a derived break, never
written to a file). A single rotated part is partial; a rotated file's `complete: true` never implies a complete run.

A coverage record that cannot be trusted is never skipped, because a skipped boundary or failed read could overstate
the valid time. It invalidates the run it may affect (`coverage_invalid: true` with the reason in `issues`, no counts,
time or status), or every run of the set when its run cannot be read: missing or corrupt envelope type, `ok` not true,
`data` not an object, unknown action, outcome, boundary, session or run action, missing or backwards `run_offset_us`,
an orphan or duplicate record (including the same signal/read evidence repeated across a rotation), a malformed `part`,
part number above 64, two parts with the same number (also the same file twice), a `previous_part_records` that differs
from the real count, offsets running backwards across a join, a record after `run_end`, more than 64 parts. A
Knowledge revision that differs from the embedded pin in any part stops the derivation (`knowledge_pin_mismatch`);
the raw evidence stays available. Recordings without `coverage.*` records (every file written before this change) open
as before and contribute no coverage (`coverage: null`).
Starting a new live or demo session clears the prior replay's coverage; only opening a recording derives it again.

The report takes several files: `obdentic-mobile-report part1.jsonl part2.jsonl part3.jsonl` prints
`{"schema":"OBDENTIC-MOBILE-REPORT-SET","version":1,"files":[…],"coverage":{…}}`, and the `coverage` section is the
same whatever the argument order. One file prints its report with the `coverage` of that file as a one-part set.
Replay scans every automatic recording in the app's folder under the recording's size and line limits for run ids,
including markers after earlier runs. It includes related files transitively (`replay.coverage.part`, `replay.coverage`;
at most 65 supplied files, so an excessive run is invalidated). A deleted or unavailable part is not supplied, so the
run is partial. The result is kept in the service state as `coverage` and shown on the Live tab (see below).

## Vehicle coverage ledger and Live card (REQ-038.6, REQ-038.8, REQ-038.10)

While Smart runs, the core keeps a run ledger and, when the identity is confirmed (the VIN of the session is the selected record), the vehicle ledger of
that record. Both are fed from the very records that go into the recording (`coverage.run`, `coverage.fact`, `coverage.boundary`), so live, replay and report
count the same way; the closed core actions `coverage.live.start|record|snapshot|export|detach_vehicle|end` and `coverage.ledger.summary` send nothing to
the adapter. The vehicle ledger is stored as `coverage_ledger` inside the vehicle record of `vehicles.json` (app-private, excluded from backup, never
exported, shared, logged or part of the service state); deleting the record deletes it, recordings stay. It is stored when a run ends, before a lost link
takes the core away, and about every 30 s during a run, so a killed app loses at most that interval.

A ledger that cannot be read (unreadable store, malformed entry, unknown version) is never replaced: the run still counts in its run ledger, the card says
"Fahrzeug-Abdeckung nicht verfügbar" with the reason (never a zero), measurement and recording go on, and the stored data stays as it was. A run whose
records could not all be folded in is shown as unreliable without figures and does not store. A failed write is announced once and the stored ledger is not
changed. Identity that is only confirmed after the start is not credited to the vehicle in that run.

The Live tab shows an "ABDECKUNG" card below the tiles and the Smart card (the first eight tiles keep their space, #239): per goal context (today only the
"Technische Basisabdeckung") the covered, partial and not covered counts, the summed measured time, the longest segment and the latest non-success states,
as plain text for the run, the vehicle in total and, without a run, the selected vehicle's stored ledger. In replay it shows the derived run; a partial set
says "nur die gelieferten Teile, nicht der ganze Lauf" with its reasons and an invalid one only its reason. Nothing here is a diagnosis or a health claim.
Simulated acceptance: `ComponentSmartAcceptance`; behaviour on the vehicle belongs to #178.

## Semantic assistant tool bridge (REQ-041)

`MobileCore` offers an in-process, versioned tool interface for a future assistant runtime. It is the contract
and safety validator of `src/semantic_tools.rs` plus the executor in `src/mobile/assistant.rs`; there is no
model client, account, network, UI or MCP server in it, and it needs none to work. Everything goes through
`NativeCore.execute` and so through the one session owner:

- `assistant.tools` lists the five tools (version 1): `vehicle_info`, `signal_list`, `signal_read`, `dtc_scan`,
  `recording_inspect`. Arguments are closed: `signal_read` takes exactly `signal`, an opaque id from `signal_list`;
  everything else takes none. A semantic snapshot tool is not part of version 1 (#332).
- `assistant.tool` runs one call, `{"tool":…,"arguments":{…}}`. The answer is always `ok:true` with an `outcome`:
  `ok`, `refused` (with `reason` `unknown_tool`, `malformed_arguments`, `unexpected_argument`, `unknown_signal`,
  `ambiguous_signal`, `expired_signal`, `unsupported`, `unsafe`, `session_change`, `no_diagnosis`), `unavailable`,
  `busy`, `disconnected`, `cancelled`, `budget_exhausted` or `failed`. Every refusal and typed outcome is decided
  before the first adapter byte. A request that names a raw, write, session, addressing or rate operation is
  `unsafe`; nothing a model writes can reach the transport.
- Ids are issued per listing and bound to the session; initializing, inventory, ECU discovery, an availability
  measurement, a UDS session change or a reconnect expires them. Only standard PIDs the vehicle advertises and
  Knowledge signals of identified ECUs are listed, minus signals the last availability job measured as rejected or
  silent. ECU identification (serial, part and software numbers) and the VIN are never listed or returned.
- `assistant.begin` starts a diagnosis: at most 20 live signal reads and one DTC scan, and no new live work after
  60 s. `NativeCore.cancel(handle)` cancels without waiting for the session owner; a read or scan already in
  flight finishes under its normal timeout and nothing further starts (a scan skips its pending-code request).
  Live tools without `assistant.begin` are refused. `assistant.*` actions never queue behind a running job: a busy
  session owner answers a tool call with the typed `busy` outcome, and `assistant.begin` and
  `assistant.recording.select` with `ok:false` (they did not run, so a selection stays as it was).
- Live tools reuse the code of the user's own reads and scans. A tool that would end an active extended session
  (a scan, or a read outside the session's own engine target) is refused with `session_change`; the existing safety
  cleanup after a failed read still applies.
- Each value carries `provenance`: evidence class (`observed`, `decoded`, `inferred`, `unknown`), source (`live` or
  `recording`), responder, timestamp, age, `freshness` (`fresh`, `stale` from 10 s, `unknown`) and, for a recording,
  the capture position. Unknown parts are `null`, never zero.
- `assistant.recording.select` (platform only, never a tool argument) names the user's recording; `recording_inspect`
  summarizes it through the same offline report as `obdentic-mobile-report` (`mobile::report`), without Bluetooth, and
  stops with `knowledge_pin_mismatch` when a recorded Knowledge repository or revision differs from the embedded pin.

Software verification uses the scripted adapter and synthetic recordings (`src/mobile/acceptance_tests/assistant_tests.rs`).
The JNI route is covered by the build only; nothing here is real-phone, adapter or vehicle evidence, which stays
with #330.

## ChatGPT account (REQ-042, REQ-043, REQ-044)

An **optional** ChatGPT account lets eligible AI requests use the user's ChatGPT plan (epic #317, ADR 0005). It is the app's first and
only network feature: connecting sends no vehicle data, nothing in the app needs the account, and live measurement, recording, replay,
export and diagnostics work unchanged without an account or network (REQ-044.1). The account code has no path to the vehicle session,
the adapter transport or the diagnostic jobs (REQ-044.4); it makes no request at start, only after **Continue with ChatGPT**, after the
disconnect confirmation, and when a caller (the assistant, #328) asks for an access token.

- **Where:** Adapter → **Konto öffnen** (two taps, no vehicle needed) opens the account section. It shows exactly one status:
  *Nicht verbunden*, *Verbunden* (with the account e-mail address, only here, and the indicator **Using ChatGPT plan** with the link
  **Manage usage** → `https://chatgpt.com/settings/usage`), *Anmeldung abgelaufen*, *ChatGPT nicht erreichbar* (credentials kept) or
  *ChatGPT-Plan-Nutzung nicht freigegeben*. The indicator and link are one view (`PlanIndicatorView`) that the assistant screen can reuse.
  The first connection with the plan permission shows the one-time dialog **You're using your ChatGPT plan** (**Got it**).
- **Sign-in** (`ChatGptAccount`, `Oidc`, `LoopbackListener`): the system browser, a loopback callback on `127.0.0.1:1455` (another free
  port only if taken) and PKCE S256, platform APIs only, no Gradle dependency. `client_id` is `dynamic_agent_client` with
  `agent_name_hint=OBDentic` on the first registration and the saved issued client id afterwards; a random `ext_agent_host_id` is created
  once. Scope: `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, `resource=https://api.openai.com/v1`.
  The callback page is static, never echoes its parameters and links back with `obdentic://account` (`ReturnActivity`, which takes no
  value from the intent). The listener serves at most 8 connections at once, cuts a request after 3 s and closes accepted connections when
  the attempt ends. State, error, code and client-id checks end the attempt without a code exchange; a cancel ends it within 1 s, also when the callback already arrived (also
  while the listener is still being opened, and then without launching the browser), 300 s without a callback end it with a timeout message.
  A sign-in whose credential cannot be stored is revoked and leaves no file or key behind. The ID token is accepted only with an RS256 signature from the discovery
  document's JWKS, issuer `https://auth.openai.com`, the client id as audience, an expiry in the future (60 s leeway) and the attempt's
  nonce.
- **Plan permission missing:** a valid sign-in whose granted scope lacks `chatgpt.tokens.use.direct` is kept (as OpenAI advises) but
  inactive: no access token is handed out and no inference request is possible until **Continue with ChatGPT** repeats the
  authorization with `prompt=consent`. A repeat that returns another subject than the stored one is rejected: the stored account stays and the
  new credential is revoked.
- **Storage** (`AccountStore`): one app-private file encrypted with an AES-256-GCM key of the Android Keystore (fresh IV per write,
  temporary file, sync, atomic rename), excluded from backup and device transfer. After a sign-out, expiry or an unreadable record only the
  host id and the issued client id remain; the Keystore key is deleted. No token, e-mail address, authorization code, verifier, state,
  nonce, URL or header is ever logged: events use a fixed vocabulary and the log masking also replaces JWT-shaped and `Bearer` text.
- **Refresh:** an access token valid for at least 300 s is used as is; otherwise one serialized refresh (never before
  `earliest_refresh_at`, also after the access token expired) rotates the tokens. Before sending a refresh, the encrypted record
  is atomically saved with `rotation_pending: true`; a failed marker
  write sends no request. A valid replacement clears the marker in the same atomic record write. If that write fails, the newest
  tokens remain in memory, the access token is delivered and storage is retried on later access. After process death, a pending
  marker means the stored token may already be spent: even HTTP 200 for its revocation cannot confirm that the replacement grant
  was revoked. Lost responses retain the marker too. HTTP 400/401 with a terminal code deletes the tokens at once; network errors,
  timeouts, 429 and 5xx keep them and retry only on the next request.
- **Disconnect:** asks first, revokes the refresh token at the discovery document's `revocation_endpoint` (up to three attempts, 1 s and
  3 s apart, 10 s timeouts) and deletes the credentials locally in every case. An unconfirmed revocation is stated explicitly,
  including unresolved rotation after a restart, with an actionable **Manage usage** link even while disconnected.
  Disconnecting cancels any sign-in or refresh in progress.
- **`INTERNET` and the sign-in service:** `INTERNET` is declared as one single-line tag (`tools/verify-release-apk.sh` compares single-line
  tags and now refuses a split tag), cleartext traffic stays disabled, and the only use is HTTPS to `auth.openai.com` (discovery, keys,
  token, revocation). While a sign-in waits for the browser a foreground service of type `dataSync` (`SignInService`, notification
  *ChatGPT-Anmeldung läuft*, permission `FOREGROUND_SERVICE_DATA_SYNC`) keeps the process in a network-allowed state; it starts with
  the tap, stops within 2 s after the attempt ends and never runs otherwise. Debug builds only: a file `debug-no-signin-service` in the
  app's files directory (`adb shell run-as de.obdentic.app.debug touch files/debug-no-signin-service`) switches it off for the
  measurement of REQ-044.6; whether the service and the callback page's return link stay is decided by the phone measurements
  (REQ-044.6, REQ-044.7), which are part of the real-phone checklist in #330.
- **Verification:** `AccountAcceptance`, `AccountTransportAcceptance` and `AccountUiAcceptance` (part of `AcceptanceRunner`, also
  `am instrument -w de.obdentic.app.debug.test/de.obdentic.app.AccountAcceptanceRunner` on its own) use an in-process fake authorization
  server, a fake browser that answers the loopback callback through a real socket and a local TLS server with a Keystore-generated
  certificate for the HTTPS transport. They never contact OpenAI and are not phone evidence (REQ-044.8).

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

**Dauer-Aufzeichnung (REQ-028):** every live connection is recorded
automatically once the Bluetooth link is up, before the first adapter byte, so initialisation, support
discovery and inventory TX/RX are in the file. The file `…-live-auto.jsonl` has the
normal `OBDENTIC-MOBILE` v1 format with profile `session-auto`; it ends on
disconnect, error or when the switch is turned off, and an attempt that fails after the link is up still leaves
a file with its error marker; an attempt that never reaches the adapter writes no file and seals no
log, only lines in the running app log (REQ-028.12). **Jetzt exportieren** ends the running file as
`rotated`, continues at once in a new one, seals the app log and exports everything completed
(REQ-028.11). A new file starts every 30 minutes or 10 MB
(`session_end` reason `rotated`). A named recording or test sequence closes the
automatic file and the automatic recording continues after it ends; Knowledge jobs (ECU
identification, availability check, learning) write into the running automatic file instead
(REQ-028.4). The switch and
**Automatische Aufnahmen löschen …** are on the recordings tab. The app never deletes
a recording on its own; demo and replay are never recorded automatically. Replay
and `obdentic-mobile-report` re-derive the vehicle and signal support from the
recorded handshake.

**App debug log (REQ-029):** a separate operational log (`logs/app.log`, text lines
with time, level, tag and message, also in Logcat) records service lifecycle, every
connection attempt and its outcome, reconnect delays, BLE status codes, diagnostic jobs,
recording events and exceptions with full stack traces (including uncaught ones). It
never contains VINs, raw adapter traffic or unmasked Bluetooth addresses. The active
file is sealed as `app-<timestamp>.log` when a connection ends (disconnect, or an error once the link was up) or at 1 MB; the newest 50
sealed files are kept. Every export run copies the sealed logs next to the recordings,
and `tools/pull-recordings.sh` pulls `*.log` too. The log is operational data, not
vehicle evidence: it cannot be replayed. Every user-visible message (including the reason
an action was refused, tag `msg`) is logged; the ~2 s tester-present cycles of an extended
session are not, only a failing one (REQ-029.7/.8).

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
  Every export runs on its own thread, so copying into a large folder never holds up
  polling, jobs or the connection (REQ-018.9).
- The list shows the newest 30 recordings and how many older ones are hidden; export and
  sharing still cover all of them (REQ-028.13).
- **Aufnahmen teilen …**: hands all completed recordings to the Android share sheet
  as read-only content URIs.

If the folder was deleted or its permission revoked, export stops with "Export-Ordner
nicht verfügbar · bitte neu wählen", the stored choice is cleared and nothing
internal changes. A copy that fails midway removes the incomplete target file.
Copies are byte-identical; export never deletes, moves or modifies the internal
recordings. It needs no storage permission and no network and adds no permission of its own
(REQ-018.8); the app's only network permission, `INTERNET`, belongs to the optional ChatGPT account
(see above). Export uses the Storage Access Framework folder grant and the internal read-only content provider.
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
`evidence/phone/` directory, one folder per day:

```sh
rtk tools/pull-recordings.sh            # copy new recordings and logs, skip existing ones
rtk tools/pull-recordings.sh --report   # also run obdentic-mobile-report on new recordings
rtk tools/pull-recordings.sh --dest evidence/phone-2   # other git-ignored destination, same layout
```

```text
evidence/phone/
  2026-10-29/20261029-181530-123-live.jsonl
  2026-10-29/app-20261029-120000-000.log
  undatiert/legacy-name.jsonl          # no usable timestamp in the file name
  konflikte/20261114T101500Z/<name>    # older version replaced by a different-size copy
```

**Layout.** The day folder comes from the timestamp the app wrote into the file name
(`yyyyMMdd-HHmmss-SSS-…jsonl` recordings, `app-yyyyMMdd-HHmmss-SSS.log` app logs). That
timestamp is the phone's local wall-clock time; the file name carries no time zone or
offset, so the script applies none and does not guess one. A recording made shortly
after local midnight therefore sits in the new day's folder, whatever the zone. A name
without a valid timestamp (no date, year outside 1970–2099, month 13, 30 February, hour
25 and the like) goes to `undatiert/`. File names and bytes are never changed.

**Reimport and collisions.** A file that already exists in its day folder with the same
size is skipped, so repeated pulls change nothing. Archives from before the day folders
kept everything flat in `evidence/phone/`: a flat file with the same name and size also
counts as present and is not imported again (migrate it into its day folder by hand if
you want the new layout). A flat copy with a different size is left alone and the
current version is pulled into its day folder. If the day folder holds a file of the same
name but a different size (for example a partial recording that has grown), the new
file is first pulled under a temporary name; only after that succeeds the old file is
moved, unchanged, to `konflikte/<UTC timestamp>/<name>` (a numeric suffix keeps the
folder unique) and the new one takes its place. A failed pull or a failed archive move
keeps the old file in place. If the move of the new file fails after the old one was
archived, the script tries to put the old file back; should that fail too, the old
version remains in
`konflikte/<UTC timestamp>/<name>` (the script names that path), so an existing file
is never lost or deleted. The temporary name `<name>.pulling` is different: a
failed new pull removes only its own partial temporary file, and a leftover
`<name>.pulling` from an earlier interrupted run is never overwritten or removed. The
script refuses it, reports that file as failed and keeps it until you move or delete
it. The script never writes through symbolic
links: it refuses a symlinked `--dest`, day folder, `undatiert/`, `konflikte/`,
target file or `*.pulling` name, reports the file as failed and leaves the
link and whatever it points to untouched. With `--report` the report runs on
the actual local path of each newly pulled recording.

The script reads from the phone only (`adb devices`, `shell ls/stat/test`, `pull`)
and refuses destinations that git does not ignore (it checks the day folders,
`undatiert/`, `konflikte/` and temporary names). It needs exactly one adb device
and labels each copied recording `complete` (it ends with `session_end`) or
`partial`; partial files are kept, and each copy is reported with its local path.
`--source` and `--dest` override the phone folder and the git-ignored local
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
upload. The app contains no Firebase SDK; its only network use is the optional ChatGPT account (`INTERNET`); the App
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

The Debug tab is one full-height TX/RX window that scrolls to the newest line after every update (REQ-033.1).

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

Live-grid acceptance follows the `MainActivity` that is currently resumed (`MainActivity.resumed`); a configuration change
can replace the instance returned by launch. The first-screen tile check (#239) needs a phone-sized viewport, for example
1080x2220 at 440 dpi. Prefer a dedicated API 33 AVD with those physical dimensions; another session can change a shared
emulator's `wm size` during a run. If overriding geometry, use `wm size 1080x2220` / `wm density 440` and restore it afterwards
with `wm size reset` / `wm density reset`. Select the intended emulator explicitly (`adb -s <emulator-serial>`); never target
the connected phone implicitly. The grid check keeps its eight-tile assertion and saves a synthetic `acceptance-grid.png`.

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
4. Apply the built-in preset "Fahrt" and record the selection while locked for at least five minutes. Stop it,
   export locally, replay on the phone and inspect the same file on the Mac.
5. Disconnect, reconnect and test adapter power loss. The UI must not retain old
   live values under a new connection or silently continue a failed recording.

Retain raw acceptance evidence locally. Publish only a sanitized pass/fail summary.
