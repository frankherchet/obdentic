from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}\n--- needle ---\n{old}")
    file.write_text(text.replace(old, new, 1))


# ---------------------------------------------------------------------------
# BLE transport: explicitly establish the ELM auto protocol with one bounded
# 01 00 request before the semantic Mode 03 job starts.
# ---------------------------------------------------------------------------
replace_once(
    "src/ble.rs",
    '''#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportDiscovery {
    pub request: [u8; 2],
    pub responder: Option<ResponderIdentity>,
    pub response: [u8; 6],
}
''',
    '''#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportDiscovery {
    pub request: [u8; 2],
    pub responder: Option<ResponderIdentity>,
    pub response: [u8; 6],
}

/// Bounded read-only evidence produced while an ELM327 using `ATSP0`
/// establishes the vehicle protocol. This is transport preparation, not a
/// semantic diagnostic-job step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolNegotiation {
    observations: Vec<SupportDiscovery>,
}

impl ProtocolNegotiation {
    pub fn observations(&self) -> &[SupportDiscovery] {
        &self.observations
    }
}

/// One initialized session whose ELM auto protocol is already established.
/// The only vehicle request performed before construction is the closed,
/// read-only `01 00` negotiation probe.
pub struct PreparedDiagnosticSession {
    session: SessionClient,
    negotiation: ProtocolNegotiation,
}

impl PreparedDiagnosticSession {
    pub fn negotiation(&self) -> &ProtocolNegotiation {
        &self.negotiation
    }

    /// Execute the one bounded stored-DTC request and deterministically close
    /// the physical session afterwards.
    pub async fn read_stored_dtcs(self) -> Result<DiagnosticResponses, String> {
        let result = self.session.read_stored_dtcs().await;
        match (result, self.session.shutdown().await) {
            (Ok(responses), Ok(())) => Ok(responses),
            (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
        }
    }

    pub async fn shutdown(self) -> Result<(), String> {
        self.session.shutdown().await
    }
}
''',
)

replace_once(
    "src/ble.rs",
    '''/// Read stored emission-related DTCs through one initialized functional ELM
/// session.  The command and addressing are fixed by this API.
pub async fn read_stored_dtcs(adapter_id: &str) -> Result<DiagnosticResponses, String> {
    let session = start_session_mode(adapter_id, false).await?;
    let result = session.read_stored_dtcs().await;
    match (result, session.shutdown().await) {
        (Ok(responses), Ok(())) => Ok(responses),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}
''',
    '''/// Connect and initialize an ELM327 session, then establish the automatic
/// vehicle protocol with exactly one bounded, standards-based `01 00` probe.
/// The returned session has not yet executed a diagnostic job request.
pub async fn prepare_diagnostic_session(
    adapter_id: &str,
) -> Result<PreparedDiagnosticSession, String> {
    let mut session =
        DiagnosticSession::connect_with_adapter_io_mode(adapter_id, false, false).await?;
    let negotiation = match session.establish_protocol().await {
        Ok(negotiation) => negotiation,
        Err(error) => {
            return match session.disconnect().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!("{error}; cleanup failed: {cleanup}")),
            };
        }
    };
    Ok(PreparedDiagnosticSession {
        session: start_session_actor(session),
        negotiation,
    })
}

/// Compatibility helper for callers that do not need the explicit protocol
/// evidence. The semantic DTC request remains exactly one Mode 03 command.
pub async fn read_stored_dtcs(adapter_id: &str) -> Result<DiagnosticResponses, String> {
    prepare_diagnostic_session(adapter_id)
        .await?
        .read_stored_dtcs()
        .await
}
''',
)

replace_once(
    "src/ble.rs",
    '''async fn start_session_mode(
    adapter_id: &str,
    discover_support: bool,
) -> Result<SessionClient, String> {
    let session =
        DiagnosticSession::connect_with_adapter_io_mode(adapter_id, false, discover_support)
            .await?;
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(session_actor(session, receiver));
    Ok(SessionClient { sender })
}
''',
    '''async fn start_session_mode(
    adapter_id: &str,
    discover_support: bool,
) -> Result<SessionClient, String> {
    let session =
        DiagnosticSession::connect_with_adapter_io_mode(adapter_id, false, discover_support)
            .await?;
    Ok(start_session_actor(session))
}

fn start_session_actor(session: DiagnosticSession) -> SessionClient {
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(session_actor(session, receiver));
    SessionClient { sender }
}
''',
)

