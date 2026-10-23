---
title: Cost-efficient model delegation
type: preference
created: 2026-10-23
updated: 2026-10-23
---

Cost efficiency matters to the user. Delegate work to the cheapest model that fits it; integration and reviewing the delegated results stay with the primary agent (Opus in Claude Code).

- **Claude Code:** Haiku (`worker`) for mechanical/bounded work (searches, discovery, checks, simple edits); Sonnet for bounded implementation and tests. Opus integrates and reviews the results itself.
- **Codex:** `gpt-6-luna` with `xhigh` reasoning for bounded workers instead of GPT-6.1 Sol medium; with collaboration overrides use `fork_turns: none` and a self-contained assignment.

**Why:** the user said explicitly on 2026-10-23: "wichtig und bitte merken: kosteneffizienz!"
**How to apply:** before doing non-trivial reading or implementation in the primary agent, check whether a Haiku/Sonnet sub-agent can do it with a concrete, bounded brief; don't spend Opus tokens on mechanical work.
