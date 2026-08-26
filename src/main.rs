use obdentic::{
    ble, hex, prepare_read, record, replay, supported_signals, telemetry::TelemetryState, tui,
    ReadRequest, Transaction,
};
use std::{env, path::Path};

const USAGE: &str = "usage: obdentic signals | obdentic scan | obdentic read <signal> --adapter <CoreBluetooth UUID> [--record recording.tsv] | obdentic demo | obdentic replay <recording.tsv> | obdentic tui demo | obdentic tui replay <recording.tsv>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Signals,
    Scan,
    Demo,
    Read {
        request: ReadRequest,
        adapter_id: String,
        recording: Option<String>,
    },
    Replay(String),
    TuiDemo,
    TuiReplay(String),
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
        Command::TuiDemo => {
            let transactions = demo_samples()?;
            tui::run(&telemetry(&transactions)?, &transactions)?;
        }
        Command::TuiReplay(path) => {
            let transactions = [replay(Path::new(&path)).await?];
            tui::run(&telemetry(&transactions)?, &transactions)?;
        }
    }
    Ok(())
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    match args {
        [command] if command == "signals" => Ok(Command::Signals),
        [command] if command == "scan" => Ok(Command::Scan),
        [command] if command == "demo" => Ok(Command::Demo),
        [command, path] if command == "replay" => Ok(Command::Replay(path.clone())),
        [command, source] if command == "tui" && source == "demo" => Ok(Command::TuiDemo),
        [command, source, path] if command == "tui" && source == "replay" => {
            Ok(Command::TuiReplay(path.clone()))
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

fn escape_field(value: &str) -> String {
    value.escape_default().collect()
}

async fn demo() -> Result<Transaction, String> {
    prepare_read("engine.rpm")?.complete("user", vec![0x41, 0x0c, 0x1a, 0xf8])
}

fn demo_samples() -> Result<Vec<Transaction>, String> {
    [
        ("engine.rpm", vec![0x41, 0x0c, 0x1a, 0xf8]),
        ("engine.coolant_temperature", vec![0x41, 0x05, 0x5a]),
        ("vehicle.speed", vec![0x41, 0x0d, 0x00]),
        ("engine.maf", vec![0x41, 0x10, 0x01, 0xf4]),
    ]
    .into_iter()
    .map(|(semantic, response)| prepare_read(semantic)?.complete("demo", response))
    .collect()
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

    #[test]
    fn parses_approved_forms() {
        let uuid = "00000000-0000-4000-8000-000000000000";
        assert_eq!(parse_command(&args(&["signals"])), Ok(Command::Signals));
        assert_eq!(parse_command(&args(&["scan"])), Ok(Command::Scan));
        assert_eq!(parse_command(&args(&["demo"])), Ok(Command::Demo));
        assert_eq!(parse_command(&args(&["tui", "demo"])), Ok(Command::TuiDemo));
        assert_eq!(
            parse_command(&args(&["replay", "session.tsv"])),
            Ok(Command::Replay("session.tsv".into()))
        );
        assert_eq!(
            parse_command(&args(&["tui", "replay", "session.tsv"])),
            Ok(Command::TuiReplay("session.tsv".into()))
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
            parse_command(&args(&["demo", "session.tsv"])),
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
    fn tui_demo_uses_only_known_decoded_signals() {
        let samples = demo_samples().unwrap();
        assert_eq!(samples.len(), 4);
        assert!(samples.iter().all(|sample| sample.source() == "demo"));
        assert_eq!(samples[0].semantic(), "engine.rpm");
        assert_eq!(samples[0].value(), 1726.0);
    }
}
