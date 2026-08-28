pub use obdentic::{capability, subscription_policy, tui};

#[path = "../layout_observation.rs"]
mod layout_observation;

use layout_observation::{polling_plan, LayoutFreshnessPolicy};
use obdentic::{
    capability::HardwareCapability, subscription_policy::SubscriptionPolicy, supported_signals,
};
use std::{env, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let layout = match args.as_slice() {
        [] => tui::engine_overview(),
        [path] => tui::load_layout(Path::new(path))?,
        _ => return Err("usage: obdentic-layout-plan [layout.tsv]".into()),
    };
    let supported = supported_signals()
        .iter()
        .map(|signal| signal.metadata().semantic)
        .collect::<Vec<_>>();
    let plan = polling_plan(
        &layout,
        LayoutFreshnessPolicy::default(),
        SubscriptionPolicy::new(HardwareCapability::conservative_default()),
        supported,
    )?;

    println!("semantic\trequested_ms\teffective_ms\tstatus\treason\tsources");
    for entry in plan.entries() {
        let effective = entry
            .effective_interval()
            .map(|interval| interval.as_millis().to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{}\t{}\t{}\t{:?}\t{:?}\t{}",
            entry.semantic(),
            entry.requested_interval().as_millis(),
            effective,
            entry.status(),
            entry.reason(),
            entry.sources().join(",")
        );
    }
    Ok(())
}