replace_once(
    "src/ble.rs",
    '''    async fn validate_functional_support(&mut self) -> Result<Vec<SupportDiscovery>, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        validate_functional_support_exchange(&mut exchange).await
    }

    async fn read_with_evidence(&mut self, request: ReadRequest) -> ReadOutcome {
''',
    '''    async fn validate_functional_support(&mut self) -> Result<Vec<SupportDiscovery>, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        validate_functional_support_exchange(&mut exchange).await
    }

    async fn establish_protocol(&mut self) -> Result<ProtocolNegotiation, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        establish_elm_protocol(&mut exchange).await
    }

    async fn read_with_evidence(&mut self, request: ReadRequest) -> ReadOutcome {
''',
)

replace_once(
    "src/ble.rs",
    '''async fn validate_functional_support_exchange<E>(
    exchange: &mut E,
) -> Result<Vec<SupportDiscovery>, String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("0100\\r", COMMAND_TIMEOUT).await?;
    normalize_pid_support_page_with_evidence(&response, 0x00).map(|(_, observations)| observations)
}
''',
    '''async fn validate_functional_support_exchange<E>(
    exchange: &mut E,
) -> Result<Vec<SupportDiscovery>, String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("0100\\r", COMMAND_TIMEOUT).await?;
    normalize_pid_support_page_with_evidence(&response, 0x00).map(|(_, observations)| observations)
}

/// Force `ATSP0` auto-selection to finish before a semantic diagnostic job.
/// `01 00` is a fixed standards-based read-only probe already used elsewhere
/// for functional support validation; no caller-supplied payload is accepted.
async fn establish_elm_protocol<E>(exchange: &mut E) -> Result<ProtocolNegotiation, String>
where
    E: ElmExchange,
{
    let observations = validate_functional_support_exchange(exchange).await?;
    Ok(ProtocolNegotiation { observations })
}
''',
)

replace_once(
    "src/ble.rs",
    '''    #[tokio::test]
    async fn stored_dtc_transport_uses_only_the_bounded_mode03_command() {
        let mut exchange = ScriptedExchange::captured(vec!["43 01 0C\\r>".into()]);

        let responses = read_elm_mode03_responses(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["03\\r"]);
        assert_eq!(responses.as_slice()[0].payload, [0x43, 0x01, 0x0c]);
    }
''',
    '''    #[tokio::test]
    async fn stored_dtc_transport_uses_only_the_bounded_mode03_command() {
        let mut exchange = ScriptedExchange::captured(vec!["43 01 0C\\r>".into()]);

        let responses = read_elm_mode03_responses(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["03\\r"]);
        assert_eq!(responses.as_slice()[0].payload, [0x43, 0x01, 0x0c]);
    }

    #[tokio::test]
    async fn protocol_negotiation_keeps_hardware_0100_evidence_out_of_mode03() {
        let mut exchange = ScriptedExchange::captured(vec![
            "SEARCHING...\\r7E8 06 41 00 98 3B A0 13 00\\r7E9 06 41 00 98 18 00 01 AA\\r>".into(),
            "7E8 03 43 00 00\\r7E9 03 43 01 0C\\r>".into(),
        ]);

        let negotiation = establish_elm_protocol(&mut exchange).await.unwrap();
        let responses = read_elm_mode03_responses(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["0100\\r", "03\\r"]);
        assert_eq!(negotiation.observations().len(), 2);
        assert_eq!(
            negotiation.observations()[0],
            SupportDiscovery {
                request: [0x01, 0x00],
                responder: Some(ResponderIdentity::ElmHeader("7E8".into())),
                response: [0x41, 0x00, 0x98, 0x3b, 0xa0, 0x13],
            }
        );
        assert_eq!(
            negotiation.observations()[1],
            SupportDiscovery {
                request: [0x01, 0x00],
                responder: Some(ResponderIdentity::ElmHeader("7E9".into())),
                response: [0x41, 0x00, 0x98, 0x18, 0x00, 0x01],
            }
        );
        assert_eq!(responses.errors(), &[]);
        assert!(responses
            .as_slice()
            .iter()
            .all(|response| response.payload.first() == Some(&0x43)));
    }

    #[tokio::test]
    async fn failed_protocol_negotiation_never_dispatches_mode03() {
        let mut exchange = ScriptedExchange::captured(vec!["NO DATA\\r>".into()]);

        assert!(establish_elm_protocol(&mut exchange).await.is_err());
        assert_eq!(exchange.commands, ["0100\\r"]);
    }
''',
)

