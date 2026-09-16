# MF4 capture backend

OBDentic can persist the same passive `CaptureEvent` stream either as JSONL or ASAM MDF4. The writer is selected by the capture path extension: `.jsonl` selects the existing JSONL writer and `.mf4` selects the MF4 writer.

## Architecture and safety boundary

The format boundary lives behind `CaptureWriterPlugin`. A writer plugin only receives already-produced `CaptureEvent` values through an `mpsc` channel. It does not receive an adapter, `DiagnosticSession`, scheduler, vehicle knowledge, transport handle, or any API that can issue diagnostic traffic.

This keeps the required direction intact:

```text
transport evidence
-> protocol normalization
-> vehicle facts
-> semantic/capture events
-> passive writer plugin
```

The MF4 backend therefore cannot add requests, scan identifiers, clear DTCs, perform coding/adaptation, execute actuator tests, or use SecurityAccess. It is a persistence backend only.

## Dependency choice

The backend uses `mdf4-rs` 0.6.0. OBDentic pins the crate and disables its default features, enabling only `std`. In particular, the optional `can` and `dbc` features are not enabled: OBDentic exports normalized diagnostic capture evidence and does not pretend that adapter-level diagnostic payloads are raw CAN frames.

`mdf4-rs` 0.6.0 is published under `MIT OR Apache-2.0` and declares Rust 1.97.1, below OBDentic's pinned Rust 1.98 toolchain.

## OBDENTIC-MF4 v1 layout

A capture contains one channel group named `OBDentic capture records`. Using a single group allows event summaries and evidence chunks to remain in one ordered streaming record sequence.

Important channels are:

- `time`: MDF master time in seconds. For successful reads this is the true `finished_us` timestamp. Other timed events use their own capture offset; untimed lifecycle records retain the preceding monotonic offset.
- `record_kind`: `1` for an event-summary record, `2` for an evidence-chunk record.
- `event_sequence`: original capture-event order.
- `event_kind`: stable OBDENTIC-MF4 v1 event discriminator.
- `requested_interval_us`, `due_us`, `started_us`, `finished_us`: scheduling/timing evidence where applicable.
- `decoded_numeric_value` and `decoded_value_kind`: convenient measurement channels. Exact non-numeric event values remain in the canonical event envelope described below.
- `semantic_fnv1a64`: convenience lookup key only. The exact semantic string is also retained as evidence; the hash is never treated as semantic identity.
- `evidence_field_kind`, `evidence_item_index`, `evidence_chunk_index`, `evidence_chunk_len`, `evidence_chunk`: lossless variable-length evidence storage.

The evidence field vocabulary is documented in the MF4 channel metadata. It includes the canonical JSONL event envelope, exact semantic text, normalized diagnostic request bytes, normalized diagnostic response bytes, responder identity, selected responder, profile, unit, source, decoder, provenance, and errors.

Every event is therefore preserved as the same versioned canonical event envelope used by JSONL. Frequently queried diagnostic evidence is additionally copied into typed binary evidence fields so offline tooling does not need to parse the envelope to recover request/response bytes.

## Responder evidence

`ResponsesObserved` preserves every responder/payload pair in capture order. Each response receives its own `evidence_item_index`; responder identity and response payload use that same index. `selected_responder` and `selection_error` are retained separately.

No writer-side plausibility selection is performed. The MF4 plugin persists the responder evidence that the diagnostic core already produced.

## Variable-length evidence

MDF channels have fixed record widths. Variable-length OBDentic evidence is therefore split into fixed 64-byte chunks. `evidence_chunk_len` states how many bytes of the chunk are meaningful, and `evidence_chunk_index` orders chunks within one `(event_sequence, evidence_field_kind, evidence_item_index)` tuple.

This avoids silent truncation and supports payloads and metadata longer than one record.

## Completion and partial files

A graceful recorder close finishes the MDF data block and finalizes/flushed the file. Tests reopen the produced file with the `mdf4-rs` parser and verify that records are readable.

The writer also flushes periodically while a capture is running. However, `mdf4-rs` patches final data-block sizes and record counts when the data block is finished. A process crash or power loss before graceful close can therefore leave an MF4 file structurally incomplete even though previously flushed bytes are present. OBDentic must not report such a file as a complete capture. Recovery, if attempted by an external MDF repair tool, is best-effort and does not restore a missing OBDentic lifecycle completion event.

This differs from JSONL, where a truncated final line can be classified explicitly as a partial capture by the JSONL reader.

## Validation

Software acceptance is split from vehicle/hardware acceptance:

- unit/integration tests create an MF4 capture, close it, reopen it through the MDF parser, and verify the channel/record structure;
- project CI runs `cargo fmt --check`, `cargo test --locked`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo build --locked`;
- MF4 export is transport-independent and needs no vehicle for software acceptance;
- interoperability with additional third-party MDF viewers/readers is a separate file-format acceptance check and does not require live vehicle access.

When validating with another MDF implementation, verify both a numeric `ReadSucceeded` sample and the chunked diagnostic request/response evidence. Those byte fields are normalized diagnostic payloads, not asserted CAN frames.
