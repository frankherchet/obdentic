use crate::{
    topology::AddressingContext, topology::Protocol, topology::RequestTarget, ReadRequest,
    Transaction,
};
use btleplug::{
    api::{
        bleuuid::uuid_from_u16, Central, CharPropFlags, Characteristic, Manager as _,
        Peripheral as _, ScanFilter, ValueNotification, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

const CARLY_SERVICE: u16 = 0xFFE0;
const CARLY_CHANNEL: u16 = 0xFFE1;
const SCAN_TIMEOUT: Duration = Duration::from_secs(5);
const FIND_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: usize = 8 * 1024;
const MODE03_COMMAND: &str = "03\r";
// Two consecutive transport failures stop a live session; data failures reset the count.
const TRANSPORT_FAILURE_THRESHOLD: u8 = 2;
const SESSION_UNHEALTHY_PREFIX: &str =
    "diagnostic session became unresponsive after repeated transport failures";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PidSupport {
    pages: Vec<u32>,
    discovery: Vec<SupportDiscovery>,
}

impl PidSupport {
    fn supports_pid(&self, pid: u8) -> bool {
        self.status(pid) == SignalSupportStatus::Supported
    }

    fn status(&self, pid: u8) -> SignalSupportStatus {
        if pid == 0 {
            return SignalSupportStatus::Unknown;
        }
        let index = (pid as usize - 1) / 0x20;
        let offset = (pid as usize - 1) % 0x20 + 1;
        match self.pages.get(index) {
            Some(bitmap) if bitmap & (1 << (32 - offset)) != 0 => SignalSupportStatus::Supported,
            Some(_) => SignalSupportStatus::Unsupported,
            None => SignalSupportStatus::Unknown,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AdapterCandidate {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalSupportStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSupport {
    pub semantic: &'static str,
    pub status: SignalSupportStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportDiscovery {
    pub request: [u8; 2],
    pub responder: Option<ResponderIdentity>,
    pub response: [u8; 6],
}

/// Identity exposed by ELM header output. This is deliberately not called a
/// CAN identifier: ELM may be speaking a non-CAN protocol and the text alone
/// does not prove a wire-level CAN frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResponderIdentity {
    ElmHeader(String),
}

/// A read-only request with an explicitly targeted physical OBD-II address.
/// The semantic request remains the closed `ReadRequest` vocabulary; callers
/// cannot inject an arbitrary payload through this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetedReadRequest {
    request: ReadRequest,
    target: RequestTarget,
    expected_responder: ResponderIdentity,
}

impl TargetedReadRequest {
    pub fn new(
        request: ReadRequest,
        target: RequestTarget,
        expected_responder: ResponderIdentity,
    ) -> Result<Self, String> {
        validate_request_target(&target)?;
        validate_elm_header(&expected_responder, "expected responder")?;
        Ok(Self {
            request,
            target,
            expected_responder: ResponderIdentity::ElmHeader(
                expected_responder.as_str().to_ascii_uppercase(),
            ),
        })
    }

    pub fn request(&self) -> ReadRequest {
        self.request
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn expected_responder(&self) -> &ResponderIdentity {
        &self.expected_responder
    }
}

impl ResponderIdentity {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ElmHeader(header) => header,
        }
    }
}

fn validate_request_target(target: &RequestTarget) -> Result<(), String> {
    if target.context().protocol() != &Protocol::Obd2
        || target.context().addressing() != &AddressingContext::Physical
    {
        return Err("targeted reads require physical OBD-II addressing".into());
    }
    let address = target
        .address()
        .ok_or_else(|| "targeted reads require a concrete request address".to_string())?;
    if address.namespace() != "elm-header" {
        return Err("targeted reads require an elm-header request address".into());
    }
    validate_header_value(address.value(), "request address")
}

fn validate_elm_header(identity: &ResponderIdentity, label: &str) -> Result<(), String> {
    validate_header_value(identity.as_str(), label)
}

fn validate_header_value(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{label} must be exactly three hexadecimal characters"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticResponse {
    pub responder: Option<ResponderIdentity>,
    pub payload: Vec<u8>,
}

/// A recoverable issue attached to one Mode 03 response line.  The raw line
/// remains available through [`DiagnosticResponses::raw_response`], while a
/// partial payload (when one can be normalized) remains in `responses`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticResponseError {
    pub responder: Option<ResponderIdentity>,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticResponses {
    responses: Vec<DiagnosticResponse>,
    raw_response: String,
    errors: Vec<DiagnosticResponseError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResponseObservation {
    pub(crate) responses: Vec<crate::capture_events::ResponderEvidence>,
    pub(crate) selected_responder: Option<String>,
    pub(crate) selection_error: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ReadOutcome {
    Succeeded {
        transaction: Transaction,
        observations: Vec<ResponseObservation>,
    },
    Failed {
        error: String,
        observations: Vec<ResponseObservation>,
    },
}

impl ReadOutcome {
    fn into_transaction(self) -> Result<Transaction, String> {
        match self {
            Self::Succeeded { transaction, .. } => Ok(transaction),
            Self::Failed { error, .. } => Err(error),
        }
    }
}

impl DiagnosticResponses {
    fn new(responses: Vec<DiagnosticResponse>, raw_response: &str) -> Self {
        Self {
            responses,
            raw_response: raw_response.into(),
            errors: Vec::new(),
        }
    }

    fn with_errors(
        responses: Vec<DiagnosticResponse>,
        raw_response: &str,
        errors: Vec<DiagnosticResponseError>,
    ) -> Self {
        Self {
            responses,
            raw_response: raw_response.into(),
            errors,
        }
    }

    pub fn as_slice(&self) -> &[DiagnosticResponse] {
        &self.responses
    }

    pub fn is_empty(&self) -> bool {
        self.responses.is_empty()
    }

    pub fn raw_response(&self) -> &str {
        &self.raw_response
    }

    pub fn errors(&self) -> &[DiagnosticResponseError] {
        &self.errors
    }

    pub fn capture_evidence(&self) -> Vec<crate::capture_events::ResponderEvidence> {
        self.responses
            .iter()
            .map(|response| crate::capture_events::ResponderEvidence {
                responder: response
                    .responder
                    .as_ref()
                    .map(|identity| identity.as_str().to_owned()),
                payload: response.payload.clone(),
            })
            .collect()
    }

    fn observation(&self, selection_error: Option<String>) -> ResponseObservation {
        let selected_responder = self
            .responses
            .first()
            .and_then(|response| response.responder.as_ref())
            .filter(|first| {
                self.responses
                    .iter()
                    .all(|response| response.responder.as_ref() == Some(*first))
            })
            .map(|responder| responder.as_str().to_owned());
        ResponseObservation {
            responses: self.capture_evidence(),
            selected_responder,
            selection_error,
        }
    }

    /// Select only a known responder. No value-based fallback is permitted.
    pub fn select(&self, target: &ResponderIdentity) -> Result<Vec<u8>, String> {
        let matches = self
            .responses
            .iter()
            .filter(|response| response.responder.as_ref() == Some(target))
            .collect::<Vec<_>>();
        let first = matches
            .first()
            .ok_or_else(|| format!("responder {} did not answer", target.as_str()))?;
        if matches
            .iter()
            .any(|response| response.payload != first.payload)
        {
            return Err(format!(
                "conflicting responses from responder {}",
                target.as_str()
            ));
        }
        Ok(first.payload.clone())
    }

    fn unambiguous_payload(&self, pid: u8) -> Result<Vec<u8>, String> {
        let first = self
            .responses
            .first()
            .ok_or_else(|| format!("01{pid:02X} response not found"))?;
        if self
            .responses
            .iter()
            .any(|response| response.payload != first.payload)
        {
            return Err(format!(
                "conflicting 01{pid:02X} responses (responders: {})",
                self.responses
                    .iter()
                    .map(|response| {
                        response
                            .responder
                            .as_ref()
                            .map_or("unknown", ResponderIdentity::as_str)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(first.payload.clone())
    }
}

pub async fn scan() -> Result<Vec<AdapterCandidate>, String> {
    let (_manager, adapter) = central().await?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|error| format!("Bluetooth scan failed: {error}"))?;
    sleep(SCAN_TIMEOUT).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|error| format!("Bluetooth scan result failed: {error}"));
    let _ = adapter.stop_scan().await;

    let mut candidates = Vec::new();
    for peripheral in peripherals? {
        let Some(properties) = peripheral.properties().await.ok().flatten() else {
            continue;
        };
        let Some(name) = properties.local_name else {
            continue;
        };
        if name.to_ascii_lowercase().contains("carly") {
            candidates.push(AdapterCandidate {
                id: peripheral.id().to_string(),
                name,
                rssi: properties.rssi,
            });
        }
    }
    Ok(candidates)
}

pub async fn read(adapter_id: &str, request: ReadRequest) -> Result<Transaction, String> {
    let mut session = DiagnosticSession::connect_with_adapter_io(adapter_id, true).await?;
    let result = tokio::select! {
        outcome = session.read_with_evidence(request) => outcome.into_transaction(),
        _ = tokio::signal::ctrl_c() => Err("cancelled".into()),
    };
    match (result, session.disconnect().await) {
        (Ok(transaction), Ok(())) => Ok(transaction),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

pub async fn read_targeted(
    adapter_id: &str,
    request: TargetedReadRequest,
) -> Result<Transaction, String> {
    let mut session = DiagnosticSession::connect_with_adapter_io(adapter_id, true).await?;
    let result = tokio::select! {
        outcome = session.read_targeted(request) => outcome,
        _ = tokio::signal::ctrl_c() => Err("cancelled".into()),
    };
    match (result, session.disconnect().await) {
        (Ok(transaction), Ok(())) => Ok(transaction),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

/// Read stored emission-related DTCs through one initialized functional ELM
/// session.  The command and addressing are fixed by this API.
pub async fn read_stored_dtcs(adapter_id: &str) -> Result<DiagnosticResponses, String> {
    let session = start_session(adapter_id).await?;
    let result = session.read_stored_dtcs().await;
    match (result, session.shutdown().await) {
        (Ok(responses), Ok(())) => Ok(responses),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

pub async fn identify(adapter_id: &str) -> Result<crate::identity::VehicleIdentity, String> {
    let mut session =
        DiagnosticSession::connect_without_support_discovery(adapter_id, true).await?;
    let result = tokio::select! {
        identity = session.identify() => identity,
        _ = tokio::signal::ctrl_c() => Err("cancelled".into()),
    };
    match (result, session.disconnect().await) {
        (Ok(identity), Ok(())) => Ok(identity),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

/// Connect, initialize the adapter, and validate functional support with one
/// bounded `0100` request. The returned observations retain each responder's
/// adapter-level identity.
pub async fn validate_functional_support(
    adapter_id: &str,
) -> Result<Vec<SupportDiscovery>, String> {
    let mut session =
        DiagnosticSession::connect_without_support_discovery(adapter_id, false).await?;
    let result = tokio::select! {
        support = session.validate_functional_support() => support,
        _ = tokio::signal::ctrl_c() => Err("cancelled".into()),
    };
    match (result, session.disconnect().await) {
        (Ok(support), Ok(())) => Ok(support),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

pub async fn supported_signals(adapter_id: &str) -> Result<Vec<SignalSupport>, String> {
    let mut session = DiagnosticSession::connect(adapter_id).await?;
    let result = Ok(session.signal_support());
    match (result, session.disconnect().await) {
        (Ok(support), Ok(())) => Ok(support),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

pub async fn start_session(adapter_id: &str) -> Result<SessionClient, String> {
    let session = DiagnosticSession::connect(adapter_id).await?;
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(session_actor(session, receiver));
    Ok(SessionClient { sender })
}

#[derive(Clone)]
pub struct SessionClient {
    sender: mpsc::Sender<SessionCommand>,
}

impl SessionClient {
    pub async fn read(&self, request: ReadRequest) -> Result<Transaction, String> {
        self.read_with_evidence(request).await?.into_transaction()
    }

    pub(crate) async fn read_with_evidence(
        &self,
        request: ReadRequest,
    ) -> Result<ReadOutcome, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::Read { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    pub async fn read_targeted(&self, request: TargetedReadRequest) -> Result<Transaction, String> {
        self.read_targeted_with_evidence(request)
            .await?
            .into_transaction()
    }

    pub async fn read_stored_dtcs(&self) -> Result<DiagnosticResponses, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::ReadStoredDtcs { reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    async fn read_targeted_with_evidence(
        &self,
        request: TargetedReadRequest,
    ) -> Result<ReadOutcome, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::ReadTargeted { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    pub async fn shutdown(self) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::Shutdown { reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before disconnecting".to_string())?
    }

    pub async fn support_discovery(&self) -> Result<Vec<SupportDiscovery>, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::SupportDiscovery { reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())
    }
}

enum SessionCommand {
    Read {
        request: ReadRequest,
        reply: oneshot::Sender<Result<ReadOutcome, String>>,
    },
    ReadTargeted {
        request: TargetedReadRequest,
        reply: oneshot::Sender<Result<ReadOutcome, String>>,
    },
    ReadStoredDtcs {
        reply: oneshot::Sender<Result<DiagnosticResponses, String>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), String>>,
    },
    SupportDiscovery {
        reply: oneshot::Sender<Vec<SupportDiscovery>>,
    },
}

async fn session_actor(
    mut session: DiagnosticSession,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    let mut health = SessionHealth::default();
    let mut disconnect_done = false;
    while let Some(command) = commands.recv().await {
        match command {
            SessionCommand::Read { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let outcome = session.read_with_evidence(request).await;
                process_read_outcome(
                    &mut session,
                    &mut health,
                    &mut disconnect_done,
                    outcome,
                    reply,
                )
                .await;
            }
            SessionCommand::ReadTargeted { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let outcome = session.read_targeted_with_evidence(request).await;
                process_read_outcome(
                    &mut session,
                    &mut health,
                    &mut disconnect_done,
                    outcome,
                    reply,
                )
                .await;
            }
            SessionCommand::ReadStoredDtcs { reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                match session.read_stored_dtcs().await {
                    Ok(responses) => {
                        health.success();
                        let _ = reply.send(Ok(responses));
                    }
                    Err(error) => {
                        if health.observe(&error) {
                            let fatal = health.unhealthy().unwrap().to_owned();
                            session.disconnect_best_effort().await;
                            disconnect_done = true;
                            let _ = reply.send(Err(fatal));
                        } else {
                            let _ = reply.send(Err(error));
                        }
                    }
                }
            }
            SessionCommand::Shutdown { reply } => {
                if !disconnect_done {
                    session.disconnect_best_effort().await;
                }
                let _ = reply.send(Ok(()));
                return;
            }
            SessionCommand::SupportDiscovery { reply } => {
                let _ = reply.send(session.supported.discovery.clone());
            }
        }
    }
    if !disconnect_done {
        session.disconnect_best_effort().await;
    }
}

async fn process_read_outcome(
    session: &mut DiagnosticSession,
    health: &mut SessionHealth,
    disconnect_done: &mut bool,
    outcome: ReadOutcome,
    reply: oneshot::Sender<Result<ReadOutcome, String>>,
) {
    if let Some(error) = health.unhealthy() {
        let _ = reply.send(Err(error.to_owned()));
        return;
    }
    match outcome {
        ReadOutcome::Succeeded { .. } => {
            health.success();
            let _ = reply.send(Ok(outcome));
        }
        ReadOutcome::Failed {
            error,
            observations,
        } => {
            if health.observe(&error) {
                let fatal = health.unhealthy().unwrap().to_owned();
                session.disconnect_best_effort().await;
                *disconnect_done = true;
                let _ = reply.send(Ok(ReadOutcome::Failed {
                    error: fatal,
                    observations,
                }));
            } else {
                let _ = reply.send(Ok(ReadOutcome::Failed {
                    error,
                    observations,
                }));
            }
        }
    }
}

#[derive(Default)]
struct SessionHealth {
    consecutive_transport_failures: u8,
    unhealthy: Option<String>,
}

impl SessionHealth {
    fn success(&mut self) {
        self.consecutive_transport_failures = 0;
    }

    /// Returns true only when this error crosses the fatal transport threshold.
    fn observe(&mut self, error: &str) -> bool {
        if self.unhealthy.is_some() {
            return false;
        }
        if !is_transport_failure(error) {
            self.consecutive_transport_failures = 0;
            return false;
        }
        self.consecutive_transport_failures = self.consecutive_transport_failures.saturating_add(1);
        if self.consecutive_transport_failures < TRANSPORT_FAILURE_THRESHOLD {
            return false;
        }
        self.unhealthy = Some(format!("{SESSION_UNHEALTHY_PREFIX}: {error}"));
        true
    }

    fn unhealthy(&self) -> Option<&str> {
        self.unhealthy.as_deref()
    }
}

pub(crate) fn is_session_unhealthy(error: &str) -> bool {
    error.starts_with(SESSION_UNHEALTHY_PREFIX)
}

fn is_transport_failure(error: &str) -> bool {
    [
        "Carly write timed out:",
        "Carly write failed:",
        "Carly command timed out:",
        "Carly notification stream ended",
        "diagnostic session is closed",
        "diagnostic session stopped before responding",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

/// One connected, initialized, read-only Carly diagnostic path.
pub struct DiagnosticSession {
    _manager: Manager,
    peripheral: Peripheral,
    channel: Characteristic,
    notifications: Notifications,
    supported: PidSupport,
    show_adapter_io: bool,
}

impl DiagnosticSession {
    pub async fn connect(adapter_id: &str) -> Result<Self, String> {
        Self::connect_with_adapter_io(adapter_id, false).await
    }

    async fn connect_without_support_discovery(
        adapter_id: &str,
        show_adapter_io: bool,
    ) -> Result<Self, String> {
        Self::connect_with_adapter_io_mode(adapter_id, show_adapter_io, false).await
    }

    async fn connect_with_adapter_io(
        adapter_id: &str,
        show_adapter_io: bool,
    ) -> Result<Self, String> {
        Self::connect_with_adapter_io_mode(adapter_id, show_adapter_io, true).await
    }

    async fn connect_with_adapter_io_mode(
        adapter_id: &str,
        show_adapter_io: bool,
        discover_support: bool,
    ) -> Result<Self, String> {
        let (manager, adapter) = central().await?;
        let peripheral = find_peripheral(&adapter, adapter_id).await?;
        let cleanup_peripheral = peripheral.clone();
        let result = async {
            timeout(CONNECT_TIMEOUT, peripheral.connect())
                .await
                .map_err(|_| "Bluetooth connection timed out".to_string())?
                .map_err(|error| format!("Bluetooth connection failed: {error}"))?;
            timeout(SETUP_TIMEOUT, peripheral.discover_services())
                .await
                .map_err(|_| "BLE service discovery timed out".to_string())?
                .map_err(|error| format!("BLE service discovery failed: {error}"))?;
            let channel = carly_channel(&peripheral)?;
            timeout(SETUP_TIMEOUT, peripheral.subscribe(&channel))
                .await
                .map_err(|_| "Carly notification activation timed out".to_string())?
                .map_err(|error| format!("Carly notifications unavailable: {error}"))?;
            let notifications = timeout(SETUP_TIMEOUT, peripheral.notifications())
                .await
                .map_err(|_| "Carly notification stream timed out".to_string())?
                .map_err(|error| format!("Carly notification stream unavailable: {error}"))?;
            let mut session = Self {
                _manager: manager,
                peripheral,
                channel,
                notifications,
                supported: PidSupport::default(),
                show_adapter_io,
            };
            session.initialize().await?;
            if discover_support {
                session.discover_support().await?;
            }
            Ok(session)
        }
        .await;
        if result.is_err() {
            best_effort_disconnect(&cleanup_peripheral).await;
        }
        result
    }

    pub async fn read(&mut self, request: ReadRequest) -> Result<Transaction, String> {
        self.read_with_evidence(request).await.into_transaction()
    }

    pub async fn read_targeted(
        &mut self,
        request: TargetedReadRequest,
    ) -> Result<Transaction, String> {
        self.read_targeted_with_evidence(request)
            .await
            .into_transaction()
    }

    async fn read_stored_dtcs(&mut self) -> Result<DiagnosticResponses, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        read_elm_mode03_responses(&mut exchange).await
    }

    pub async fn identify(&mut self) -> Result<crate::identity::VehicleIdentity, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        let response = exchange.exchange("0902\r", COMMAND_TIMEOUT).await?;
        let segments = normalize_mode09_segments(&response)?;
        crate::identity::decode_mode09_pid02(&segments).map_err(|error| error.to_string())
    }

    async fn validate_functional_support(&mut self) -> Result<Vec<SupportDiscovery>, String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        validate_functional_support_exchange(&mut exchange).await
    }

    async fn read_with_evidence(&mut self, request: ReadRequest) -> ReadOutcome {
        if !supports_pid(&self.supported, request.pid()) {
            return ReadOutcome::Failed {
                error: format!(
                    "vehicle does not advertise support for {}",
                    crate::hex(&request.bytes())
                ),
                observations: Vec::new(),
            };
        }
        let read = {
            let mut exchange = LiveExchange {
                peripheral: &self.peripheral,
                channel: &self.channel,
                notifications: &mut self.notifications,
                show_adapter_io: self.show_adapter_io,
            };
            read_elm_with_evidence(&mut exchange, request).await
        };
        match read {
            Ok(read) => match request.complete("user", read.payload) {
                Ok(transaction) => ReadOutcome::Succeeded {
                    transaction,
                    observations: read.observations,
                },
                Err(error) => ReadOutcome::Failed {
                    error,
                    observations: read.observations,
                },
            },
            Err(error) => ReadOutcome::Failed {
                error: error.error,
                observations: error.observations,
            },
        }
    }

    async fn read_targeted_with_evidence(&mut self, request: TargetedReadRequest) -> ReadOutcome {
        let read = {
            let mut exchange = LiveExchange {
                peripheral: &self.peripheral,
                channel: &self.channel,
                notifications: &mut self.notifications,
                show_adapter_io: self.show_adapter_io,
            };
            read_elm_targeted_with_evidence(&mut exchange, &request).await
        };
        match read {
            Ok(read) => match request.request().complete("user", read.payload) {
                Ok(transaction) => ReadOutcome::Succeeded {
                    transaction,
                    observations: read.observations,
                },
                Err(error) => ReadOutcome::Failed {
                    error,
                    observations: read.observations,
                },
            },
            Err(error) => ReadOutcome::Failed {
                error: error.error,
                observations: error.observations,
            },
        }
    }

    pub async fn disconnect(&mut self) -> Result<(), String> {
        timeout(CONNECT_TIMEOUT, self.peripheral.disconnect())
            .await
            .map_err(|_| "Bluetooth disconnect timed out".to_string())?
            .map_err(|error| format!("Bluetooth disconnect failed: {error}"))
    }

    async fn disconnect_best_effort(&mut self) {
        let _ = timeout(SHUTDOWN_DISCONNECT_TIMEOUT, self.peripheral.disconnect()).await;
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        initialize_elm(&mut exchange).await
    }

    async fn discover_support(&mut self) -> Result<(), String> {
        let mut exchange = LiveExchange {
            peripheral: &self.peripheral,
            channel: &self.channel,
            notifications: &mut self.notifications,
            show_adapter_io: self.show_adapter_io,
        };
        self.supported = discover_pid_support(&mut exchange).await?;
        Ok(())
    }

    fn signal_support(&self) -> Vec<SignalSupport> {
        crate::vehicle::signals()
            .iter()
            .map(|signal| SignalSupport {
                semantic: signal.metadata().semantic,
                status: self.supported.status(signal.request().pid()),
            })
            .collect()
    }
}

async fn best_effort_disconnect(peripheral: &Peripheral) {
    let _ = timeout(CONNECT_TIMEOUT, peripheral.disconnect()).await;
}

async fn central() -> Result<(Manager, Adapter), String> {
    let manager = Manager::new()
        .await
        .map_err(|error| format!("Bluetooth manager unavailable: {error}"))?;
    let adapter = manager
        .adapters()
        .await
        .map_err(|error| format!("Bluetooth adapter lookup failed: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no Bluetooth adapter available".to_string())?;
    Ok((manager, adapter))
}

async fn find_peripheral(adapter: &Adapter, requested_id: &str) -> Result<Peripheral, String> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|error| format!("Bluetooth scan failed: {error}"))?;
    sleep(FIND_TIMEOUT).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|error| format!("Bluetooth scan result failed: {error}"));
    let _ = adapter.stop_scan().await;
    peripherals?
        .into_iter()
        .find(|peripheral| {
            peripheral
                .id()
                .to_string()
                .eq_ignore_ascii_case(requested_id)
        })
        .ok_or_else(|| format!("adapter {requested_id} was not found after 10 seconds"))
}

fn carly_channel(peripheral: &Peripheral) -> Result<Characteristic, String> {
    let service = uuid_from_u16(CARLY_SERVICE);
    let channel = uuid_from_u16(CARLY_CHANNEL);
    if !peripheral
        .services()
        .iter()
        .any(|item| item.uuid == service)
    {
        return Err("Carly FFE0 service unavailable".into());
    }
    let channel = peripheral
        .characteristics()
        .iter()
        .find(|item| item.service_uuid == service && item.uuid == channel)
        .cloned()
        .ok_or_else(|| "Carly FFE1 characteristic unavailable".to_string())?;
    if !channel.properties.contains(CharPropFlags::NOTIFY) {
        return Err("Carly FFE1 notifications unavailable".into());
    }
    if !channel
        .properties
        .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
    {
        return Err("Carly FFE1 write unavailable".into());
    }
    Ok(channel)
}

pub(crate) trait ElmExchange {
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String>;
}

struct LiveExchange<'a, S> {
    peripheral: &'a Peripheral,
    channel: &'a Characteristic,
    notifications: &'a mut S,
    show_adapter_io: bool,
}

impl<S> ElmExchange for LiveExchange<'_, S>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String> {
        elm_exchange(
            self.peripheral,
            self.channel,
            self.notifications,
            command,
            command_timeout,
            self.show_adapter_io,
        )
        .await
    }
}

async fn initialize_elm<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    let ati = exchange.exchange("ATI\r", Duration::from_secs(3)).await?;
    require_response(
        &ati,
        "ELM327",
        false,
        "ATI did not identify an ELM327 adapter",
    )?;
    let identity = exchange.exchange("AT@1\r", Duration::from_secs(3)).await?;
    require_response(
        &identity,
        "CARLY-UNIVERSAL",
        false,
        "AT@1 did not identify a Carly adapter",
    )?;
    let reset = exchange.exchange("ATZ\r", Duration::from_secs(3)).await?;
    require_response(
        &reset,
        "ELM327",
        false,
        "ATZ did not reset an ELM327 adapter",
    )?;
    // Keep separators and adapter headers so responder identity survives the
    // ELM normalization boundary. No identity is synthesized when absent.
    for command in ["ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"] {
        let response = exchange.exchange(command, Duration::from_secs(3)).await?;
        require_response(&response, "OK", true, &format!("{} failed", command.trim()))?;
    }
    Ok(())
}

async fn discover_pid_support<E>(exchange: &mut E) -> Result<PidSupport, String>
where
    E: ElmExchange,
{
    let mut pages = Vec::new();
    let mut discovery = Vec::new();
    let highest_page = highest_catalog_page();
    let mut page = 0_u8;
    loop {
        let command = format!("01{page:02X}\r");
        let response = exchange.exchange(&command, Duration::from_secs(10)).await?;
        let (normalized, observations) = normalize_pid_support_page_with_evidence(&response, page)?;
        let bitmap = u32::from_be_bytes(normalized[2..].try_into().unwrap());
        pages.push(bitmap);
        discovery.extend(observations);

        if page >= highest_page {
            break;
        }
        let Some(next_page) = page.checked_add(0x20) else {
            break;
        };
        if !bitmap_supports_pid(bitmap, next_page) {
            break;
        }
        page = next_page;
    }
    Ok(PidSupport { pages, discovery })
}

async fn validate_functional_support_exchange<E>(
    exchange: &mut E,
) -> Result<Vec<SupportDiscovery>, String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("0100\r", COMMAND_TIMEOUT).await?;
    normalize_pid_support_page_with_evidence(&response, 0x00).map(|(_, observations)| observations)
}

#[cfg(test)]
async fn read_elm<E>(exchange: &mut E, request: ReadRequest) -> Result<Vec<u8>, String>
where
    E: ElmExchange,
{
    read_elm_with_evidence(exchange, request)
        .await
        .map(|read| read.payload)
        .map_err(|error| error.error)
}

#[derive(Debug)]
struct ReadEvidence {
    payload: Vec<u8>,
    observations: Vec<ResponseObservation>,
}

#[derive(Debug)]
struct ReadEvidenceError {
    error: String,
    observations: Vec<ResponseObservation>,
}

async fn read_elm_with_evidence<E>(
    exchange: &mut E,
    request: ReadRequest,
) -> Result<ReadEvidence, ReadEvidenceError>
where
    E: ElmExchange,
{
    let first = match read_elm_responses(exchange, request).await {
        Ok(first) => first,
        Err(error) => {
            return Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            });
        }
    };
    match first.unambiguous_payload(request.pid()) {
        Ok(payload) => Ok(ReadEvidence {
            payload,
            observations: vec![first.observation(None)],
        }),
        Err(error)
            if error.starts_with(&format!("conflicting 01{:02X} responses", request.pid())) =>
        {
            let mut observations = vec![first.observation(Some(error.clone()))];
            let retry = match read_elm_responses(exchange, request).await {
                Ok(retry) => retry,
                Err(retry_error) => {
                    return Err(ReadEvidenceError {
                        error: format!(
                            "{error}; first ELM response={}; retry failed: {retry_error}",
                            first.raw_response().escape_default()
                        ),
                        observations,
                    });
                }
            };
            match retry.unambiguous_payload(request.pid()) {
                Ok(payload) => {
                    observations.push(retry.observation(None));
                    Ok(ReadEvidence {
                        payload,
                        observations,
                    })
                }
                Err(retry_error) => {
                    observations.push(retry.observation(Some(retry_error.clone())));
                    Err(ReadEvidenceError {
                        error: format!(
                            "{retry_error}; first ELM response={}; retry ELM response={}",
                            first.raw_response().escape_default(),
                            retry.raw_response().escape_default()
                        ),
                        observations,
                    })
                }
            }
        }
        Err(error) => Err(ReadEvidenceError {
            error: error.clone(),
            observations: vec![first.observation(Some(error))],
        }),
    }
}

async fn read_elm_targeted_with_evidence<E>(
    exchange: &mut E,
    request: &TargetedReadRequest,
) -> Result<ReadEvidence, ReadEvidenceError>
where
    E: ElmExchange,
{
    if let Err(error) = configure_target(exchange, request).await {
        let restore = restore_functional(exchange).await;
        return Err(ReadEvidenceError {
            error: combine_setup_errors(error, restore),
            observations: Vec::new(),
        });
    }

    let read = match read_elm_responses(exchange, request.request()).await {
        Ok(responses) => match targeted_payload(&responses, request.expected_responder()) {
            Ok(payload) => Ok(ReadEvidence {
                payload,
                observations: vec![responses.observation(None)],
            }),
            Err(error) => Err(ReadEvidenceError {
                error: error.clone(),
                observations: vec![responses.observation(Some(error))],
            }),
        },
        Err(error) => Err(ReadEvidenceError {
            error,
            observations: Vec::new(),
        }),
    };
    let restore = restore_functional(exchange).await;
    match (read, restore) {
        (Ok(read), Ok(())) => Ok(read),
        (Err(error), Ok(())) => Err(error),
        (Ok(read), Err(error)) => Err(ReadEvidenceError {
            error: format!(
                "targeted read succeeded; restoring functional addressing failed: {error}"
            ),
            observations: read.observations,
        }),
        (Err(mut error), Err(restore)) => {
            error.error = format!(
                "{}; restoring functional addressing failed: {restore}",
                error.error
            );
            Err(error)
        }
    }
}

async fn configure_target<E>(exchange: &mut E, request: &TargetedReadRequest) -> Result<(), String>
where
    E: ElmExchange,
{
    let address = request
        .target()
        .address()
        .expect("validated targeted request has a concrete address");
    let target_header = address.value().to_ascii_uppercase();
    let expected_header = request.expected_responder().as_str();
    for (command, error) in [
        (
            format!("ATSH {target_header}\r"),
            "target request header setup failed",
        ),
        (
            format!("ATCRA {expected_header}\r"),
            "target response filter setup failed",
        ),
    ] {
        let response = exchange.exchange(&command, Duration::from_secs(3)).await?;
        require_response(&response, "OK", true, error)?;
    }
    Ok(())
}

async fn restore_functional<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    for (command, error) in [
        ("ATSP0\r", "functional protocol reset failed"),
        ("ATSH 7DF\r", "functional request header reset failed"),
        ("ATCRA\r", "functional response filter reset failed"),
    ] {
        let response = exchange.exchange(command, Duration::from_secs(3)).await?;
        require_response(&response, "OK", true, error)?;
    }
    Ok(())
}

fn combine_setup_errors(error: String, restore: Result<(), String>) -> String {
    match restore {
        Ok(()) => error,
        Err(restore) => format!("{error}; restoring functional addressing failed: {restore}"),
    }
}

fn targeted_payload(
    responses: &DiagnosticResponses,
    expected: &ResponderIdentity,
) -> Result<Vec<u8>, String> {
    if let Some(unexpected) = responses
        .as_slice()
        .iter()
        .find(|response| response.responder.as_ref() != Some(expected))
    {
        let observed = unexpected
            .responder
            .as_ref()
            .map_or("unknown", ResponderIdentity::as_str);
        return Err(format!(
            "unexpected responder {observed} for targeted read; expected {}",
            expected.as_str()
        ));
    }
    responses.select(expected)
}

pub(crate) async fn read_elm_responses<E>(
    exchange: &mut E,
    request: ReadRequest,
) -> Result<DiagnosticResponses, String>
where
    E: ElmExchange,
{
    let command = obd_command(request);
    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;
    normalize_mode01_responses(&response, request.pid(), request.data_len())
}

pub(crate) async fn read_elm_mode03_responses<E>(
    exchange: &mut E,
) -> Result<DiagnosticResponses, String>
where
    E: ElmExchange,
{
    let response = exchange.exchange(MODE03_COMMAND, COMMAND_TIMEOUT).await?;
    normalize_mode03_responses(&response)
}

fn supports_pid(support: &PidSupport, pid: u8) -> bool {
    support.supports_pid(pid)
}

fn bitmap_supports_pid(bitmap: u32, pid: u8) -> bool {
    let offset = pid & 0x1f;
    let shift = if offset == 0 { 0 } else { 32 - offset };
    bitmap & (1 << shift) != 0
}

async fn elm_exchange<S>(
    peripheral: &Peripheral,
    channel: &Characteristic,
    notifications: &mut S,
    command: &str,
    command_timeout: Duration,
    show_adapter_io: bool,
) -> Result<String, String>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    let write_type = if channel
        .properties
        .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
    {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    };
    timeout(
        command_timeout,
        peripheral.write(channel, command.as_bytes(), write_type),
    )
    .await
    .map_err(|_| format!("Carly write timed out: {}", command.trim()))?
    .map_err(|error| format!("Carly write failed: {error}"))?;
    if let Some(line) = adapter_io_line(show_adapter_io, "tx", command.trim()) {
        println!("{line}");
    }

    let response = timeout(command_timeout, wait_for_prompt(notifications, channel))
        .await
        .map_err(|_| format!("Carly command timed out: {}", command.trim()))??;
    if let Some(line) = adapter_io_line(show_adapter_io, "rx", response.escape_default()) {
        println!("{line}");
    }
    Ok(response)
}

fn adapter_io_line(
    show_adapter_io: bool,
    direction: &str,
    value: impl std::fmt::Display,
) -> Option<String> {
    show_adapter_io.then(|| format!("adapter {direction}  {value}"))
}

async fn wait_for_prompt<S>(
    notifications: &mut S,
    channel: &Characteristic,
) -> Result<String, String>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    let mut response = Vec::new();
    loop {
        let notification = notifications
            .next()
            .await
            .ok_or_else(|| "Carly notification stream ended".to_string())?;
        if notification.uuid != channel.uuid {
            continue;
        }
        if append_notification(&mut response, &notification.value)? {
            return Ok(String::from_utf8_lossy(&response).into_owned());
        }
    }
}

fn append_notification(response: &mut Vec<u8>, fragment: &[u8]) -> Result<bool, String> {
    response.extend_from_slice(fragment);
    if response.len() > MAX_RESPONSE {
        return Err("Carly response exceeded 8 KiB".into());
    }
    Ok(response.contains(&b'>'))
}

fn obd_command(request: ReadRequest) -> String {
    let mut command = String::with_capacity(request.bytes().len() * 2 + 1);
    for byte in request.bytes() {
        command.push_str(&format!("{byte:02X}"));
    }
    command.push('\r');
    command
}

fn require_response(
    response: &str,
    expected: &str,
    exact: bool,
    error: &str,
) -> Result<(), String> {
    let upper = response.to_ascii_uppercase();
    if upper.split(['\r', '\n']).any(|line| {
        let line = line.trim().trim_end_matches('>').trim();
        line == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| line.contains(status))
    }) {
        return Err(format!("{error}: {response:?}"));
    }
    upper
        .split(['\r', '\n'])
        .map(|line| line.trim().trim_end_matches('>').trim())
        .any(|line| {
            if exact {
                line == expected
            } else {
                line.starts_with(expected)
            }
        })
        .then_some(())
        .ok_or_else(|| format!("{error}: {response:?}"))
}

#[cfg(test)]
fn normalize_mode01(response: &str, pid: u8, data_len: usize) -> Result<Vec<u8>, String> {
    normalize_mode01_responses(response, pid, data_len)?.unambiguous_payload(pid)
}

pub(crate) fn normalize_mode01_responses(
    response: &str,
    pid: u8,
    data_len: usize,
) -> Result<DiagnosticResponses, String> {
    Ok(DiagnosticResponses::new(
        mode01_responses(response, pid, data_len)?,
        response,
    ))
}

pub(crate) fn normalize_mode03_responses(response: &str) -> Result<DiagnosticResponses, String> {
    let mut matches = Vec::new();
    let mut errors = Vec::new();

    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let compact = upper.split_ascii_whitespace().collect::<String>();
        if compact == "03"
            || upper.starts_with("SEARCHING")
            || (upper.starts_with("BUS INIT") && !upper.contains("ERROR"))
        {
            continue;
        }

        let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
        let header = tokens.first().filter(|token| token.len() == 3).copied();
        let responder =
            header.map(|value| ResponderIdentity::ElmHeader(value.to_ascii_uppercase()));
        let data = if header.is_some() {
            &tokens[1..]
        } else {
            tokens.as_slice()
        };

        if ["?", "NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
            .iter()
            .any(|status| upper == *status || upper.contains(status))
        {
            errors.push(DiagnosticResponseError {
                responder,
                error: format!("ELM327 rejected Mode 03 response: {line}"),
            });
            continue;
        }

        if let Some(header) = header {
            if !header.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                errors.push(DiagnosticResponseError {
                    responder,
                    error: format!("malformed ELM327 responder header: {line:?}"),
                });
                continue;
            }
        }

        let mut bytes = Vec::new();
        let mut malformed = None;
        for token in data {
            if token.len() % 2 != 0 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                malformed = Some(format!("malformed ELM327 Mode 03 response line: {line:?}"));
                break;
            }
            for pair in token.as_bytes().as_chunks::<2>().0 {
                let pair = match std::str::from_utf8(pair) {
                    Ok(pair) => pair,
                    Err(error) => {
                        malformed = Some(error.to_string());
                        break;
                    }
                };
                match u8::from_str_radix(pair, 16) {
                    Ok(byte) => bytes.push(byte),
                    Err(error) => {
                        malformed = Some(error.to_string());
                        break;
                    }
                }
            }
            if malformed.is_some() {
                break;
            }
        }

        if let Some(error) = malformed {
            if let Some(start) = bytes.iter().position(|byte| *byte == 0x43) {
                matches.push(DiagnosticResponse {
                    responder: responder.clone(),
                    payload: bytes[start..].to_vec(),
                });
            }
            errors.push(DiagnosticResponseError { responder, error });
            continue;
        }

        let Some(start) = bytes.iter().position(|byte| *byte == 0x43) else {
            if !bytes.is_empty() {
                matches.push(DiagnosticResponse {
                    responder: responder.clone(),
                    payload: bytes,
                });
            }
            errors.push(DiagnosticResponseError {
                responder,
                error: format!("Mode 03 positive response not found: {line:?}"),
            });
            continue;
        };
        let payload = bytes[start..].to_vec();
        if payload.len() < 3 || (payload.len() - 1) % 2 != 0 {
            errors.push(DiagnosticResponseError {
                responder: responder.clone(),
                error: format!("malformed normalized Mode 03 payload: {line:?}"),
            });
        }
        matches.push(DiagnosticResponse { responder, payload });
    }

    if matches.is_empty() && errors.is_empty() {
        errors.push(DiagnosticResponseError {
            responder: None,
            error: "Mode 03 response not found".into(),
        });
    }
    Ok(DiagnosticResponses::with_errors(matches, response, errors))
}

fn normalize_mode09_segments(response: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut segments = Vec::new();
    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty()
            || line.eq_ignore_ascii_case("0902")
            || line.to_ascii_uppercase().starts_with("SEARCHING")
            || line.to_ascii_uppercase().starts_with("BUS INIT")
        {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| upper.contains(status))
        {
            return Err(format!("ELM327 rejected 0902: {line}"));
        }
        let mut tokens = line.split_ascii_whitespace();
        let has_header = tokens
            .next()
            .filter(|token| token.len() == 3 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .is_some();
        if !has_header {
            tokens = line.split_ascii_whitespace();
        }
        let mut bytes = Vec::new();
        for token in tokens {
            if token.is_empty()
                || token.len() % 2 != 0
                || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("malformed ELM327 Mode 09 response line: {line:?}"));
            }
            for pair in token.as_bytes().as_chunks::<2>().0 {
                let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
                bytes.push(u8::from_str_radix(pair, 16).map_err(|error| error.to_string())?);
            }
        }
        if !bytes.is_empty() {
            segments.push(bytes);
        }
    }
    (!segments.is_empty())
        .then_some(segments)
        .ok_or_else(|| "0902 response not found".into())
}

#[cfg(test)]
fn normalize_pid_support_page(response: &str, page: u8) -> Result<[u8; 6], String> {
    normalize_pid_support_page_with_evidence(response, page).map(|(normalized, _)| normalized)
}

fn normalize_pid_support_page_with_evidence(
    response: &str,
    page: u8,
) -> Result<([u8; 6], Vec<SupportDiscovery>), String> {
    let matches = normalize_mode01_responses(response, page, 4)?;
    let mut bitmap = 0_u32;
    let mut observations = Vec::new();
    for value in matches.as_slice() {
        bitmap |= u32::from_be_bytes(value.payload[2..].try_into().unwrap());
        observations.push(SupportDiscovery {
            request: [0x01, page],
            responder: value.responder.clone(),
            response: value.payload.as_slice().try_into().unwrap(),
        });
    }
    observations.sort_by(|left, right| {
        left.responder
            .as_ref()
            .map(ResponderIdentity::as_str)
            .cmp(&right.responder.as_ref().map(ResponderIdentity::as_str))
            .then_with(|| left.response.cmp(&right.response))
    });
    let bytes = bitmap.to_be_bytes();
    Ok((
        [0x41, page, bytes[0], bytes[1], bytes[2], bytes[3]],
        observations,
    ))
}

fn highest_catalog_page() -> u8 {
    crate::vehicle::signals()
        .iter()
        .map(|signal| signal.request().pid().saturating_sub(1) & !0x1f)
        .max()
        .unwrap_or(0)
}

fn mode01_responses(
    response: &str,
    pid: u8,
    data_len: usize,
) -> Result<Vec<DiagnosticResponse>, String> {
    let mut matches = Vec::new();
    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let compact = upper.split_ascii_whitespace().collect::<String>();
        if compact == format!("01{pid:02X}")
            || upper.starts_with("SEARCHING")
            || (upper.starts_with("BUS INIT") && !upper.contains("ERROR"))
        {
            continue;
        }
        if upper == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| upper.contains(status))
        {
            return Err(format!("ELM327 rejected 01{pid:02X}: {line}"));
        }
        let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
        let has_separators = tokens.len() > 1;
        let header_token = tokens.first().filter(|token| token.len() == 3).copied();
        if header_token.is_none()
            && (compact.len() % 2 != 0 || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(format!("malformed ELM327 response line: {line:?}"));
        }
        if header_token.is_some()
            && !header_token
                .unwrap()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("malformed ELM327 responder header: {line:?}"));
        }
        let mut bytes = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            if index == 0 && header_token.is_some() {
                continue;
            }
            if token.len() % 2 != 0 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(if index == 0 && has_separators {
                    format!("malformed ELM327 responder header: {line:?}")
                } else {
                    format!("malformed ELM327 response line: {line:?}")
                });
            }
            for pair in token.as_bytes().as_chunks::<2>().0 {
                let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
                bytes.push(u8::from_str_radix(pair, 16).map_err(|error| error.to_string())?);
            }
        }
        let expected_len = data_len + 2;
        let negative = if bytes.len() > 1 && bytes[1] == 0x7f && (bytes[0] as usize) < bytes.len() {
            &bytes[1..]
        } else {
            &bytes[..]
        };
        if negative.first() == Some(&0x7f) {
            return Err(format!("negative OBD-II response: {line}"));
        }
        let mut search_from = 0;
        while let Some(relative) = bytes[search_from..]
            .windows(2)
            .position(|pair| pair == [0x41, pid])
        {
            let payload_start = search_from + relative;
            let Some(payload_end) = payload_start.checked_add(expected_len) else {
                break;
            };
            if payload_end > bytes.len() {
                break;
            }
            let frame_start = if payload_start > 0 && bytes[payload_start - 1] == expected_len as u8
            {
                payload_start - 1
            } else if payload_start == 0 {
                payload_start
            } else {
                search_from = payload_end;
                continue;
            };
            if bytes[payload_end..]
                .iter()
                .any(|byte| !matches!(byte, 0x00 | 0xaa))
            {
                return Err(format!("unexpected bytes after OBD-II response: {line}"));
            }
            let responder = if let Some(header) = header_token {
                Some(ResponderIdentity::ElmHeader(header.to_ascii_uppercase()))
            } else if has_separators && frame_start > 0 {
                Some(ResponderIdentity::ElmHeader(
                    tokens[..frame_start]
                        .iter()
                        .map(|token| token.to_ascii_uppercase())
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            } else {
                None
            };
            matches.push(DiagnosticResponse {
                responder,
                payload: bytes[payload_start..payload_end].to_vec(),
            });
            search_from = payload_end;
        }
    }
    (!matches.is_empty())
        .then_some(matches)
        .ok_or_else(|| format!("01{pid:02X} response not found in {response:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    const INIT_COMMANDS: [&str; 9] = [
        "ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r", "0100\r",
    ];

    struct ScriptedExchange {
        responses: VecDeque<Result<String, String>>,
        commands: Vec<String>,
    }

    impl ScriptedExchange {
        fn captured(responses: Vec<String>) -> Self {
            Self {
                responses: responses.into_iter().map(Ok).collect(),
                commands: Vec::new(),
            }
        }
    }

    impl ElmExchange for ScriptedExchange {
        async fn exchange(
            &mut self,
            command: &str,
            _command_timeout: Duration,
        ) -> Result<String, String> {
            self.commands.push(command.into());
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err("script ended before adapter response".into()))
        }
    }

    async fn initialize_with_support<E>(exchange: &mut E) -> Result<PidSupport, String>
    where
        E: ElmExchange,
    {
        initialize_elm(exchange).await?;
        discover_pid_support(exchange).await
    }

    fn captured_responses() -> Vec<String> {
        [
            "ELM327 v1.4 v100\r>",
            "carly-universal v200\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            // Keep the fixture on the first page; continuation is tested separately.
            "4100BE3EB812\r>",
            "410C0000\r>",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn targeted_request() -> TargetedReadRequest {
        TargetedReadRequest::new(
            crate::prepare_read("engine.rpm").unwrap(),
            crate::topology::RequestTarget::concrete(
                crate::topology::ProtocolContext::new(
                    crate::topology::Protocol::Obd2,
                    crate::topology::AddressingContext::Physical,
                ),
                crate::topology::RequestAddress::new("elm-header", "7e0"),
            ),
            ResponderIdentity::ElmHeader("7e8".into()),
        )
        .unwrap()
    }

    fn channel() -> Characteristic {
        Characteristic {
            uuid: uuid_from_u16(CARLY_CHANNEL),
            service_uuid: uuid_from_u16(CARLY_SERVICE),
            properties: CharPropFlags::NOTIFY,
            descriptors: BTreeSet::new(),
        }
    }

    fn notification(value: &[u8]) -> ValueNotification {
        ValueNotification {
            uuid: uuid_from_u16(CARLY_CHANNEL),
            service_uuid: uuid_from_u16(CARLY_SERVICE),
            value: value.into(),
        }
    }

    #[tokio::test]
    async fn session_client_serializes_read_commands_and_shutdown() {
        let (sender, mut commands) = mpsc::channel(4);
        let actor = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(command) = commands.recv().await {
                match command {
                    SessionCommand::Read { request, reply } => {
                        seen.push(request.bytes());
                        let response = match request.bytes() {
                            [0x01, 0x0c] => vec![0x41, 0x0c, 0x00, 0x00],
                            [0x01, 0x05] => vec![0x41, 0x05, 0x5a],
                            _ => unreachable!("closed test request vocabulary"),
                        };
                        let _ = reply.send(Ok(ReadOutcome::Succeeded {
                            transaction: request.complete("user", response).unwrap(),
                            observations: Vec::new(),
                        }));
                    }
                    SessionCommand::ReadTargeted { reply, .. } => {
                        let _ = reply.send(Err("targeted test request not scripted".into()));
                    }
                    SessionCommand::ReadStoredDtcs { reply } => {
                        let _ = reply.send(Err("stored DTC test request not scripted".into()));
                    }
                    SessionCommand::Shutdown { reply } => {
                        let _ = reply.send(Ok(()));
                        return seen;
                    }
                    SessionCommand::SupportDiscovery { reply } => {
                        let _ = reply.send(Vec::new());
                    }
                }
            }
            seen
        });
        let client = SessionClient { sender };

        assert_eq!(
            client
                .read(crate::prepare_read("engine.rpm").unwrap())
                .await
                .unwrap()
                .value(),
            0.0
        );
        assert_eq!(
            client
                .read(crate::prepare_read("engine.coolant_temperature").unwrap())
                .await
                .unwrap()
                .value(),
            50.0
        );
        client.shutdown().await.unwrap();
        assert_eq!(actor.await.unwrap(), vec![[0x01, 0x0c], [0x01, 0x05]]);
    }

    #[tokio::test]
    async fn session_client_requests_stored_dtcs_without_a_protocol_argument() {
        let (sender, mut commands) = mpsc::channel(1);
        let actor = tokio::spawn(async move {
            if let Some(SessionCommand::ReadStoredDtcs { reply }) = commands.recv().await {
                let _ = reply.send(normalize_mode03_responses("7E8 03 43 01 0C\r>"));
            }
        });
        let client = SessionClient { sender };

        let responses = client.read_stored_dtcs().await.unwrap();

        assert_eq!(responses.as_slice()[0].payload, [0x43, 0x01, 0x0c]);
        actor.await.unwrap();
    }

    #[tokio::test]
    async fn session_client_returns_a_clone_of_support_discovery() {
        let (sender, mut commands) = mpsc::channel(1);
        let discovery = vec![SupportDiscovery {
            request: [0x01, 0x00],
            responder: None,
            response: [0x41, 0x00, 0x80, 0x00, 0x00, 0x01],
        }];
        let expected = discovery.clone();
        let actor = tokio::spawn(async move {
            if let Some(SessionCommand::SupportDiscovery { reply }) = commands.recv().await {
                let _ = reply.send(discovery);
            }
        });
        let client = SessionClient { sender };

        assert_eq!(client.support_discovery().await.unwrap(), expected);
        actor.await.unwrap();
    }

    #[tokio::test]
    async fn session_client_reports_a_closed_actor() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let client = SessionClient { sender };
        assert!(client
            .read(crate::prepare_read("engine.rpm").unwrap())
            .await
            .is_err());
    }

    #[test]
    fn transport_health_stops_after_two_consecutive_transport_failures() {
        let mut health = SessionHealth::default();

        assert!(!health.observe("Carly write timed out: 010C"));
        assert!(health.observe("Carly command timed out: 010D"));

        let error = health.unhealthy().unwrap();
        assert!(is_session_unhealthy(error));
        assert!(error.contains("Carly command timed out: 010D"));
        assert!(!health.observe("Carly write timed out: 0105"));
    }

    #[test]
    fn recoverable_read_errors_reset_transport_failure_count() {
        let mut health = SessionHealth::default();

        assert!(!health.observe("Carly write timed out: 010C"));
        assert!(!health.observe("conflicting 010C responses"));
        assert!(!health.observe("Carly write timed out: 010D"));
        assert!(health.unhealthy().is_none());
        assert!(!is_transport_failure("conflicting 010C responses"));
    }

    #[test]
    fn unhealthy_health_gate_prevents_a_third_transport_request() {
        let mut health = SessionHealth::default();
        let mut dispatched = 0;

        for error in [
            "Carly write timed out: 010C",
            "Carly write timed out: 010D",
            "Carly write timed out: 0105",
        ] {
            if health.unhealthy().is_some() {
                break;
            }
            dispatched += 1;
            health.observe(error);
        }

        assert_eq!(dispatched, 2);
        assert!(health.unhealthy().is_some());
    }

    #[test]
    fn normalizes_prompt_terminated_rpm_response() {
        assert_eq!(
            normalize_mode01("010C\r410C1AF8\r>", 0x0c, 2),
            Ok(vec![0x41, 0x0c, 0x1a, 0xf8])
        );
        assert_eq!(
            normalize_mode01(
                "04 41 0C 00 00 00 00 00\r04 41 0C 00 00 AA AA AA\r>",
                0x0c,
                2
            ),
            Ok(vec![0x41, 0x0c, 0x00, 0x00])
        );
        assert_eq!(
            normalize_mode01("SEARCHING...\r064100BE3EB813\r>", 0x00, 4),
            Ok(vec![0x41, 0x00, 0xbe, 0x3e, 0xb8, 0x13])
        );
    }

    #[test]
    fn normalizes_stored_dtc_responses_and_accepts_no_dtcs() {
        let responses =
            normalize_mode03_responses("03\r7E8 03 43 01 0C\r7E9 05 43 00 00 00 00\r>").unwrap();

        assert_eq!(responses.errors(), &[]);
        assert_eq!(responses.as_slice().len(), 2);
        assert_eq!(
            responses.as_slice()[0],
            DiagnosticResponse {
                responder: Some(ResponderIdentity::ElmHeader("7E8".into())),
                payload: vec![0x43, 0x01, 0x0c],
            }
        );
        assert_eq!(
            responses.as_slice()[1],
            DiagnosticResponse {
                responder: Some(ResponderIdentity::ElmHeader("7E9".into())),
                payload: vec![0x43, 0x00, 0x00, 0x00, 0x00],
            }
        );
    }

    #[test]
    fn retains_valid_responder_payloads_when_another_line_is_malformed() {
        let responses =
            normalize_mode03_responses("7E8 03 43 01 0C\r7E9 03 43 01\r7EA ERROR\r>").unwrap();

        assert_eq!(responses.as_slice().len(), 2);
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
        assert_eq!(responses.as_slice()[0].payload, [0x43, 0x01, 0x0c]);
        assert_eq!(
            responses.as_slice()[1].responder,
            Some(ResponderIdentity::ElmHeader("7E9".into()))
        );
        assert_eq!(responses.as_slice()[1].payload, [0x43, 0x01]);
        assert_eq!(responses.errors().len(), 2);
        assert_eq!(
            responses.errors()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E9".into()))
        );
        assert_eq!(
            responses.errors()[1].responder,
            Some(ResponderIdentity::ElmHeader("7EA".into()))
        );
    }

    #[tokio::test]
    async fn stored_dtc_transport_uses_only_the_bounded_mode03_command() {
        let mut exchange = ScriptedExchange::captured(vec!["43 01 0C\r>".into()]);

        let responses = read_elm_mode03_responses(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["03\r"]);
        assert_eq!(responses.as_slice()[0].payload, [0x43, 0x01, 0x0c]);
    }

    #[test]
    fn exposes_an_empty_mode03_response_as_recoverable_error() {
        let responses = normalize_mode03_responses("03\r>").unwrap();

        assert!(responses.is_empty());
        assert_eq!(responses.errors().len(), 1);
        assert_eq!(responses.errors()[0].responder, None);
    }

    #[test]
    fn preserves_elm_header_identity_without_calling_it_a_can_id() {
        let responses = normalize_mode01_responses(
            "7E8 04 41 0C 00 00 00 00\r7E9 04 41 0C 00 00 AA AA\r>",
            0x0c,
            2,
        )
        .unwrap();
        assert_eq!(responses.as_slice()[0].payload, [0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
        assert_eq!(
            responses.as_slice()[1].responder,
            Some(ResponderIdentity::ElmHeader("7E9".into()))
        );
        assert!(
            normalize_mode01("7E8 04 41 0C 00 00\r7E9 04 41 0C 00 04\r>", 0x0c, 2)
                .unwrap_err()
                .contains("responders: 7E8, 7E9")
        );
    }

    #[test]
    fn accepts_duplicate_payloads_but_selects_only_a_matching_responder() {
        let responses =
            normalize_mode01_responses("7E8 04 41 0C 00 00\r7E9 04 41 0C 00 00\r>", 0x0c, 2)
                .unwrap();
        assert_eq!(responses.as_slice().len(), 2);
        assert_eq!(
            responses
                .select(&ResponderIdentity::ElmHeader("7E9".into()))
                .unwrap(),
            [0x41, 0x0c, 0x00, 0x00]
        );
        assert!(responses
            .select(&ResponderIdentity::ElmHeader("7EA".into()))
            .is_err());
    }

    #[test]
    fn preserves_multi_byte_elm_headers_when_the_length_prefix_is_present() {
        let responses = normalize_mode01_responses("48 6B 10 04 41 0C 00 00\r>", 0x0c, 2).unwrap();
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("48 6B 10".into()))
        );
    }

    #[test]
    fn rejects_malformed_elm_header_explicitly() {
        assert!(normalize_mode01_responses("7XZ 04 41 0C 00 00\r>", 0x0c, 2)
            .unwrap_err()
            .contains("responder header"));
    }

    #[test]
    fn rejects_missing_or_truncated_rpm_response() {
        assert!(normalize_mode01("NO DATA\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("STOPPED\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("?\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("410C00\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("410C0000\r410C0004\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("7F0111\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("037F0111\r410C0000\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("41 0C ZZ 00\r>", 0x0c, 2).is_err());
        assert!(require_response("OK\rERROR\r>", "OK", true, "command failed").is_err());
        assert!(require_response("NOTCARLY\r>", "CARLY-UNIVERSAL", false, "identity").is_err());
    }

    #[test]
    fn suppresses_adapter_io_for_live_sessions_but_keeps_one_shot_format() {
        assert_eq!(adapter_io_line(false, "tx", "010C"), None);
        assert_eq!(
            adapter_io_line(true, "tx", "010C"),
            Some("adapter tx  010C".into())
        );
        assert_eq!(
            adapter_io_line(true, "rx", "410C0000\\r>"),
            Some("adapter rx  410C0000\\r>".into())
        );
    }

    #[test]
    fn mode01_support_bitmap_gates_target_pids() {
        let response = [0x41, 0x00, 0x08, 0x18, 0x00, 0x00];
        let support = PidSupport {
            pages: vec![u32::from_be_bytes(response[2..].try_into().unwrap())],
            discovery: Vec::new(),
        };
        assert!(supports_pid(&support, 0x05));
        assert!(supports_pid(&support, 0x0c));
        assert!(supports_pid(&support, 0x0d));
        assert!(!supports_pid(&support, 0x10));
        assert!(!supports_pid(&support, 0x00));

        let combined = normalize_pid_support_page("410008180000\r410000010000\r>", 0x00).unwrap();
        assert_eq!(combined, [0x41, 0x00, 0x08, 0x19, 0x00, 0x00]);
        let combined = PidSupport {
            pages: vec![u32::from_be_bytes(combined[2..].try_into().unwrap())],
            discovery: Vec::new(),
        };
        assert!(supports_pid(&combined, 0x05));
        assert!(supports_pid(&combined, 0x10));
    }

    #[tokio::test]
    async fn follows_support_pages_only_when_the_continuation_bit_is_set() {
        let mut exchange = ScriptedExchange::captured(vec![
            "410080000001\r>".into(),
            "412080000001\r>".into(),
            "414080000001\r>".into(),
        ]);

        let support = discover_pid_support(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["0100\r", "0120\r", "0140\r"]);
        assert_eq!(support.status(0x01), SignalSupportStatus::Supported);
        assert_eq!(support.status(0x20), SignalSupportStatus::Supported);
        assert_eq!(support.status(0x21), SignalSupportStatus::Supported);
        assert_eq!(support.status(0x40), SignalSupportStatus::Supported);
        assert_eq!(support.status(0x41), SignalSupportStatus::Supported);
        assert_eq!(support.status(0x42), SignalSupportStatus::Unsupported);
        assert_eq!(support.status(0x61), SignalSupportStatus::Unknown);
        assert_eq!(
            support.discovery,
            [
                SupportDiscovery {
                    request: [0x01, 0x00],
                    responder: None,
                    response: [0x41, 0x00, 0x80, 0x00, 0x00, 0x01],
                },
                SupportDiscovery {
                    request: [0x01, 0x20],
                    responder: None,
                    response: [0x41, 0x20, 0x80, 0x00, 0x00, 0x01],
                },
                SupportDiscovery {
                    request: [0x01, 0x40],
                    responder: None,
                    response: [0x41, 0x40, 0x80, 0x00, 0x00, 0x01],
                },
            ]
        );
    }

    #[tokio::test]
    async fn support_discovery_preserves_each_responder_and_payload() {
        let mut exchange = ScriptedExchange::captured(vec![
            "7E9 06 41 00 00 00 00 00\r7E8 06 41 00 80 00 00 00\r>".into(),
        ]);

        let support = discover_pid_support(&mut exchange).await.unwrap();

        assert_eq!(
            support.discovery,
            [
                SupportDiscovery {
                    request: [0x01, 0x00],
                    responder: Some(ResponderIdentity::ElmHeader("7E8".into())),
                    response: [0x41, 0x00, 0x80, 0x00, 0x00, 0x00],
                },
                SupportDiscovery {
                    request: [0x01, 0x00],
                    responder: Some(ResponderIdentity::ElmHeader("7E9".into())),
                    response: [0x41, 0x00, 0x00, 0x00, 0x00, 0x00],
                },
            ]
        );
        assert_eq!(support.status(0x01), SignalSupportStatus::Supported);
    }

    #[tokio::test]
    async fn elm_initialization_does_not_probe_pid_support() {
        let mut exchange = ScriptedExchange::captured(vec![
            "ELM327 v1.4 v100\r>".into(),
            "carly-universal v200\r>".into(),
            "ELM327 v1.4 v100\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
        ]);

        initialize_elm(&mut exchange).await.unwrap();

        assert_eq!(
            exchange.commands,
            ["ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r",]
        );
    }

    #[tokio::test]
    async fn bounded_functional_validation_queries_once_and_preserves_responders() {
        let mut exchange = ScriptedExchange::captured(vec![
            "7E9 06 41 00 00 00 00 00\r7E8 06 41 00 80 00 00 00\r>".into(),
        ]);

        let validation = validate_functional_support_exchange(&mut exchange)
            .await
            .unwrap();

        assert_eq!(exchange.commands, ["0100\r"]);
        assert_eq!(validation.len(), 2);
        assert_eq!(
            validation[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
        assert_eq!(
            validation[1].responder,
            Some(ResponderIdentity::ElmHeader("7E9".into()))
        );
    }

    #[test]
    fn assembles_fragmented_notifications_through_prompt_with_a_size_bound() {
        let mut response = Vec::new();
        assert!(!append_notification(&mut response, b"41 0C 00").unwrap());
        assert!(append_notification(&mut response, b" 00\r>").unwrap());
        assert_eq!(response, b"41 0C 00 00\r>");

        let mut oversized = vec![b'0'; MAX_RESPONSE];
        assert!(append_notification(&mut oversized, b"0").is_err());
    }

    #[tokio::test]
    async fn frames_fragments_and_rejects_missing_prompt_or_oversize_response() {
        let mut fragments =
            futures_util::stream::iter(vec![notification(b"41 0C 00"), notification(b" 00\r>")]);
        assert_eq!(
            wait_for_prompt(&mut fragments, &channel()).await,
            Ok("41 0C 00 00\r>".into())
        );

        let mut missing_prompt = futures_util::stream::iter(vec![notification(b"41 0C 00 00\r")]);
        assert!(wait_for_prompt(&mut missing_prompt, &channel())
            .await
            .is_err());

        let mut oversized =
            futures_util::stream::iter(vec![notification(&vec![b'0'; MAX_RESPONSE + 1])]);
        assert!(wait_for_prompt(&mut oversized, &channel()).await.is_err());
    }

    #[tokio::test]
    async fn prompt_wait_honors_timeout_without_hardware() {
        let mut notifications = futures_util::stream::pending::<ValueNotification>();
        assert!(timeout(
            Duration::from_millis(1),
            wait_for_prompt(&mut notifications, &channel())
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn replays_captured_zero_rpm_session_in_exact_command_order() {
        let mut exchange = ScriptedExchange::captured(captured_responses());
        let request = crate::prepare_read("engine.rpm").unwrap();
        let supported = initialize_with_support(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, request.pid()));
        let transaction = request
            .complete("user", read_elm(&mut exchange, request).await.unwrap())
            .unwrap();

        assert_eq!(exchange.commands[..9], INIT_COMMANDS);
        assert_eq!(exchange.commands[9], "010C\r");
        assert_eq!(transaction.response(), [0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(transaction.value(), 0.0);

        let path = std::env::temp_dir().join(format!(
            "obdentic-session-replay-{}-{}.tsv",
            std::process::id(),
            transaction.timestamp_ms()
        ));
        crate::record(&path, &transaction).unwrap();
        let replayed = crate::replay(&path).await.unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(replayed.response(), transaction.response());
        assert_eq!(replayed.value(), transaction.value());
    }

    #[tokio::test]
    async fn retries_conflicting_response_once_and_accepts_a_valid_retry() {
        let mut responses = captured_responses();
        responses[9] = "410C0000\r410C0004\r>".into();
        responses.push("410C0000\r>".into());
        let mut exchange = ScriptedExchange::captured(responses);
        let request = crate::prepare_read("engine.rpm").unwrap();
        let supported = initialize_with_support(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, request.pid()));

        assert_eq!(
            read_elm(&mut exchange, request).await.unwrap(),
            vec![0x41, 0x0c, 0x00, 0x00]
        );
        assert_eq!(&exchange.commands[9..], ["010C\r", "010C\r"]);
    }

    #[tokio::test]
    async fn targeted_read_configures_expected_headers_and_restores_functional_state() {
        let mut exchange = ScriptedExchange::captured(vec![
            "OK\r>".into(),
            "OK\r>".into(),
            "7E8 04 41 0C 00 00 00 00\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "410C0000\r>".into(),
        ]);
        let request = targeted_request();

        let read = read_elm_targeted_with_evidence(&mut exchange, &request)
            .await
            .unwrap();
        let functional = read_elm(&mut exchange, crate::prepare_read("engine.rpm").unwrap())
            .await
            .unwrap();

        assert_eq!(read.payload, vec![0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(functional, vec![0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(
            exchange.commands,
            [
                "ATSH 7E0\r",
                "ATCRA 7E8\r",
                "010C\r",
                "ATSP0\r",
                "ATSH 7DF\r",
                "ATCRA\r",
                "010C\r",
            ]
        );
    }

    #[tokio::test]
    async fn targeted_read_rejects_an_unexpected_responder() {
        let mut exchange = ScriptedExchange::captured(vec![
            "OK\r>".into(),
            "OK\r>".into(),
            "7E9 04 41 0C 00 00 00 00\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
            "OK\r>".into(),
        ]);

        let error = read_elm_targeted_with_evidence(&mut exchange, &targeted_request())
            .await
            .unwrap_err()
            .error;

        assert!(error.contains("unexpected responder 7E9"));
        assert_eq!(
            &exchange.commands[..3],
            ["ATSH 7E0\r", "ATCRA 7E8\r", "010C\r"]
        );
        assert_eq!(
            &exchange.commands[3..],
            ["ATSP0\r", "ATSH 7DF\r", "ATCRA\r"]
        );
    }

    #[test]
    fn targeted_request_cannot_escape_the_read_allowlist_or_address_namespace() {
        let request = crate::prepare_read("engine.rpm").unwrap();
        let context = crate::topology::ProtocolContext::new(
            crate::topology::Protocol::Obd2,
            crate::topology::AddressingContext::Physical,
        );
        assert!(TargetedReadRequest::new(
            request,
            crate::topology::RequestTarget::functional(context.clone()),
            ResponderIdentity::ElmHeader("7E8".into()),
        )
        .is_err());
        assert!(TargetedReadRequest::new(
            request,
            crate::topology::RequestTarget::concrete(
                context,
                crate::topology::RequestAddress::new("raw-can", "7E0"),
            ),
            ResponderIdentity::ElmHeader("7E8".into()),
        )
        .is_err());
        assert!(crate::prepare_read("dtc.clear").is_err());
    }

    #[tokio::test]
    async fn read_evidence_preserves_responder_and_payload_before_decode() {
        let mut exchange = ScriptedExchange::captured(vec!["7e8 04 41 0C 00 00 00 00\r>".into()]);
        let request = crate::prepare_read("engine.rpm").unwrap();

        let read = read_elm_with_evidence(&mut exchange, request)
            .await
            .unwrap();

        assert_eq!(read.observations.len(), 1);
        assert_eq!(
            read.observations[0].responses,
            vec![crate::capture_events::ResponderEvidence {
                responder: Some("7E8".into()),
                payload: vec![0x41, 0x0c, 0x00, 0x00],
            }]
        );
        assert_eq!(
            read.observations[0].selected_responder.as_deref(),
            Some("7E8")
        );
        assert_eq!(read.payload, vec![0x41, 0x0c, 0x00, 0x00]);
    }

    #[tokio::test]
    async fn read_evidence_keeps_both_attempts_when_conflict_remains() {
        let conflict = "7E8 04 41 0C 00 00 00 00\r7E9 04 41 0C 00 04 00 00\r>";
        let mut exchange = ScriptedExchange::captured(vec![conflict.into(), conflict.into()]);
        let request = crate::prepare_read("engine.rpm").unwrap();

        let failure = read_elm_with_evidence(&mut exchange, request)
            .await
            .unwrap_err();

        assert_eq!(failure.observations.len(), 2);
        assert_eq!(failure.observations[0].responses.len(), 2);
        assert_eq!(failure.observations[1].responses.len(), 2);
        assert_eq!(failure.observations[0].selected_responder, None);
        assert!(failure.observations[0].selection_error.is_some());
        assert!(failure.error.contains("conflicting 010C responses"));
    }

    #[tokio::test]
    async fn reports_raw_responses_after_one_conflict_retry() {
        let conflict = "410C0000\r410C0004\r>";
        let mut responses = captured_responses();
        responses[9] = conflict.into();
        responses.push(conflict.into());
        let mut exchange = ScriptedExchange::captured(responses);
        let request = crate::prepare_read("engine.rpm").unwrap();
        let supported = initialize_with_support(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, request.pid()));

        let error = read_elm(&mut exchange, request).await.unwrap_err();
        assert!(error.contains("conflicting 010C responses"));
        assert!(error.contains("410C0000\\r410C0004\\r>"), "{error}");
        assert_eq!(&exchange.commands[9..], ["010C\r", "010C\r"]);
    }

    #[tokio::test]
    async fn does_not_retry_other_normalization_errors() {
        let mut responses = captured_responses();
        responses[9] = "NO DATA\r>".into();
        let mut exchange = ScriptedExchange::captured(responses);
        let request = crate::prepare_read("engine.rpm").unwrap();
        let supported = initialize_with_support(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, request.pid()));

        assert!(read_elm(&mut exchange, request).await.is_err());
        assert_eq!(&exchange.commands[9..], ["010C\r"]);
    }

    #[tokio::test]
    async fn scripted_session_reads_each_closed_standard_signal() {
        for (semantic, command, response, value, unit) in [
            (
                "engine.coolant_temperature",
                "0105\r",
                "41055A\r>",
                50.0,
                "°C",
            ),
            ("vehicle.speed", "010D\r", "410D64\r>", 100.0, "km/h"),
            ("engine.maf", "0110\r", "411001F4\r>", 5.0, "g/s"),
        ] {
            let mut responses = captured_responses();
            responses[8] = "410008190000\r>".into();
            responses[9] = response.into();
            let mut exchange = ScriptedExchange::captured(responses);
            let request = crate::prepare_read(semantic).unwrap();
            let supported = initialize_with_support(&mut exchange).await.unwrap();
            assert!(supports_pid(&supported, request.pid()));
            let transaction = request
                .complete("user", read_elm(&mut exchange, request).await.unwrap())
                .unwrap();

            assert_eq!(exchange.commands[..9], INIT_COMMANDS);
            assert_eq!(exchange.commands[9], command);
            assert_eq!(transaction.value(), value);
            assert_eq!(transaction.unit(), unit);
        }
    }

    #[tokio::test]
    async fn initialization_and_pid_support_are_cached_across_sequential_reads() {
        let mut responses = captured_responses();
        responses.push("41055A\r>".into());
        let mut exchange = ScriptedExchange::captured(responses);
        let rpm = crate::prepare_read("engine.rpm").unwrap();
        let coolant = crate::prepare_read("engine.coolant_temperature").unwrap();

        let supported = initialize_with_support(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, rpm.pid()));
        assert!(supports_pid(&supported, coolant.pid()));
        let rpm = rpm
            .complete("user", read_elm(&mut exchange, rpm).await.unwrap())
            .unwrap();
        let coolant = coolant
            .complete("user", read_elm(&mut exchange, coolant).await.unwrap())
            .unwrap();

        assert_eq!(rpm.value(), 0.0);
        assert_eq!(coolant.value(), 50.0);
        assert_eq!(
            exchange.commands,
            [INIT_COMMANDS.as_slice(), &["010C\r", "0105\r"],].concat()
        );
        assert_eq!(
            exchange
                .commands
                .iter()
                .filter(|command| *command == "0100\r")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn session_failures_stop_before_later_commands() {
        for (index, response) in [
            (0, "unknown adapter\r>"),
            (1, "unknown identity\r>"),
            (3, "?\r>"),
            (8, "NO DATA\r>"),
            (9, "NO DATA\r>"),
        ] {
            let mut responses = captured_responses();
            responses[index] = response.into();
            let mut exchange = ScriptedExchange::captured(responses);

            let request = crate::prepare_read("engine.rpm").unwrap();
            let failed = if index < INIT_COMMANDS.len() {
                initialize_with_support(&mut exchange).await.is_err()
            } else {
                initialize_with_support(&mut exchange).await.unwrap();
                read_elm(&mut exchange, request).await.is_err()
            };
            assert!(failed);
            assert_eq!(
                exchange.commands,
                [INIT_COMMANDS.as_slice(), &["010C\r"]][..].concat()[..=index]
            );
        }
    }
}
