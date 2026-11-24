# 0010. Audit write strategy and destination for the 25 ms p95 budget
Date: 2026-11-24
Status: proposed

## Context

Spike #378 follows #363 (ADR 0007). REQ-045.33 (durable outbound append of up to 64 KiB, p95 ≤ 25 ms, measured from the write request through durability confirmation, never dispatching before durability) and REQ-045.28 (fail closed) stay unchanged. The measured SAF numbers in #363 (cold existing file p95 ≈ 58–60 ms, cold new file p95 ≈ 113 ms, on the same device class) missed the budget, so this spike asks whether another write strategy on a supported destination meets it.

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

**Publication:** raw per-sample durations stay local (assignment for this run); the ADR publishes aggregates and a SHA-256 of every raw file. A6 asks for raw durations to be published; the ADR records that this is not met and why.

## Addendum to the protocol (after the smoke runs, before any verdict sample)

- Strategy `pre` recovers the logical end with a binary search for the first block whose first byte is NUL, then scans the block before it. A first smoke run with a sequential byte-wise scan cost ≈ 630 ms in a cold process (interpreted loop), which measures the implementation, not the strategy. `prescan` (sequential array scan of the whole prefix) is kept as a separate strategy for existing-file strata.
- Added command `idle` (warm process, fixed pause between appends) to test the hypothesis that the ≈ 20 ms first-sync cost is related to storage idle time rather than to the fresh process.
- Private-directory cleanup uses an in-app command because `run-as` is denied on this build.

## Measurements

The device session was cut short at the request of the user (the phone had to be unplugged). **No verdict-grade stratum was measured**: no stratum reached 100 samples, no group-commit run, idle run, interruption run (I1/I2) or control rerun was taken. Only 3-sample smoke runs of the prototype exist; they are sanity checks of the harness, not measurements under A6, and are recorded here as observations only. They were taken with the commands `cold.sh` (`N=3`) and a single `probe`, on the Mi 9T, Android 16 (API 36.1), on mains power, `/data` f2fs mounted with `fsync_mode=nobarrier`, /storage/emulated a FUSE mount, ≈ 78 GB free.

- `probe`: `posix_fallocate` of 1 MiB succeeded (2048 blocks allocated) on the SAF descriptor, on the path, and in `noBackupFilesDir`, `filesDir` and cache; `FileChannel.force(true)` succeeded everywhere. A parent-directory `fsync` returned OK in 126 ms through a SAF-opened directory descriptor (first call), 4.2 ms through the path (all-files access), 0.6–2.8 ms for the private directories. Call results, not proof of persistence.
- Smoke, cold process, total request-to-ack time over 3 samples (ms, range): saf-pre new 107–180 (create ≈ 45–87, open ≈ 11–16, dirsync ≈ 46–69); saf-pre existing ≈ 51–80 (open ≈ 20–37, scan ≈ 5.4 with the binary search); path-wa new ≈ 45–52 (create ≈ 18–41); path-wa existing ≈ 24–27; path-pre existing ≈ 32.4–32.9; nobackup-pre new ≈ 29–32; files-wa new ≈ 11–30; files-wa existing ≈ 6–24. In most samples a single data or full sync call took ≈ 20–22 ms and the second and third append in the same process ≈ 1.5–3 ms.
- `idle` with pause 0 ms (path, pre, 16 KiB, 12 samples, printed to the console only, no file kept): 1.4–2.0 ms per append. The pauses 50/600/3000 ms did not complete (the interval run was interrupted when the phone was unplugged), so H2 below is untested.
- The 4 KiB and 64 KiB smoke samples showed no visible size dependence. Three samples cannot support a percentile.

Raw files (local only, not published; `spikes/378/data/device-pull/`) are smoke outputs plus an empty `idle-t.csv` from the interrupted run.

## Criteria verdict

| Predeclared criterion (A1–A6) | Verdict | Rests on |
|---|---|---|
| A4: a candidate has p95 ≤ 25 ms in both cold file cases, 1/16/64 KiB | inconclusive | no stratum has ≥ 100 samples; smoke ranges suggest SAF cold new/existing exceed 25 ms, but this is not a percentile |
| A4: proven durability | inconclusive | unprovable by design without a power-loss oracle (A3) |
| A3: no loss after force-stop or process kill | inconclusive | I1/I2 not run |
| A3: abrupt power/storage interruption with independent oracle | not measured | cannot be produced within the session constraints |
| A5: batching with and without concurrent writers | not measured | group commit runs not taken |
| A6: ≥ 100 samples per stratum, raw durations published | not met | sample count not reached; raw durations kept local by assignment |
| Refute (every candidate exceeds 25 ms p95 or loses an entry) | inconclusive | no candidate fully measured |
| Confirm | not reached | see above |

## Uncertainty

- Everything above the smoke level is unmeasured. The smoke ranges are 3 samples per stratum and do not decide any criterion.
- Hypotheses, untested: (H1) a fresh process pays ≈ 20 ms on its first data/full sync because of per-process or provider/FUSE handle setup; (H2) the ≈ 20 ms is storage idle wake-up, so every audit append after a pause (LLM turns last seconds) would pay it even in a warm process. H2 would invalidate the earlier warm numbers of #363 for real usage. Experiment: the prototype command `idle` with pauses 0/50/600/3000 ms on the path and SAF destinations (prepared, never completed).
- The SAF provider round trips (create ≈ 45–87 ms, open ≈ 20–37 ms, directory sync ≈ 46–126 ms) are large compared with the budget in the smoke runs; whether any SAF write strategy can meet 25 ms cold is unmeasured.
- Path-based access needs all-files access, a different permission mechanism from the SAF selection of REQ-045.23; whether it is acceptable is a product decision, not measured.
- Abrupt power loss is not testable in these session constraints; durability stays inconclusive for every candidate regardless of latency (A3).
- A6's raw-duration publication is not satisfied by the assignment to keep raw data local.

## Decision

**No decision.** The spike is parked for the next device session. The ADR stays `proposed`.

## Consequences

Next session (Mi 9T connected, unlocked, no vehicle/adapter), in this order, with the committed prototype `spikes/378/` and `scripts/cold.sh`:
1. `idle` hypothesis test (H2), 5 minutes.
2. Cold strata for the saf and path candidates, then baselines, 100 samples each, plus the control rerun (≈ 70 min).
3. Group commit with 1/2/4/8 writers, windows 0 and 5 ms.
4. I1/I2 interruption runs per candidate.
Question for the maintainer: is path access with all-files permission an acceptable candidate beside the SAF selection, and may the synthetic raw durations be published (A6)?
