# OBDentic agent instructions

## Delegation

Use sub-agents deliberately instead of implementing every non-trivial task in the primary agent. Recreate these workers when useful at the start of a new session:

- `low_worker`: `gpt-5.6-luna`, `xhigh` reasoning — repository discovery, mechanical checks, focused searches, small isolated changes and test execution.
- `mid_worker`: `gpt-5.6-terra`, `high` reasoning — normal feature work, debugging, integration design and bounded implementation tasks.
- `high_worker`: `gpt-5.6-sol`, `medium` reasoning — architecture review, difficult protocol reasoning, safety/security review and ambiguous high-impact decisions.

Choose the lowest-cost worker that can reliably handle the task. Give every worker a concrete, bounded assignment and a non-overlapping write scope. Parallelize independent work only; the primary agent owns integration and final verification. Do not delegate trivial work when coordination would cost more than doing it directly.

## Project constraints

- OBDentic is read-only. Never send vehicle coding, adaptation, actuator-test, DTC-clear, SecurityAccess, write-oriented UDS or arbitrary CAN commands.
- Keep adapter transport separate from deterministic vehicle decoding.
- Preserve raw TX/RX visibility and test through the highest practical transport/replay seam.
- Treat Bluetooth captures as sensitive: they may contain VINs, device identifiers and authentication material. Keep `captures/` untracked and never publish raw captures.
- Prefer the smallest working vertical slice and avoid speculative abstractions or dependencies.
- Prefix shell commands with `rtk`. On this host, `/opt/local/bin/cargo` is an
  obsolete Cargo 1.61; run Rust commands with
  `rtk env PATH=/Users/frankherchet/.cargo/bin:/opt/local/bin:/usr/bin:/bin cargo +1.98.0 ...`.
- Before handoff, run the smallest relevant checks; for the current codebase this includes `cargo test`, `cargo clippy --all-targets -- -D warnings`, and Swift compilation when the BLE probe changes.
