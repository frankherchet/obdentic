# OBDentic

**Transparent, local-first and strictly read-only vehicle diagnostics for humans and agents.**

OBDentic is building a deterministic diagnostic layer between a vehicle and higher-level diagnostic tooling. It owns the physical adapter connection, turns protocol traffic into typed vehicle facts, preserves the underlying evidence, and keeps unsafe operations outside the executable vocabulary.

The long-term goal is not a single EA189/EGR tool. The VW EA189 is the first real vehicle profile and a useful proving ground for deeper diagnostics such as DPF, EGR and intake analysis. The project itself is intended to become a **generic agentic OBD vehicle-diagnostics platform**: an AI agent should be able to reason about a vehicle, request bounded semantic reads, inspect provenance and raw evidence, and explain a diagnosis without ever receiving an unrestricted CAN/UDS command path.

## Design principles

### Strictly read-only

Safety is enforced below the CLI, TUI and future agent interface. The executable vocabulary is a closed set of known read-only semantic operations.

No raw CAN/UDS command API is exposed to consumers. Coding, adaptation, actuator tests, DTC clearing, SecurityAccess and other state-changing operations are outside the current safety boundary.

```text
semantic request
    -> read-only safety policy
    -> vehicle/protocol knowledge
    -> bounded transport request
```

### Capture first, interpret later

Live access produces evidence first. Interpretation can then be repeated offline without touching the vehicle again.

```text
vehicle
  -> raw responder evidence
  -> deterministic normalization
  -> persisted capture

persisted capture
  -> deterministic re-decode
  -> correlation
  -> diagnosis / agent reasoning
```

Responder identity and non-selected evidence are preserved. OBDentic must never choose a value merely because it looks more plausible.

### Separate protocol, vehicle and diagnostic knowledge

OBDentic keeps three concerns deliberately separate:

1. **Protocol knowledge** — BLE, ELM327-style adapters, CAN, ISO-TP, OBD-II, UDS and later manufacturer-specific transports.
2. **Vehicle knowledge** — ECU identity, addressing, supported PIDs/DIDs, typed layouts, scaling, units and provenance.
3. **Diagnostic knowledge** — correlation of facts into explanations, hypotheses and recommended next reads. This is where local agentic reasoning belongs.

The vehicle profile translates bytes into facts. The diagnostic layer translates facts into understanding.

## Current architecture

The first production path is Rust on macOS with a BLE ELM327-style adapter. One session actor owns the physical adapter and serializes all commands; schedulers, the TUI and future MCP consumers share semantic state rather than opening competing connections.

The runtime model is explicit and deterministic. Lifecycle phase and current activity are separate so observation, bounded reads and diagnostic jobs do not create accidental state combinations.

```text
OBDentic runtime
├── phase
│   ├── init
│   ├── discover
│   ├── ready
│   ├── stopping
│   ├── stopped
│   └── fault
└── activity
    ├── idle
    ├── observe
    ├── read
    ├── diagnose
    └── write        # representable, not executable today
```

A pure reducer applies typed events, while effects such as BLE I/O and persistence happen outside the reducer. This makes runtime history replayable and auditable.

## What works today

The repository already contains the main building blocks of the generic read-only path:

- CoreBluetooth discovery and BLE communication with the current Carly/ELM-compatible adapter
- bounded ELM initialization and protocol negotiation
- generic OBD-II Mode 01 support discovery and a typed scalar signal catalog
- vehicle identity, topology evidence, cache validation and targeted routing
- deterministic multi-responder handling without plausibility-based selection
- read-only stored-DTC scanning through a typed diagnostic job
- JSONL capture with raw responder evidence, timings and runtime-state transitions
- offline capture inspection and transport-capability reporting
- deterministic transaction replay
- a terminal TUI and declarative layouts
- explicit runtime state, audit state and read-only safety policy

Manufacturer-specific vehicle knowledge is intentionally added only when a read-only request and response have been evidenced.

## Quick start

`rust-toolchain.toml` selects Rust 1.98.0. If another Cargo installation appears earlier in `PATH`, activate rustup first:

```sh
source "$HOME/.cargo/env"
cargo --version
```

Run the offline checks:

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

Discover the adapter and establish vehicle identity/topology:

```sh
cargo run -- scan
export ADAPTER_UUID='PASTE_COREBLUETOOTH_UUID'

cargo run -- vehicle identify --adapter "$ADAPTER_UUID"
cargo run -- vehicle discover --adapter "$ADAPTER_UUID"
cargo run -- vehicle show
```

Inspect the semantic signal vocabulary and the subset advertised by the current vehicle:

