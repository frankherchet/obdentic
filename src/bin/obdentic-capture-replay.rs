pub use obdentic::{capture_events, jsonl_capture, prepare_read, telemetry, Transaction};

#[path = "../capture_replay.rs"]
mod capture_replay;

use capture_replay::CaptureReplay;
use obdentic::tui;
use std::{env, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let (capture_path, layout_path) = match args.as_slice() {
        [capture] => (capture.as_str(), None),
        [capture, flag, layout] if flag == "--layout" => (capture.as_str(), Some(layout.as_str())),
        _ => {
            return Err(
                "usage: obdentic-capture-replay <capture.jsonl> [--layout <layout.tsv>]".into(),
            )
        }
    };

    let capture = jsonl_capture::read(Path::new(capture_path))?;
    let replay = CaptureReplay::from_capture(&capture);
    if replay.transactions().is_empty() {
        let detail = replay
            .issues()
            .first()
            .map(|issue| format!("; first replay issue: {}", issue.detail()))
            .unwrap_or_default();
        return Err(format!(
            "capture contains no replayable semantic reads{detail}"
        ));
    }

    let capacity = replay.transactions().len().max(1);
    let telemetry = replay.telemetry_full(capacity)?;
    let layout = layout_path.map_or_else(
        || Ok(tui::engine_overview()),
        |path| tui::load_layout(Path::new(path)),
    )?;

    tui::run(&layout, &telemetry, replay.transactions())?;
    println!(
        "capture replay  reads={} timed_reads={} issues={} duration={:.3}s",
        replay.transactions().len(),
        replay.offsets_us().len(),
        replay.issues().len(),
        replay.duration_us() as f64 / 1_000_000.0
    );
    for issue in replay.issues().iter().take(10) {
        println!(
            "replay issue  {:.3}s {:?} {} {}",
            issue.at_us() as f64 / 1_000_000.0,
            issue.kind(),
            issue.semantic(),
            issue.detail()
        );
    }
    if replay.issues().len() > 10 {
        println!("replay issue  ... {} more", replay.issues().len() - 10);
    }
    Ok(())
}
