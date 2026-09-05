from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {text.count(old)}")
    file.write_text(text.replace(old, new, 1))


path = "src/main.rs"

replace_once(
    path,
    "async fn run_diagnose_ea189_dpf_probe(\n",
    "#[derive(Clone, Copy)]\nstruct Ea189DpfCaptureConfig<'a> {\n    profile: &'a str,\n    cycles: u16,\n    interval: Option<Duration>,\n    include_drive_context: bool,\n}\n\nasync fn run_diagnose_ea189_dpf_probe(\n",
)

replace_once(
    path,
    "    run_ea189_dpf_capture(\n        adapter_id,\n        recording,\n        \"ea189.dpf.probe\",\n        1,\n        None,\n        false,\n        runtime,\n        state,\n    )\n",
    "    run_ea189_dpf_capture(\n        adapter_id,\n        recording,\n        Ea189DpfCaptureConfig {\n            profile: \"ea189.dpf.probe\",\n            cycles: 1,\n            interval: None,\n            include_drive_context: false,\n        },\n        runtime,\n        state,\n    )\n",
)

replace_once(
    path,
    "    run_ea189_dpf_capture(\n        adapter_id,\n        Some(recording),\n        trace_profile.capture_profile(),\n        cycles,\n        Some(interval),\n        trace_profile.includes_drive_context(),\n        runtime,\n        state,\n    )\n",
    "    run_ea189_dpf_capture(\n        adapter_id,\n        Some(recording),\n        Ea189DpfCaptureConfig {\n            profile: trace_profile.capture_profile(),\n            cycles,\n            interval: Some(interval),\n            include_drive_context: trace_profile.includes_drive_context(),\n        },\n        runtime,\n        state,\n    )\n",
)

replace_once(
    path,
    "async fn run_ea189_dpf_capture(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    profile: &str,\n    cycles: u16,\n    interval: Option<Duration>,\n    include_drive_context: bool,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    let started = Instant::now();\n",
    "async fn run_ea189_dpf_capture(\n    adapter_id: &str,\n    recording: Option<&Path>,\n    config: Ea189DpfCaptureConfig<'_>,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n) -> Result<(), String> {\n    let Ea189DpfCaptureConfig {\n        profile,\n        cycles,\n        interval,\n        include_drive_context,\n    } = config;\n    let started = Instant::now();\n",
)

replace_once(
    path,
    "        run_diagnose_ea189_dpf_probe_inner(\n            adapter_id,\n            runtime,\n            state,\n            sender,\n            started,\n            cycles,\n            interval,\n            include_drive_context,\n        )\n",
    "        run_diagnose_ea189_dpf_probe_inner(\n            adapter_id, runtime, state, sender, started, config,\n        )\n",
)

replace_once(
    path,
    "async fn run_diagnose_ea189_dpf_probe_inner(\n    adapter_id: &str,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n    recorder: Option<&jsonl_capture::Sender>,\n    started: Instant,\n    cycles: u16,\n    interval: Option<Duration>,\n    include_drive_context: bool,\n) -> Result<(), String> {\n    let mapping = cached_engine_mapping(adapter_id).await?;\n",
    "async fn run_diagnose_ea189_dpf_probe_inner(\n    adapter_id: &str,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n    recorder: Option<&jsonl_capture::Sender>,\n    started: Instant,\n    config: Ea189DpfCaptureConfig<'_>,\n) -> Result<(), String> {\n    let Ea189DpfCaptureConfig {\n        cycles,\n        interval,\n        include_drive_context,\n        ..\n    } = config;\n    let mapping = cached_engine_mapping(adapter_id).await?;\n",
)

replace_once(
    path,
    "async fn capture_longitudinal_context_cycle(\n    prepared: &ble::PreparedDiagnosticSession,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n    recorder: Option<&jsonl_capture::Sender>,\n    started: Instant,\n    due_us: u64,\n    context: &[LongitudinalContextRead],\n    cancellation: Option<&tokio::task::JoinHandle<()>>,\n) -> Result<bool, String> {\n    apply_runtime_event(runtime, state, recorder, RuntimeEvent::ObservationStarted).await?;\n",
    "struct LongitudinalCycleContext<'a> {\n    recorder: Option<&'a jsonl_capture::Sender>,\n    started: Instant,\n    due_us: u64,\n    reads: &'a [LongitudinalContextRead],\n    cancellation: Option<&'a tokio::task::JoinHandle<()>>,\n}\n\nasync fn capture_longitudinal_context_cycle(\n    prepared: &ble::PreparedDiagnosticSession,\n    runtime: &RuntimeClient,\n    state: &mut RuntimeState,\n    cycle: LongitudinalCycleContext<'_>,\n) -> Result<bool, String> {\n    let LongitudinalCycleContext {\n        recorder,\n        started,\n        due_us,\n        reads,\n        cancellation,\n    } = cycle;\n    apply_runtime_event(runtime, state, recorder, RuntimeEvent::ObservationStarted).await?;\n",
)

replace_once(
    path,
    "    for context_read in context {\n",
    "    for context_read in reads {\n",
)

replace_once(
    path,
    "                cancelled = capture_longitudinal_context_cycle(\n                    &prepared,\n                    runtime,\n                    state,\n                    recorder,\n                    started,\n                    due_us,\n                    &context,\n                    cancellation.as_ref(),\n                )\n",
    "                cancelled = capture_longitudinal_context_cycle(\n                    &prepared,\n                    runtime,\n                    state,\n                    LongitudinalCycleContext {\n                        recorder,\n                        started,\n                        due_us,\n                        reads: &context,\n                        cancellation: cancellation.as_ref(),\n                    },\n                )\n",
)
