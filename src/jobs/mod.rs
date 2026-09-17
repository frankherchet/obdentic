//! Diagnostic jobs: the orchestration that used to live in `main.rs`.
//!
//! A job takes a lazily-awaited connect future (never an `adapter_id`), a
//! [`JobRuntime`] to sequence runtime transitions and capture events
//! through, and returns a typed outcome. `main.rs` keeps CLI parsing,
//! dispatch and rendering; it owns no runtime-event sequencing of its own.

mod capture_session;

pub use capture_session::{
    plan_capture, record_capture_start_failure, CapturePlan, CaptureSession,
};

use crate::{
    capture_events::CaptureEvent, capture_format::CaptureSender, runtime_actor::RuntimeClient,
    runtime_reducer::RuntimeEvent, runtime_state::RuntimeState,
};

/// The runtime-transition and capture-event seam every job goes through.
/// This is the only caller of [`crate::scheduler::apply_runtime_event`]; a
/// job body never calls it directly.
pub struct JobRuntime<'a> {
    runtime: &'a RuntimeClient,
    state: &'a mut RuntimeState,
    capture: Option<CaptureSender>,
}

impl<'a> JobRuntime<'a> {
    pub fn new(
        runtime: &'a RuntimeClient,
        state: &'a mut RuntimeState,
        capture: Option<CaptureSender>,
    ) -> Self {
        Self {
            runtime,
            state,
            capture,
        }
    }

    pub fn state(&self) -> &RuntimeState {
        self.state
    }

    pub fn capture(&self) -> Option<&CaptureSender> {
        self.capture.as_ref()
    }

    /// A cloneable handle to the runtime actor this job's transitions go
    /// through. Some jobs (the capture scheduler) need their own clone to
    /// hand to a background task, independent of the `&mut RuntimeState`
    /// this `JobRuntime` borrows for its own transitions.
    pub fn runtime_client(&self) -> RuntimeClient {
        self.runtime.clone()
    }

    /// Apply one authoritative runtime transition and record its evidence.
    pub async fn apply(&mut self, event: RuntimeEvent) -> Result<(), String> {
        crate::scheduler::apply_runtime_event(self.runtime, self.state, self.capture.as_ref(), event)
            .await
    }

    /// Emit a capture event directly, without a runtime transition. A no-op
    /// when the job isn't recording.
    pub async fn emit(&self, event: CaptureEvent) -> Result<(), String> {
        let Some(capture) = self.capture.as_ref() else {
            return Ok(());
        };
        capture
            .send(event)
            .await
            .map_err(|_| "capture recorder is closed".to_string())
    }

    /// Stop observing (if active) and shut down (if not already stopped).
    pub async fn finish(&mut self) -> Result<(), String> {
        if self.state.activity() == crate::runtime_state::Activity::Observe {
            self.apply(RuntimeEvent::ObservationStopped).await?;
        }
        if self.state.phase() != crate::runtime_state::Phase::Stopped {
            self.apply(RuntimeEvent::ShutdownRequested).await?;
            self.apply(RuntimeEvent::ShutdownCompleted).await?;
        }
        Ok(())
    }

    /// Drops this `JobRuntime`'s own clone of the capture sender.
    ///
    /// A [`crate::capture_format::CaptureSink`] only finishes closing once
    /// every clone of its sender is dropped. A job that emits events
    /// through both a `CaptureSink` and a `JobRuntime` built from the same
    /// sender must call this after its last event, before closing the
    /// sink, or the sink's writer task waits forever for a sender this
    /// `JobRuntime` is still holding.
    pub fn release_capture(&mut self) {
        self.capture = None;
    }
}