# ---------------------------------------------------------------------------
# Capture vocabulary: keep protocol-establishment evidence distinct from both
# ordinary support discovery and semantic DTC response/fact evidence.
# ---------------------------------------------------------------------------
replace_once(
    "src/capture_events.rs",
    '''    SupportDiscovery {
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    },
    /// Preserves every normalized response before semantic selection. This
''',
    '''    SupportDiscovery {
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    },
    /// Evidence from the fixed read-only `01 00` probe used only to make an
    /// ELM327 `ATSP0` protocol choice explicit before a diagnostic job.
    ProtocolNegotiationObserved {
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    },
    /// Preserves every normalized response before semantic selection. This
''',
)

replace_once(
    "src/capture_events.rs",
    '''    pub fn support_discovery_with_responder(
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    ) -> Self {
        Self::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        }
    }

    pub fn responses_observed(
''',
    '''    pub fn support_discovery_with_responder(
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    ) -> Self {
        Self::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        }
    }

    pub fn protocol_negotiation_observed(
        request_payload: Vec<u8>,
        responder: Option<String>,
        response_payload: Vec<u8>,
    ) -> Result<Self, String> {
        if request_payload.as_slice() != [0x01, 0x00] {
            return Err("protocol negotiation request must be OBD-II 01 00".into());
        }
        if response_payload.len() != 6
            || response_payload[0] != 0x41
            || response_payload[1] != 0x00
        {
            return Err("protocol negotiation evidence must be a normalized 41 00 response".into());
        }
        validate_diagnostic_source(responder.as_deref())?;
        Ok(Self::ProtocolNegotiationObserved {
            request_payload,
            responder,
            response_payload,
        })
    }

    pub fn responses_observed(
''',
)

# JSONL serializer/parser for the new backward-compatible event type.
replace_once(
    "src/jsonl_capture.rs",
    '''        CaptureEvent::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        } => {
            object.push_str("\\\"support_discovery\\\",\\\"sequence\\\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\\\"request_payload\\\":");
            push_string(&mut object, &hex(request_payload));
            object.push_str(",\\\"responder\\\":");
            push_option_string(&mut object, responder.as_deref());
            object.push_str(",\\\"response_payload\\\":");
            push_string(&mut object, &hex(response_payload));
        }
        CaptureEvent::ResponsesObserved {
''',
    '''        CaptureEvent::SupportDiscovery {
            request_payload,
            responder,
            response_payload,
        } => {
            object.push_str("\\\"support_discovery\\\",\\\"sequence\\\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\\\"request_payload\\\":");
            push_string(&mut object, &hex(request_payload));
            object.push_str(",\\\"responder\\\":");
            push_option_string(&mut object, responder.as_deref());
            object.push_str(",\\\"response_payload\\\":");
            push_string(&mut object, &hex(response_payload));
        }
        CaptureEvent::ProtocolNegotiationObserved {
            request_payload,
            responder,
            response_payload,
        } => {
            object.push_str("\\\"protocol_negotiation_observed\\\",\\\"sequence\\\":");
            object.push_str(&sequence.to_string());
            object.push_str(",\\\"request_payload\\\":");
            push_string(&mut object, &hex(request_payload));
            object.push_str(",\\\"responder\\\":");
            push_option_string(&mut object, responder.as_deref());
            object.push_str(",\\\"response_payload\\\":");
            push_string(&mut object, &hex(response_payload));
        }
        CaptureEvent::ResponsesObserved {
''',
)

replace_once(
    "src/jsonl_capture.rs",
    '''        "responses_observed" => {
''',
    '''        "protocol_negotiation_observed" => {
            fields_exact(
                object,
                &[
                    "schema",
                    "version",
                    "type",
                    "sequence",
                    "request_payload",
                    "responder",
                    "response_payload",
                ],
                line_number,
            )?;
            CaptureEvent::protocol_negotiation_observed(
                parse_hex(
                    &string_field(object, "request_payload", line_number)?,
                    line_number,
                )?,
                optional_string_field(object, "responder", line_number)?,
                parse_hex(
                    &string_field(object, "response_payload", line_number)?,
                    line_number,
                )?,
            )
            .map_err(|error| format!("line {line_number}: {error}"))
        }
        "responses_observed" => {
''',
)

