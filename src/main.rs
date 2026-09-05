use obdentic::vehicle_cache::{TargetMappingSnapshot, VehicleCache};
use obdentic::vehicle_knowledge::{
    EcuTargetMapping, FallbackPolicy, ReadRouting, VehicleKnowledge,
};
use obdentic::{
    audit::AuditState,
    ble,
    capability::HardwareCapability,
    capture,
    capture_events::{
        CaptureEvent, CaptureSubscription, DiagnosticJobStepStatus, DtcObservationFact,
        DtcTransportOutcome, SubscriptionFilterOutcome,
    },
    capture_replay::CaptureReplay,
    capture_report, capture_tui,
    diagnostic_job::{DiagnosticJob, DiagnosticScope, JobStatus, KnownTarget},
    dtc,
    ecu_identification::EcuIdentificationPlan,
    ecu_identification_discovery, hex, jsonl_capture,
    knowledge_db::KnowledgeCatalog,
    layout_observation::{self, LayoutFreshnessPolicy},
    prepare_read, record, replay,
    runtime_actor::RuntimeClient,
    runtime_reducer::RuntimeEvent,
    runtime_state::{
        Activity, RecordingState, RuntimeState, SourceState, TopologyState, TransportState,
        VehicleState,
    },
    safety::{DtcReadKind, Operation, OperationRequest, SafetyPolicy},
    scheduler::{apply_runtime_event, ObservationPlan, Subscription, TelemetryScheduler},
    subscription_policy::SubscriptionPolicy,
    supported_signals,
    telemetry::TelemetryState,
    topology::{
        AddressingContext, Confidence, EcuRole, Protocol, ProtocolContext, Provenance,
        RequestAddress, RequestTarget, ResponderIdentity, RoleAssignment,
    },
    tui, ReadRequest, Transaction,
};
use std::{
    env,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const USAGE: &str = "usage: obdentic signals | obdentic signals --adapter <CoreBluetooth UUID> --supported | obdentic scan | obdentic diagnose dtc.scan --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic diagnose ea189.dpf.probe --adapter <CoreBluetooth UUID> [--record capture.jsonl] | obdentic vehicle identify --adapter <CoreBluetooth UUID> | obdentic vehicle discover --adapter <CoreBluetooth UUID> | obdentic vehicle refresh --adapter <CoreBluetooth UUID> | obdentic vehicle scan --adapter <CoreBluetooth UUID> | obdentic vehicle show | obdentic read <signal> --adapter <CoreBluetooth UUID> [--record recording.tsv] | obdentic capture --adapter <CoreBluetooth UUID> --profile <profile> --record <capture.jsonl> | obdentic capture --adapter <CoreBluetooth UUID> --profile ea189-dpf --record <capture.jsonl> --cycles <1..=1440> --interval-seconds <>=30> | obdentic capture inspect <capture.jsonl> | obdentic capture capability <capture.jsonl> | obdentic capture dpf-report <capture.jsonl>... | obdentic demo | obdentic replay <recording.tsv> | obdentic layout save engine-overview <layout.tsv> | obdentic tui demo [--layout layout.tsv] | obdentic tui replay <recording.tsv> [--layout layout.tsv] | obdentic tui capture <capture.jsonl> [--layout layout.tsv] | obdentic tui live --adapter <CoreBluetooth UUID> [--layout layout.tsv] [--record capture.jsonl]";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Signals,
    SupportedSignals {
        adapter_id: String,
    },
    Scan,
    DiagnoseDtcScan {
        adapter_id: String,
        recording: Option<String>,
    },
    DiagnoseEa189DpfProbe {
        adapter_id: String,
        recording: Option<String>,
    },
    VehicleIdentify {
        adapter_id: String,
    },
    VehicleDiscover {
        adapter_id: String,
    },
    VehicleRefresh {
        adapter_id: String,
    },
    VehicleScan {
        adapter_id: String,
    },
    VehicleShow,
    Demo,
    Capture {
        adapter_id: String,
        profile: String,
        recording: String,
    },
    CaptureInspect(String),
    CaptureCapability(String),
    CaptureDpfReport(Vec<String>),
    CaptureEa189DpfTrace {
        adapter_id: String,
        recording: String,
        cycles: u16,
        interval: Duration,
    },
    Read {
        request: ReadRequest,
        adapter_id: String,
        recording: Option<String>,
    },
    Replay(String),
    LayoutSave(String),
    TuiDemo(Option<String>),
    TuiReplay {
        recording: String,
        layout: Option<String>,
    },
    TuiCapture {
        capture: String,
        layout: Option<String>,
    },
    TuiLive {
        adapter_id: String,
        layout: Option<String>,
        recording: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    let command = parse_command(&args)?;
    let (runtime, runtime_task) = obdentic::runtime_actor::start();
    let mut runtime_state = RuntimeState::default();
    let mut result = apply_runtime_event(
        &runtime,
        &mut runtime_state,
        None,
        RuntimeEvent::InitializationCompleted,
    )
    .await
    .map(|_| ());
    if result.is_ok() {
        result = match command {
            Command::Signals => {
                print!("{}", render_signals());
                Ok(())
            }
            Command::SupportedSignals { adapter_id } => {
                let support = ble::supported_signals(&adapter_id).await?;
                print!("{}", render_supported_signals(&support));
                Ok(())
            }
            Command::Scan => {
                for candidate in ble::scan().await? {
                    let rssi = candidate
                        .rssi
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    println!(
                        "adapter  {}  {}  RSSI {}",
                        candidate.id.escape_default(),
                        candidate.name.escape_default(),
                        rssi
                    );
                }
                Ok(())
            }
            Command::DiagnoseDtcScan {
                adapter_id,
                recording,
            } => {
                run_diagnose_dtc_scan(
                    &adapter_id,
                    recording.as_deref().map(Path::new),
                    &runtime,
                    &mut runtime_state,
                )
                .await
            }
            Command::DiagnoseEa189DpfProbe {
                adapter_id,
                recording,
            } => {
                run_diagnose_ea189_dpf_probe(
                    &adapter_id,
                    recording.as_deref().map(Path::new),
                    &runtime,
                    &mut runtime_state,
                )
                .await
            }
            Command::VehicleIdentify { adapter_id } => {
                run_vehicle_identify(&adapter_id, &runtime, &mut runtime_state).await
            }
            Command::VehicleDiscover { adapter_id } => {
                run_vehicle_discover(&adapter_id, false, &runtime, &mut runtime_state).await
            }
            Command::VehicleRefresh { adapter_id } => {
                run_vehicle_discover(&adapter_id, true, &runtime, &mut runtime_state).await
            }
            Command::VehicleScan { adapter_id } => {
                run_vehicle_scan(&adapter_id, &runtime, &mut runtime_state).await
            }
            Command::VehicleShow => run_vehicle_show(),
            Command::Capture {
                adapter_id,
                profile,
                recording,
            } => {
                run_capture(
                    &adapter_id,
                    &profile,
                    Path::new(&recording),
                    &runtime,
                    &mut runtime_state,
                )
                .await
            }
            Command::CaptureInspect(path) => {
                let capture = jsonl_capture::read(Path::new(&path))?;
                print!("{}", capture_report::render_inspection(&path, &capture));
                Ok(())
            }
            Command::CaptureCapability(path) => {
                let capture = jsonl_capture::read(Path::new(&path))?;
                print!("{}", capture_report::render_capability(&path, &capture));
                Ok(())
            }
            Command::CaptureDpfReport(paths) => run_capture_dpf_report(&paths),
            Command::CaptureEa189DpfTrace {
                adapter_id,
                recording,
                cycles,
                interval,
            } => {
                run_capture_ea189_dpf_trace(
                    &adapter_id,
                    Path::new(&recording),
                    cycles,
                    interval,
                    &runtime,
                    &mut runtime_state,
                )
                .await
            }
            Command::Demo => {
                show(&demo().await?);
                Ok(())
            }
            Command::Read {
                request,
                adapter_id,
                recording,
            } => {
                run_read(
                    request,
                    &adapter_id,
                    recording.as_deref().map(Path::new),
                    &runtime,
                    &mut runtime_state,
                )
                .await
            }
            Command::Replay(path) => {
                show(&replay(Path::new(&path)).await?);
                Ok(())
            }
            Command::LayoutSave(path) => {
                tui::save_layout(Path::new(&path), &tui::engine_overview())?;
                println!("saved layout  {path}");
                Ok(())
            }
            Command::TuiDemo(layout_path) => {
                let transactions = demo_samples()?;
                let layout = load_layout(layout_path.as_deref())?;
                tui::run(&layout, &telemetry(&transactions)?, &transactions)?;
                Ok(())
            }
            Command::TuiReplay { recording, layout } => {
                let transactions = [replay(Path::new(&recording)).await?];
                let layout = load_layout(layout.as_deref())?;
                tui::run(&layout, &telemetry(&transactions)?, &transactions)?;
                Ok(())
            }
            Command::TuiCapture { capture, layout } => {
                capture_tui::run_capture_tui(Path::new(&capture), layout.as_deref().map(Path::new))
            }
            Command::TuiLive {
                adapter_id,
                layout,
                recording,
            } => {
                let layout = load_layout(layout.as_deref())?;
                let advertised = ble::supported_signals(&adapter_id).await?;
                let (policy, subscriptions) = planned_layout_subscriptions(&layout, &advertised)?;
                let policy_state = Arc::new(Mutex::new(policy));
                let telemetry = Arc::new(Mutex::new(TelemetryState::new(600)?));
                let audit = Arc::new(Mutex::new(AuditState::new(600)?));
                if subscriptions.is_empty() {
                    tui::run_live(&layout, telemetry, audit, policy_state)
                } else {
                    let plans = routed_observation_plans(&adapter_id, subscriptions).await?;
                    let (recorder, writer) = match recording.as_deref() {
                        Some(path) => {
                            let (sender, writer) = jsonl_capture::start(Path::new(path))?;
                            (Some(sender), Some(writer))
                        }
                        None => (None, None),
                    };
                    if let Some(sender) = recorder.as_ref() {
                        apply_runtime_event(
                            &runtime,
                            &mut runtime_state,
                            Some(sender),
                            RuntimeEvent::recording(RecordingState::Active),
                        )
                        .await?;
                    }
                    let scheduler = match TelemetryScheduler::start_with_runtime(
                        &adapter_id,
                        plans,
                        telemetry.clone(),
                        audit.clone(),
                        recorder.clone(),
                        None,
                        None,
                        runtime.clone(),
                        Some(policy_state.clone()),
                    )
                    .await
                    {
                        Ok(scheduler) => scheduler,
                        Err(error) => {
                            if let Some(sender) = recorder.as_ref() {
                                apply_runtime_event(
                                    &runtime,
                                    &mut runtime_state,
                                    Some(sender),
                                    RuntimeEvent::recording(RecordingState::Inactive),
                                )
                                .await?;
                            }
                            let shutdown =
                                finish_runtime(&runtime, &mut runtime_state, recorder.as_ref())
                                    .await;
                            let recorded = finish_capture(recorder, writer).await;
                            shutdown?;
                            recorded?;
                            return Err(error);
                        }
                    };
                    let result = tui::run_live(&layout, telemetry, audit, policy_state);
                    let stopped = scheduler.stop().await;
                    let inactive = if let Some(sender) = recorder.as_ref() {
                        apply_runtime_event(
                            &runtime,
                            &mut runtime_state,
                            Some(sender),
                            RuntimeEvent::recording(RecordingState::Inactive),
                        )
                        .await
                    } else {
                        Ok(())
                    };
                    let shutdown =
                        finish_runtime(&runtime, &mut runtime_state, recorder.as_ref()).await;
                    let recorded = finish_capture(recorder, writer).await;
                    result?;
                    stopped?;
                    inactive?;
                    shutdown?;
                    recorded?;
                    Ok(())
                }
            }
        };
    }
    let shutdown = if runtime_state.phase() == obdentic::runtime_state::Phase::Stopped {
        Ok(())
    } else {
        finish_runtime(&runtime, &mut runtime_state, None).await
    };
    drop(runtime);
    let actor = runtime_task
        .await
        .map_err(|error| format!("runtime actor stopped unexpectedly: {error}"));
    match (result, shutdown, actor) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(()), Ok(())) => Err(error),
        (Ok(()), Err(error), Ok(())) => Err(error),
        (Ok(()), Ok(()), Err(error)) => Err(error),
        (Err(error), Err(shutdown), Ok(())) => {
            Err(format!("{error}; runtime shutdown failed: {shutdown}"))
        }
        (Err(error), Ok(()), Err(actor)) => Err(format!("{error}; {actor}")),
        (Ok(()), Err(shutdown), Err(actor)) => Err(format!("{shutdown}; {actor}")),
        (Err(error), Err(shutdown), Err(actor)) => Err(format!(
            "{error}; runtime shutdown failed: {shutdown}; {actor}"
        )),
    }
}

