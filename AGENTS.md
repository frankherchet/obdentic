# OBDentic agent instructions

These instructions apply to any coding agent working in this repository, regardless of harness (Codex CLI, Claude Code, or otherwise). Harness-specific mechanics — which sub-agents exist and how to invoke them — live in that harness's own configuration; see "Harness-specific configuration" at the end of this file.

## Delegation

Delegate bounded, non-trivial work to sub-agents instead of implementing everything in the primary agent. Give every sub-agent a concrete, bounded assignment and a non-overlapping write scope. Choose the lowest-cost sub-agent that can reliably handle the task. Parallelize independent work only; the primary agent owns integration and final verification. Do not delegate trivial work when coordination would cost more than doing it directly.

Good delegation targets: repository discovery, mechanical checks, focused searches, bounded implementation, tests, and isolated bug fixes.

## Issue workflow

When work is tracked by a GitHub issue, completing that work includes committing its intended changes on a feature branch, opening a pull request, and merging it into `main` only after CI succeeds. Enable auto-merge for a ready pull request so GitHub merges it once its required CI checks pass. Immediately after that successful merge, close the implemented issue. Keep the milestone open when a separately recorded hardware acceptance remains; close it after all its issues and required acceptance work are complete.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues for `frankherchet/obdentic`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context layout: `CONTEXT.md` and `docs/adr/`. See `docs/agents/domain.md`.

## Project constraints

- Treat `docs/project-goals.md` as the long-term product contract. If an implementation decision changes those goals, update that document deliberately rather than allowing architecture to drift implicitly.
- OBDentic is read-only by default. Existing read-only invariants remain authoritative unless a separately reviewed issue explicitly implements one of the narrow process-start service capabilities allowed by `docs/project-goals.md`.
- The first and currently only intended mutating capability is DTC clearing. It must be unavailable by default, enabled only explicitly at process start, represented as a closed typed operation/job, and must never become a generic `--write`, raw CAN/UDS/ELM, arbitrary-address, coding, adaptation, actuator, SecurityAccess or RoutineControl escape hatch.
- Profiles, layouts, Knowledge DB content, MCP calls and AI prompts must never elevate the running process from read-only into a mutating capability. Until a reviewed DTC-clear capability issue lands, do not send DTC-clear traffic at all.
- Keep adapter transport separate from deterministic vehicle decoding.
- Preserve raw TX/RX visibility and test through the highest practical transport/replay seam.
- Treat vehicle/Bluetooth evidence as sensitive because it can contain VINs, ECU serials, mileage, device identifiers and authentication material. Local analysis, replay and explicit local-terminal display of the user's own data are allowed. The boundary is publication: never stage, commit, attach to a PR, paste into a GitHub issue/comment, or otherwise publish raw captures or vehicle-specific evidence. Keep `captures/` and `evidence/` untracked; before every commit, inspect the staged diff for such data.
- Prefer the smallest working vertical slice and avoid speculative abstractions or dependencies.
- Prefix shell commands with `rtk`. On this host, `/opt/local/bin/cargo` is an
  obsolete Cargo 1.61; run Rust commands with
  `rtk env PATH=/Users/frankherchet/.cargo/bin:/opt/local/bin:/usr/bin:/bin cargo +1.98.0 ...`.
- Before handoff, run the smallest relevant checks; for the current codebase this includes `cargo test`, `cargo clippy --all-targets -- -D warnings`, and Swift compilation when the BLE probe changes.
- A meaningful feature or bug-fix task is complete only after its relevant checks pass, its intended changes are committed to a feature branch, its pull request is merged into `main`, and the merge is visible on `origin/main`.

## Harness-specific configuration

- **Codex CLI**: define named sub-agent/model profiles in your own `~/.codex/config.toml` (or `agents/*.toml`). This repository does not pin sub-agent names or models — use whatever profile your environment provides, and apply the Delegation principles above when choosing one.
- **Claude Code**: sub-agents are defined in `.claude/agents/*.md`; `.claude/agents/worker.md` is the general delegation target for the tasks described above. `CLAUDE.md` at the repository root imports this file (`@AGENTS.md`), so these instructions apply automatically.
