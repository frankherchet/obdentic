use obdentic::{
    capture_events::{
        CaptureEvent, DiagnosticJobStepStatus, DtcObservationFact, ResponderEvidence,
    },
    diagnostic_job::{DiagnosticJob, DiagnosticScope},
    dtc::{decode_mode03, DtcResponse, ResponderIdentity, ResponseEvidence},
    jsonl_capture::{self, CaptureStatus, JsonlRecorder},
    runtime_actor,
    runtime_reducer::{self, RuntimeEvent, TransitionError},
    runtime_state::{Activity, Phase, RuntimeState},
    safety::{OperationKind, OperationRequest, SafetyError, SafetyPolicy},
};

struct ScriptedMode03Transport {
    commands: Vec<[u8; 1]>,
    responses: Vec<ResponseEvidence>,
}

impl ScriptedMode03Transport {
    fn read_stored(&mut self) -> Vec<ResponseEvidence> {
        self.commands.push([0x03]);
        self.responses.clone()
    }
}

#[test]
fn reducer_covers_read_observe_diagnose_shutdown_and_fault_lifecycles() {
    let init = RuntimeState::default();
    let ready = runtime_reducer::transition(&init, RuntimeEvent::InitializationCompleted).unwrap();
    assert_eq!(ready.identity(), (Phase::Ready, Activity::Idle));

    let reading = runtime_reducer::transition(&ready, RuntimeEvent::ReadStarted).unwrap();
    assert_eq!(reading.identity(), (Phase::Ready, Activity::Read));
    let idle = runtime_reducer::transition(&reading, RuntimeEvent::ReadFailedRecoverable).unwrap();
    assert_eq!(idle.identity(), (Phase::Ready, Activity::Idle));

    let observing = runtime_reducer::transition(&idle, RuntimeEvent::ObservationStarted).unwrap();
    let observed_read =
        runtime_reducer::transition(&observing, RuntimeEvent::ObservationReadStarted).unwrap();
    assert_eq!(observed_read.identity(), (Phase::Ready, Activity::Observe));
    let observed = runtime_reducer::transition(
        &observed_read,
        RuntimeEvent::ObservationReadFailedRecoverable,
    )
    .unwrap();
    assert_eq!(observed.identity(), (Phase::Ready, Activity::Observe));
    let idle = runtime_reducer::transition(&observed, RuntimeEvent::ObservationStopped).unwrap();

    let diagnosing =
        runtime_reducer::transition(&idle, RuntimeEvent::DiagnosticJobStarted).unwrap();
    assert_eq!(diagnosing.identity(), (Phase::Ready, Activity::Diagnose));
    let idle =
        runtime_reducer::transition(&diagnosing, RuntimeEvent::DiagnosticJobCompleted).unwrap();
    assert_eq!(idle.identity(), (Phase::Ready, Activity::Idle));

    let stopping = runtime_reducer::transition(&idle, RuntimeEvent::ShutdownRequested).unwrap();
    let stopped = runtime_reducer::transition(&stopping, RuntimeEvent::ShutdownCompleted).unwrap();
    assert_eq!(stopped.identity(), (Phase::Stopped, Activity::Idle));

    let fault = runtime_reducer::transition(&init, RuntimeEvent::InitializationFailed).unwrap();
    assert_eq!(fault.identity(), (Phase::Fault, Activity::Idle));
    assert_eq!(
        runtime_reducer::transition(&fault, RuntimeEvent::ReadStarted),
        Err(TransitionError::InvalidOrder {
            phase: Phase::Fault,
            activity: Activity::Idle,
            event: RuntimeEvent::ReadStarted.kind(),
        })
    );
}

#[tokio::test]
async fn actor_serializes_lifecycle_and_shutdown() {
    let (client, task) = runtime_actor::start();
    assert_eq!(
        client.snapshot().await.unwrap().state().identity(),
        (Phase::Init, Activity::Idle)
    );
    assert_eq!(
        client
            .send(RuntimeEvent::InitializationCompleted)
            .await
            .unwrap()
            .activity(),
        Activity::Idle
    );
    assert_eq!(
        client
            .send(RuntimeEvent::DiagnosticJobStarted)
            .await
            .unwrap()
            .activity(),
        Activity::Diagnose
    );
    assert_eq!(
        client
            .send(RuntimeEvent::DiagnosticJobCompleted)
            .await
            .unwrap()
            .activity(),
        Activity::Idle
    );
    client.shutdown().await.unwrap();
    assert_eq!(client.snapshot().await.unwrap().phase(), Phase::Stopped);
    drop(client);
    task.await.unwrap();
}

#[test]
fn read_only_safety_rejects_mutation_and_raw_injection() {
    let policy = SafetyPolicy::read_only();
    for request in [
        OperationRequest::ClearDtcs,
        OperationRequest::SecurityAccess,
        OperationRequest::RawCanInjection,
        OperationRequest::RawUdsInjection,
        OperationRequest::RawElmInjection,
    ] {
        assert!(policy.authorize(request).is_err());
    }
    assert_eq!(
        policy.authorize(OperationRequest::ClearDtcs),
        Err(SafetyError::OperationBlocked(OperationKind::DtcClear))
    );
}