```sh
cargo run -- signals
cargo run -- signals --adapter "$ADAPTER_UUID" --supported
```

Perform a bounded semantic read:

```sh
cargo run -- read engine.rpm --adapter "$ADAPTER_UUID"
```

Record an observation session and inspect it offline:

```sh
cargo run -- capture \
  --adapter "$ADAPTER_UUID" \
  --profile engine-drive \
  --record evidence/drive.jsonl

cargo run -- capture inspect evidence/drive.jsonl
cargo run -- capture capability evidence/drive.jsonl
```

Run the currently supported generic diagnostic job:

```sh
cargo run -- diagnose dtc.scan \
  --adapter "$ADAPTER_UUID" \
  --record evidence/dtc-scan.jsonl
```

`dtc.scan` is currently a strictly read-only generic stored-DTC workflow. DTC clearing is deliberately not part of the executable API.

## Evidence and privacy

Captures are intended to remain local. They preserve enough transport and responder evidence for deterministic offline analysis without making identity data part of ordinary telemetry.

VIN is used as a local vehicle-identity anchor where necessary, but it is not intended as a normal capture/log field. Raw Bluetooth captures, authentication material and other sensitive vehicle-specific evidence should remain local and untracked.

Adapter-specific findings for the current Carly CUA-V200 are documented in [`docs/carly-cua-v200.md`](docs/carly-cua-v200.md). Research notes live under [`docs/research/`](docs/research/).

## Roadmap

The roadmap is intentionally broader than the first EA189 use cases. GitHub issues and milestones track the executable work; the stages below describe the architectural direction.

### 1. Finish the generic OBD/ELM foundation

Make identity, topology, targeted routing, capture/replay, transport-capability calibration and deterministic scheduling boring and reliable. A vehicle session should be reproducible, auditable and free of hidden state changes.

### 2. Expand the generic diagnostic vocabulary

Build standards-based read-only workflows beyond scalar Mode 01 values: readiness and monitor status, pending/permanent DTCs, freeze-frame data, Mode 06 monitor results and compound facts. These remain semantic operations rather than arbitrary protocol commands.

### 3. Add evidence-backed vehicle profiles

Use manufacturer-specific knowledge only where public documentation or owned evidence supports it. VW/EA189 is the first deep profile, with ECU-specific DTC access and DPF/EGR/intake diagnostics as concrete proving cases — not as the boundary of the project.

### 4. Build semantic diagnostic snapshots

Combine responder-scoped facts, ECU identity, timing, provenance and confidence into deterministic snapshots that can be compared across idle, load, drive and fault conditions. The same capture should always produce the same decoded facts.

### 5. Expose a local agent interface

Add a local MCP-facing semantic API so Codex or another local agent can inspect current state, request approved read-only measurements and diagnostic jobs, and reason over persisted captures.

The agent should see:

```text
intent / hypothesis
    -> approved semantic read or DiagnosticJob
    -> exact TX + complete RX evidence
    -> deterministic facts
    -> diagnostic reasoning
```

It should **not** receive a raw CAN console.

### 6. Generalize across vehicles and adapters

Grow the protocol and vehicle-knowledge layers independently: more ELM-compatible adapters, additional manufacturer profiles, more ECU roles and eventually other safe transport backends. Vehicle-specific knowledge remains versioned and provenance-aware so support can grow without weakening the generic safety model.

## Why EA189 still matters

The EA189 remains an excellent first deep-diagnostics target because it gives OBDentic real, non-trivial questions to solve: DPF loading/regeneration, EGR behavior, intake plausibility, ECU topology and manufacturer-specific diagnostic data.

Those use cases now serve a larger purpose: proving that a generic diagnostic agent can move from protocol evidence to vehicle facts to an explainable diagnosis while the software remains local, deterministic and read-only.

## TUI

The TUI can render decoded telemetry from demo, replay and live sources. Layouts remain semantic: panels refer to signal names, not raw PIDs or addresses.

```sh
cargo run -- tui demo
cargo run -- tui live --adapter "$ADAPTER_UUID"

cargo run -- layout save engine-overview engine-overview.tsv
cargo run -- tui demo --layout engine-overview.tsv
```

This keeps presentation independent from transport and vehicle addressing, which is also the model intended for the future agent interface.

## Development rule of thumb

When adding functionality, keep this dependency direction:

```text
transport evidence
    -> protocol normalization
    -> vehicle facts
    -> diagnostic interpretation
    -> presentation / agent reasoning
```

Never invert it by letting a diagnosis, UI or agent guess an ECU address, select a responder by plausibility, or bypass the read-only vocabulary.
