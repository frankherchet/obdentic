# 0004. The signal key routes decoding in one module
Date: 2026-11-13
Status: accepted

## Context
A signal is either a standard Mode 01 PID or a Knowledge definition of one ECU. A Knowledge semantic can coincide with a PID semantic (`engine.oil_temperature`) or with the same semantic of another ECU, so #256 addressed Knowledge signals as `<semantic>@<target id>`. The routing and decoding rules were still written once per context: live reads, `replay.read`, the availability sweep, learning, CLI capture replay and the mobile report each decided "PID or Knowledge" and decoded on their own. REQ-024.4 and #300 each fixed one such copy; the report still collapsed signals that share a semantic.

## Decision
`src/signal_read.rs` owns the rule and the decoding. It has no transport, session or authorisation.
- `SignalRoute::of(key)`: `x@t` is Knowledge signal `x` of target `t`; a known PID semantic is standard; anything else is a bare Knowledge semantic that the caller resolves against its identified ECUs.
- `key_of_recording` derives the key of an old recording (`semantic@target_id` when it has a `definition_id`, else the semantic) and rejects a recorded `key` that contradicts the record.
- `decode(key, Decoder, Evidence)` turns a decoder and plain evidence (response or error) into one `SignalOutcome`: value, rejection (NRC) or no answer. The views `read_json`, `sweep_entry` and `learning_json` keep each context's existing JSON vocabulary.
- Each context still finds the evidence and resolves the definition: live transport, UDS sessions and `SafetyPolicy` stay in `mobile.rs`; pinned-catalog lookup and recorded-evidence extraction stay in `mobile/knowledge.rs`; CLI captures resolve the definition from their recorded reference.
- Every read result carries `key`; the mobile report keys its last values by it.

## Consequences
- One place changes when routing or a decoder changes; the equivalence of live, replay, sweep and CLI decoding is tested on the same evidence.
- The report's `last_values` keys of Knowledge reads change from `semantic` to `semantic@target`.
- CLI capture replay still falls back to the plain semantic for a definition that is not in the pinned catalog; mobile replay still rejects it. Unifying that is a separate decision.
