from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one patch anchor")
    file.write_text(text.replace(old, new, 1))


replace_once(
    "src/lib.rs",
    "pub mod ecu_identification;\npub(crate) mod elm;",
    "pub mod ecu_identification;\npub mod ecu_identification_discovery;\npub(crate) mod elm;",
)

replace_once(
    "src/ecu_identification.rs",
    "            IdentificationResultStatus::Supported => {\n                if self.value.is_none() || self.nrc.is_some() || !self.errors.is_empty() {\n                    return Err(\"supported ECU identification evidence is inconsistent\".into());\n                }\n            }",
    "            IdentificationResultStatus::Supported => {\n                if self.value.is_none() || self.nrc.is_some() {\n                    return Err(\"supported ECU identification evidence is inconsistent\".into());\n                }\n            }",
)

replace_once(
    "src/main.rs",
    "    diagnostic_job::{DiagnosticJob, DiagnosticScope, JobStatus, KnownTarget},\n    dtc, hex, jsonl_capture,\n    layout_observation::{self, LayoutFreshnessPolicy},",
    "    diagnostic_job::{DiagnosticJob, DiagnosticScope, JobStatus, KnownTarget},\n    dtc,\n    ecu_identification::EcuIdentificationPlan,\n    ecu_identification_discovery, hex, jsonl_capture,\n    knowledge_db::KnowledgeCatalog,\n    layout_observation::{self, LayoutFreshnessPolicy},",
)

replace_once(
    "src/main.rs",
    "async fn run_vehicle_discover_inner(adapter_id: &str, refresh: bool) -> Result<(), String> {\n    let identity = ble::identify(adapter_id).await?;",
    "async fn run_vehicle_discover_inner(adapter_id: &str, refresh: bool) -> Result<(), String> {\n    let catalog = KnowledgeCatalog::load_pinned(Path::new(env!(\"CARGO_MANIFEST_DIR\")))\n        .map_err(|error| error.to_string())?;\n    let identification_plan = EcuIdentificationPlan::from_catalog(&catalog)?;\n    let identity = ble::identify(adapter_id).await?;",
)

replace_once(
    "src/main.rs",
    "                    if cache.snapshot().target_mappings().is_empty() {\n                        println!(\"cache\\tmissing-engine-target; running full discovery\");\n                    } else {\n                        print_cached_vehicle_discovery(cache);\n                        return Ok(());\n                    }",
    "                    if cache.snapshot().target_mappings().is_empty() {\n                        println!(\"cache\\tmissing-engine-target; running full discovery\");\n                    } else if cache.snapshot().ecu_identification().is_empty() {\n                        println!(\"cache\\tmissing-ecu-identification; running full discovery\");\n                    } else {\n                        print_cached_vehicle_discovery(cache);\n                        return Ok(());\n                    }",
)

replace_once(
    "src/main.rs",
    "    let target_mapping = match &discovery {\n        Ok(discovery) => validate_engine_target(&session, discovery).await,\n        Err(_) => Ok(None),\n    };\n    let shutdown = session.shutdown().await;\n    let discovery = discovery?;\n    let target_mapping = target_mapping?;\n    shutdown?;",
    "    let target_mapping = match &discovery {\n        Ok(discovery) => validate_engine_target(&session, discovery).await,\n        Err(_) => Ok(None),\n    };\n    let ecu_identification = match &target_mapping {\n        Ok(Some(mapping)) => {\n            ecu_identification_discovery::discover_known_ecus(\n                &session,\n                &identification_plan,\n                std::slice::from_ref(mapping),\n            )\n            .await\n        }\n        Ok(None) | Err(_) => Ok(Vec::new()),\n    };\n    let shutdown = session.shutdown().await;\n    let discovery = discovery?;\n    let target_mapping = target_mapping?;\n    let ecu_identification = ecu_identification?;\n    shutdown?;",
)

replace_once(
    "src/main.rs",
    "    let snapshot = obdentic::vehicle_cache::VehicleCacheSnapshot::new(\n        base_snapshot.topology().to_vec(),\n        base_snapshot.ecu_capabilities().to_vec(),\n        target_mapping,\n    );",
    "    let snapshot = obdentic::vehicle_cache::VehicleCacheSnapshot::with_ecu_identification(\n        base_snapshot.topology().to_vec(),\n        base_snapshot.ecu_capabilities().to_vec(),\n        target_mapping,\n        ecu_identification,\n    );",
)

elm = Path("src/elm.rs")
text = elm.read_text()
if not text.endswith("}\n"):
    raise SystemExit("src/elm.rs: unexpected file ending")
negative_test = r'''

    #[tokio::test]
    async fn canonical_ecu_identification_preserves_negative_response_payload() {
        let catalog =
            crate::knowledge_db::KnowledgeCatalog::load_pinned(env!("CARGO_MANIFEST_DIR")).unwrap();
        let plan =
            crate::ecu_identification::EcuIdentificationPlan::from_catalog(&catalog).unwrap();
        let candidate = plan
            .candidates()
            .iter()
            .find(|candidate| candidate.did() == 0xF189)
            .unwrap();
        let context = crate::topology::ProtocolContext::new(
            crate::topology::Protocol::Obd2,
            crate::topology::AddressingContext::Physical,
        );
        let target = crate::topology::RequestTargetEvidence::new(
            crate::topology::RequestTarget::concrete(
                context.clone(),
                crate::topology::RequestAddress::new("elm-header", "7E0"),
            ),
            crate::topology::Provenance::new("test target", crate::topology::Confidence::High)
                .unwrap(),
        );
        let responder = crate::topology::ResponderIdentity::address(context, "7E8");
        let request =
            TargetedEcuIdentificationRequest::from_evidence(candidate, &target, &responder)
                .unwrap();
        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 03 7F 22 31 55 55\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);
        let mut session = ElmSession::new(exchange);

        let read = session
            .read_ecu_identification_with_evidence(&request)
            .await
            .unwrap();

        assert_eq!(read.responses.as_slice()[0].payload, [0x7f, 0x22, 0x31]);
        assert!(read.responses.errors().is_empty());
        assert_eq!(
            session.into_exchange().commands,
            [
                "ATSH 7E0\r",
                "ATCRA 7E8\r",
                "22F189\r",
                "ATSP0\r",
                "ATSH 7DF\r",
                "ATCRA\r",
            ]
        );
    }
'''
elm.write_text(text[:-2] + negative_test + "}\n")
