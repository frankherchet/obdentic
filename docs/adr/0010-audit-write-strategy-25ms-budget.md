# 0010. Audit write strategy and destination for the 25 ms p95 budget
Date: 2026-11-24
Status: accepted

## Context

Spike #378 follows #363 (ADR 0007). This ADR retires the latency criterion investigated in ADR 0007; ADR 0007 otherwise remains proposed. At the start of the spike, REQ-045.33 specified durable outbound append of up to 64 KiB, p95 ≤ 25 ms, measured from the write request through durability confirmation, never dispatching before durability; REQ-045.28 specified fail closed. The measured SAF numbers in #363 (cold existing file p95 ≈ 58–60 ms, cold new file p95 ≈ 113 ms, on the same device class) missed the budget, so this spike asks whether another write strategy on a supported destination meets it.

Maintainer-approved amendments A1–A6 (issue #378, 2026-11-23) are the criteria and are not changed here:

- A1: REQ-045.23/.29 stay. Only write strategies on the shared, user-selected internal directory are candidates; app-private destinations are measured as baselines only.
- A2: reference device is the Mi 9T on Android 16; on any other device the verdict is "not measured".
- A3: a force-stop or process-kill result alone is inconclusive for durability; Confirm needs it **and** an abrupt power/storage interruption with an independent ack oracle.
- A4: judged per candidate. Confirm needs one candidate with p95 ≤ 25 ms in both file cases (cold process, existing and new file, parent-directory sync included where needed) measured to proven durability and no loss. Refute only when every candidate fails at least one of these.
- A5: batching is measured with and without concurrent writers; each dispatch waits for the sync covering its own entry; latency includes any batch wait.
- A6: ≥ 100 successful samples per stratum (1/16/64 KiB × existing/new file), nearest-rank p95, failures counted separately.

## Options considered

Candidates (shared internal directory chosen through `ACTION_OPEN_DOCUMENT_TREE`; `saf` = descriptor from `ContentResolver.openFileDescriptor`):

| id | destination access | strategy |
|---|---|---|
| saf-wa | SAF descriptor | `wa` (O_APPEND), `FileDescriptor.sync()` (fsync). Control = the #363 candidate, rerun in this session. |
| saf-wfd | SAF descriptor | `wa`, `FileChannel.force(false)` (fdatasync) |
| saf-pre | SAF descriptor | file preallocated to 8 MiB, opened `rw`, positional write at the logical end, `force(false)` |
| path-wa / path-wfd / path-pre | file path in the same directory, all-files access | the same three strategies through the path. This is a different permission mechanism than SAF (not the SAF design of ADR 0007); it is measured because it is the only route to a parent-directory `fsync` and to avoid the provider round trip. It is reported as such, not as a SAF result. |
| gc | saf and path, wa and pre | group commit: one writer thread drains a queue, writes all queued records, one sync, acknowledges each waiting appender; 1, 2, 4, 8 concurrent appenders; batch window 0 ms and 5 ms. |

Baselines only (A1): app-private `noBackupFilesDir` with wa, wfd and pre; `filesDir` and cache with wa. These are not candidate destinations.

## Pre-measurement protocol (fixed before the first sample; amended only by a labeled addendum)

**Dry run: no conflicts** (issue #378, comment of 2026-11-24). Interpretation stated before data, not a change of criteria: unprovable durability is *inconclusive* under A3, neither a failure nor a pass, so Confirm cannot be reached in this session and the possible outcomes are Refute or Inconclusive.

**Reference configuration:** Mi 9T, Android 16 (API 36), recorded with `getprop`/`dumpsys`; free space, power and thermal state recorded at start and end of every run block. Prototype app `de.obdentic.spike378` (targetSdk 36), throwaway, installed and removed again; tree and file names under `Documents/obdentic-spike378-audit`. Synthetic data only. No vehicle, adapter or BLE.

**Latency timer:** `System.nanoTime`; the record is fully encoded before the timer. Timer from the append request to the ack decision.
- *cold*: every sample is `am force-stop`, then a broadcast that starts a fresh process; one first append.
  - *new*: the timer includes document/file creation, open, preallocation including the full `force(true)` that makes the size durable (pre only), write, data sync and the parent-directory `fsync` (so that the name is as durable as the call can make it).
  - *existing*: the timer includes resolving and opening the existing file; for pre it also includes the scan that recovers the logical end (read until the first NUL, logical end = after the last newline before it). The existing file holds 256 KiB of complete records (16 KiB each) and, for pre, is 8 MiB preallocated; 100 samples of up to 64 KiB stay within it.
  - After the verdict timer, two more appends in the same process are timed separately (hypothesis test for the first-sync cost) and component durations (create, open, scan, prealloc, write, sync, dirsync) are stored.
- *warm group commit*: closed-loop appenders, 100 appends each after 5 flagged warm-ups; latency per entry from submit to its own ack (includes batch wait); batch size stored.
- Each cold stratum: 3 sizes (1, 16, 64 KiB) × 2 file cases × 100 samples per candidate/baseline. Nearest-rank p50/p95/p99, max, count above 25 ms and failures per stratum. A stratum above 25 ms p95 is "benchmark exceeds target" for that candidate and is never averaged with passing strata. The control (saf-wa) is rerun at the end (16 KiB, both file cases) as a drift check.

**Preallocation semantics:** the logical length is the prefix of complete newline-terminated JSONL records; the zero tail is not audit data; recovery stops at the first NUL or torn line. File size is never used as byte count.

**Interruption evidence (A3):** ack oracle = `ACK run seq` line in logcat emitted only after the sync call returned, captured on the host with timestamps; `DISPATCH` only after an ACK of the same seq (the code throws otherwise, any failure stops the session, no retry). Post-run, the host reads the file through `adb shell cat` (independent of the app and SAF) and checks every ACKed record is present, intact (CRC) and inside the complete prefix; unacknowledged records may be absent, present or torn and are classified.
- I1: self-`SIGKILL` at `after_create`, `after_write`, `after_sync`, `after_ack`, 20 runs per point per candidate.
- I2: `am force-stop` at a random time during continuous 4 KiB appends (interval 20 ms to keep the logcat oracle below its rate limit), 10 runs per candidate.
- **Not run, hence inconclusive for the matching claims:** abrupt power or storage-controller loss (it needs a power cut, and power/sleep keys, reboot and any other touch of the device are out of bounds), full storage, short-write and fsync-failure injection, OEM-independence. Process death does not drop the kernel page cache; I1/I2 test application ordering, not persistence on stable media.

**Publication:** see addendum 2 (superseded the original plan to keep raw durations local).

## Addendum to the protocol (after the smoke runs, before any verdict sample)

- Strategy `pre` recovers the logical end with a binary search for the first block whose first byte is NUL, then scans the block before it. A first smoke run with a sequential byte-wise scan cost ≈ 630 ms in a cold process (interpreted loop), which measures the implementation, not the strategy. `prescan` (sequential array scan of the whole prefix) is kept as a separate strategy for existing-file strata.
- Added command `idle` (warm process, fixed pause between appends) to test the hypothesis that the ≈ 20 ms first-sync cost is related to storage idle time rather than to the fresh process.
- Private-directory cleanup uses an in-app command because `run-as` is denied on this build.

## Addendum 2 to the protocol (second device session, after the first sample of that session)

Maintainer decisions of 2026-11-24: direct path access with all-files permission is a measured candidate with no product commitment (REQ-045.23 unchanged until this ADR is decided); raw synthetic durations may be published as CSV after a scan for device identifiers. Published under `spikes/378/results/` (`raw/` per-sample CSVs, `SHA256SUMS`, `summary-*.csv`); the scan covered the serial, model/device/build identifiers beyond "Mi 9T", the account and home-path names, and found none. A6 is therefore met for publication.

Deviations and harness defects found during the session (each affects only the cells named):
1. Run order: baselines were measured after group commit and I1/I2 (not directly after the candidates), so a cut session would have left the candidate evidence. No effect on values.
2. `am broadcast` to a `goAsync` receiver is limited to about 60 s; the first `idle` run with 3000 ms pauses stalled for that reason. The 3000 ms stratum was run as 3 × (3 warm-up + 10) appends in three fresh processes (30 samples, not 100); the 50/600 ms pauses had 100/60 samples. The `idle` test is a hypothesis test, not a verdict stratum.
3. Existing-file strata with `pre` at 64 KiB **overflowed the 8 MiB preallocation**: three appends (verdict + two component appends) × 64 KiB × 100 samples exceeds 8 MiB, contradicting the protocol sentence that 100 samples stay within it. After about 42 samples the logical-end scan degenerates (≈ 770 ms). The affected files `raw-cold-saf-pre-existing-65536.csv` and `raw-cold-path-pre-existing-65536.csv` are kept but invalid; both strata were rerun with the two extra appends disabled (`noextra`, files `*-noextra.csv`) and only the reruns count. The same rerun was taken for the `prescan` strata at 64 KiB.
4. The first I2 attempt did not interrupt anything, because the harness awaited the blocking broadcast and the runs ended by completion or timeout; a second attempt overlapped with a stale job. Both were discarded (not published) and I2 was rerun alone, one run at a time. Interval 20 ms as protocol; force-stop delay uniform 1.0–3.0 s after the broadcast.
5. Group commit was measured at 16 KiB only (sizes 1/64 KiB not run) and on warm, closed-loop appenders without pauses.

## Measurements
Paths beginning with `results/` in this section refer to `spikes/378/results/` in this repository.


Device: Mi 9T, Android 16 (API 36.1), build with LineageOS properties present (a custom build, not a stock vendor image; hypothesis: the stock-looking vendor fingerprint is stale). `/data` f2fs, `/storage/emulated` FUSE; on mains power, screen kept awake by the existing setting; battery 57 % at the start, 80 % at the end; battery temperature 23.6 °C at the start, 33.4 °C at the maximum between blocks, 30.8 °C at the end (`spikes/378/results/context-*.txt`; the first lines of `context-run.txt` are the smoke runs of the earlier session); ≈ 77.7 GB free at the end. The commands `scripts/idle.sh`, `scripts/cold.sh`, `scripts/gc.sh`, `scripts/interrupt.py i1|i2`, and `scripts/stats.py` (nearest-rank, `ceil(q·n)`-th value) are in the [`spike-378-audit-write` branch](https://github.com/frankherchet/obdentic/tree/spike-378-audit-write/spikes/378/scripts), not on main. All numbers are request-to-ack milliseconds in the app; no failures occurred in any stratum (0 of 100 in each).

**Cold process (every sample: `am force-stop`, fresh process, one first append), p95 in ms, 100 samples per cell** (1 KiB / 16 KiB / 64 KiB):

| destination, strategy | new file | existing file |
|---|---|---|
| saf, wa (control) | 200.3 / 221.9 / 241.5 | 46.1 / 50.1 / 54.1 |
| saf, wfd | 210.2 / 207.6 / 231.9 | 50.7 / 54.5 / 53.8 |
| saf, pre (binary-search end) | 213.8 / 220.9 / 224.7 | 52.9 / 62.0 / 57.1 (noextra rerun) |
| path, wa | 55.4 / 53.7 / 55.0 | 27.9 / 28.0 / 28.4 |
| path, wfd | 54.1 / 54.5 / 55.8 | 28.0 / 28.1 / 29.1 |
| path, pre | 56.0 / 56.3 / 59.5 | 32.6 / 33.5 / 34.3 (noextra rerun) |
| saf, prescan (sequential scan) | not run | 478 / 939 / 917 (noextra) |
| path, prescan | not run | 454 / 966 / 863 (noextra) |
| baseline noBackupFilesDir wa | 30.4 / 29.8 / 30.0 | 24.2 / 24.6 / 25.2 |
| baseline noBackupFilesDir wfd | 30.4 / 30.5 / 30.1 | 24.8 / 24.4 / 25.2 |
| baseline noBackupFilesDir pre (noextra) | 33.4 / 31.5 / 32.6 | 29.5 / 29.9 / 30.4 |
| baseline filesDir wa | 30.2 / 29.9 / 30.3 | 24.1 / 24.4 / 25.1 |
| baseline cache wa | 30.0 / 29.9 / 30.2 | 27.9 / 24.7 / 25.4 |

p50, p99, max, the count above 25 ms and the component medians (create, open, scan, prealloc, write, sync, dirsync) per stratum are in `results/summary-cold.csv`. Every candidate stratum has more than 90 of 100 samples above 25 ms except saf/path `wa`/`wfd` existing, where 71–94 of 100 are above 25 ms (lowest: path wfd existing 1 KiB, 71). Component medians, 16 KiB: saf new: create ≈ 51 ms, open ≈ 11 ms, directory sync ≈ 90 ms; saf existing: open ≈ 17–19 ms, data sync ≈ 20 ms; path new: create ≈ 19–20 ms, open ≈ 2 ms, data sync ≈ 21 ms (wa) or ≈ 1.7 ms after the full `force(true)` of the preallocation (pre), directory sync ≈ 5 ms; path existing: open ≈ 4 ms, data sync ≈ 21 ms. With `pre`, the 20 ms is paid by the preallocation `force(true)` in the new-file case instead of the data sync. The `prescan` strata grow with the logical prefix over the 100 samples (the file is never truncated); they are reported as an implementation cost, not a steady state.

**Control rerun at the end (saf wa, 16 KiB, 100 samples):** new p95 221.2 ms (first run 221.9), existing p95 52.8 ms (first run 50.1).

**Idle test (warm process, held descriptor, 16 KiB, pause between appends; per candidate path-pre, saf-pre, path-wa, saf-wa; 100 samples at 0 and 50 ms, 60 at 600 ms, 30 at 3000 ms, 3 warm-ups excluded):**

| pause | p50 range over the 4 candidates | p95 range | samples above 25 ms (of 4 × n) |
|---|---|---|---|
| 0 ms | 1.4–1.6 ms | 2.1–2.5 ms | 0 of 400 |
| 50 ms | 5.2–5.6 ms | 23.7–23.8 ms | 0 of 400 |
| 600 ms | 23.5–24.0 ms | 24.7–25.0 ms | 11 of 240 |
| 3000 ms | 18.5 (path-pre), 23.5–24.1 ms | 24.2–26.5 ms | 6 of 120 |

**Group commit (warm, 16 KiB, closed loop, 100 appends per writer after 5 warm-ups; per row each of saf-wa, path-wa, saf-pre, path-pre):** one writer, window 0: p95 1.8–3.3 ms; one writer, window 5 ms: p95 8.7–9.4 ms (the batch wait is included). Eight writers, window 0: p95 6.2–8.4 ms (mean batch ≈ 4.1–4.2 entries), p99 ≤ 15 ms, with 4 of 800 samples above 25 ms (path pre); eight writers, window 5 ms: p95 14.9–16.2 ms (mean batch ≈ 8), 8 of 800 above 25 ms (saf wa; max 29.3 ms). Two and four writers lie between (`results/summary-gc.csv`). No cold or post-pause group-commit run was taken.

**Interruption (A3), host oracle = `ACK` line in logcat after the sync call returned, file read afterwards with `adb shell cat`:**
- I1, self-`SIGKILL`, 6 candidates × 4 points × 20 runs = 480 runs, 1200 acknowledged records: 0 acknowledged records missing, 0 corrupt lines, 0 torn tails, 0 dispatches before the ack of the same sequence number. At `after_write` and `after_sync` the unacknowledged record was present in the file in all 120 + 120 runs; at `after_create` and `after_ack` nothing unacknowledged was present.
- I2, `am force-stop` during 4 KiB appends every 20 ms, 6 candidates × 10 runs: 3536 acknowledged records, 0 missing, 0 corrupt lines, 0 torn tails, 0 dispatches before ack; 10 runs had one extra unacknowledged record present (allowed).
- Per candidate and point: `results/summary-interruption.csv`.

## Criteria verdict

| Predeclared criterion (A1–A6) | Verdict | Rests on |
|---|---|---|
| A4, per candidate: p95 ≤ 25 ms in both cold file cases (1/16/64 KiB) | **not met by all 6 candidates** | Best candidates path wa/wfd: new p95 53.7–55.8 ms, existing 27.9–29.1 ms; saf candidates new 208–242 ms, existing 46–62 ms. Every candidate stratum above 25 ms. |
| Refute: every candidate fails at least one of {p95 ≤ 25 ms in both cases, proven durability, no loss} | **met on the latency part** (judged on this device, these candidates and strategies only) | all 6 candidates × (new, existing) strata above 25 ms p95; no durability evidence needed for this limb |
| Confirm | **not reachable** | A3 needs a power/storage interruption; not produced |
| A3: no loss after force-stop or process kill | met for the application ordering, 0 loss | I1 480 runs, I2 60 runs. Counts as inconclusive for durability by A3 itself |
| A3: abrupt power/storage interruption with independent oracle | not measured | cannot be produced within the session limits |
| A5: batching with and without concurrent writers | measured (warm, 16 KiB only) | batching reduces per-entry latency of concurrent warm writers; cold and post-pause single appends not covered, so no statement about the budget case |
| A6: ≥ 100 successful samples per stratum, nearest-rank p95, failures counted, raw durations published | met for the cold strata (the idle 600 ms and 3000 ms strata have 60 and 30 samples and are hypothesis tests only); published in `results/` | `results/raw`, `SHA256SUMS`; two published files are invalid (addendum 2, item 3) |

## Uncertainty

- The refute is a statement about the six declared candidates on one device (Mi 9T, Android 16 custom build). It is not a statement about other write strategies (for example a retained descriptor opened at app start with pause-free warm appends, or asynchronous or io_uring based syncs), other devices, or other storage states. On any other device the verdict is "not measured" (A2).
- Durability is unproven for every candidate (A3). Process death does not drop the kernel page cache; I1/I2 test application ordering only. Full storage, short-write and fsync-failure injection, and OEM independence were not run.
- Hypothesis H2 (the ≈ 20–24 ms of the first sync after a pause is storage idle wake-up, not process freshness): consistent with the data (warm appends with a 600 ms or 3000 ms pause take ≈ 24 ms, appends without pause ≈ 1.4 ms, 50 ms pause is bimodal: p50 5 ms, p95 23.7 ms), but not isolated; the experiment that would isolate it (a storage-independent sync timing, for example against a tmpfs file, or a read-only touch of the device before the append) was not run. H1 (fresh process needed) is not needed to explain the numbers, since a warm process after a pause pays the same cost.
- Consequence if H2 holds: the warm numbers of #363 for back-to-back appends do not represent an audit append after an LLM turn; this is not established for the real app.
- The cold-new strata include file creation and the parent-directory sync. A variant with the file created before the first request was promised separately; it was not measured as its own stratum. A derived estimate from the component medians (total minus create and directory sync, path wa new 16 KiB ≈ 49 − 19 − 5 ≈ 25 ms) is arithmetic on medians, not a percentile.
- The `pre` strategy needs a logical-end recovery: the sequential scan costs 0.35–1.0 s, the binary search ≈ 5 ms, and relies on the gap-free prefix assumption; the preallocation of 8 MiB was larger than the strata first assumed (addendum 2, item 3).
- Group commit was not measured with idle pauses, cold processes, or sizes other than 16 KiB.
- Whether path access with all-files permission is acceptable is a product decision; REQ-045.23 is unchanged.
- Only the app's own timer was used; the logcat `ACK` times were not used for latency.

## Decision

**Accepted finding (maintainer decision, 2026-11-24):** on the reference device the A4 Refute condition is met on latency for all six declared candidates (shared directory via SAF or direct path; `wa`, `wfd`, `pre`); no candidate meets 25 ms p95 cold. Confirm cannot be reached in this setting. App-private baselines span 24–30.4 ms p95 for existing files and about 30–33.4 ms for new files, consistent with a first sync after a pause of ≈ 20–24 ms on this storage; the data do not isolate its cause (H2 above). Durability remains inconclusive: no power-loss test was performed.

The maintainer retired the REQ-045.33 latency budget and removed the criterion; its ID is not reused. The SAF folder choice remains the destination (REQ-045.23); durable-before-dispatch and fail-closed rules remain (REQ-045.10 and REQ-045.28).

## Consequences

- The 25 ms budget is no longer an acceptance criterion; no replacement latency target is established by this decision.
- The user-selected SAF folder remains in scope. Direct path access with all-files permission was measured without a product commitment and is not a latency solution on this device.
- Durability is still unproven: process death tests demonstrate ordering, not survival of abrupt power or storage loss; no power-loss test was performed.
- Batching helps concurrent warm writers in the measured workload; cold and post-pause cases were not established.
- Published synthetic results remain under `spikes/378/results/`; the throwaway prototype and harness remain only on the [`spike-378-audit-write` branch](https://github.com/frankherchet/obdentic/tree/spike-378-audit-write/spikes/378).