async fn run_vehicle_identify(
    adapter_id: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::source(SourceState::Live),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryStarted).await?;
    match ble::identify(adapter_id).await {
        Ok(identity) => {
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::vehicle(VehicleState::Identified),
            )
            .await?;
            apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryCompleted).await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await?;
            println!("VIN       {}", identity.vin());
            Ok(())
        }
        Err(error) => {
            finish_discovery_failure(&error, runtime, state).await?;
            Err(error)
        }
    }
}

async fn run_vehicle_discover(
    adapter_id: &str,
    refresh: bool,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::source(SourceState::Live),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryStarted).await?;

    match run_vehicle_discover_inner(adapter_id, refresh).await {
        Ok(()) => {
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::vehicle(VehicleState::Identified),
            )
            .await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::topology(TopologyState::Validated),
            )
            .await?;
            apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryCompleted).await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await
        }
        Err(error) => {
            finish_discovery_failure(&error, runtime, state).await?;
            Err(error)
        }
    }
}

async fn finish_discovery_failure(
    error: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryFailed).await?;
    if obdentic::scheduler::is_fatal_runtime_error(error) {
        apply_runtime_event(
            runtime,
            state,
            None,
            RuntimeEvent::transport(TransportState::Unhealthy),
        )
        .await?;
        apply_runtime_event(runtime, state, None, RuntimeEvent::FatalRuntimeError).await?;
    } else {
        apply_runtime_event(
            runtime,
            state,
            None,
            RuntimeEvent::transport(TransportState::Disconnected),
        )
        .await?;
    }
    Ok(())
}

async fn run_vehicle_scan(
    adapter_id: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::source(SourceState::Live),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryStarted).await?;

    match run_vehicle_scan_inner(adapter_id).await {
        Ok(()) => {
            apply_runtime_event(runtime, state, None, RuntimeEvent::DiscoveryCompleted).await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await
        }
        Err(error) => {
            finish_discovery_failure(&error, runtime, state).await?;
            Err(error)
        }
    }
}

async fn run_vehicle_scan_inner(adapter_id: &str) -> Result<(), String> {
    let session = ble::start_session(adapter_id).await?;
    let scan: Result<(), String> = async {
        let discovery =
            obdentic::functional_discovery::discover_functional_responders(&session).await?;
        print!("{}", render_vehicle_scan(&discovery));
        Ok(())
    }
    .await;
    let shutdown = session.shutdown().await;
    scan?;
    shutdown
}

