use obdentic::{
    audit::AuditState,
    ble, capture,
    capture_events::{CaptureEvent, CaptureSubscription, SubscriptionFilterOutcome},
    capture_report, hex, jsonl_capture, prepare_read, record, replay,
    scheduler::{Subscription, TelemetryScheduler},
    supported_signals,
    telemetry::TelemetryState,
    tui, ReadRequest, Transaction,
};
use std::{
    env,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const USAGE: &str = "usage: obdentic signals | obdentic signals --adapter <CoreBluetooth UUID> --supported | obdentic scan | obdentic vehicle identify --adapter <CoreBluetooth UUID> | obdentic read <signal> --adapter <CoreBluetooth UUID> [--record recording.tsv] | obdentic capture --adapter <CoreBluetooth UUID> --profile engine-baseline --record <capture.jsonl> | obdentic capture inspect <capture.jsonl> | obdentic capture capability <capture.jsonl> | obdentic demo | obdentic replay <recording.tsv> | obdentic layout save engine-overview <layout.tsv> | obdentic tui demo [--layout layout.tsv] | obdentic tui replay <recording.tsv> [--layout layout.tsv] | obdentic tui live --adapter <CoreBluetooth UUID> [--layout layout.tsv] [--record capture.jsonl]";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Signals,
    SupportedSignals {
        adapter_id: String,
    },
    Scan,
    VehicleIdentify {
        adapter_id: String,
    },
    Demo,
    Capture {
        adapter_id: String,
        profile: String,
        recording: String,
    },
    CaptureInspect(String),
    CaptureCapability(String),
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
    match parse_command(&args)? {
        Command::Signals => print!("{}", render_signals()),
        Command::SupportedSignals { adapter_id } => {
            let support = ble::supported_signals(&adapter_id).await?;
            print!("{}", render_supported_signals(&support));
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
        }
        Command::VehicleIdentify { adapter_id } => {
            let identity = ble::identify(&adapter_id).await?;
            println!("VIN       {}", identity.vin());
        }
        Command::Capture {
            adapter_id,
            profile,
            recording,
        } => run_capture(&adapter_id, &profile, Path::new(&recording)).await?,
        Command::CaptureInspect(path) => {
            let capture = jsonl_capture::read(Path::new(&path))?;
            print!("{}", capture_report::render_inspection(&path, &capture));
        }
        Command::CaptureCapability(path) => {
            let capture = jsonl_capture::read(Path::new(&path))?;
            print!("{}", capture_report::render_capability(&path, &capture));
        }
        Command::Demo => show(&demo().await?),
        Command::Read {
            request,
            adapter_id,
            recording,
        } => {
            let transaction = ble::read(&adapter_id, request).await?;
            if let Some(path) = recording.as_deref() {
                record(Path::new(path), &transaction)?;
            }
            show(&transaction);
            if let Some(path) = recording {
                println!("recorded  {path}");
            }
        }
        Command::Replay(path) => show(&replay(Path::new(&path)).await?),
        Command::LayoutSave(path) => {
            tui::save_layout(Path::new(&path), &tui::engine_overview())?;
            println!("saved layout  {path}");
        }
        Command::TuiDemo(layout_path) => {
            let transactions = demo_samples()?;
            let layout = load_layout(layout_path.as_deref())?;
            tui::run(&layout, &telemetry(&transactions)?, &transactions)?;
        }
        Command::TuiReplay { recording, layout } => {
            let transactions = [replay(Path::new(&recording)).await?];
            let layout = load_layout(layout.as_deref())?;
            tui::run(&layout, &telemetry(&transactions)?, &transactions)?;
        }
        Command::TuiLive {
            adapter_id,
            layout,
            recording,
        } => {
            let telemetry = Arc::new(Mutex::new(TelemetryState::new(600)?));
            let audit = Arc::new(Mutex::new(AuditState::new(600)?));
            let (recorder, writer) = match recording.as_deref() {
                Some(path) => {
                    let (sender, writer) = jsonl_capture::start(Path::new(path))?;
                    (Some(sender), Some(writer))
                }
                None => (None, None),
            };
            let scheduler = match TelemetryScheduler::start(
                &adapter_id,
                live_subscriptions()?,
                telemetry.clone(),
                audit.clone(),
                recorder.clone(),
                None,
                None,
            )
            .await
            {
                Ok(scheduler) => scheduler,
                Err(error) => {
                    finish_capture(recorder, writer).await?;
                    return Err(error);
                }
            };
            let result = tui::run_live(&load_layout(layout.as_deref())?, telemetry, audit);
            let stopped = scheduler.stop().await;
            let recorded = finish_capture(recorder, writer).await;
            result?;
            stopped?;
            recorded?;
        }
    }
    Ok(())
}

