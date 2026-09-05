from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {text.count(old)}")
    file.write_text(text.replace(old, new, 1))


# Protocol-normalized responder evidence is passive data and may cross the
# library/binary boundary. It still carries no transport handle or request API.
replace_once(
    "src/elm.rs",
    "#[derive(Clone, Debug, PartialEq, Eq)]\npub(crate) struct ResponseObservation {\n    pub(crate) responses: Vec<crate::capture_events::ResponderEvidence>,\n    pub(crate) selected_responder: Option<String>,\n    pub(crate) selection_error: Option<String>,\n}\n",
    "#[derive(Clone, Debug, PartialEq, Eq)]\npub struct ResponseObservation {\n    pub(crate) responses: Vec<crate::capture_events::ResponderEvidence>,\n    pub(crate) selected_responder: Option<String>,\n    pub(crate) selection_error: Option<String>,\n}\n\nimpl ResponseObservation {\n    pub fn responses(&self) -> &[crate::capture_events::ResponderEvidence] {\n        &self.responses\n    }\n\n    pub fn selected_responder(&self) -> Option<&str> {\n        self.selected_responder.as_deref()\n    }\n\n    pub fn selection_error(&self) -> Option<&str> {\n        self.selection_error.as_deref()\n    }\n}\n",
)

replace_once(
    "src/ble.rs",
    "pub(crate) use crate::elm::{ReadEvidenceError, ResponseObservation};\n",
    "pub(crate) use crate::elm::ReadEvidenceError;\npub use crate::elm::ResponseObservation;\n",
)
replace_once(
    "src/ble.rs",
    "pub(crate) enum ReadOutcome {\n",
    "pub enum ReadOutcome {\n",
)
replace_once(
    "src/ble.rs",
    "    /// Execute one closed EA189 candidate probe while retaining this session.\n    pub async fn read_dpf_probe(\n",
    "    /// Execute one already-typed targeted semantic read while retaining this session.\n    /// The outcome preserves every normalized responder observation.\n    pub async fn read_targeted_with_evidence(\n        &self,\n        request: TargetedReadRequest,\n    ) -> Result<ReadOutcome, String> {\n        self.session.read_targeted_with_evidence(request).await\n    }\n\n    /// Execute one closed EA189 candidate probe while retaining this session.\n    pub async fn read_dpf_probe(\n",
)

