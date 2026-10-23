---
title: Knowledge checkout mismatch
type: gotcha
created: 2026-10-23
updated: 2026-10-23
---

On 2026-10-23, the working `knowledge` submodule differs from `knowledge.lock` and causes unrelated schema-loading failures in the full Rust suite. Checks against the pinned revision pass in a separate local verification checkout.

**How to apply:** Compare the current submodule revision with `knowledge.lock` before attributing schema failures to a change. Preserve the user's local checkout; verify the intended pin in isolation. Remove this memo when the mismatch is resolved.