async fn run_capture(adapter_id: &str, profile_name: &str, path: &Path) -> Result<(), String> {
    let profile = capture::profile(profile_name)?;
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

    let (sender, writer) = jsonl_capture::start(path)?;
    println!("capture profile  {profile_name}");
    println!(
        "capture signals  {}/{} supported",
        subscriptions.len(),
        total
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

    let scheduler = match TelemetryScheduler::start(
        adapter_id,
        subscriptions,
        Arc::new(Mutex::new(TelemetryState::new(600)?)),
        Arc::new(Mutex::new(AuditState::new(600)?)),
        Some(sender.clone()),
        Some(profile.name().into()),
        Some(capture_subscriptions),
    )
    .await
    {
        Ok(scheduler) => scheduler,
        Err(error) => {
            record_capture_start_failure(&sender, profile.name(), &error).await?;
            finish_capture(Some(sender), Some(writer)).await?;
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
    let recorded = finish_capture(Some(sender), Some(writer)).await;
    stopped?;
    wait_result?;
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

fn live_subscriptions() -> Result<Vec<Subscription>, String> {
    [
        ("engine.rpm", 200),
        ("engine.maf", 500),
        ("engine.coolant_temperature", 1_000),
        ("vehicle.speed", 1_000),
    ]
    .into_iter()
    .map(|(signal, milliseconds)| Subscription::new(signal, Duration::from_millis(milliseconds)))
    .collect()
}

fn load_layout(path: Option<&str>) -> Result<tui::DashboardLayout, String> {
    path.map_or_else(
        || Ok(tui::engine_overview()),
        |path| tui::load_layout(Path::new(path)),
    )
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
        [command, action, adapter_flag, adapter_id]
            if command == "vehicle" && action == "identify" && adapter_flag == "--adapter" =>
        {
            require_uuid(adapter_id)?;
            Ok(Command::VehicleIdentify {
                adapter_id: adapter_id.clone(),
            })
        }
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
        [command, action, path] if command == "capture" && action == "inspect" => {
            Ok(Command::CaptureInspect(path.clone()))
        }
        [command, action, path] if command == "capture" && action == "capability" => {
            Ok(Command::CaptureCapability(path.clone()))
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
            parse_command(&args(&["vehicle", "identify", "--adapter", uuid,])),
            Ok(Command::VehicleIdentify {
                adapter_id: uuid.into(),
            })
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
            parse_command(&args(&["vehicle", "identify", "--adapter", "UUID"])),
            Err("adapter must be a CoreBluetooth UUID".into())
        );
        assert_eq!(
            parse_command(&args(&["vehicle", "identify", "--adapter"])),
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
        assert_eq!(scheduled[0].interval(), Duration::from_millis(250));
        assert_eq!(decisions.len(), 15);
        assert_eq!(decisions[0].filter(), SubscriptionFilterOutcome::Scheduled);
        assert_eq!(
            decisions[1].filter(),
            SubscriptionFilterOutcome::Unsupported
        );
        assert_eq!(decisions[2].filter(), SubscriptionFilterOutcome::Unknown);
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
}