fn render_vehicle_scan(
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> String {
    let capabilities = discovery.capabilities();
    let mut output = String::from("vehicle obd scan\nscope\tfunctional OBD-II Mode 01 only\n");
    output.push_str(&format!("responders\t{}\n", capabilities.len()));
    for capability in capabilities {
        let responder = capability.responder().value().unwrap_or("unknown");
        output.push_str(&format!("responder\t{}\n", responder.escape_default()));
        for signal in supported_signals() {
            let status = match capability.status(signal.metadata().semantic) {
                Ok(obdentic::functional_discovery::CapabilityStatus::Supported) => "advertised",
                Ok(obdentic::functional_discovery::CapabilityStatus::Unsupported) => {
                    "not-advertised"
                }
                Ok(obdentic::functional_discovery::CapabilityStatus::Unknown) | Err(_) => "unknown",
            };
            output.push_str(&format!(
                "signal\t{}\t{}\t{}\n",
                responder.escape_default(),
                signal.metadata().semantic,
                status
            ));
        }
    }
    output
}

async fn run_vehicle_discover_inner(adapter_id: &str, refresh: bool) -> Result<(), String> {
    let catalog = KnowledgeCatalog::load_pinned(Path::new(env!("CARGO_MANIFEST_DIR")))
        .map_err(|error| error.to_string())?;
    let identification_plan = EcuIdentificationPlan::from_catalog(&catalog)?;
    let identity = ble::identify(adapter_id).await?;
    let root = vehicle_cache_root()?;
    let store = obdentic::vehicle_cache::CacheStore::new(&root);
    let index = obdentic::vehicle_cache::VehicleIndex::new(&root);
    let existing_key = index.key_for(identity.vin())?;
    let existing = existing_key
        .as_deref()
        .map(|key| store.load(key))
        .transpose()?
        .flatten();

    if !refresh {
        if let Some(cache) = existing.as_ref() {
            let validation = ble::validate_functional_support(adapter_id)
                .await
                .and_then(snapshot_from_support_validation);
            match obdentic::cache_validation::validate_snapshot(cache, validation) {
                obdentic::cache_validation::CacheValidation::Validated => {
                    if cache.snapshot().target_mappings().is_empty() {
                        println!("cache\tmissing-engine-target; running full discovery");
                    } else if cache.snapshot().ecu_identification().is_empty() {
                        println!("cache\tmissing-ecu-identification; running full discovery");
                    } else {
                        print_cached_vehicle_discovery(cache);
                        return Ok(());
                    }
                }
                obdentic::cache_validation::CacheValidation::StaleMissingExpected(_) => {
                    println!("cache\tstale-missing; running full discovery");
                }
                obdentic::cache_validation::CacheValidation::StaleUnexpected(_) => {
                    println!("cache\tstale-unexpected; running full discovery");
                }
                obdentic::cache_validation::CacheValidation::TransportError(error) => {
                    println!("cache\tvalidation-error ({error}); running full discovery");
                }
            }
        }
    }

    let session = ble::start_session(adapter_id).await?;
    let discovery = obdentic::functional_discovery::discover_functional_responders(&session).await;
    let target_mappings = match &discovery {
        Ok(discovery) => match validate_engine_target(&session, discovery).await {
            Ok(Some(engine)) => {
                let secondary = match validate_secondary_target(&session, discovery).await {
                    Ok(mapping) => mapping,
                    Err(error) => {
                        eprintln!("secondary target unavailable; continuing discovery: {error}");
                        None
                    }
                };
                Ok(merge_target_mappings(engine, secondary))
            }
            Ok(None) => Ok(Vec::new()),
            Err(error) => Err(error),
        },
        Err(_) => Ok(Vec::new()),
    };
    let ecu_identification = match &target_mappings {
        Ok(mappings) if !mappings.is_empty() => {
            ecu_identification_discovery::discover_known_ecus(
                &session,
                &identification_plan,
                mappings,
            )
            .await
        }
        Ok(_) | Err(_) => Ok(Vec::new()),
    };
    let shutdown = session.shutdown().await;
    let discovery = discovery?;
    let target_mappings = target_mappings?;
    let ecu_identification = ecu_identification?;
    shutdown?;

    let (local_key, _) = index.key_for_or_create(identity.vin())?;
    let now = wallclock_ms()?;
    let first_seen = existing
        .as_ref()
        .map(VehicleCache::first_seen_ms)
        .unwrap_or(now);
    let base_snapshot = obdentic::vehicle_cache::VehicleCacheSnapshot::from_discovery(
        &discovery.topology(),
        &discovery.capabilities(),
    );
    let snapshot = obdentic::vehicle_cache::VehicleCacheSnapshot::with_ecu_identification(
        base_snapshot.topology().to_vec(),
        base_snapshot.ecu_capabilities().to_vec(),
        target_mappings,
        ecu_identification,
    );
    let engine_target_validated = snapshot.target_mappings().iter().any(|mapping| {
        mapping
            .role()
            .is_some_and(|role| role.role() == &EcuRole::Engine)
    });
    let mut history = existing
        .as_ref()
        .map(|cache| cache.history().to_vec())
        .unwrap_or_default();
    history.extend(discovery_evidence(&discovery));
    store.save(&VehicleCache::with_snapshot(
        local_key.clone(),
        first_seen,
        now,
        snapshot,
        history,
    ))?;

    println!("vehicle discovery");
    println!("local_id\t{local_key}");
    println!(
        "cache\t{}",
        if refresh {
            "refreshed"
        } else {
            "miss-or-stale"
        }
    );
    println!("responders\t{}", discovery.responders().len());
    for responder in discovery.responders() {
        println!(
            "responder\t{}",
            responder.value().unwrap_or("unknown").escape_default()
        );
    }
    println!("evidence\t{}", discovery.observations().len());
    if !engine_target_validated {
        println!("target\tengine.rpm\tunavailable");
    } else {
        println!("target\tengine.rpm\t7E0 -> 7E8\tvalidated");
    }
    Ok(())
}

async fn validate_engine_target(
    session: &ble::SessionClient,
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> Result<Option<TargetMappingSnapshot>, String> {
    if !engine_responder_observed(discovery) {
        return Ok(None);
    }

    let transaction = session.read_targeted(engine_target_request()?).await?;
    validate_engine_target_transaction(&transaction)?;
    Ok(Some(confirmed_engine_target()?))
}

async fn validate_secondary_target(
    session: &ble::SessionClient,
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> Result<Option<TargetMappingSnapshot>, String> {
    if !secondary_target_allowed(discovery) {
        return Ok(None);
    }

    let transaction = session.read_targeted(secondary_target_request()?).await?;
    validate_vehicle_speed_target_transaction(&transaction)?;
    Ok(Some(confirmed_secondary_target()?))
}

fn secondary_target_allowed(
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> bool {
    discovery.capabilities().iter().any(|capability| {
        capability
            .responder()
            .value()
            .is_some_and(|value| value.eq_ignore_ascii_case("7E9"))
            && matches!(
                capability.status("vehicle.speed"),
                Ok(obdentic::functional_discovery::CapabilityStatus::Supported)
            )
    })
}

fn validate_vehicle_speed_target_transaction(transaction: &Transaction) -> Result<(), String> {
    if transaction.semantic() != "vehicle.speed"
        || transaction.request() != [0x01, 0x0D]
        || transaction.response().len() != 3
        || transaction.response().first() != Some(&0x41)
        || transaction.response().get(1) != Some(&0x0D)
    {
        return Err("targeted secondary validation returned an invalid 010D response".into());
    }
    Ok(())
}

fn secondary_target_request() -> Result<ble::TargetedReadRequest, String> {
    let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
    ble::TargetedReadRequest::new(
        prepare_read("vehicle.speed")?,
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E1")),
        ble::ResponderIdentity::ElmHeader("7E9".into()),
    )
}

fn confirmed_secondary_target() -> Result<TargetMappingSnapshot, String> {
    let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
    let provenance = Provenance::new(
        "targeted vehicle.speed Mode 01 validation",
        Confidence::Verified,
    )
    .map_err(|error| error.to_string())?;
    Ok(TargetMappingSnapshot::new(
        None,
        Some(ResponderIdentity::address(context.clone(), "7E9")),
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E1")),
        provenance,
    ))
}

fn merge_target_mappings(
    engine: TargetMappingSnapshot,
    secondary: Option<TargetMappingSnapshot>,
) -> Vec<TargetMappingSnapshot> {
    let mut mappings = vec![engine];
    if let Some(secondary) = secondary {
        mappings.push(secondary);
    }
    mappings.sort();
    mappings
}

fn engine_responder_observed(
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> bool {
    discovery.responders().iter().any(|responder| {
        responder
            .value()
            .is_some_and(|value| value.eq_ignore_ascii_case("7E8"))
    })
}

fn validate_engine_target_transaction(transaction: &Transaction) -> Result<(), String> {
    if transaction.semantic() != "engine.rpm"
        || transaction.request() != [0x01, 0x0C]
        || transaction.response().len() != 4
        || transaction.response().first() != Some(&0x41)
        || transaction.response().get(1) != Some(&0x0C)
    {
        return Err("targeted engine validation returned an invalid 010C response".into());
    }
    Ok(())
}

fn engine_target_request() -> Result<ble::TargetedReadRequest, String> {
    let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
    ble::TargetedReadRequest::new(
        prepare_read("engine.rpm")?,
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E0")),
        ble::ResponderIdentity::ElmHeader("7E8".into()),
    )
}

fn confirmed_engine_target() -> Result<TargetMappingSnapshot, String> {
    let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
    let provenance = Provenance::new(
        "targeted engine.rpm Mode 01 validation",
        Confidence::Verified,
    )
    .map_err(|error| error.to_string())?;
    Ok(TargetMappingSnapshot::new(
        Some(RoleAssignment::new(EcuRole::Engine, provenance.clone())),
        Some(ResponderIdentity::address(context.clone(), "7E8")),
        RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E0")),
        provenance,
    ))
}

async fn run_read(
    request: ReadRequest,
    adapter_id: &str,
    recording: Option<&Path>,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::source(SourceState::Live),
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        None,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    apply_runtime_event(runtime, state, None, RuntimeEvent::ReadStarted).await?;

    let (mappings, cache_valid) = cached_routing_mappings(adapter_id).await;
    if cache_valid {
        apply_runtime_event(
            runtime,
            state,
            None,
            RuntimeEvent::vehicle(VehicleState::Identified),
        )
        .await?;
        apply_runtime_event(
            runtime,
            state,
            None,
            RuntimeEvent::topology(TopologyState::Validated),
        )
        .await?;
    }
    let outcome = match route_request(request.metadata().semantic, &mappings, cache_valid)? {
        ReadRouting::Functional(request) => ble::read(adapter_id, request).await,
        ReadRouting::Targeted(request) => ble::read_targeted(adapter_id, request).await,
    };
    match outcome {
        Ok(transaction) => {
            apply_runtime_event(runtime, state, None, RuntimeEvent::ReadCompleted).await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await?;
            if let Some(path) = recording {
                apply_runtime_event(
                    runtime,
                    state,
                    None,
                    RuntimeEvent::recording(RecordingState::Active),
                )
                .await?;
                let recorded = record(path, &transaction);
                let inactive = apply_runtime_event(
                    runtime,
                    state,
                    None,
                    RuntimeEvent::recording(RecordingState::Inactive),
                )
                .await;
                match (recorded, inactive) {
                    (Ok(()), Ok(())) => {}
                    (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
                    (Err(error), Err(inactive)) => {
                        return Err(format!("{error}; recording shutdown failed: {inactive}"))
                    }
                }
                println!("recorded  {}", path.display());
            }
            show(&transaction);
            Ok(())
        }
        Err(error) if obdentic::scheduler::is_fatal_runtime_error(&error) => {
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Unhealthy),
            )
            .await?;
            apply_runtime_event(runtime, state, None, RuntimeEvent::FatalRuntimeError).await?;
            Err(error)
        }
        Err(error) => {
            apply_runtime_event(runtime, state, None, RuntimeEvent::ReadFailedRecoverable).await?;
            apply_runtime_event(
                runtime,
                state,
                None,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await?;
            Err(error)
        }
    }
}

async fn run_diagnose_dtc_scan(
    adapter_id: &str,
    recording: Option<&Path>,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    let started = Instant::now();
    let recorder = recording
        .map(jsonl_capture::JsonlRecorder::start)
        .transpose()?;
    let sender = recorder.as_ref().map(jsonl_capture::JsonlRecorder::sender);
    let result = async {
        if sender.is_some() {
            emit_capture_event(
                sender,
                CaptureEvent::capture_started(Some(wallclock_ms()?), Some("dtc.scan".into())),
            )
            .await?;
            apply_runtime_event(
                runtime,
                state,
                sender,
                RuntimeEvent::recording(RecordingState::Active),
            )
            .await?;
        }

        run_diagnose_dtc_scan_inner(adapter_id, runtime, state, sender).await
    }
    .await;
    let inactive = if sender.is_some() {
        apply_runtime_event(
            runtime,
            state,
            sender,
            RuntimeEvent::recording(RecordingState::Inactive),
        )
        .await
    } else {
        Ok(())
    };
    let shutdown = finish_runtime(runtime, state, sender).await;
    let stopped = emit_capture_event(
        sender,
        CaptureEvent::SessionStopped {
            offset_us: started
                .elapsed()
                .as_micros()
                .try_into()
                .map_err(|_| "DTC capture duration exceeds supported range")?,
        },
    )
    .await;
    let result = result.and(inactive).and(shutdown).and(stopped);
    let recorded = match recorder {
        Some(recorder) => recorder.close().await,
        None => Ok(()),
    };
    result.and(recorded)
}

async fn run_diagnose_dtc_scan_inner(
    adapter_id: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&jsonl_capture::Sender>,
) -> Result<(), String> {
    let job = DiagnosticJob::dtc_scan(DiagnosticScope::VehicleWide);
    let plan = job.plan();
    match SafetyPolicy::read_only().authorize_activity(
        Activity::Diagnose,
        OperationRequest::read_dtcs(DtcReadKind::Stored),
    ) {
        Ok(Operation::ReadDtcs(DtcReadKind::Stored)) => {}
        Ok(_) => return Err("read-only safety policy returned the wrong DTC operation".into()),
        Err(error) => return Err(error.to_string()),
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
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    let prepared = match ble::prepare_diagnostic_session(adapter_id).await {
        Ok(prepared) => prepared,
        Err(error) if obdentic::scheduler::is_fatal_runtime_error(&error) => {
            apply_runtime_event(
                runtime,
                state,
                recorder,
                RuntimeEvent::transport(TransportState::Unhealthy),
            )
            .await?;
            apply_runtime_event(runtime, state, recorder, RuntimeEvent::FatalRuntimeError).await?;
            return Err(error);
        }
        Err(error) => {
            apply_runtime_event(
                runtime,
                state,
                recorder,
                RuntimeEvent::transport(TransportState::Disconnected),
            )
            .await?;
            return Err(error);
        }
    };
    record_protocol_negotiation(recorder, prepared.negotiation()).await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::transport(TransportState::Connected),
    )
    .await?;
    apply_runtime_event(runtime, state, recorder, RuntimeEvent::DiagnosticJobStarted).await?;
    emit_capture_event(recorder, CaptureEvent::diagnostic_job_started(&job)).await?;

    match prepared.read_stored_dtcs().await {
        Ok(responses) => {
            record_dtc_transport_evidence(recorder, &job, &responses).await?;
            if !responses.is_empty() {
                emit_capture_event(
                    recorder,
                    CaptureEvent::responses_observed(
                        job.id().to_string(),
                        [0x03].into(),
                        responses.capture_evidence(),
                        None,
                        None,
                    )?,
                )
                .await?;
            }
            let evidence = match dtc_evidence(&responses) {
                Ok(evidence) => evidence,
                Err(error) => {
                    emit_capture_event(
                        recorder,
                        CaptureEvent::diagnostic_job_step(
                            job.id().to_string(),
                            0,
                            0x03,
                            None,
                            DiagnosticJobStepStatus::Recoverable,
                            Some("malformed_evidence".into()),
                        )?,
                    )
                    .await?;
                    emit_capture_event(
                        recorder,
                        CaptureEvent::DiagnosticJobCompleted {
                            job_id: job.id().to_string(),
                            status: JobStatus::CompletedWithErrors,
                        },
                    )
                    .await?;
                    finish_diagnostic(runtime, state, recorder).await?;
                    println!(
                        "{}",
                        render_dtc_scan_error(&job, &plan, &error, "completed_with_errors")
                    );
                    return Err(error);
                }
            };
            let decoded = dtc::decode_mode03(&evidence);
            record_dtc_facts(recorder, &job, &decoded).await?;
            let rendered = render_dtc_scan(&job, &plan, &decoded, responses.errors());
            let recoverable = !responses.errors().is_empty()
                || decoded.observations().iter().any(|observation| {
                    matches!(observation.response(), dtc::DtcResponse::Error(_))
                });
            emit_capture_event(
                recorder,
                CaptureEvent::diagnostic_job_step(
                    job.id().to_string(),
                    0,
                    0x03,
                    None,
                    if recoverable {
                        DiagnosticJobStepStatus::Recoverable
                    } else {
                        DiagnosticJobStepStatus::Success
                    },
                    recoverable.then_some("malformed_evidence".into()),
                )?,
            )
            .await?;
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
            finish_diagnostic(runtime, state, recorder).await?;
            print!("{rendered}");
            Ok(())
        }
        Err(error) if obdentic::scheduler::is_fatal_runtime_error(&error) => {
            apply_runtime_event(
                runtime,
                state,
                recorder,
                RuntimeEvent::transport(TransportState::Unhealthy),
            )
            .await?;
            apply_runtime_event(runtime, state, recorder, RuntimeEvent::FatalRuntimeError).await?;
            emit_capture_event(
                recorder,
                CaptureEvent::diagnostic_job_step(
                    job.id().to_string(),
                    0,
                    0x03,
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
            println!("{}", render_dtc_scan_error(&job, &plan, &error, "failed"));
            Err(error)
        }
        Err(error) => {
            emit_capture_event(
                recorder,
                CaptureEvent::diagnostic_job_step(
                    job.id().to_string(),
                    0,
                    0x03,
                    None,
                    DiagnosticJobStepStatus::Recoverable,
                    Some("read_failed".into()),
                )?,
            )
            .await?;
            emit_capture_event(
                recorder,
                CaptureEvent::DiagnosticJobCompleted {
                    job_id: job.id().to_string(),
                    status: JobStatus::CompletedWithErrors,
                },
            )
            .await?;
            finish_diagnostic(runtime, state, recorder).await?;
            println!(
                "{}",
                render_dtc_scan_error(&job, &plan, &error, "completed_with_errors")
            );
            Err(error)
        }
    }
}

async fn run_diagnose_ea189_dpf_probe(
    adapter_id: &str,
    recording: Option<&Path>,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    run_ea189_dpf_capture(
        adapter_id,
        recording,
        "ea189.dpf.probe",
        1,
        None,
        runtime,
        state,
    )
    .await
}

async fn run_capture_ea189_dpf_trace(
    adapter_id: &str,
    recording: &Path,
    cycles: u16,
    interval: Duration,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    println!("capture profile  ea189-dpf");
    println!("capture cycles   {cycles}");
    println!("capture interval {} s after each cycle", interval.as_secs());
    println!("capture record   {}", recording.display());
    println!("capture running  press Ctrl-C to stop after the active read");
    run_ea189_dpf_capture(
        adapter_id,
        Some(recording),
        "ea189.dpf.trace",
        cycles,
        Some(interval),
        runtime,
        state,
    )
    .await
}

async fn run_ea189_dpf_capture(
    adapter_id: &str,
    recording: Option<&Path>,
    profile: &str,
    cycles: u16,
    interval: Option<Duration>,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
) -> Result<(), String> {
    let started = Instant::now();
    let recorder = recording
        .map(jsonl_capture::JsonlRecorder::start)
        .transpose()?;
    let sender = recorder.as_ref().map(jsonl_capture::JsonlRecorder::sender);
    let result = async {
        if sender.is_some() {
            emit_capture_event(
                sender,
                CaptureEvent::capture_started(Some(wallclock_ms()?), Some(profile.into())),
            )
            .await?;
            apply_runtime_event(
                runtime,
                state,
                sender,
                RuntimeEvent::recording(RecordingState::Active),
            )
            .await?;
        }
        run_diagnose_ea189_dpf_probe_inner(
            adapter_id, runtime, state, sender, started, cycles, interval,
        )
        .await
    }
    .await;
    let inactive = if sender.is_some() {
        apply_runtime_event(
            runtime,
            state,
            sender,
            RuntimeEvent::recording(RecordingState::Inactive),
        )
        .await
    } else {
        Ok(())
    };
    let shutdown = finish_runtime(runtime, state, sender).await;
    let stopped = emit_capture_event(
        sender,
        CaptureEvent::SessionStopped {
            offset_us: started
                .elapsed()
                .as_micros()
                .try_into()
                .map_err(|_| "EA189 DPF capture duration exceeds supported range")?,
        },
    )
    .await;
    let recorded = match recorder {
        Some(recorder) => recorder.close().await,
        None => Ok(()),
    };
    result
        .and(inactive)
        .and(shutdown)
        .and(stopped)
        .and(recorded)
}

async fn run_diagnose_ea189_dpf_probe_inner(
    adapter_id: &str,
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&jsonl_capture::Sender>,
    started: Instant,
    cycles: u16,
    interval: Option<Duration>,
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
    apply_runtime_event(runtime, state, recorder, RuntimeEvent::DiagnosticJobStarted).await?;
    emit_capture_event(recorder, CaptureEvent::diagnostic_job_started(&job)).await?;

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
                    return Err("read-only safety policy returned the wrong EA189 operation".into())
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
                Err(error) => {
                    recoverable = true;
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
        if cancelled {
            break;
        }
        completed_cycles += 1;
    }
    let shutdown = prepared.shutdown().await;
    if let Some(cancellation) = cancellation {
        cancellation.abort();
    }
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
    finish_diagnostic(runtime, state, recorder).await?;
    shutdown
}

fn capture_offset_us(started: Instant) -> Result<u64, String> {
    started
        .elapsed()
        .as_micros()
        .try_into()
        .map_err(|_| "EA189 DPF capture duration exceeds supported range".into())
}

fn job_target(job: &DiagnosticJob) -> Result<KnownTarget, String> {
    match job.scope() {
        DiagnosticScope::KnownEcu { target, .. } => Ok(target.clone()),
        _ => Err("EA189 DPF job lost its known engine target".into()),
    }
}

async fn cached_engine_mapping(adapter_id: &str) -> Result<EcuTargetMapping, String> {
    let (mappings, cache_valid) = cached_routing_mappings(adapter_id).await;
    if !cache_valid {
        return Err("EA189 DPF probe requires a validated engine mapping; run `obdentic vehicle discover --adapter <UUID>` first".into());
    }
    mappings
        .into_iter()
        .find(|mapping| mapping.role().role() == &EcuRole::Engine)
        .ok_or_else(|| "EA189 DPF probe requires a validated engine mapping; run `obdentic vehicle discover --adapter <UUID>` first".into())
}

async fn finish_diagnostic(
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&jsonl_capture::Sender>,
) -> Result<(), String> {
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::DiagnosticJobCompleted,
    )
    .await?;
    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::transport(TransportState::Disconnected),
    )
    .await
}

fn dtc_evidence(
    responses: &ble::DiagnosticResponses,
) -> Result<Vec<dtc::ResponseEvidence>, String> {
    responses
        .as_slice()
        .iter()
        .map(|response| {
            let responder = response
                .responder
                .as_ref()
                .map(|responder| dtc::ResponderIdentity::new(responder.as_str()))
                .transpose()
                .map_err(|error| error.to_string())?;
            Ok(dtc::ResponseEvidence::new(
                responder,
                response.payload.clone(),
            ))
        })
        .collect()
}

async fn record_protocol_negotiation(
    recorder: Option<&jsonl_capture::Sender>,
    negotiation: &ble::ProtocolNegotiation,
) -> Result<(), String> {
    for observation in negotiation.observations() {
        emit_capture_event(
            recorder,
            CaptureEvent::protocol_negotiation_observed(
                observation.request.into(),
                observation
                    .responder
                    .as_ref()
                    .map(|responder| responder.as_str().into()),
                observation.response.into(),
            )?,
        )
        .await?;
    }
    Ok(())
}

async fn record_dtc_transport_evidence(
    recorder: Option<&jsonl_capture::Sender>,
    job: &DiagnosticJob,
    responses: &ble::DiagnosticResponses,
) -> Result<(), String> {
    for response in responses.as_slice() {
        emit_capture_event(
            recorder,
            CaptureEvent::dtc_transport_observed(
                job.id().to_string(),
                0,
                response
                    .responder
                    .as_ref()
                    .map(|responder| responder.as_str().into()),
                DtcTransportOutcome::Response,
            )?,
        )
        .await?;
    }
    for error in responses.errors() {
        emit_capture_event(
            recorder,
            CaptureEvent::dtc_transport_observed(
                job.id().to_string(),
                0,
                error
                    .responder
                    .as_ref()
                    .map(|responder| responder.as_str().into()),
                DtcTransportOutcome::Error(error.error.clone()),
            )?,
        )
        .await?;
    }
    Ok(())
}

async fn record_dtc_facts(
    recorder: Option<&jsonl_capture::Sender>,
    job: &DiagnosticJob,
    result: &dtc::DtcScanResult,
) -> Result<(), String> {
    for observation in result.observations() {
        let responder = observation
            .source()
            .responder()
            .map(|responder| responder.as_str().into());
        let facts = match observation.response() {
            dtc::DtcResponse::NoDtcs => vec![DtcObservationFact::NoDtcs],
            dtc::DtcResponse::Stored(codes) => codes
                .iter()
                .map(|code| DtcObservationFact::DtcCode(code.to_string()))
                .collect(),
            dtc::DtcResponse::Error(error) => {
                vec![DtcObservationFact::DecodeError(error.to_string())]
            }
        };
        for fact in facts {
            emit_capture_event(
                recorder,
                CaptureEvent::dtc_observation(
                    job.id().to_string(),
                    0,
                    responder.clone(),
                    fact,
                    "obdii.mode03",
                    "SAE J1979 Mode 03",
                )?,
            )
            .await?;
        }
    }
    Ok(())
}

fn render_dtc_scan(
    job: &DiagnosticJob,
    plan: &obdentic::diagnostic_job::JobPlan,
    result: &dtc::DtcScanResult,
    errors: &[ble::DiagnosticResponseError],
) -> String {
    let has_decode_errors = result
        .observations()
        .iter()
        .any(|observation| matches!(observation.response(), dtc::DtcResponse::Error(_)));
    let status = if errors.is_empty() && !has_decode_errors {
        "completed"
    } else {
        "completed_with_errors"
    };
    let mut output = render_dtc_header(job, plan, status);
    for observation in result.observations() {
        let responder = observation
            .source()
            .responder()
            .map_or("unknown", dtc::ResponderIdentity::as_str);
        match observation.response() {
            dtc::DtcResponse::NoDtcs => output.push_str(&format!(
                "responder\t{}\tno_dtcs\n",
                escape_field(responder)
            )),
            dtc::DtcResponse::Stored(dtcs) => {
                for dtc in dtcs {
                    output.push_str(&format!(
                        "responder\t{}\tdtc\t{}\n",
                        escape_field(responder),
                        dtc
                    ));
                }
            }
            dtc::DtcResponse::Error(error) => output.push_str(&format!(
                "responder\t{}\terror\t{}\n",
                escape_field(responder),
                escape_field(&error.to_string())
            )),
        }
    }
    for error in errors {
        let responder = error
            .responder
            .as_ref()
            .map_or("unknown", ble::ResponderIdentity::as_str);
        output.push_str(&format!(
            "responder\t{}\terror\t{}\n",
            escape_field(responder),
            escape_field(&error.error)
        ));
    }
    output
}

fn render_dtc_scan_error(
    job: &DiagnosticJob,
    plan: &obdentic::diagnostic_job::JobPlan,
    error: &str,
    status: &str,
) -> String {
    format!(
        "{}error\t{}\n",
        render_dtc_header(job, plan, status),
        escape_field(error)
    )
}

fn render_dtc_header(
    job: &DiagnosticJob,
    plan: &obdentic::diagnostic_job::JobPlan,
    status: &str,
) -> String {
    let mut output = format!("job\t{}\nstatus\t{}\n", job.id(), status);
    for step in plan.steps() {
        output.push_str(&format!("step\t{}\tread_dtc\tvehicle\n", step.sequence()));
    }
    output
}

async fn finish_runtime(
    runtime: &RuntimeClient,
    state: &mut RuntimeState,
    recorder: Option<&tokio::sync::mpsc::Sender<CaptureEvent>>,
) -> Result<(), String> {
    if state.activity() == obdentic::runtime_state::Activity::Observe {
        apply_runtime_event(runtime, state, recorder, RuntimeEvent::ObservationStopped).await?;
    }
    if state.phase() != obdentic::runtime_state::Phase::Stopped {
        apply_runtime_event(runtime, state, recorder, RuntimeEvent::ShutdownRequested).await?;
        apply_runtime_event(runtime, state, recorder, RuntimeEvent::ShutdownCompleted).await?;
    }
    Ok(())
}

fn snapshot_from_support_validation(
    pages: Vec<ble::SupportDiscovery>,
) -> Result<obdentic::vehicle_cache::VehicleCacheSnapshot, String> {
    let discovery =
        obdentic::functional_discovery::FunctionalResponderDiscovery::from_support_discovery(
            &pages,
        )
        .map_err(|error| error.to_string())?;
    Ok(
        obdentic::vehicle_cache::VehicleCacheSnapshot::from_discovery(
            &discovery.topology(),
            &discovery.capabilities(),
        ),
    )
}

fn print_cached_vehicle_discovery(cache: &VehicleCache) {
    let signature = cache.snapshot().validation_signature();
    println!("vehicle discovery");
    println!("local_id\t{}", cache.local_key());
    println!("cache\tvalidated-reused");
    println!("responders\t{}", signature.topology().len());
    for observation in signature.topology() {
        println!(
            "responder\t{}",
            observation
                .responder()
                .value()
                .unwrap_or("unknown")
                .escape_default()
        );
    }
    println!("evidence\t{}", signature.topology().len());
}

fn run_vehicle_show() -> Result<(), String> {
    let caches = obdentic::vehicle_cache::CacheStore::new(vehicle_cache_root()?).load_all()?;
    print!("{}", render_vehicle_summaries(&caches));
    Ok(())
}

fn vehicle_cache_root() -> Result<std::path::PathBuf, String> {
    let home = env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("obdentic")
        .join("vehicles"))
}

fn wallclock_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "wall clock timestamp exceeds supported range".into())
}

fn discovery_evidence(
    discovery: &obdentic::functional_discovery::FunctionalResponderDiscovery,
) -> Vec<String> {
    let mut evidence = discovery
        .observations()
        .iter()
        .map(|observation| {
            format!(
                "functional request={} responder={} payload={} provenance={} confidence={:?}",
                hex(&observation.request()),
                observation.responder().value().unwrap_or("unknown"),
                hex(observation.payload()),
                observation.provenance().source(),
                observation.provenance().confidence(),
            )
        })
        .collect::<Vec<_>>();
    for capability in discovery.capabilities() {
        let responder = capability.responder().value().unwrap_or("unknown");
        let statuses = supported_signals()
            .iter()
            .filter_map(|signal| {
                capability
                    .status(signal.metadata().semantic)
                    .ok()
                    .map(|status| format!("{}={status:?}", signal.metadata().semantic))
            })
            .collect::<Vec<_>>();
        evidence.push(format!(
            "capabilities responder={} pages={} {}",
            responder,
            capability.mode01_pages().len(),
            statuses.join(",")
        ));
    }
    evidence
}

fn render_vehicle_summaries(caches: &[obdentic::vehicle_cache::VehicleCache]) -> String {
    if caches.is_empty() {
        return "no cached vehicles\n".into();
    }
    let mut output = String::new();
    for cache in caches {
        output.push_str("vehicle\n");
        output.push_str(&format!("local_id\t{}\n", cache.local_key()));
        output.push_str(&format!("first_seen_ms\t{}\n", cache.first_seen_ms()));
        output.push_str(&format!("last_seen_ms\t{}\n", cache.last_seen_ms()));
        output.push_str(&format!(
            "topology\t{}\n",
            cache.snapshot().topology().len()
        ));
        output.push_str(&format!(
            "capabilities\t{}\n",
            cache.snapshot().capabilities().len()
        ));
        output.push_str(&format!("history\t{}\n", cache.history().len()));
        for evidence in cache.history() {
            output.push_str(&format!("  {}\n", evidence.escape_default()));
        }
    }
    output
}

async fn run_capture(
    adapter_id: &str,
    profile_name: &str,
    path: &Path,
    runtime: &RuntimeClient,
    runtime_state: &mut RuntimeState,
) -> Result<(), String> {
    let profile = capture::profile(profile_name)?;
    let capability = obdentic::capability::HardwareCapability::conservative_default();
    profile.admit(capability)?;
    let configured = profile.subscriptions()?;
    let advertised = ble::supported_signals(adapter_id).await?;
    let total = configured.len();
    let (subscriptions, capture_subscriptions) =
        filter_capture_subscriptions(configured, &advertised);
    if subscriptions.is_empty() {
        return Err(format!(
            "capture profile {profile_name} has no signals supported by the adapter"
        ));
    }
    let plans = routed_observation_plans(adapter_id, subscriptions.clone()).await?;

    let (sender, writer) = jsonl_capture::start(path)?;
    apply_runtime_event(
        runtime,
        runtime_state,
        Some(&sender),
        RuntimeEvent::recording(RecordingState::Active),
    )
    .await?;
    println!("capture profile  {profile_name}");
    println!(
        "capture signals  {}/{} supported",
        subscriptions.len(),
        total
    );
    println!(
        "capture budget   {} reads/s conservative",
        capability.request_budget_per_second()
    );
    for subscription in &capture_subscriptions {
        println!(
            "capture signal   {}  {} ms  {}",
            subscription.semantic(),
            subscription.requested_interval_us() / 1_000,
            match subscription.filter() {
                SubscriptionFilterOutcome::Scheduled => "scheduled",
                SubscriptionFilterOutcome::Unsupported => "unsupported (omitted)",
                SubscriptionFilterOutcome::Unknown => "unknown (omitted)",
            },
        );
    }
    println!("capture record   {}", path.display());
    println!("capture connecting...  wait for session initialization");

    let scheduler = match TelemetryScheduler::start_with_runtime(
        adapter_id,
        plans,
        Arc::new(Mutex::new(TelemetryState::new(600)?)),
        Arc::new(Mutex::new(AuditState::new(600)?)),
        Some(sender.clone()),
        Some(profile.name().into()),
        Some(capture_subscriptions),
        runtime.clone(),
        None,
    )
    .await
    {
        Ok(scheduler) => scheduler,
        Err(error) => {
            record_capture_start_failure(&sender, profile.name(), &error).await?;
            let inactive = apply_runtime_event(
                runtime,
                runtime_state,
                Some(&sender),
                RuntimeEvent::recording(RecordingState::Inactive),
            )
            .await;
            let shutdown = finish_runtime(runtime, runtime_state, Some(&sender)).await;
            let recorded = finish_capture(Some(sender), Some(writer)).await;
            inactive?;
            shutdown?;
            recorded?;
            return Err(error);
        }
    };
    println!("capture running  press Ctrl-C to stop");

    let wait_result = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            println!("capture stopping...");
            signal.map_err(|error| format!("Ctrl-C listener failed: {error}"))
        },
        _ = wait_for_scheduler(&scheduler) => Err("capture session stopped unexpectedly".into()),
    };
    let stopped = scheduler.stop().await;
    let inactive = apply_runtime_event(
        runtime,
        runtime_state,
        Some(&sender),
        RuntimeEvent::recording(RecordingState::Inactive),
    )
    .await;
    let shutdown = finish_runtime(runtime, runtime_state, Some(&sender)).await;
    let recorded = finish_capture(Some(sender), Some(writer)).await;
    stopped?;
    wait_result?;
    inactive?;
    shutdown?;
    recorded?;
    println!("capture stopped");
    Ok(())
}