# Main CLI/profile bridge.
replace_once(
    "src/main.rs",
    "        CaptureEvent, CaptureSubscription, DiagnosticJobStepStatus, DtcObservationFact,\n        DtcTransportOutcome, SubscriptionFilterOutcome,\n",
    "        CaptureEvent, CaptureSubscription, DiagnosticJobStepStatus, DtcObservationFact,\n        DtcTransportOutcome, ReadTiming, SubscriptionFilterOutcome,\n",
)
replace_once(
    "src/main.rs",
    "    subscription_policy::SubscriptionPolicy,\n",
    "    subscription_policy::{ObservationRequest, PlanStatus, SubscriptionPolicy},\n",
)
replace_once(
    "src/main.rs",
    "const USAGE: &str = \"usage: obdentic signals | obdentic signals --adapter <CoreBluetooth UUID> --supported | obdentic scan | obdentic diagnose dtc.scan --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic diagnose ea189.dpf.probe --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic vehicle identify --adapter <CoreBluetooth UUID> | obdentic vehicle discover --adapter <CoreBluetooth UUID> | obdentic vehicle refresh --adapter <CoreBluetooth UUID> | obdentic vehicle show | obdentic read <signal> --adapter <CoreBluetooth UUID> [--record recording.tsv] | obdentic capture --adapter <CoreBluetooth UUID> --profile <profile> --record <capture.jsonl> | obdentic capture --adapter <CoreBluetooth UUID> --profile ea189-dpf --record <capture.jsonl> --cycles <1..=1440> --interval-seconds <>=30> | obdentic capture inspect <capture.jsonl> | obdentic capture capability <capture.jsonl> | obdentic capture dpf-report <capture.jsonl>... | obdentic demo | obdentic replay <recording.tsv> | obdentic layout save engine-overview <layout.tsv> | obdentic tui demo [--layout layout.tsv] | obdentic tui replay <recording.tsv> [--layout layout.tsv] | obdentic tui capture <capture.jsonl> [--layout layout.tsv] | obdentic tui live --adapter <CoreBluetooth UUID> [--layout layout.tsv] [--record capture.jsonl]\";\n\n#[derive(Debug, PartialEq, Eq)]\nenum Command {\n",
    "const USAGE: &str = \"usage: obdentic signals | obdentic signals --adapter <CoreBluetooth UUID> --supported | obdentic scan | obdentic diagnose dtc.scan --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic diagnose ea189.dpf.probe --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic vehicle identify --adapter <CoreBluetooth UUID> | obdentic vehicle discover --adapter <CoreBluetooth UUID> | obdentic vehicle refresh --adapter <CoreBluetooth UUID> | obdentic vehicle show | obdentic read <signal> --adapter <CoreBluetooth UUID> [--record recording.tsv] | obdentic capture --adapter <CoreBluetooth UUID> --profile <profile> --record <capture.jsonl> | obdentic capture --adapter <CoreBluetooth UUID> --profile <ea189-dpf|ea189-dpf-longitudinal> --record <capture.jsonl> --cycles <1..=1440> --interval-seconds <>=30> | obdentic capture inspect <capture.jsonl> | obdentic capture capability <capture.jsonl> | obdentic capture dpf-report <capture.jsonl>... | obdentic demo | obdentic replay <recording.tsv> | obdentic layout save engine-overview <layout.tsv> | obdentic tui demo [--layout layout.tsv] | obdentic tui replay <recording.tsv> [--layout layout.tsv] | obdentic tui capture <capture.jsonl> [--layout layout.tsv] | obdentic tui live --adapter <CoreBluetooth UUID> [--layout layout.tsv] [--record capture.jsonl]\";\n\nconst EA189_DPF_LONGITUDINAL_CONTEXT: [&str; 5] = [\n    \"engine.rpm\",\n    \"vehicle.speed\",\n    \"engine.load\",\n    \"engine.maf\",\n    \"engine.coolant_temperature\",\n];\n\n#[derive(Clone, Copy, Debug, PartialEq, Eq)]\nenum Ea189DpfTraceProfile {\n    DpfOnly,\n    Longitudinal,\n}\n\nimpl Ea189DpfTraceProfile {\n    fn parse(value: &str) -> Option<Self> {\n        match value {\n            \"ea189-dpf\" => Some(Self::DpfOnly),\n            \"ea189-dpf-longitudinal\" => Some(Self::Longitudinal),\n            _ => None,\n        }\n    }\n\n    const fn cli_name(self) -> &'static str {\n        match self {\n            Self::DpfOnly => \"ea189-dpf\",\n            Self::Longitudinal => \"ea189-dpf-longitudinal\",\n        }\n    }\n\n    const fn capture_profile(self) -> &'static str {\n        match self {\n            Self::DpfOnly => \"ea189.dpf.trace\",\n            Self::Longitudinal => \"ea189-dpf-longitudinal\",\n        }\n    }\n\n    const fn includes_drive_context(self) -> bool {\n        matches!(self, Self::Longitudinal)\n    }\n}\n\n#[derive(Debug, PartialEq, Eq)]\nenum Command {\n",
)
replace_once(
    "src/main.rs",
    "    CaptureEa189DpfTrace {\n        adapter_id: String,\n        recording: String,\n        cycles: u16,\n        interval: Duration,\n    },\n",
    "    CaptureEa189DpfTrace {\n        adapter_id: String,\n        profile: Ea189DpfTraceProfile,\n        recording: String,\n        cycles: u16,\n        interval: Duration,\n    },\n",
)
replace_once(
    "src/main.rs",
    "            Command::CaptureEa189DpfTrace {\n                adapter_id,\n                recording,\n                cycles,\n                interval,\n            } => {\n                run_capture_ea189_dpf_trace(\n                    &adapter_id,\n                    Path::new(&recording),\n                    cycles,\n                    interval,\n                    &runtime,\n                    &mut runtime_state,\n                )\n",
    "            Command::CaptureEa189DpfTrace {\n                adapter_id,\n                profile,\n                recording,\n                cycles,\n                interval,\n            } => {\n                run_capture_ea189_dpf_trace(\n                    &adapter_id,\n                    profile,\n                    Path::new(&recording),\n                    cycles,\n                    interval,\n                    &runtime,\n                    &mut runtime_state,\n                )\n",
)
replace_once(
    "src/main.rs",
    "async fn run_diagnose_ea189_dpf_probe(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    run_ea189_dpf_capture(\n        adapter_id,\n        recording,\n        \"ea189.dpf.probe\",\n        1,\n        None,\n        runtime,\n        state,\n    )\n    .await\n}\n\nasync fn run_capture_ea189_dpf_trace(\n    adapter_id: &str,\n    recording: &Path,\n    cycles: u16,\n    interval: Duration,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    println!(\"capture profile  ea189-dpf\");\n    println!(\"capture cycles   {cycles}\");\n    println!(\"capture interval {} s after each cycle\", interval.as_secs());\n    println!(\"capture record   {}\", recording.display());\n    println!(\"capture running  press Ctrl-C to stop after the active read\");\n    run_ea189_dpf_capture(\n        adapter_id,\n        Some(recording),\n        \"ea189.dpf.trace\",\n        cycles,\n        Some(interval),\n        runtime,\n        state,\n    )\n    .await\n}\n\nasync fn run_ea189_dpf_capture(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    profile: &str,\n    cycles: u16,\n    interval: Option<Duration>,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n",
    "async fn run_diagnose_ea189_dpf_probe(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    run_ea189_dpf_capture(\n        adapter_id,\n        recording,\n        \"ea189.dpf.probe\",\n        1,\n        None,\n        false,\n        runtime,\n        state,\n    )\n    .await\n}\n\nasync fn run_capture_ea189_dpf_trace(\n    adapter_id: &str,\n    trace_profile: Ea189DpfTraceProfile,\n    recording: &Path,\n    cycles: u16,\n    interval: Duration,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    println!(\"capture profile  {}\", trace_profile.cli_name());\n    println!(\"capture cycles   {cycles}\");\n    println!(\"capture interval {} s after each cycle\", interval.as_secs());\n    if trace_profile.includes_drive_context() {\n        println!(\n            \"capture context  {}\",\n            EA189_DPF_LONGITUDINAL_CONTEXT.join(\", \"),\n        );\n    }\n    println!(\"capture record   {}\", recording.display());\n    println!(\"capture running  press Ctrl-C to stop after the active read\");\n    run_ea189_dpf_capture(\n        adapter_id,\n        Some(recording),\n        trace_profile.capture_profile(),\n        cycles,\n        Some(interval),\n        trace_profile.includes_drive_context(),\n        runtime,\n        state,\n    )\n    .await\n}\n\nasync fn run_ea189_dpf_capture(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    profile: &str,\n    cycles: u16,\n    interval: Option<Duration>,\n    include_drive_context: bool,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n",
)
replace_once(
    "src/main.rs",
    "        run_diagnose_ea189_dpf_probe_inner(\n            adapter_id, runtime, state, sender, started, cycles, interval,\n        )\n",
    "        run_diagnose_ea189_dpf_probe_inner(\n            adapter_id,\n            runtime,\n            state,\n            sender,\n            started,\n            cycles,\n            interval,\n            include_drive_context,\n        )\n",
)