#[test]
fn mode03_decode_keeps_responder_scoped_results_separate() {
    let first = ResponderIdentity::new("7E8").unwrap();
    let second = ResponderIdentity::new("7E9").unwrap();
    let result = decode_mode03(&[
        ResponseEvidence::new(Some(second), [0x43, 0x40, 0x00]),
        ResponseEvidence::new(Some(first), [0x43, 0x01, 0x0c]),
        ResponseEvidence::unknown([0x43, 0x00, 0x00]),
    ]);
    assert_eq!(result.observations().len(), 3);
    let response = |responder| {
        result
            .observations()
            .iter()
            .find(|observation| {
                observation
                    .source()
                    .responder()
                    .is_some_and(|id| id.as_str() == responder)
            })
            .unwrap()
            .response()
    };
    assert_eq!(response("7E8").dtcs()[0].to_string(), "P010C");
    assert_eq!(response("7E9").dtcs()[0].to_string(), "C0000");
    assert!(result
        .observations()
        .iter()
        .any(|observation| observation.response() == &DtcResponse::NoDtcs));
}

#[tokio::test]
async fn jsonl_round_trip_keeps_runtime_and_diagnostic_events_ordered() {
    let path = std::env::temp_dir().join(format!(
        "obdentic-m2-6-acceptance-{}.jsonl",
        std::process::id()
    ));
    let recorder = JsonlRecorder::start(&path).unwrap();
    let (client, actor) = runtime_actor::start();
    let initialized = client
        .send(RuntimeEvent::InitializationCompleted)
        .await
        .unwrap();
    let job = DiagnosticJob::dtc_scan(DiagnosticScope::vehicle_wide());
    let diagnosing = client
        .send(RuntimeEvent::DiagnosticJobStarted)
        .await
        .unwrap();
    let mut transport = ScriptedMode03Transport {
        commands: Vec::new(),
        responses: vec![ResponseEvidence::new(
            Some(ResponderIdentity::new("7E8").unwrap()),
            [0x43, 0x01, 0x0c],
        )],
    };
    let evidence = transport.read_stored();
    assert_eq!(transport.commands, [[0x03]]);
    let decoded = decode_mode03(&evidence);
    assert_eq!(
        decoded.observations()[0].response().dtcs()[0].to_string(),
        "P010C"
    );
    let completed = client
        .send(RuntimeEvent::DiagnosticJobCompleted)
        .await
        .unwrap();
    let events = vec![
        CaptureEvent::runtime_transition(
            initialized.sequence(),
            initialized.from(),
            initialized.to(),
            initialized.event(),
        ),
        CaptureEvent::runtime_transition(
            diagnosing.sequence(),
            diagnosing.from(),
            diagnosing.to(),
            diagnosing.event(),
        ),
        CaptureEvent::diagnostic_job_started(&job),
        CaptureEvent::responses_observed(
            "dtc.scan",
            vec![0x03],
            vec![ResponderEvidence::new(Some("7E8".into()), vec![0x43, 0x01, 0x0c]).unwrap()],
            None,
            None,
        )
        .unwrap(),
        CaptureEvent::diagnostic_job_step(
            "dtc.scan",
            0,
            0x03,
            Some("7E8".into()),
            DiagnosticJobStepStatus::Success,
            None,
        )
        .unwrap(),
        CaptureEvent::dtc_observation(
            "dtc.scan",
            0,
            Some("7E8".into()),
            DtcObservationFact::DtcCode("P010C".into()),
            "obdii.mode03",
            "SAE J1979 Mode 03",
        )
        .unwrap(),
        CaptureEvent::runtime_transition(
            completed.sequence(),
            completed.from(),
            completed.to(),
            completed.event(),
        ),
        CaptureEvent::DiagnosticJobCompleted {
            job_id: "dtc.scan".into(),
            status: obdentic::diagnostic_job::JobStatus::Completed,
        },
    ];
    for event in events.iter().cloned() {
        recorder.sender().send(event).await.unwrap();
    }
    recorder.close().await.unwrap();

    let parsed = jsonl_capture::read(&path).unwrap();
    assert_eq!(parsed.status, CaptureStatus::Partial);
    assert_eq!(parsed.events, events);
    assert!(matches!(
        parsed.events.first(),
        Some(CaptureEvent::RuntimeStateChanged {
            transition_sequence: 1,
            ..
        })
    ));
    let mut replayed = RuntimeState::default();
    for event in &parsed.events {
        if let CaptureEvent::RuntimeStateChanged {
            from, to, event, ..
        } = event
        {
            assert_eq!(*from, replayed);
            replayed = runtime_reducer::transition(&replayed, *event).unwrap();
            assert_eq!(*to, replayed);
        }
    }
    assert_eq!(replayed.identity(), (Phase::Ready, Activity::Idle));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(!contents.contains("VIN"));
    assert!(!contents.contains("device_id"));
    assert!(!contents.contains("raw_command"));
    std::fs::remove_file(path).unwrap();
    drop(client);
    actor.await.unwrap();
}
