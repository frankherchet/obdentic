use obdentic::{ecu_identification::EcuIdentificationPlan, hex, knowledge_db::KnowledgeCatalog};
use std::{env, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let project_root = args
        .next()
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into());
    if args.next().is_some() {
        return Err("usage: obdentic-ecu-identification-plan [project-root]".into());
    }

    let catalog = KnowledgeCatalog::load_pinned(Path::new(&project_root))
        .map_err(|error| error.to_string())?;
    let plan = EcuIdentificationPlan::from_catalog(&catalog)?;

    println!("bounded ECU identification dry-run");
    println!("transport\tdisabled");
    println!("knowledge_repository\t{}", plan.knowledge_repository());
    println!("knowledge_revision\t{}", plan.knowledge_revision());
    println!("knowledge_schema\t{}", plan.knowledge_schema_version());
    println!("set\t{}@{}", plan.set_id(), plan.set_version());
    for candidate in plan.candidates() {
        println!(
            "candidate\t{}\t{}@{}\tDID {:04X}\t{}",
            candidate.semantic(),
            candidate.definition_id(),
            candidate.definition_version(),
            candidate.did(),
            hex(&candidate.request_bytes())
        );
    }
    Ok(())
}