# Replace the inner loop while preserving the existing DPF-only behavior and
# introducing per-cycle Observe -> Diagnose transitions only for the bridge.
main = Path("src/main.rs")
text = main.read_text()
pattern = re.compile(r"async fn run_diagnose_ea189_dpf_probe_inner\(.*?\n}\n\nfn capture_offset_us", re.S)
match = pattern.search(text)
if not match:
    raise SystemExit("src/main.rs: inner DPF capture function not found")
replacement = r'''async fn run_diagnose_ea189_dpf_probe_inner(
    adapter_id: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&jsonl_capture::Sender>,
    started: Instant,
    cycles: u16,
    interval: Option<Duration>,
    include_drive_context: bool,
) -> Result<(), String> {
    let mapping = cached_engine_mapping(adapter_id).await?;
    let target = mapping
        .target()
        .target()
        .address()
        .ok_or_else(|| "validated engine target is missing a concrete address".to_string())?;
    let job = DiagnosticJob::ea189_dpf_probe(
        KnownTarget::new(target.value()).map_err(|error| error.to_string())?,
    );
    let plan = job.plan();
    let context = if include_drive_context {
        longitudinal_context_plan(
            &mapping,
            interval.expect("longitudinal traces require an interval"),
        )?
    } else {
        Vec::new()
    };

    for context_read in &context {
        emit_capture_event(
            recorder,
            CaptureSubscription::new(
                context_read.semantic,
                context_read.requested_interval_us,
                SubscriptionFilterOutcome::Scheduled,
            )
            .into_event(),
        )
        .await?;
    }

    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::source(SourceState::Live),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::vehicle(VehicleState::Identified),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::topology(TopologyState::Validated),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    let prepared = ble::prepare_diagnostic_session(adapter_id).await?;
    record_protocol_negotiation(recorder, prepared.negotiation()).await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::transport(TransportState::Connected),
    )
    .await?;

    if !include_drive_context {
        apply_runtime_event(runtime, state, recorder, RuntimeEvent::DiagnosticJobStarted).await?;
        emit_capture_event(recorder, CaptureEvent::diagnostic_job_started(&job)).await?;
    }

    let run_result = async {
        let mut recoverable = false;
        let mut completed_cycles = 0_u16;
        let mut cancelled = false;
        let mut cancellation = interval.map(|_| {
            tokio::spawn(async {
                let _ = tokio::signal::ctrl_c().await;
            })
        });

        while completed_cycles < cycles {
            if cancellation
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                cancelled = true;
                break;
            }
            if completed_cycles > 0 {
                let delay = interval.expect("trace cycles require an interval");
                println!("capture pause  {} s", delay.as_secs());
                if let Some(cancellation) = &mut cancellation {
                    cancelled = tokio::select! {
                        _ = tokio::time::sleep(delay) => false,
                        _ = cancellation => true,
                    };
                } else {
                    tokio::time::sleep(delay).await;
                }
                if cancelled {
                    break;
                }
            }
            if cycles > 1 {
                println!("capture cycle  {}/{}", completed_cycles + 1, cycles);
            }

            if include_drive_context {
                let due_us = capture_offset_us(started)?;
                cancelled = capture_longitudinal_context_cycle(
                    &prepared,
                    runtime,
                    state,
                    recorder,
                    started,
                    due_us,
                    &context,
                    cancellation.as_ref(),
                )
                .await?;
                if cancelled {
                    break;
                }
                apply_runtime_event(
                    runtime,
                    state,
                    recorder,
                    RuntimeEvent::DiagnosticJobStarted,
                )
                .await?;
                emit_capture_event(recorder, CaptureEvent::diagnostic_job_started(&job)).await?;
            }

            let mut cycle_recoverable = false;
            for step in plan.steps() {
                if cancellation
                    .as_ref()
                    .is_some_and(tokio::task::JoinHandle::is_finished)
                {
                    cancelled = true;
                    break;
                }
                let probe = step
                    .dpf_probe()
                    .ok_or_else(|| "EA189 DPF plan contains a non-probe step".to_string())?;
                match SafetyPolicy::read_only().authorize_activity(
                    Activity::Diagnose,
                    OperationRequest::ea189_dpf_probe(probe, job_target(&job)?),
                ) {
                    Ok(Operation::Ea189DpfProbe(_)) => {}
                    Ok(_) => {
                        return Err(
                            "read-only safety policy returned the wrong EA189 operation".into(),
                        )
                    }
                    Err(error) => return Err(error.to_string()),
                }
                let request = ble::TargetedDpfProbeRequest::from_mapping(probe, &mapping)?;
                match prepared.read_dpf_probe(request).await {
                    Ok(responses) => {
                        let response = responses.as_slice().first().ok_or_else(|| {
                            format!("{} returned no normalized response", probe.semantic())
                        })?;
                        let expected_responder = mapping.expected_responder().value();
                        let responder_matches = response
                            .responder
                            .as_ref()
                            .is_some_and(|responder| Some(responder.as_str()) == expected_responder);
                        let status = if response.payload.starts_with(&[
                            0x62,
                            probe.request_bytes()[1],
                            probe.request_bytes()[2],
                        ]) && responder_matches
                        {
                            DiagnosticJobStepStatus::Success
                        } else {
                            recoverable = true;
                            cycle_recoverable = true;
                            DiagnosticJobStepStatus::Recoverable
                        };
                        let selected = responder_matches.then(|| {
                            response
                                .responder
                                .as_ref()
                                .expect("matching responder is present")
                                .as_str()
                                .to_owned()
                        });
                        emit_capture_event(
                            recorder,
                            CaptureEvent::responses_observed_at(
                                probe.semantic(),
                                probe.request_bytes().into(),
                                responses.capture_evidence(),
                                selected.clone(),
                                (status != DiagnosticJobStepStatus::Success)
                                    .then_some("unexpected_or_negative_uds_response".into()),
                                capture_offset_us(started)?,
                            )?,
                        )
                        .await?;
                        emit_capture_event(
                            recorder,
                            CaptureEvent::diagnostic_job_step(
                                job.id().to_string(),
                                step.sequence()
                                    .try_into()
                                    .map_err(|_| "EA189 DPF step index exceeds u64")?,
                                0x22,
                                selected.clone(),
                                status,
                                (status != DiagnosticJobStepStatus::Success)
                                    .then_some("negative_or_malformed_response".into()),
                            )?,
                        )
                        .await?;
                        println!(
                            "probe\t{}\t{:04X}\t{}\t{}",
                            probe.semantic(),
                            probe.id(),
                            selected.unwrap_or_else(|| "unknown".into()),
                            hex(&response.payload),
                        );
                    }
                    Err(error)
                        if include_drive_context
                            && obdentic::scheduler::is_fatal_runtime_error(&error) =>
                    {
                        emit_capture_event(
                            recorder,
                            CaptureEvent::diagnostic_job_step(
                                job.id().to_string(),
                                step.sequence()
                                    .try_into()
                                    .map_err(|_| "EA189 DPF step index exceeds u64")?,
                                0x22,
                                None,
                                DiagnosticJobStepStatus::Fatal,
                                Some("session_failed".into()),
                            )?,
                        )
                        .await?;
                        emit_capture_event(
                            recorder,
                            CaptureEvent::DiagnosticJobFailed {
                                job_id: job.id().to_string(),
                                error: "session_failed".into(),
                            },
                        )
                        .await?;
                        apply_runtime_event(
                            runtime,
                            state,
                            recorder,
                            RuntimeEvent::transport(TransportState::Unhealthy),
                        )
                        .await?;
                        apply_runtime_event(
                            runtime,
                            state,
                            recorder,
                            RuntimeEvent::FatalRuntimeError,
                        )
                        .await?;
                        return Err(error);
                    }
                    Err(error) => {
                        recoverable = true;
                        cycle_recoverable = true;
                        emit_capture_event(
                            recorder,
                            CaptureEvent::diagnostic_job_step(
                                job.id().to_string(),
                                step.sequence()
                                    .try_into()
                                    .map_err(|_| "EA189 DPF step index exceeds u64")?,
                                0x22,
                                None,
                                DiagnosticJobStepStatus::Recoverable,
                                Some(error.clone()),
                            )?,
                        )
                        .await?;
                        println!(
                            "probe\t{}\t{:04X}\terror\t{error}",
                            probe.semantic(),
                            probe.id()
                        );
                    }
                }
            }

            if include_drive_context {
                if cancelled {
                    emit_capture_event(
                        recorder,
                        CaptureEvent::diagnostic_job_cancelled(job.id().to_string()),
                    )
                    .await?;
                } else {
                    emit_capture_event(
                        recorder,
                        CaptureEvent::DiagnosticJobCompleted {
                            job_id: job.id().to_string(),
                            status: if cycle_recoverable {
                                JobStatus::CompletedWithErrors
                            } else {
                                JobStatus::Completed
                            },
                        },
                    )
                    .await?;
                }
                apply_runtime_event(
                    runtime,
                    state,
                    recorder,
                    RuntimeEvent::DiagnosticJobCompleted,
                )
                .await?;
            }

            if cancelled {
                break;
            }
            completed_cycles += 1;
        }

        if let Some(cancellation) = cancellation {
            cancellation.abort();
        }

        if include_drive_context {
            Ok(())
        } else {
            if cancelled {
                emit_capture_event(
                    recorder,
                    CaptureEvent::diagnostic_job_cancelled(job.id().to_string()),
                )
                .await?;
            } else {
                emit_capture_event(
                    recorder,
                    CaptureEvent::DiagnosticJobCompleted {
                        job_id: job.id().to_string(),
                        status: if recoverable {
                            JobStatus::CompletedWithErrors
                        } else {
                            JobStatus::Completed
                        },
                    },
                )
                .await?;
            }
            finish_diagnostic(runtime, state, recorder).await
        }
    }
    .await;

    let shutdown = prepared.shutdown().await;
    let disconnected = if include_drive_context
        && state.phase() != obdentic::runtime_state::Phase::Fault
        && state.phase() != obdentic::runtime_state::Phase::Stopped
    {
        apply_runtime_event(
            runtime,
            state,
            recorder,
            RuntimeEvent::transport(TransportState::Disconnected),
        )
        .await
    } else {
        Ok(())
    };

    run_result.and(shutdown).and(disconnected)
}

#[derive(Clone, Debug)]
struct LongitudinalContextRead {
    semantic: &'static str,
    request: ReadRequest,
    targeted: ble::TargetedReadRequest,
    requested_interval_us: u64,
}

fn longitudinal_context_plan(
    mapping: &EcuTargetMapping,
    interval: Duration,
) -> Result<Vec<LongitudinalContextRead>, String> {
    if mapping.role().role() != &EcuRole::Engine {
        return Err("EA189 longitudinal context requires a validated engine mapping".into());
    }
    let requested = EA189_DPF_LONGITUDINAL_CONTEXT
        .iter()
        .map(|semantic| ObservationRequest::new("ea189-dpf-longitudinal", *semantic, interval))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let policy = SubscriptionPolicy::new(HardwareCapability::conservative_default()).plan(
        &requested,
        EA189_DPF_LONGITUDINAL_CONTEXT,
    );
    if policy
        .entries()
        .iter()
        .any(|entry| entry.status() != PlanStatus::Accepted)
    {
        return Err("EA189 longitudinal context exceeds the conservative session budget".into());
    }

    let requested_interval_us = interval
        .as_micros()
        .try_into()
        .map_err(|_| "EA189 longitudinal interval exceeds supported range")?;
    let responder = mapping
        .expected_responder()
        .value()
        .ok_or_else(|| "EA189 longitudinal context requires an expected engine responder".to_string())?;

    EA189_DPF_LONGITUDINAL_CONTEXT
        .iter()
        .map(|semantic| {
            let request = prepare_read(semantic)?;
            let request = match SafetyPolicy::read_only().authorize_activity(
                Activity::Observe,
                OperationRequest::read_signal_typed(request),
            ) {
                Ok(Operation::ReadSignal(request)) => request,
                Ok(_) => {
                    return Err(
                        "read-only safety policy returned the wrong longitudinal operation".into(),
                    )
                }
                Err(error) => return Err(error.to_string()),
            };
            let targeted = ble::TargetedReadRequest::new(
                request,
                mapping.target().target().clone(),
                ble::ResponderIdentity::ElmHeader(responder.to_owned()),
            )?;
            Ok(LongitudinalContextRead {
                semantic,
                request,
                targeted,
                requested_interval_us,
            })
        })
        .collect()
}

async fn capture_longitudinal_context_cycle(
    prepared: &ble::PreparedDiagnosticSession,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&jsonl_capture::Sender>,
    started: Instant,
    due_us: u64,
    context: &[LongitudinalContextRead],
    cancellation: Option<&tokio::task::JoinHandle<()>>,
) -> Result<bool, String> {
    apply_runtime_event(runtime, state, recorder, RuntimeEvent::ObservationStarted).await?;
    let mut cancelled = false;

    for context_read in context {
        if cancellation.is_some_and(tokio::task::JoinHandle::is_finished) {
            cancelled = true;
            break;
        }
        apply_runtime_event(
            runtime,
            state,
            recorder,
            RuntimeEvent::ObservationReadStarted,
        )
        .await?;
        let read_started_us = capture_offset_us(started)?;
        let outcome = prepared
            .read_targeted_with_evidence(context_read.targeted.clone())
            .await;
        let finished_us = capture_offset_us(started)?;
        let timing = ReadTiming::new(due_us, read_started_us, finished_us);

        match outcome {
            Ok(ble::ReadOutcome::Succeeded {
                transaction,
                observations,
            }) => {
                emit_longitudinal_response_observations(
                    recorder,
                    context_read,
                    &observations,
                    finished_us,
                )
                .await?;
                emit_capture_event(
                    recorder,
                    CaptureEvent::read_succeeded_from_transaction(
                        &transaction,
                        context_read.requested_interval_us,
                        timing,
                    )?,
                )
                .await?;
                apply_runtime_event(
                    runtime,
                    state,
                    recorder,
                    RuntimeEvent::ObservationReadCompleted,
                )
                .await?;
                println!(
                    "context\t{}\t{} {}",
                    context_read.semantic,
                    transaction.value(),
                    transaction.unit()
                );
            }
            Ok(ble::ReadOutcome::Failed {
                error,
                observations,
            }) => {
                emit_longitudinal_response_observations(
                    recorder,
                    context_read,
                    &observations,
                    finished_us,
                )
                .await?;
                emit_capture_event(
                    recorder,
                    CaptureEvent::read_failed(
                        context_read.semantic,
                        context_read.requested_interval_us,
                        Some(timing),
                        Some(context_read.request.bytes().into()),
                        error.clone(),
                    ),
                )
                .await?;
                println!("context\t{}\terror\t{error}", context_read.semantic);
                if obdentic::scheduler::is_fatal_runtime_error(&error) {
                    apply_runtime_event(
                        runtime,
                        state,
                        recorder,
                        RuntimeEvent::transport(TransportState::Unhealthy),
                    )
                    .await?;
                    apply_runtime_event(
                        runtime,
                        state,
                        recorder,
                        RuntimeEvent::FatalRuntimeError,
                    )
                    .await?;
                    return Err(error);
                }
                apply_runtime_event(
                    runtime,
                    state,
                    recorder,
                    RuntimeEvent::ObservationReadFailedRecoverable,
                )
                .await?;
            }
            Err(error) => {
                emit_capture_event(
                    recorder,
                    CaptureEvent::read_failed(
                        context_read.semantic,
                        context_read.requested_interval_us,
                        Some(timing),
                        Some(context_read.request.bytes().into()),
                        error.clone(),
                    ),
                )
                .await?;
                println!("context\t{}\terror\t{error}", context_read.semantic);
                if obdentic::scheduler::is_fatal_runtime_error(&error) {
                    apply_runtime_event(
                        runtime,
                        state,
                        recorder,
                        RuntimeEvent::transport(TransportState::Unhealthy),
                    )
                    .await?;
                    apply_runtime_event(
                        runtime,
                        state,
                        recorder,
                        RuntimeEvent::FatalRuntimeError,
                    )
                    .await?;
                    return Err(error);
                }
                apply_runtime_event(
                    runtime,
                    state,
                    recorder,
                    RuntimeEvent::ObservationReadFailedRecoverable,
                )
                .await?;
            }
        }
    }

    if state.activity() == Activity::Observe {
        apply_runtime_event(runtime, state, recorder, RuntimeEvent::ObservationStopped).await?;
    }
    Ok(cancelled)
}

async fn emit_longitudinal_response_observations(
    recorder: Option<&jsonl_capture::Sender>,
    context_read: &LongitudinalContextRead,
    observations: &[ble::ResponseObservation],
    offset_us: u64,
) -> Result<(), String> {
    for observation in observations {
        if observation.responses().is_empty() {
            continue;
        }
        emit_capture_event(
            recorder,
            CaptureEvent::responses_observed_at(
                context_read.semantic,
                context_read.request.bytes().into(),
                observation.responses().to_vec(),
                observation.selected_responder().map(str::to_owned),
                observation.selection_error().map(str::to_owned),
                offset_us,
            )?,
        )
        .await?;
    }
    Ok(())
}

fn capture_offset_us'''
text = text[: match.start()] + replacement + text[match.end() :]
main.write_text(text)

