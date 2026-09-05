# EA189 DPF longitudinal capture bridge

`ea189-dpf-longitudinal` is a temporary read-only capture bridge for collecting
DPF evidence together with a small drive-context snapshot in one exclusive
physical adapter session.

It exists to support hardware validation for #14 before the final declarative
profile architecture in #88 is available. It is intentionally not a second
profile framework and should be removed once effective Vehicle Knowledge and
YAML profiles can express the same intent.

## Closed observation set

Each cycle reads these existing standard semantic facts, in this fixed order:

- `engine.rpm`
- `vehicle.speed`
- `engine.load`
- `engine.maf`
- `engine.coolant_temperature`

The context snapshot is followed by the existing closed seven-step
`ea189.dpf.probe` job. No PID, DID, address, service, decoder or raw command is
accepted from the profile name or CLI.

At the minimum 30 second pause the bridge requests twelve logical reads per
cycle, or at most 0.4 logical reads/s averaged over the requested pause. Reads
remain sequential. The actual cycle period is the time required for the
context + DPF reads plus `--interval-seconds`.

## Safety and evidence

- the cached engine mapping must validate before capture;
- standard context reads are authorized as `Activity::Observe` / `SignalRead`;
- each DPF step is separately authorized as `Activity::Diagnose` /
  `Ea189DpfProbe`;
- one prepared diagnostic session owns the physical adapter for the complete
  trace;
- every normalized responder observation from targeted context reads is
  retained before the selected semantic transaction;
- DPF responder evidence remains unchanged from the existing trace path;
- no `DiagnosticSessionControl`, `SecurityAccess`, write, actuator, coding,
  adaptation, DTC-clear, raw CAN, raw UDS or raw ELM path is added.

The experimental DPF decoders remain experimental. A long capture is evidence
for later offline validation; it is not itself knowledge promotion.

## Usage

```bash
cargo run --release -- \
  capture \
  --adapter "$ADAPTER_UUID" \
  --profile ea189-dpf-longitudinal \
  --record captures/dpf-longitudinal.jsonl \
  --cycles 120 \
  --interval-seconds 60
```

Use JSONL until another capture writer has independently merged into `main`.
Keep real vehicle captures local/private unless explicitly sanitized.
