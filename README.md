# OBDentic

Transparent, local-first and read-only vehicle diagnostics.

The first vertical slice reads standard OBD-II engine RPM through one diagnostic-core seam, shows the semantic request and raw bytes, records the transaction, and replays it deterministically. The Rust BLE path is implemented; its final targeted hardware acceptance is still pending.

The current split is intentionally narrow: `protocol` contains Mode-01 framing and
response validation; `vehicle` contains signal/profile metadata and deterministic
decoders; `ble` contains the Carly transport. EA189 is a metadata-only profile
skeleton until a read-only request and response have been evidenced.

This Mac also has an obsolete MacPorts Cargo earlier in `PATH`. Activate rustup
before running the project; `rust-toolchain.toml` then selects Rust 1.98.0:

```sh
source "$HOME/.cargo/env"
cargo --version
```

```sh
cargo run -- demo
cargo run -- replay session.tsv
cargo run -- signals
cargo run -- scan
cargo run -- tui demo
cargo run -- tui replay session.tsv
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

Example transaction:

```text
semantic  engine.rpm
tx        01 0C
rx        41 0C 1A F8
decoded   1726 rpm
```

The core only accepts known read-only semantic signals. Unsupported or mutating requests are rejected before a transport can be called.

Supported standard Mode 01 read-only signals:

| Semantic | Request | Formula | Unit |
| --- | --- | --- | --- |
| `engine.rpm` | `01 0C` | `((A × 256) + B) / 4` | rpm |
| `engine.coolant_temperature` | `01 05` | `A - 40` | °C |
| `vehicle.speed` | `01 0D` | `A` | km/h |
| `engine.maf` | `01 10` | `((A × 256) + B) / 100` | g/s |

Decoder and replay coverage for the three additions is offline-only for now;
their targeted hardware validation is pending. The next hardware acceptance
run remains RPM-only.

`cargo run -- signals` prints the same catalog as escaped, tab-separated rows
with semantic, profile, protocol, request, decoder, plausible range, unit,
subsystem, provenance, confidence, hardware-validation and description columns.

`tui` is an offline viewer: it renders already-decoded demo or replay samples,
their signal metadata and raw diagnostic TX/RX. Press `q` or `Esc` to close it;
it never opens Bluetooth or sends a diagnostic request.

For MIUI Android 10 bugreports, extract the embedded Bluetooth capture with:

```sh
mkdir -p captures
python3 tools/extract_miui_btsnooz.py bugreport.txt captures/capture.btsnoop
```

Confirmed Carly CUA-V200 BLE handles and protocol findings are documented in [`docs/carly-cua-v200.md`](docs/carly-cua-v200.md).

With ignition on, use `scan` to obtain the current CoreBluetooth UUID, then pass
that exact UUID to `read`. On macOS, grant Bluetooth access to Terminal/iTerm in
System Settings → Privacy & Security → Bluetooth. A bundled app additionally
needs `NSBluetoothAlwaysUsageDescription` in its `Info.plist`.

The existing Swift probe remains available for hardware parity checks:

```sh
swift tools/carly_probe.swift
```

## Targeted hardware acceptance (pending)

Run this checklist with the Carly adapter plugged in, ignition on and engine
off. Do not connect the Carly app at the same time. On macOS, grant Bluetooth
access to Terminal/iTerm before starting. This exercises read-only RPM only;
do not add VIN, UDS session-control, coding or DTC-clear commands.

```sh
# Build and offline checks first.
cargo test --locked
cargo clippy --all-targets -- -D warnings

# Copy the exact current CoreBluetooth UUID printed by scan; do not reuse a stale UUID.
cargo run -- scan
export ADAPTER_UUID='PASTE_EXACT_UUID_FROM_SCAN'

# Three independent processes mean three fresh BLE connections.
for attempt in 1 2 3; do
  cargo run -- read engine.rpm --adapter "$ADAPTER_UUID" --record "session-$attempt.tsv" || exit 1
done

# Every run must show 01 0C, 41 0C 00 00 and 0 rpm, then replay identically.
for attempt in 1 2 3; do
  cargo run -- replay "session-$attempt.tsv" || exit 1
done
```

Acceptance also requires starting the same `read` command, pressing
`Ctrl-C` during scan/connect/response wait, and confirming it exits cleanly;
repeat the command afterward to prove reconnectability. Repeat once with the
adapter unavailable long enough to hit the command timeout, then restore the
adapter and confirm a fresh read succeeds. Keep the Swift probe until these
three Rust reads, record/replays, interruption, timeout and reconnect checks
match; remove it only after that parity is demonstrated.