fn filter_capture_subscriptions(
    configured: Vec<Subscription>,
    advertised: &[ble::SignalSupport],
) -> (Vec<Subscription>, Vec<CaptureSubscription>) {
    let mut scheduled = Vec::new();
    let mut decisions = Vec::new();
    for subscription in configured {
        let filter = match advertised
            .iter()
            .find(|signal| signal.semantic == subscription.semantic())
            .map(|signal| signal.status)
            .unwrap_or(ble::SignalSupportStatus::Unknown)
        {
            ble::SignalSupportStatus::Supported => SubscriptionFilterOutcome::Scheduled,
            ble::SignalSupportStatus::Unsupported => SubscriptionFilterOutcome::Unsupported,
            ble::SignalSupportStatus::Unknown => SubscriptionFilterOutcome::Unknown,
        };
        decisions.push(CaptureSubscription::new(
            subscription.semantic(),
            subscription.interval_us(),
            filter,
        ));
        if filter == SubscriptionFilterOutcome::Scheduled {
            scheduled.push(subscription);
        }
    }
    (scheduled, decisions)
}

async fn wait_for_scheduler(scheduler: &TelemetryScheduler) {
    while !scheduler.is_finished() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn finish_capture(
    sender: Option<jsonl_capture::Sender>,
    writer: Option<jsonl_capture::Writer>,
) -> Result<(), String> {
    match (sender, writer) {
        (Some(sender), Some(writer)) => jsonl_capture::close(sender, writer).await,
        (None, None) => Ok(()),
        _ => Err("JSONL recorder has incomplete ownership".into()),
    }
}

async fn emit_capture_event(
    recorder: Option<&jsonl_capture::Sender>,
    event: CaptureEvent,
) -> Result<(), String> {
    if let Some(recorder) = recorder {
        recorder
            .send(event)
            .await
            .map_err(|_| "capture recorder is closed".to_string())?;
    }
    Ok(())
}

async fn record_capture_start_failure(
    sender: &jsonl_capture::Sender,
    profile: &str,
    error: &str,
) -> Result<(), String> {
    let wallclock_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "wall clock timestamp exceeds supported range")?;
    for event in [
        CaptureEvent::capture_started(Some(wallclock_ms), Some(profile.into())),
        CaptureEvent::session_error(error),
        CaptureEvent::SessionStopped { offset_us: 0 },
    ] {
        sender
            .send(event)
            .await
            .map_err(|_| "capture recorder is closed".to_string())?;
    }
    Ok(())
}