replace_once(
    "src/main.rs",
    "        [command, adapter_flag, adapter_id, profile_flag, profile, record_flag, path, cycles_flag, cycles, interval_flag, interval_seconds]\n            if command == \"capture\"\n                && profile == \"ea189-dpf\"\n                && adapter_flag == \"--adapter\"\n                && profile_flag == \"--profile\"\n                && record_flag == \"--record\"\n                && cycles_flag == \"--cycles\"\n                && interval_flag == \"--interval-seconds\" =>\n        {\n            require_uuid(adapter_id)?;\n            Ok(Command::CaptureEa189DpfTrace {\n                adapter_id: adapter_id.clone(),\n                recording: path.clone(),\n                cycles: parse_trace_cycles(cycles)?,\n                interval: Duration::from_secs(parse_trace_interval_seconds(interval_seconds)?),\n            })\n        }\n",
    "        [command, adapter_flag, adapter_id, profile_flag, profile, record_flag, path, cycles_flag, cycles, interval_flag, interval_seconds]\n            if command == \"capture\"\n                && Ea189DpfTraceProfile::parse(profile).is_some()\n                && adapter_flag == \"--adapter\"\n                && profile_flag == \"--profile\"\n                && record_flag == \"--record\"\n                && cycles_flag == \"--cycles\"\n                && interval_flag == \"--interval-seconds\" =>\n        {\n            require_uuid(adapter_id)?;\n            Ok(Command::CaptureEa189DpfTrace {\n                adapter_id: adapter_id.clone(),\n                profile: Ea189DpfTraceProfile::parse(profile)\n                    .expect(\"guard accepts only known EA189 trace profiles\"),\n                recording: path.clone(),\n                cycles: parse_trace_cycles(cycles)?,\n                interval: Duration::from_secs(parse_trace_interval_seconds(interval_seconds)?),\n            })\n        }\n",
)
replace_once(
    "src/main.rs",
    "            Ok(Command::CaptureEa189DpfTrace {\n                adapter_id: uuid.into(),\n                recording: \"trace.jsonl\".into(),\n                cycles: 120,\n                interval: Duration::from_secs(60),\n            })\n        );\n",
    "            Ok(Command::CaptureEa189DpfTrace {\n                adapter_id: uuid.into(),\n                profile: Ea189DpfTraceProfile::DpfOnly,\n                recording: \"trace.jsonl\".into(),\n                cycles: 120,\n                interval: Duration::from_secs(60),\n            })\n        );\n        assert_eq!(\n            parse_command(&args(&[\n                \"capture\",\n                \"--adapter\",\n                uuid,\n                \"--profile\",\n                \"ea189-dpf-longitudinal\",\n                \"--record\",\n                \"longitudinal.jsonl\",\n                \"--cycles\",\n                \"120\",\n                \"--interval-seconds\",\n                \"60\",\n            ])),\n            Ok(Command::CaptureEa189DpfTrace {\n                adapter_id: uuid.into(),\n                profile: Ea189DpfTraceProfile::Longitudinal,\n                recording: \"longitudinal.jsonl\".into(),\n                cycles: 120,\n                interval: Duration::from_secs(60),\n            })\n        );\n",
)