replace_once(
    "src/jsonl_capture.rs",
    '''    #[tokio::test]
    async fn round_trips_dtc_facts_without_raw_command_fields() {
''',
    '''    #[tokio::test]
    async fn round_trips_protocol_negotiation_as_separate_responder_evidence() {
        let path = temp_path("protocol-negotiation");
        let (sender, writer) = start(&path).unwrap();
        let expected = vec![
            CaptureEvent::protocol_negotiation_observed(
                vec![0x01, 0x00],
                Some("7E8".into()),
                vec![0x41, 0x00, 0x98, 0x3b, 0xa0, 0x13],
            )
            .unwrap(),
            CaptureEvent::protocol_negotiation_observed(
                vec![0x01, 0x00],
                Some("7E9".into()),
                vec![0x41, 0x00, 0x98, 0x18, 0x00, 0x01],
            )
            .unwrap(),
        ];
        for event in expected.iter().cloned() {
            sender.send(event).await.unwrap();
        }
        finish(sender, writer).await;

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\\\"type\\\":\\\"protocol_negotiation_observed\\\""));
        assert!(contents.contains("\\\"request_payload\\\":\\\"01 00\\\""));
        assert!(!contents.contains("dtc_observation"));
        assert_eq!(read_events(&path).unwrap(), expected);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn round_trips_dtc_facts_without_raw_command_fields() {
''',
)

# capture_report has an exhaustive match and should count negotiation as lifecycle
# evidence without interpreting the support bitmap as a diagnostic fact.
replace_once(
    "src/capture_report.rs",
    '''            CaptureEvent::SessionInitialized
            | CaptureEvent::SupportDiscovery { .. }
            | CaptureEvent::RuntimeStateChanged { .. }
''',
    '''            CaptureEvent::SessionInitialized
            | CaptureEvent::SupportDiscovery { .. }
            | CaptureEvent::ProtocolNegotiationObserved { .. }
            | CaptureEvent::RuntimeStateChanged { .. }
''',
)

# ---------------------------------------------------------------------------
# CLI/job orchestration: negotiation runs while ready.idle, is recorded, then
# the actor enters ready.diagnose and exactly one semantic Mode 03 step runs.
# ---------------------------------------------------------------------------
replace_once(
    "src/main.rs",
    '''    apply_runtime_event(
        runtime,
        state,
        recorder,
        RuntimeEvent::transport(TransportState::Connecting),
    )
    .await?;
    apply_runtime_event(runtime, state, recorder, RuntimeEvent::DiagnosticJobStarted).await?;
    emit_capture_event(recorder, CaptureEvent::diagnostic_job_started(&job)).await?;

    match ble::read_stored_dtcs(adapter_id).await {
''',
    '''    apply_runtime_event(
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
''',
)

replace_once(
    "src/main.rs",
    '''async fn record_dtc_transport_evidence(
    recorder: Option<&jsonl_capture::Sender>,
    job: &DiagnosticJob,
    responses: &ble::DiagnosticResponses,
) -> Result<(), String> {
''',
    '''async fn record_protocol_negotiation(
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
''',
)

# Acceptance fixture: protocol negotiation appears before diagnose state/job and
# survives JSONL round-trip/reducer replay as non-diagnostic evidence.
replace_once(
    "tests/m2_6_acceptance.rs",
    '''        CaptureEvent::runtime_transition(
            diagnosing.sequence(),
            diagnosing.from(),
            diagnosing.to(),
            diagnosing.event(),
        ),
''',
    '''        CaptureEvent::protocol_negotiation_observed(
            vec![0x01, 0x00],
            Some("7E8".into()),
            vec![0x41, 0x00, 0x98, 0x3b, 0xa0, 0x13],
        )
        .unwrap(),
        CaptureEvent::runtime_transition(
            diagnosing.sequence(),
            diagnosing.from(),
            diagnosing.to(),
            diagnosing.event(),
        ),
''',
)

replace_once(
    "tests/m2_6_acceptance.rs",
    '''    assert!(!contents.contains("VIN"));
    assert!(!contents.contains("device_id"));
''',
    '''    assert!(contents.contains("protocol_negotiation_observed"));
    assert!(contents.contains("01 00"));
    assert!(!contents.contains("VIN"));
    assert!(!contents.contains("device_id"));
''',
)

print("issue #56 patch applied")
