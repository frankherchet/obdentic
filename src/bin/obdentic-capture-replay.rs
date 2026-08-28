use obdentic::{capture_replay::CaptureReplay, jsonl_capture, tui};
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
        "capture replay  reads={} issues={} duration={:.6}s first_us={} last_us={}",
        replay.transactions().len(),
        replay.issues().len(),
        replay.duration_us() as f64 / 1_000_000.0,
        replay.offsets_us().first().copied().unwrap_or(0),
        replay.offsets_us().last().copied().unwrap_or(0),
    );
    for issue in replay.issues().iter().take(10) {
        println!(
            "replay issue  {:.6}s {:?} {} {}",
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
