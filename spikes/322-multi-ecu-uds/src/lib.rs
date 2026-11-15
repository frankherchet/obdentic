//! Throwaway spike for #322 (never merged): deterministic simulation of concurrent per-ECU UDS
//! sessions over one serial (ELM327-style) adapter, plus the proposed controller rules.
//!
//! `sim` is a stateful adapter with a virtual clock: ATSH/ATCRA state, one ECU model per
//! request id with its own S3 timer, scripted faults. All data is synthetic.
//! `controller` is the architecture under test. It can only emit the closed vocabulary
//! `ATSH`, `ATCRA`, `10 03`, `10 01`, `3E 80`, `22 <DID>`, `01 <PID>`.

pub mod controller;
pub mod sim;
