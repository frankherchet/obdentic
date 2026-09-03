use obdentic::knowledge_db::{KnowledgeCatalog, STANDARD_UDS_ECU_IDENTIFICATION_SET};
use std::{env, path::Path};

const VIN_DID: u16 = 0xF190;

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlannedIdentification {
    semantic: String,
    definition_id: String,
    definition_version: u32,
    did: u16,
    request: [u8; 3],
}

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
    let plan = bounded_identification_plan(&catalog)?;

    println!("bounded ECU identification dry-run");
    println!("transport\tdisabled");
    println!("knowledge_repository\t{}", catalog.pin().repository());
    println!("knowledge_revision\t{}", catalog.pin().revision());
    println!("knowledge_schema\t{}", catalog.pin().schema_version());
    let set = catalog
        .set(STANDARD_UDS_ECU_IDENTIFICATION_SET)
        .ok_or_else(|| {
            format!("canonical knowledge is missing set {STANDARD_UDS_ECU_IDENTIFICATION_SET:?}")
        })?;
    println!("set\t{}@{}", set.id(), set.version());
    for item in plan {
        println!(
            "candidate\t{}\t{}@{}\tDID {:04X}\t{}",
            item.semantic,
            item.definition_id,
            item.definition_version,
            item.did,
            obdentic::hex(&item.request)
        );
    }
    Ok(())
}

fn bounded_identification_plan(
    catalog: &KnowledgeCatalog,
) -> Result<Vec<PlannedIdentification>, String> {
    let set = catalog
        .set(STANDARD_UDS_ECU_IDENTIFICATION_SET)
        .ok_or_else(|| {
            format!("canonical knowledge is missing set {STANDARD_UDS_ECU_IDENTIFICATION_SET:?}")
        })?;

    set.members()
        .iter()
        .map(|semantic| {
            let definition = catalog.semantic(semantic).ok_or_else(|| {
                format!(
                    "knowledge set {:?} references unresolved semantic {semantic:?}",
                    set.id()
                )
            })?;
            let operation = definition.operation();
            let did = operation.did();
            if did == VIN_DID {
                return Err("VIN/F190 must not enter bounded ECU identification discovery".into());
            }
            let request = operation.request_bytes();
            if request[0] != 0x22 || u16::from_be_bytes([request[1], request[2]]) != did {
                return Err(format!(
                    "knowledge definition {:?} did not resolve to a typed UDS ReadDataByIdentifier request",
                    definition.id()
                ));
            }
            Ok(PlannedIdentification {
                semantic: semantic.clone(),
                definition_id: definition.id().into(),
                definition_version: definition.version(),
                did,
                request,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> KnowledgeCatalog {
        KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap()
    }

    #[test]
    fn plan_is_exactly_the_pinned_canonical_set_in_declared_order() {
        let catalog = catalog();
        let set = catalog.set(STANDARD_UDS_ECU_IDENTIFICATION_SET).unwrap();
        let plan = bounded_identification_plan(&catalog).unwrap();

        assert_eq!(
            plan.iter()
                .map(|item| item.semantic.as_str())
                .collect::<Vec<_>>(),
            set.members().iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn plan_contains_only_typed_rdbi_requests_and_excludes_vin() {
        let plan = bounded_identification_plan(&catalog()).unwrap();

        assert!(!plan.is_empty());
        for item in plan {
            assert_ne!(item.did, VIN_DID);
            assert_eq!(item.request[0], 0x22);
            assert_eq!(
                u16::from_be_bytes([item.request[1], item.request[2]]),
                item.did
            );
        }
    }

    #[test]
    fn f189_resolves_through_canonical_knowledge_without_a_second_did_list() {
        let plan = bounded_identification_plan(&catalog()).unwrap();
        let item = plan
            .iter()
            .find(|item| item.semantic == "ecu.manufacturer_software_version")
            .unwrap();

        assert_eq!(item.definition_id, "uds.f189.manufacturer_software_version");
        assert_eq!(item.did, 0xF189);
        assert_eq!(item.request, [0x22, 0xF1, 0x89]);
    }

    #[test]
    fn repeated_planning_is_deterministic() {
        let catalog = catalog();
        assert_eq!(
            bounded_identification_plan(&catalog).unwrap(),
            bounded_identification_plan(&catalog).unwrap()
        );
    }
}
