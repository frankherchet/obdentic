# OBDentic agent instructions

## Delegation

Use sub-agents deliberately instead of implementing every non-trivial task in the primary agent. Use `gpt-5.6-luna` exclusively for sub-agents; do not delegate to `gpt-5.6-terra` or `gpt-5.6-sol`. Recreate this worker when useful at the start of a new session:

- `luna_worker`: `gpt-5.6-luna`, `xhigh` reasoning — repository discovery, mechanical checks, focused searches, bounded implementation, tests and isolated bug fixes.

Choose the lowest-cost worker that can reliably handle the task. Give every worker a concrete, bounded assignment and a non-overlapping write scope. Parallelize independent work only; the primary agent owns integration and final verification. Do not delegate trivial work when coordination would cost more than doing it directly.

## Issue workflow

When work is tracked by a GitHub issue, completing that work includes committing its intended changes on a feature branch, opening a pull request, and merging it into `main` only after CI succeeds. Enable auto-merge for a ready pull request so GitHub merges it once its required CI checks pass. Immediately after that successful merge, close the implemented issue. Keep the milestone open when a separately recorded hardware acceptance remains; close it after all its issues and required acceptance work are complete.

## Project constraints

- Treat `docs/project-goals.md` as the long-term product contract. If an implementation decision changes those goals, update that document deliberately rather than allowing architecture to drift implicitly.
- OBDentic is read-only by default. Existing read-only invariants remain authoritative unless a separately reviewed issue explicitly implements one of the narrow process-start service capabilities allowed by `docs/project-goals.md`.
- The first and currently only intended mutating capability is DTC clearing. It must be unavailable by default, enabled only explicitly at process start, represented as a closed typed operation/job, and must never become a generic `--write`, raw CAN/UDS/ELM, arbitrary-address, coding, adaptation, actuator, SecurityAccess or RoutineControl escape hatch.
- Profiles, layouts, Knowledge DB content, MCP calls and AI prompts must never elevate the running process from read-only into a mutating capability. Until a reviewed DTC-clear capability issue lands, do not send DTC-clear traffic at all.
- Keep adapter transport separate from deterministic vehicle decoding.
- Preserve raw TX/RX visibility and test through the highest practical transport/replay seam.
- Treat Bluetooth captures as sensitive: they may contain VINs, device identifiers and authentication material. Keep `captures/` untracked and never publish raw captures.
- Prefer the smallest working vertical slice and avoid speculative abstractions or dependencies.
- Prefix shell commands with `rtk`. On this host, `/opt/local/bin/cargo` is an
  obsolete Cargo 1.61; run Rust commands with
  `rtk env PATH=/Users/frankherchet/.cargo/bin:/opt/local/bin:/usr/bin:/bin cargo +1.98.0 ...`.
- Before handoff, run the smallest relevant checks; for the current codebase this includes `cargo test`, `cargo clippy --all-targets -- -D warnings`, and Swift compilation when the BLE probe changes.
- A meaningful feature or bug-fix task is complete only after its relevant checks pass, its intended changes are committed to a feature branch, its pull request is merged into `main`, and the merge is visible on `origin/main`.