fn planned_layout_subscriptions(
    layout: &tui::DashboardLayout,
    advertised: &[ble::SignalSupport],
) -> Result<
    (
        obdentic::subscription_policy::PollingPlan,
        Vec<Subscription>,
    ),
    String,
> {
    let supported = advertised.iter().filter_map(|signal| {
        (signal.status == ble::SignalSupportStatus::Supported).then_some(signal.semantic)
    });
    let policy = layout_observation::polling_plan(
        layout,
        LayoutFreshnessPolicy::default(),
        SubscriptionPolicy::new(HardwareCapability::conservative_default()),
        supported,
    )?;
    let subscriptions = policy
        .scheduled_entries()
        .map(|entry| {
            Subscription::new(
                entry.semantic(),
                entry
                    .effective_interval()
                    .expect("scheduled entry has an effective interval"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((policy, subscriptions))
}

async fn routed_observation_plans(
    adapter_id: &str,
    subscriptions: Vec<Subscription>,
) -> Result<Vec<ObservationPlan>, String> {
    let (mappings, cache_valid) = cached_routing_mappings(adapter_id).await;
    subscriptions
        .into_iter()
        .map(|subscription| {
            ObservationPlan::new(
                route_request(subscription.semantic(), &mappings, cache_valid)?,
                subscription.interval(),
            )
        })
        .collect()
}

fn route_request(
    semantic: &str,
    mappings: &[EcuTargetMapping],
    cache_valid: bool,
) -> Result<ReadRouting, String> {
    let knowledge = VehicleKnowledge::generic_obd2();
    let mapping = knowledge.rule(semantic).and_then(|rule| {
        mappings
            .iter()
            .find(|mapping| mapping.role().role() == rule.role())
    });
    ReadRouting::from_decision(
        knowledge
            .route(semantic, mapping, cache_valid, FallbackPolicy::Functional)
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

async fn cached_routing_mappings(adapter_id: &str) -> (Vec<EcuTargetMapping>, bool) {
    let result: Result<Option<Vec<EcuTargetMapping>>, String> = async {
        let identity = ble::identify(adapter_id).await?;
        let root = vehicle_cache_root()?;
        let store = obdentic::vehicle_cache::CacheStore::new(&root);
        let index = obdentic::vehicle_cache::VehicleIndex::new(&root);
        let Some(key) = index.key_for(identity.vin())? else {
            return Ok(None);
        };
        let Some(cache) = store.load(&key)? else {
            return Ok(None);
        };
        let validation = ble::validate_functional_support(adapter_id)
            .await
            .and_then(snapshot_from_support_validation);
        if !matches!(
            obdentic::cache_validation::validate_snapshot(&cache, validation),
            obdentic::cache_validation::CacheValidation::Validated
        ) {
            return Ok(None);
        }
        Ok(Some(
            cache
                .snapshot()
                .target_mappings()
                .iter()
                .filter_map(|mapping| mapping.to_vehicle_knowledge_mapping())
                .collect::<Vec<_>>(),
        ))
    }
    .await;

    match result {
        Ok(Some(mappings)) => (mappings, true),
        Ok(None) => (Vec::new(), false),
        Err(error) => {
            eprintln!("routing cache unavailable; using functional requests: {error}");
            (Vec::new(), false)
        }
    }
}

fn load_layout(path: Option<&str>) -> Result<tui::DashboardLayout, String> {
    path.map_or_else(
        || Ok(tui::engine_overview()),
        |path| tui::load_layout(Path::new(path)),
    )
}

fn run_capture_dpf_report(paths: &[String]) -> Result<(), String> {
    let mut replays = Vec::with_capacity(paths.len());
    for path in paths {
        replays.push(CaptureReplay::from_capture(&jsonl_capture::read(
            Path::new(path),
        )?));
    }

    for (path, replay) in paths.iter().zip(&replays) {
        print!(
            "{}",
            render_dpf_report(
                path,
                &obdentic::dpf_report::DpfReport::from_replay(replay),
                "Capture duration",
            )
        );
    }
    if replays.len() > 1 {
        print!(
            "{}",
            render_dpf_report(
                "across input order",
                &obdentic::dpf_report::DpfReport::from_replays(&replays),
                "Accumulated capture duration",
            )
        );
    }
    Ok(())
}

fn render_dpf_report(
    label: &str,
    report: &obdentic::dpf_report::DpfReport,
    duration_label: &str,
) -> String {
    let mut output = format!(
        "DPF report: {label}\nStatus: vehicle-specific experimental replay\n{duration_label}: {:.3} s\n",
        report.duration_us() as f64 / 1_000_000.0,
    );
    if report.is_empty() {
        output.push_str("No decodable experimental DPF readings.\n\n");
        return output;
    }
    output.push_str("Signals\n");
    for summary in report.summaries() {
        let offsets = summary
            .first_offset_us()
            .zip(summary.last_offset_us())
            .map(|(first, last)| {
                format!(
                    "; offsets: {:.3} s / {:.3} s",
                    first as f64 / 1_000_000.0,
                    last as f64 / 1_000_000.0,
                )
            })
            .unwrap_or_default();
        output.push_str(&format!(
            "  {}\n    samples: {}; first/last: {:.3} / {:.3} {}; delta: {:+.3}; range: {:.3} .. {:.3}{}\n",
            summary.semantic(),
            summary.count(),
            summary.first_value(),
            summary.last_value(),
            summary.unit(),
            summary.delta(),
            summary.min(),
            summary.max(),
            offsets,
        ));
    }
    if label == "across input order" {
        output.push_str("Input order compares snapshots; it is not a continuous DPF timeline.\n");
    }
    output.push('\n');
    output
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    match args {
        [command] if command == "signals" => Ok(Command::Signals),
        [command, adapter_flag, adapter_id, supported_flag]
            if command == "signals"
                && adapter_flag == "--adapter"
                && supported_flag == "--supported" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::SupportedSignals {
                adapter_id: adapter_id.clone(),
            })
        }
        [command] if command == "scan" => Ok(Command::Scan),
        [command, job, adapter_flag, adapter_id]
            if command == "diagnose" && job == "dtc.scan" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::DiagnoseDtcScan {
                adapter_id: adapter_id.clone(),
                recording: None,
            })
        }
        [command, job, adapter_flag, adapter_id]
            if command == "diagnose" && job == "ea189.dpf.probe" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::DiagnoseEa189DpfProbe {
                adapter_id: adapter_id.clone(),
                recording: None,
            })
        }
        [command, job, adapter_flag, adapter_id, record_flag, path]
            if command == "diagnose"
                && job == "dtc.scan"
                && adapter_flag == "--adapter"
                && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::DiagnoseDtcScan {
                adapter_id: adapter_id.clone(),
                recording: Some(path.clone()),
            })
        }
        [command, job, adapter_flag, adapter_id, record_flag, path]
            if command == "diagnose"
                && job == "ea189.dpf.probe"
                && adapter_flag == "--adapter"
                && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::DiagnoseEa189DpfProbe {
                adapter_id: adapter_id.clone(),
                recording: Some(path.clone()),
            })
        }
        [command, action, adapter_flag, adapter_id]
            if command == "vehicle" && action == "identify" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::VehicleIdentify {
                adapter_id: adapter_id.clone(),
            })
        }
        [command, action, adapter_flag, adapter_id]
            if command == "vehicle"
                && (action == "discover" || action == "refresh")
                && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(if action == "discover" {
                Command::VehicleDiscover {
                    adapter_id: adapter_id.clone(),
                }
            } else {
                Command::VehicleRefresh {
                    adapter_id: adapter_id.clone(),
                }
            })
        }
        [command, action, adapter_flag, adapter_id]
            if command == "vehicle" && action == "scan" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::VehicleScan {
                adapter_id: adapter_id.clone(),
            })
        }
        [command, action] if command == "vehicle" && action == "show" => Ok(Command::VehicleShow),
        [command] if command == "demo" => Ok(Command::Demo),
        [command, adapter_flag, adapter_id, profile_flag, profile_name, record_flag, path]
            if command == "capture"
                && adapter_flag == "--adapter"
                && profile_flag == "--profile"
                && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            capture::profile(profile_name)?;
            Ok(Command::Capture {
                adapter_id: adapter_id.clone(),
                profile: profile_name.clone(),
                recording: path.clone(),
            })
        }
        [command, adapter_flag, adapter_id, profile_flag, profile, record_flag, path, cycles_flag, cycles, interval_flag, interval_seconds]
            if command == "capture"
                && profile == "ea189-dpf"
                && adapter_flag == "--adapter"
                && profile_flag == "--profile"
                && record_flag == "--record"
                && cycles_flag == "--cycles"
                && interval_flag == "--interval-seconds" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::CaptureEa189DpfTrace {
                adapter_id: adapter_id.clone(),
                recording: path.clone(),
                cycles: parse_trace_cycles(cycles)?,
                interval: Duration::from_secs(parse_trace_interval_seconds(interval_seconds)?),
            })
        }
        [command, action, path] if command == "capture" && action == "inspect" => {
            Ok(Command::CaptureInspect(path.clone()))
        }
        [command, action, path] if command == "capture" && action == "capability" => {
            Ok(Command::CaptureCapability(path.clone()))
        }
        [command, action, paths @ ..]
            if command == "capture" && action == "dpf-report" && !paths.is_empty() =>
        {
            Ok(Command::CaptureDpfReport(paths.to_vec()))
        }
        [command, path] if command == "replay" => Ok(Command::Replay(path.clone())),
        [command, action, name, path]
            if command == "layout" && action == "save" && name == "engine-overview" =>
        {
            Ok(Command::LayoutSave(path.clone()))
        }
        [command, source] if command == "tui" && source == "demo" => Ok(Command::TuiDemo(None)),
        [command, source, layout_flag, path]
            if command == "tui" && source == "demo" && layout_flag == "--layout" =>
        {
            Ok(Command::TuiDemo(Some(path.clone())))
        }
        [command, source, path] if command == "tui" && source == "replay" => {
            Ok(Command::TuiReplay {
                recording: path.clone(),
                layout: None,
            })
        }
        [command, source, recording, layout_flag, path]
            if command == "tui" && source == "replay" && layout_flag == "--layout" =>
        {
            Ok(Command::TuiReplay {
                recording: recording.clone(),
                layout: Some(path.clone()),
            })
        }
        [command, source, capture] if command == "tui" && source == "capture" => {
            Ok(Command::TuiCapture {
                capture: capture.clone(),
                layout: None,
            })
        }
        [command, source, capture, layout_flag, path]
            if command == "tui" && source == "capture" && layout_flag == "--layout" =>
        {
            Ok(Command::TuiCapture {
                capture: capture.clone(),
                layout: Some(path.clone()),
            })
        }
        [command, source, adapter_flag, adapter_id]
            if command == "tui" && source == "live" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::TuiLive {
                adapter_id: adapter_id.clone(),
                layout: None,
                recording: None,
            })
        }
        [command, source, adapter_flag, adapter_id, layout_flag, path]
            if command == "tui"
                && source == "live"
                && adapter_flag == "--adapter"
                && layout_flag == "--layout" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::TuiLive {
                adapter_id: adapter_id.clone(),
                layout: Some(path.clone()),
                recording: None,
            })
        }
        [command, source, adapter_flag, adapter_id, record_flag, path]
            if command == "tui"
                && source == "live"
                && adapter_flag == "--adapter"
                && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::TuiLive {
                adapter_id: adapter_id.clone(),
                layout: None,
                recording: Some(path.clone()),
            })
        }
        [command, source, adapter_flag, adapter_id, layout_flag, layout, record_flag, recording]
            if command == "tui"
                && source == "live"
                && adapter_flag == "--adapter"
                && layout_flag == "--layout"
                && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::TuiLive {
                adapter_id: adapter_id.clone(),
                layout: Some(layout.clone()),
                recording: Some(recording.clone()),
            })
        }
        [command, signal, adapter_flag, adapter_id]
            if command == "read" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::Read {
                request: prepare_read(signal)?,
                adapter_id: adapter_id.clone(),
                recording: None,
            })
        }
        [command, signal, adapter_flag, adapter_id, record_flag, path]
            if command == "read" && adapter_flag == "--adapter" && record_flag == "--record" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::Read {
                request: prepare_read(signal)?,
                adapter_id: adapter_id.clone(),
                recording: Some(path.clone()),
            })
        }
        _ => Err(USAGE.into()),
    }
}