# Add a pure plan/safety test next to the existing engine-target tests.
replace_once(
    "src/main.rs",
    "    #[test]\n    fn confirmed_engine_target_preserves_distinct_role_target_and_responder() {\n",
    "    #[test]\n    fn longitudinal_context_is_explicit_policy_admitted_and_engine_targeted() {\n        let mapping = confirmed_engine_target()\n            .unwrap()\n            .to_vehicle_knowledge_mapping()\n            .unwrap();\n        let plan = longitudinal_context_plan(&mapping, Duration::from_secs(30)).unwrap();\n        assert_eq!(\n            plan.iter().map(|entry| entry.semantic).collect::<Vec<_>>(),\n            EA189_DPF_LONGITUDINAL_CONTEXT,\n        );\n        assert!(plan.iter().all(|entry| {\n            entry.targeted.target().address().unwrap().value() == \"7E0\"\n                && entry.targeted.expected_responder().as_str() == \"7E8\"\n                && entry.requested_interval_us == 30_000_000\n        }));\n        assert!(longitudinal_context_plan(&mapping, Duration::from_millis(1)).is_err());\n    }\n\n    #[test]\n    fn confirmed_engine_target_preserves_distinct_role_target_and_responder() {\n",
)

Path("docs/ea189-dpf-longitudinal.md").write_text("""# EA189 DPF longitudinal capture bridge

`ea189-dpf-longitudinal` is a temporary read-only capture bridge for collecting
DPF evidence together with a small drive-context snapshot in one exclusive
physical adapter session.

It exists to support hardware validation for #14 before the final declarative
profile architecture in #88 is available. It is intentionally not a second
profile framework and should be removed once effective Vehicle Knowledge and
YAML profiles can express the same intent.

## Closed observation set

Each cycle reads these existing standard semantic facts, in this fixed order:

- `engine.rpm`
- `vehicle.speed`
- `engine.load`
- `engine.maf`
- `engine.coolant_temperature`

The context snapshot is followed by the existing closed seven-step
`ea189.dpf.probe` job. No PID, DID, address, service, decoder or raw command is
accepted from the profile name or CLI.

At the minimum 30 second pause the bridge requests twelve logical reads per
cycle, or at most 0.4 logical reads/s averaged over the requested pause. Reads
remain sequential. The actual cycle period is the time required for the
context + DPF reads plus `--interval-seconds`.

## Safety and evidence

- the cached engine mapping must validate before capture;
- standard context reads are authorized as `Activity::Observe` / `SignalRead`;
- each DPF step is separately authorized as `Activity::Diagnose` /
  `Ea189DpfProbe`;
- one prepared diagnostic session owns the physical adapter for the complete
  trace;
- every normalized responder observation from targeted context reads is
  retained before the selected semantic transaction;
- DPF responder evidence remains unchanged from the existing trace path;
- no `DiagnosticSessionControl`, `SecurityAccess`, write, actuator, coding,
  adaptation, DTC-clear, raw CAN, raw UDS or raw ELM path is added.

The experimental DPF decoders remain experimental. A long capture is evidence
for later offline validation; it is not itself knowledge promotion.

## Usage

```bash
cargo run --release -- \\
  capture \\
  --adapter \"$ADAPTER_UUID\" \\
  --profile ea189-dpf-longitudinal \\
  --record captures/dpf-longitudinal.jsonl \\
  --cycles 120 \\
  --interval-seconds 60
```

Use JSONL until another capture writer has independently merged into `main`.
Keep real vehicle captures local/private unless explicitly sanitized.
""")