fn require_uuid(value: &str) -> Result<(), String> {
    let valid = value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        });
    valid
        .then_some(())
        .ok_or_else(|| "adapter must be a CoreBluetooth UUID".into())
}

fn parse_trace_cycles(value: &str) -> Result<u16, String> {
    let cycles = value
        .parse::<u16>()
        .map_err(|_| "EA189 DPF trace cycles must be an integer from 1 to 1440".to_string())?;
    if cycles == 0 || cycles > 1_440 {
        return Err("EA189 DPF trace cycles must be an integer from 1 to 1440".into());
    }
    Ok(cycles)
}

fn parse_trace_interval_seconds(value: &str) -> Result<u64, String> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "EA189 DPF trace interval must be at least 30 seconds".to_string())?;
    (seconds >= 30)
        .then_some(seconds)
        .ok_or_else(|| "EA189 DPF trace interval must be at least 30 seconds".into())
}

fn render_signals() -> String {
    let mut output = String::from(
        "semantic\tprofile\tprotocol\trequest\tdecoder\tminimum\tmaximum\tunit\tsubsystem\tprovenance\tconfidence\thardware_validation\tdescription\n",
    );
    for signal in supported_signals() {
        let metadata = signal.metadata();
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            escape_field(metadata.semantic),
            escape_field(metadata.profile),
            escape_field(metadata.protocol),
            hex(&metadata.request),
            escape_field(metadata.decoder),
            metadata.minimum,
            metadata.maximum,
            escape_field(metadata.unit),
            escape_field(metadata.subsystem),
            escape_field(metadata.provenance),
            escape_field(metadata.confidence),
            escape_field(metadata.hardware_validation),
            escape_field(metadata.description),
        ));
    }
    output
}

fn render_supported_signals(support: &[ble::SignalSupport]) -> String {
    let mut output = String::from("semantic\tstatus\thardware_validation\n");
    for signal in supported_signals() {
        let metadata = signal.metadata();
        let status = match support
            .iter()
            .find(|reported| reported.semantic == metadata.semantic)
            .map(|reported| reported.status)
            .unwrap_or(ble::SignalSupportStatus::Unknown)
        {
            ble::SignalSupportStatus::Supported => "supported",
            ble::SignalSupportStatus::Unsupported => "unsupported",
            ble::SignalSupportStatus::Unknown => "unknown",
        };
        output.push_str(&escape_field(metadata.semantic));
        output.push('\t');
        output.push_str(status);
        output.push('\t');
        output.push_str(&escape_field(metadata.hardware_validation));
        output.push('\n');
    }
    output
}

fn escape_field(value: &str) -> String {
    value.escape_default().collect()
}

async fn demo() -> Result<Transaction, String> {
    prepare_read("engine.rpm")?.complete("user", vec![0x41, 0x0c, 0x1a, 0xf8])
}

fn demo_samples() -> Result<Vec<Transaction>, String> {
    let mut samples = Vec::new();
    for index in 0_u16..60 {
        let timestamp_ms = 1_700_000_000_000 + u128::from(index) * 200 + u128::from(index % 5) * 20;
        let rpm = 800 + index * 25;
        samples.push(demo_sample(
            "engine.rpm",
            vec![0x41, 0x0c, ((rpm * 4) >> 8) as u8, (rpm * 4) as u8],
            timestamp_ms,
        )?);
        if index % 2 == 0 {
            samples.push(demo_sample(
                "engine.coolant_temperature",
                vec![0x41, 0x05, 120 + (index % 8) as u8],
                timestamp_ms + 30,
            )?);
            samples.push(demo_sample(
                "engine.maf",
                vec![0x41, 0x10, 0x01, 0x90 + (index % 40) as u8],
                timestamp_ms + 70,
            )?);
        }
        if index % 3 == 0 {
            samples.push(demo_sample(
                "vehicle.speed",
                vec![0x41, 0x0d, (index * 2) as u8],
                timestamp_ms + 110,
            )?);
        }
    }
    Ok(samples)
}

fn demo_sample(
    semantic: &str,
    response: Vec<u8>,
    timestamp_ms: u128,
) -> Result<Transaction, String> {
    Ok(prepare_read(semantic)?
        .complete("demo", response)?
        .with_timestamp_ms(timestamp_ms))
}

fn telemetry(transactions: &[Transaction]) -> Result<TelemetryState, String> {
    let mut state = TelemetryState::new(600)?;
    for transaction in transactions {
        state.ingest(transaction);
    }
    Ok(state)
}

fn show(transaction: &Transaction) {
    println!("OBDentic — transparent read-only diagnostics");
    println!("source    {}", transaction.source());
    println!("profile   {}", transaction.profile());
    println!("semantic  {}", transaction.semantic());
    println!("tx        {}", hex(transaction.request()));
    println!("rx        {}", hex(transaction.response()));
    println!("decoded   {} {}", transaction.value(), transaction.unit());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[tokio::test]
    async fn startup_failure_is_preserved_in_the_capture_event_stream() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(3);
        record_capture_start_failure(&sender, "engine-baseline", "Carly setup timed out")
            .await
            .unwrap();

        assert!(matches!(
            receiver.recv().await,
            Some(CaptureEvent::CaptureStarted {
                profile: Some(profile),
                ..
            }) if profile == "engine-baseline"
        ));
        assert_eq!(
            receiver.recv().await,
            Some(CaptureEvent::session_error("Carly setup timed out"))
        );
        assert_eq!(
            receiver.recv().await,
            Some(CaptureEvent::SessionStopped { offset_us: 0 })
        );
    }

    #[tokio::test]
    async fn runtime_shutdown_documents_observation_idle_then_stopped() {
        let (runtime, task) = obdentic::runtime_actor::start();
        let mut state = RuntimeState::default();
        apply_runtime_event(
            &runtime,
            &mut state,
            None,
            RuntimeEvent::InitializationCompleted,
        )
        .await
        .unwrap();
        apply_runtime_event(&runtime, &mut state, None, RuntimeEvent::ObservationStarted)
            .await
            .unwrap();

        finish_runtime(&runtime, &mut state, None).await.unwrap();

        assert_eq!(
            state.identity(),
            (
                obdentic::runtime_state::Phase::Stopped,
                obdentic::runtime_state::Activity::Idle
            )
        );
        drop(runtime);
        task.await.unwrap();
    }

    #[test]
    fn parses_approved_forms() {
        let uuid = "00000000-0000-4000-8000-000000000000";
        assert_eq!(parse_command(&args(&["signals"])), Ok(Command::Signals));
        assert_eq!(
            parse_command(&args(&["signals", "--adapter", uuid, "--supported",])),
            Ok(Command::SupportedSignals {
                adapter_id: uuid.into(),
            })
        );
        assert_eq!(parse_command(&args(&["scan"])), Ok(Command::Scan));
        assert_eq!(
            parse_command(&args(&["diagnose", "dtc.scan", "--adapter", uuid,])),
            Ok(Command::DiagnoseDtcScan {
                adapter_id: uuid.into(),
                recording: None,
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "diagnose",
                "dtc.scan",
                "--adapter",
                uuid,
                "--record",
                "dtc.jsonl",
            ])),
            Ok(Command::DiagnoseDtcScan {
                adapter_id: uuid.into(),
                recording: Some("dtc.jsonl".into()),
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "diagnose",
                "ea189.dpf.probe",
                "--adapter",
                uuid,
                "--record",
                "dpf.jsonl",
            ])),
            Ok(Command::DiagnoseEa189DpfProbe {
                adapter_id: uuid.into(),
                recording: Some("dpf.jsonl".into()),
            })
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "identify", "--adapter", uuid,])),
            Ok(Command::VehicleIdentify {
                adapter_id: uuid.into(),
            })
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "discover", "--adapter", uuid,])),
            Ok(Command::VehicleDiscover {
                adapter_id: uuid.into(),
            })
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "refresh", "--adapter", uuid,])),
            Ok(Command::VehicleRefresh {
                adapter_id: uuid.into(),
            })
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "scan", "--adapter", uuid,])),
            Ok(Command::VehicleScan {
                adapter_id: uuid.into(),
            })
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "show"])),
            Ok(Command::VehicleShow)
        );
        assert_eq!(parse_command(&args(&["demo"])), Ok(Command::Demo));
        assert_eq!(
            parse_command(&args(&["capture", "inspect", "capture.jsonl"])),
            Ok(Command::CaptureInspect("capture.jsonl".into()))
        );
        assert_eq!(
            parse_command(&args(&["capture", "capability", "capture.jsonl"])),
            Ok(Command::CaptureCapability("capture.jsonl".into()))
        );
        assert_eq!(
            parse_command(&args(&[
                "capture",
                "dpf-report",
                "drive.jsonl",
                "idle.jsonl",
            ])),
            Ok(Command::CaptureDpfReport(vec![
                "drive.jsonl".into(),
                "idle.jsonl".into(),
            ]))
        );
        assert_eq!(
            parse_command(&args(&[
                "capture",
                "--adapter",
                uuid,
                "--profile",
                "engine-baseline",
                "--record",
                "capture.jsonl",
            ])),
            Ok(Command::Capture {
                adapter_id: uuid.into(),
                profile: "engine-baseline".into(),
                recording: "capture.jsonl".into(),
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "capture",
                "--adapter",
                uuid,
                "--profile",
                "ea189-dpf",
                "--record",
                "trace.jsonl",
                "--cycles",
                "120",
                "--interval-seconds",
                "60",
            ])),
            Ok(Command::CaptureEa189DpfTrace {
                adapter_id: uuid.into(),
                recording: "trace.jsonl".into(),
                cycles: 120,
                interval: Duration::from_secs(60),
            })
        );
        assert_eq!(
            parse_command(&args(&["tui", "demo"])),
            Ok(Command::TuiDemo(None))
        );
        assert_eq!(
            parse_command(&args(&["tui", "demo", "--layout", "custom.tsv"])),
            Ok(Command::TuiDemo(Some("custom.tsv".into())))
        );
        assert_eq!(
            parse_command(&args(&["layout", "save", "engine-overview", "saved.tsv"])),
            Ok(Command::LayoutSave("saved.tsv".into()))
        );
        assert_eq!(
            parse_command(&args(&["replay", "session.tsv"])),
            Ok(Command::Replay("session.tsv".into()))
        );
        assert_eq!(
            parse_command(&args(&["tui", "replay", "session.tsv"])),
            Ok(Command::TuiReplay {
                recording: "session.tsv".into(),
                layout: None
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "tui",
                "replay",
                "session.tsv",
                "--layout",
                "custom.tsv",
            ])),
            Ok(Command::TuiReplay {
                recording: "session.tsv".into(),
                layout: Some("custom.tsv".into()),
            })
        );
        assert_eq!(
            parse_command(&args(&["tui", "capture", "drive.jsonl"])),
            Ok(Command::TuiCapture {
                capture: "drive.jsonl".into(),
                layout: None,
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "tui",
                "capture",
                "drive.jsonl",
                "--layout",
                "custom.tsv",
            ])),
            Ok(Command::TuiCapture {
                capture: "drive.jsonl".into(),
                layout: Some("custom.tsv".into()),
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "tui",
                "live",
                "--adapter",
                uuid,
                "--record",
                "evidence.tsv",
            ])),
            Ok(Command::TuiLive {
                adapter_id: uuid.into(),
                layout: None,
                recording: Some("evidence.tsv".into()),
            })
        );
        assert_eq!(
            parse_command(&args(&[
                "tui",
                "live",
                "--adapter",
                uuid,
                "--layout",
                "custom.tsv",
                "--record",
                "evidence.tsv",
            ])),
            Ok(Command::TuiLive {
                adapter_id: uuid.into(),
                layout: Some("custom.tsv".into()),
                recording: Some("evidence.tsv".into()),
            })
        );
        for signal in [
            "engine.rpm",
            "engine.coolant_temperature",
            "vehicle.speed",
            "engine.maf",
        ] {
            assert_eq!(
                parse_command(&args(&["read", signal, "--adapter", uuid])),
                Ok(Command::Read {
                    request: prepare_read(signal).unwrap(),
                    adapter_id: uuid.into(),
                    recording: None,
                })
            );
            assert_eq!(
                parse_command(&args(&[
                    "read",
                    signal,
                    "--adapter",
                    uuid,
                    "--record",
                    "session.tsv",
                ])),
                Ok(Command::Read {
                    request: prepare_read(signal).unwrap(),
                    adapter_id: uuid.into(),
                    recording: Some("session.tsv".into()),
                })
            );
        }
    }

    #[test]
    fn renders_functional_scan_for_each_responder_without_physical_targets() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let provenance = Provenance::new("test", Confidence::High).unwrap();
        let discovery = obdentic::functional_discovery::FunctionalResponderDiscovery::new([
            obdentic::functional_discovery::FunctionalPageObservation::new(
                [0x01, 0x00],
                ResponderIdentity::opaque(context.clone(), "7E8"),
                vec![0x41, 0x00, 0x00, 0x18, 0x00, 0x00],
                provenance.clone(),
            )
            .unwrap(),
            obdentic::functional_discovery::FunctionalPageObservation::new(
                [0x01, 0x00],
                ResponderIdentity::opaque(context, "7E9"),
                vec![0x41, 0x00, 0x00, 0x08, 0x00, 0x00],
                provenance,
            )
            .unwrap(),
        ]);

        let output = render_vehicle_scan(&discovery);
        assert!(output.contains("scope\tfunctional OBD-II Mode 01 only\n"));
        assert!(output.contains("responders\t2\n"));
        assert!(output.contains("signal\t7E8\tengine.rpm\tadvertised\n"));
        assert!(output.contains("signal\t7E9\tengine.rpm\tnot-advertised\n"));
        assert!(output.contains("signal\t7E9\tvehicle.speed\tadvertised\n"));
        assert!(!output.contains("7E0"));
    }

    #[test]
    fn missing_or_unvalidated_mapping_stays_functional() {
        assert!(matches!(
            route_request("engine.rpm", &[], false),
            Ok(ReadRouting::Functional(_))
        ));
    }

    #[test]
    fn engine_target_validation_is_explicit_and_read_only() {
        let request = engine_target_request().unwrap();
        assert_eq!(request.request().bytes(), [0x01, 0x0C]);
        assert_eq!(request.target().address().unwrap().value(), "7E0");
        assert_eq!(request.expected_responder().as_str(), "7E8");
    }

    #[test]
    fn confirmed_engine_target_preserves_distinct_role_target_and_responder() {
        let mapping = confirmed_engine_target().unwrap();
        assert_eq!(mapping.role().unwrap().role(), &EcuRole::Engine);
        assert_eq!(mapping.target().address().unwrap().value(), "7E0");
        assert_eq!(mapping.responder().unwrap().value(), Some("7E8"));
        assert_ne!(
            mapping.target().address().unwrap().value(),
            mapping.responder().unwrap().value().unwrap()
        );
    }

    #[test]
    fn target_validation_requires_the_expected_engine_transaction() {
        let valid = prepare_read("engine.rpm")
            .unwrap()
            .complete("test", vec![0x41, 0x0C, 0x00, 0x00])
            .unwrap();
        assert!(validate_engine_target_transaction(&valid).is_ok());

        let wrong_signal = prepare_read("vehicle.speed")
            .unwrap()
            .complete("test", vec![0x41, 0x0D, 0x00])
            .unwrap();
        assert!(validate_engine_target_transaction(&wrong_signal).is_err());
    }

    #[test]
    fn secondary_target_request_and_validation_are_explicit_and_read_only() {
        let request = secondary_target_request().unwrap();
        assert_eq!(request.request().bytes(), [0x01, 0x0D]);
        assert_eq!(request.target().address().unwrap().value(), "7E1");
        assert_eq!(request.expected_responder().as_str(), "7E9");

        let valid = prepare_read("vehicle.speed")
            .unwrap()
            .complete("test", vec![0x41, 0x0D, 0x00])
            .unwrap();
        assert!(validate_vehicle_speed_target_transaction(&valid).is_ok());
    }

    #[test]
    fn secondary_target_requires_7e9_functional_support() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let provenance = Provenance::new("test", Confidence::High).unwrap();
        let observation = obdentic::functional_discovery::FunctionalPageObservation::new(
            [0x01, 0x00],
            ResponderIdentity::opaque(context, "7E8"),
            vec![0x41, 0x00, 0x00, 0x08, 0x00, 0x00],
            provenance,
        )
        .unwrap();
        let discovery =
            obdentic::functional_discovery::FunctionalResponderDiscovery::new([observation]);

        assert!(!secondary_target_allowed(&discovery));
    }

    #[test]
    fn confirmed_secondary_target_has_no_inferred_logical_role() {
        let mapping = confirmed_secondary_target().unwrap();
        assert!(mapping.role().is_none());
        assert_eq!(mapping.target().address().unwrap().value(), "7E1");
        assert_eq!(mapping.responder().unwrap().value(), Some("7E9"));
        assert_eq!(mapping.confidence(), Confidence::Verified);
    }

    #[test]
    fn target_validation_is_not_attempted_without_7e8_evidence() {
        let discovery = obdentic::functional_discovery::FunctionalResponderDiscovery::new([]);
        assert!(!engine_responder_observed(&discovery));

        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let provenance = Provenance::new("test", Confidence::High).unwrap();
        let observation = obdentic::functional_discovery::FunctionalPageObservation::new(
            [0x01, 0x00],
            ResponderIdentity::opaque(context, "7E9"),
            vec![0x41, 0x00, 0, 0, 0, 0],
            provenance,
        )
        .unwrap();
        let discovery =
            obdentic::functional_discovery::FunctionalResponderDiscovery::new([observation]);
        assert!(!engine_responder_observed(&discovery));
    }

    #[test]
    fn rejects_missing_uuid_unknown_signal_and_extra_arguments() {
        assert_eq!(
            parse_command(&args(&["read", "engine.rpm", "--adapter"])),
            Err(USAGE.into())
        );
        for signal in ["dtc.clear", "security.access", "engine.unknown"] {
            assert_eq!(
                parse_command(&args(&[
                    "read",
                    signal,
                    "--adapter",
                    "00000000-0000-4000-8000-000000000000",
                ])),
                Err(format!(
                    "read-only core rejected unsupported signal: {signal}"
                ))
            );
        }
        assert_eq!(
            parse_command(&args(&["read", "engine.rpm", "--adapter", "UUID"])),
            Err("adapter must be a CoreBluetooth UUID".into())
        );
        assert_eq!(parse_command(&args(&["scan", "extra"])), Err(USAGE.into()));
        assert_eq!(
            parse_command(&args(&[
                "diagnose",
                "dtc.read",
                "--adapter",
                "00000000-0000-4000-8000-000000000000",
            ])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&[
                "diagnose",
                "dtc.scan",
                "--adapter",
                "00000000-0000-4000-8000-000000000000",
                "--service",
                "03",
            ])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "identify", "--adapter", "UUID"])),
            Err("adapter must be a CoreBluetooth UUID".into())
        );
        for value in ["0", "1441", "not-a-number"] {
            assert!(parse_trace_cycles(value).is_err());
        }
        for value in ["0", "29", "not-a-number"] {
            assert!(parse_trace_interval_seconds(value).is_err());
        }
        assert_eq!(
            parse_command(&args(&["vehicle", "identify", "--adapter"])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "discover", "--adapter", "UUID"])),
            Err("adapter must be a CoreBluetooth UUID".into())
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "discover", "--adapter"])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "show", "extra"])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&[
                "capture",
                "--adapter",
                "00000000-0000-4000-8000-000000000000",
                "--profile",
                "unknown",
                "--record",
                "capture.jsonl",
            ])),
            Err("unknown capture profile: unknown".into())
        );
        assert_eq!(
            parse_command(&args(&[
                "capture",
                "--adapter",
                "00000000-0000-4000-8000-000000000000",
                "--profile",
                "engine-baseline",
            ])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&["signals", "--adapter", "UUID", "--supported",])),
            Err("adapter must be a CoreBluetooth UUID".into())
        );
        assert_eq!(
            parse_command(&args(&["demo", "session.tsv"])),
            Err(USAGE.into())
        );
        assert_eq!(
            parse_command(&args(&["layout", "save", "unknown", "saved.tsv"])),
            Err(USAGE.into())
        );
    }

    #[test]
    fn renders_signal_catalog_as_escaped_auditable_tsv() {
        let output = render_signals();
        assert_eq!(
            output.lines().next(),
            Some("semantic\tprofile\tprotocol\trequest\tdecoder\tminimum\tmaximum\tunit\tsubsystem\tprovenance\tconfidence\thardware_validation\tdescription")
        );
        for semantic in [
            "engine.rpm",
            "engine.coolant_temperature",
            "vehicle.speed",
            "engine.maf",
        ] {
            assert!(output
                .lines()
                .skip(1)
                .any(|line| line.starts_with(&format!("{semantic}\t"))));
        }
        assert!(output
            .lines()
            .skip(1)
            .all(|line| line.split('\t').count() == 13));
        assert!(!output.contains('\r'));
    }

    #[test]
    fn renders_support_and_hardware_validation_in_catalog_order() {
        let output = render_supported_signals(&[
            ble::SignalSupport {
                semantic: "engine.rpm",
                status: ble::SignalSupportStatus::Supported,
            },
            ble::SignalSupport {
                semantic: "engine.maf",
                status: ble::SignalSupportStatus::Unsupported,
            },
        ]);
        assert_eq!(
            output.lines().next(),
            Some("semantic\tstatus\thardware_validation")
        );
        assert!(output
            .lines()
            .any(|line| line.starts_with("engine.rpm\tsupported\t")));
        assert!(output
            .lines()
            .any(|line| line.starts_with("engine.maf\tunsupported\t")));
        assert!(output
            .lines()
            .any(|line| line.starts_with("engine.load\tunknown\t")));
        assert_eq!(output.lines().count(), supported_signals().len() + 1);
    }

    #[test]
    fn capture_filter_schedules_only_advertised_signals_and_records_all_decisions() {
        let configured = capture::profile("engine-baseline")
            .unwrap()
            .subscriptions()
            .unwrap();
        let (scheduled, decisions) = filter_capture_subscriptions(
            configured,
            &[
                ble::SignalSupport {
                    semantic: "engine.rpm",
                    status: ble::SignalSupportStatus::Supported,
                },
                ble::SignalSupport {
                    semantic: "engine.maf",
                    status: ble::SignalSupportStatus::Unsupported,
                },
            ],
        );

        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].semantic(), "engine.rpm");
        assert_eq!(scheduled[0].interval(), Duration::from_secs(1));
        assert_eq!(decisions.len(), 13);
        assert_eq!(decisions[0].filter(), SubscriptionFilterOutcome::Scheduled);
        assert_eq!(
            decisions[1].filter(),
            SubscriptionFilterOutcome::Unsupported
        );
        assert_eq!(decisions[2].filter(), SubscriptionFilterOutcome::Unknown);
    }

    #[test]
    fn layout_planning_schedules_only_supported_semantics() {
        let layout = tui::DashboardLayout {
            name: "custom".into(),
            panels: vec![
                tui::Panel {
                    title: "RPM".into(),
                    view: tui::View::Value,
                    signals: vec!["engine.rpm".into()],
                },
                tui::Panel {
                    title: "Future".into(),
                    view: tui::View::Value,
                    signals: vec!["vehicle.future_fact".into()],
                },
            ],
        };
        let (plan, subscriptions) = planned_layout_subscriptions(
            &layout,
            &[ble::SignalSupport {
                semantic: "engine.rpm",
                status: ble::SignalSupportStatus::Supported,
            }],
        )
        .unwrap();

        assert_eq!(subscriptions.len(), 1);
        assert_eq!(subscriptions[0].semantic(), "engine.rpm");
        let future = plan
            .entries()
            .iter()
            .find(|entry| entry.semantic() == "vehicle.future_fact")
            .unwrap();
        assert_eq!(
            future.status(),
            obdentic::subscription_policy::PlanStatus::Unsupported
        );
        assert!(future.effective_interval().is_none());
    }

    #[test]
    fn tui_demo_uses_only_known_decoded_signals() {
        let samples = demo_samples().unwrap();
        assert!(samples.len() > 100);
        assert!(samples.iter().all(|sample| sample.source() == "demo"));
        assert_eq!(samples[0].semantic(), "engine.rpm");
        assert_eq!(samples[0].value(), 800.0);
        assert!(samples
            .windows(2)
            .any(|pair| pair[0].timestamp_ms() != pair[1].timestamp_ms()));
    }

    #[test]
    fn renders_responder_scoped_dtc_facts_without_vehicle_diagnosis() {
        let evidence = [dtc::ResponseEvidence::new(
            Some(dtc::ResponderIdentity::new("7E8").unwrap()),
            [0x43, 0x01, 0x0c],
        )];
        let result = dtc::decode_mode03(&evidence);
        let job = DiagnosticJob::dtc_scan(DiagnosticScope::VehicleWide);
        let output = render_dtc_scan(&job, &job.plan(), &result, &[]);

        assert!(output.contains("job\tdtc.scan"));
        assert!(output.contains("status\tcompleted"));
        assert!(output.contains("responder\t7E8\tdtc\tP010C"));
        assert!(!output.contains("VIN"));
        assert!(!output.contains("diagnosis"));
    }

    #[tokio::test]
    async fn discovery_data_failures_return_to_ready_without_faulting_transport() {
        let (runtime, task) = obdentic::runtime_actor::start();
        let mut state = RuntimeState::default();
        apply_runtime_event(
            &runtime,
            &mut state,
            None,
            RuntimeEvent::InitializationCompleted,
        )
        .await
        .unwrap();
        apply_runtime_event(&runtime, &mut state, None, RuntimeEvent::DiscoveryStarted)
            .await
            .unwrap();

        finish_discovery_failure("invalid identity payload", &runtime, &mut state)
            .await
            .unwrap();

        assert_eq!(
            state.identity(),
            (
                obdentic::runtime_state::Phase::Ready,
                obdentic::runtime_state::Activity::Idle
            )
        );
        assert_eq!(state.context().transport(), TransportState::Disconnected);
        drop(runtime);
        task.await.unwrap();
    }
}
