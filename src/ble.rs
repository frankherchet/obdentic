use crate::{
    adapter::CarlyCuaV200,
    capability::{CapabilityProvenance, HardwareCapability},
    ReadRequest, Transaction,
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub use crate::adapter::AdapterCandidate;
#[cfg(test)]
use crate::elm::ElmExchange;
use crate::elm::ElmSession;
#[cfg(test)]
pub(crate) use crate::elm::{
    discover_pid_support as discover_pid_support_with_limit, establish_elm_protocol,
    normalize_mode01, normalize_mode01_responses, normalize_mode03_responses,
    normalize_mode09_segments, normalize_pid_support_page, normalize_uds_responses, read_elm,
    read_elm_identity, read_elm_mode03_responses, read_elm_targeted_with_evidence,
    read_elm_with_evidence, require_response, supports_pid, validate_functional_support_exchange,
    PidSupport,
};
pub use crate::elm::{
    mode09_support_bitmap, DiagnosticResponse, DiagnosticResponseError, DiagnosticResponses,
    Mode09Pid, ProtocolNegotiation, ResponderIdentity, SignalSupport, SignalSupportStatus,
    SupportDiscovery, TargetedDpfProbeRequest, TargetedEcuIdentificationRequest,
    TargetedMode09Request, TargetedReadRequest,
};
pub(crate) use crate::elm::{ReadEvidenceError, ResponseObservation};

// Two consecutive transport failures stop a live session; data failures reset the count.
const TRANSPORT_FAILURE_THRESHOLD: u8 = 2;
const SESSION_UNHEALTHY_PREFIX: &str =
    "diagnostic session became unresponsive after repeated transport failures";

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

    /// Execute one closed EA189 candidate probe while retaining this session.
    pub async fn read_dpf_probe(
        &self,
        request: TargetedDpfProbeRequest,
    ) -> Result<DiagnosticResponses, String> {
        self.session.read_dpf_probe(request).await
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

#[derive(Debug, PartialEq)]
pub(crate) enum DpfProbeOutcome {
    Succeeded {
        responses: DiagnosticResponses,
        observations: Vec<ResponseObservation>,
    },
    Failed {
        error: String,
        observations: Vec<ResponseObservation>,
    },
}

impl DpfProbeOutcome {
    fn into_result(self) -> Result<DiagnosticResponses, String> {
        match self {
            Self::Succeeded { responses, .. } => Ok(responses),
            Self::Failed { error, .. } => Err(error),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum EcuIdentificationOutcome {
    Succeeded {
        responses: DiagnosticResponses,
        observations: Vec<ResponseObservation>,
    },
    Failed {
        error: String,
        observations: Vec<ResponseObservation>,
    },
}

impl EcuIdentificationOutcome {
    fn into_result(self) -> Result<DiagnosticResponses, String> {
        match self {
            Self::Succeeded { responses, .. } => Ok(responses),
            Self::Failed { error, .. } => Err(error),
        }
    }
}

pub async fn scan() -> Result<Vec<AdapterCandidate>, String> {
    crate::adapter::scan().await
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

/// Connect and initialize an ELM327 session, then establish the automatic
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
    start_session_mode(adapter_id, true).await
}

/// Start the same closed session while mirroring adapter TX/RX for a bounded
/// diagnostic probe. No caller-controlled ELM command path is exposed.
pub async fn start_session_with_adapter_io(adapter_id: &str) -> Result<SessionClient, String> {
    let session = DiagnosticSession::connect_with_adapter_io_mode(adapter_id, true, true).await?;
    Ok(start_session_actor(session))
}

async fn start_session_mode(
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

#[derive(Clone)]
pub struct SessionClient {
    sender: mpsc::Sender<SessionCommand>,
}

impl SessionClient {
    /// Return the conservative capability learned by the sequential session
    /// actor. The first result is the built-in fallback; completed reads then
    /// replace its latency and budget with measured evidence.
    pub async fn hardware_capability(&self) -> Result<HardwareCapability, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::Capability { reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())
    }

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

    pub async fn read_mode09(
        &self,
        request: TargetedMode09Request,
    ) -> Result<DiagnosticResponses, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::ReadMode09 { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    /// Execute the closed EA189 DPF UDS probe and return its raw normalized
    /// responses.  Negative or malformed responses are returned as errors,
    /// while the crate-visible evidence variant retains responder payloads.
    pub async fn read_dpf_probe(
        &self,
        request: TargetedDpfProbeRequest,
    ) -> Result<DiagnosticResponses, String> {
        self.read_dpf_probe_with_evidence(request)
            .await?
            .into_result()
    }

    pub(crate) async fn read_dpf_probe_with_evidence(
        &self,
        request: TargetedDpfProbeRequest,
    ) -> Result<DpfProbeOutcome, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::ReadDpfProbe { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    pub async fn read_ecu_identification(
        &self,
        request: TargetedEcuIdentificationRequest,
    ) -> Result<DiagnosticResponses, String> {
        self.read_ecu_identification_with_evidence(request)
            .await?
            .into_result()
    }

    pub(crate) async fn read_ecu_identification_with_evidence(
        &self,
        request: TargetedEcuIdentificationRequest,
    ) -> Result<EcuIdentificationOutcome, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::ReadEcuIdentification { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
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
    ReadMode09 {
        request: TargetedMode09Request,
        reply: oneshot::Sender<Result<DiagnosticResponses, String>>,
    },
    ReadDpfProbe {
        request: TargetedDpfProbeRequest,
        reply: oneshot::Sender<Result<DpfProbeOutcome, String>>,
    },
    ReadEcuIdentification {
        request: TargetedEcuIdentificationRequest,
        reply: oneshot::Sender<Result<EcuIdentificationOutcome, String>>,
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
    Capability {
        reply: oneshot::Sender<HardwareCapability>,
    },
}

/// Conservative, bounded session estimate. Keeping only the slowest completed
/// request makes the budget deterministic and errs toward under-booking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RequestServiceEstimator {
    maximum_service_time: Option<Duration>,
}

impl RequestServiceEstimator {
    fn observe(&mut self, service_time: Duration) {
        let service_time = service_time.max(Duration::from_nanos(1));
        self.maximum_service_time = Some(
            self.maximum_service_time
                .map_or(service_time, |maximum| maximum.max(service_time)),
        );
    }

    fn capability(self) -> HardwareCapability {
        let (latency, provenance) = match self.maximum_service_time {
            Some(latency) => (latency, CapabilityProvenance::MeasuredFromCapture),
            None => {
                let fallback = HardwareCapability::conservative_default();
                (
                    fallback.representative_read_latency(),
                    CapabilityProvenance::BuiltInDefault,
                )
            }
        };
        let budget = request_budget_for(latency);
        HardwareCapability::new(budget, latency, provenance)
            .expect("request service estimator always produces a valid capability")
    }
}

fn request_budget_for(service_time: Duration) -> u32 {
    let nanos = service_time.as_nanos().max(1);
    let budget = (1_000_000_000_u128 / nanos).max(1);
    u32::try_from(budget).unwrap_or(u32::MAX)
}

async fn session_actor(
    mut session: DiagnosticSession,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    let mut health = SessionHealth::default();
    let mut service = RequestServiceEstimator::default();
    let mut disconnect_done = false;
    while let Some(command) = commands.recv().await {
        match command {
            SessionCommand::Read { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                let outcome = session.read_with_evidence(request).await;
                process_read_outcome(
                    &mut session,
                    &mut health,
                    &mut service,
                    &mut disconnect_done,
                    outcome,
                    started.elapsed(),
                    reply,
                )
                .await;
            }
            SessionCommand::ReadTargeted { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                let outcome = session.read_targeted_with_evidence(request).await;
                process_read_outcome(
                    &mut session,
                    &mut health,
                    &mut service,
                    &mut disconnect_done,
                    outcome,
                    started.elapsed(),
                    reply,
                )
                .await;
            }
            SessionCommand::ReadMode09 { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                match session.read_mode09(request).await {
                    Ok(responses) => {
                        service.observe(started.elapsed());
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
                            service.observe(started.elapsed());
                            let _ = reply.send(Err(error));
                        }
                    }
                }
            }
            SessionCommand::ReadDpfProbe { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                let outcome = session.read_dpf_probe_with_evidence(request).await;
                process_dpf_probe_outcome(
                    &mut session,
                    &mut health,
                    &mut service,
                    &mut disconnect_done,
                    outcome,
                    started.elapsed(),
                    reply,
                )
                .await;
            }
            SessionCommand::ReadEcuIdentification { request, reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                let outcome = session.read_ecu_identification_with_evidence(request).await;
                process_ecu_identification_outcome(
                    &mut session,
                    &mut health,
                    &mut service,
                    &mut disconnect_done,
                    outcome,
                    started.elapsed(),
                    reply,
                )
                .await;
            }
            SessionCommand::ReadStoredDtcs { reply } => {
                if let Some(error) = health.unhealthy() {
                    let _ = reply.send(Err(error.to_owned()));
                    continue;
                }
                let started = Instant::now();
                match session.read_stored_dtcs().await {
                    Ok(responses) => {
                        service.observe(started.elapsed());
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
                            service.observe(started.elapsed());
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
                let _ = reply.send(
                    session
                        .elm_mut()
                        .map_or_else(|_| Vec::new(), |elm| elm.supported().discovery.clone()),
                );
            }
            SessionCommand::Capability { reply } => {
                let _ = reply.send(service.capability());
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
    service: &mut RequestServiceEstimator,
    disconnect_done: &mut bool,
    outcome: ReadOutcome,
    service_time: Duration,
    reply: oneshot::Sender<Result<ReadOutcome, String>>,
) {
    if let Some(error) = health.unhealthy() {
        let _ = reply.send(Err(error.to_owned()));
        return;
    }
    match outcome {
        ReadOutcome::Succeeded { .. } => {
            service.observe(service_time);
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
                service.observe(service_time);
                let _ = reply.send(Ok(ReadOutcome::Failed {
                    error,
                    observations,
                }));
            }
        }
    }
}

async fn process_dpf_probe_outcome(
    session: &mut DiagnosticSession,
    health: &mut SessionHealth,
    service: &mut RequestServiceEstimator,
    disconnect_done: &mut bool,
    outcome: DpfProbeOutcome,
    service_time: Duration,
    reply: oneshot::Sender<Result<DpfProbeOutcome, String>>,
) {
    if let Some(error) = health.unhealthy() {
        let _ = reply.send(Err(error.to_owned()));
        return;
    }
    match outcome {
        DpfProbeOutcome::Succeeded { .. } => {
            service.observe(service_time);
            health.success();
            let _ = reply.send(Ok(outcome));
        }
        DpfProbeOutcome::Failed {
            error,
            observations,
        } => {
            if health.observe(&error) {
                let fatal = health.unhealthy().unwrap().to_owned();
                session.disconnect_best_effort().await;
                *disconnect_done = true;
                let _ = reply.send(Ok(DpfProbeOutcome::Failed {
                    error: fatal,
                    observations,
                }));
            } else {
                service.observe(service_time);
                let _ = reply.send(Ok(DpfProbeOutcome::Failed {
                    error,
                    observations,
                }));
            }
        }
    }
}

async fn process_ecu_identification_outcome(
    session: &mut DiagnosticSession,
    health: &mut SessionHealth,
    service: &mut RequestServiceEstimator,
    disconnect_done: &mut bool,
    outcome: EcuIdentificationOutcome,
    service_time: Duration,
    reply: oneshot::Sender<Result<EcuIdentificationOutcome, String>>,
) {
    if let Some(error) = health.unhealthy() {
        let _ = reply.send(Err(error.to_owned()));
        return;
    }
    match outcome {
        EcuIdentificationOutcome::Succeeded { .. } => {
            service.observe(service_time);
            health.success();
            let _ = reply.send(Ok(outcome));
        }
        EcuIdentificationOutcome::Failed {
            error,
            observations,
        } => {
            if health.observe(&error) {
                let fatal = health.unhealthy().unwrap().to_owned();
                session.disconnect_best_effort().await;
                *disconnect_done = true;
                let _ = reply.send(Ok(EcuIdentificationOutcome::Failed {
                    error: fatal,
                    observations,
                }));
            } else {
                service.observe(service_time);
                let _ = reply.send(Ok(EcuIdentificationOutcome::Failed {
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

/// One connected, initialized, read-only Carly diagnostic path.
pub struct DiagnosticSession {
    elm: Option<ElmSession<CarlyCuaV200>>,
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
        let backend = CarlyCuaV200::connect(adapter_id, show_adapter_io).await?;
        let mut session = Self {
            elm: Some(ElmSession::new(backend)),
        };
        if discover_support {
            if let Err(error) = session.discover_support().await {
                session.disconnect_best_effort().await;
                return Err(error);
            }
        }
        Ok(session)
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

    async fn read_mode09(
        &mut self,
        request: TargetedMode09Request,
    ) -> Result<DiagnosticResponses, String> {
        self.elm_mut()?.read_mode09(&request).await
    }

    async fn read_dpf_probe_with_evidence(
        &mut self,
        request: TargetedDpfProbeRequest,
    ) -> DpfProbeOutcome {
        let read = match self.elm_mut() {
            Ok(session) => session.read_dpf_probe_with_evidence(&request).await,
            Err(error) => Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            }),
        };
        match read {
            Ok(read) => DpfProbeOutcome::Succeeded {
                responses: read.responses,
                observations: read.observations,
            },
            Err(error) => DpfProbeOutcome::Failed {
                error: error.error,
                observations: error.observations,
            },
        }
    }

    async fn read_ecu_identification_with_evidence(
        &mut self,
        request: TargetedEcuIdentificationRequest,
    ) -> EcuIdentificationOutcome {
        let read = match self.elm_mut() {
            Ok(session) => {
                session
                    .read_ecu_identification_with_evidence(&request)
                    .await
            }
            Err(error) => Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            }),
        };
        match read {
            Ok(read) => EcuIdentificationOutcome::Succeeded {
                responses: read.responses,
                observations: read.observations,
            },
            Err(error) => EcuIdentificationOutcome::Failed {
                error: error.error,
                observations: error.observations,
            },
        }
    }

    async fn read_stored_dtcs(&mut self) -> Result<DiagnosticResponses, String> {
        self.elm_mut()?.read_stored_dtcs().await
    }

    pub async fn identify(&mut self) -> Result<crate::identity::VehicleIdentity, String> {
        self.elm_mut()?.identify().await
    }

    async fn validate_functional_support(&mut self) -> Result<Vec<SupportDiscovery>, String> {
        self.elm_mut()?.validate_functional_support().await
    }

    async fn establish_protocol(&mut self) -> Result<ProtocolNegotiation, String> {
        self.elm_mut()?.establish_protocol().await
    }

    async fn read_with_evidence(&mut self, request: ReadRequest) -> ReadOutcome {
        let read = match self.elm_mut() {
            Ok(session) => session.read_with_evidence(request).await,
            Err(error) => Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            }),
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
        let read = match self.elm_mut() {
            Ok(session) => session.read_targeted_with_evidence(&request).await,
            Err(error) => Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            }),
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
        let Some(session) = self.elm.take() else {
            return Ok(());
        };
        let mut backend = session.into_exchange();
        backend.disconnect().await
    }

    async fn disconnect_best_effort(&mut self) {
        if let Some(session) = self.elm.take() {
            let mut backend = session.into_exchange();
            backend.disconnect_best_effort().await;
        }
    }

    async fn discover_support(&mut self) -> Result<(), String> {
        self.elm_mut()?
            .discover_support(highest_catalog_page())
            .await
    }

    fn signal_support(&self) -> Vec<SignalSupport> {
        crate::vehicle::signals()
            .iter()
            .map(|signal| SignalSupport {
                semantic: signal.metadata().semantic,
                status: self
                    .elm
                    .as_ref()
                    .map_or(SignalSupportStatus::Unknown, |session| {
                        session.supported().status(signal.request().pid())
                    }),
            })
            .collect()
    }

    fn elm_mut(&mut self) -> Result<&mut ElmSession<CarlyCuaV200>, String> {
        self.elm
            .as_mut()
            .ok_or_else(|| "diagnostic session is closed".to_string())
    }
}

#[cfg(test)]
async fn discover_pid_support<E>(exchange: &mut E) -> Result<PidSupport, String>
where
    E: ElmExchange,
{
    discover_pid_support_with_limit(exchange, highest_catalog_page()).await
}

fn highest_catalog_page() -> u8 {
    crate::vehicle::signals()
        .iter()
        .map(|signal| signal.request().pid().saturating_sub(1) & !0x1f)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

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
        crate::adapter::initialize_carly(exchange).await?;
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

    #[test]
    fn normalizes_a_closed_uds_did_response_with_carly_padding() {
        let responses = normalize_uds_responses("7E8 05 62 11 4F 04 F8 55 55\r>", 0x114f).unwrap();
        assert_eq!(responses.as_slice().len(), 1);
        assert_eq!(
            responses.as_slice()[0].responder.as_ref().unwrap().as_str(),
            "7E8"
        );
        assert_eq!(
            responses.as_slice()[0].payload,
            [0x62, 0x11, 0x4f, 0x04, 0xf8]
        );
        assert!(responses.errors().is_empty());
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
                    SessionCommand::ReadMode09 { reply, .. } => {
                        let _ = reply.send(Err("Mode 09 test request not scripted".into()));
                    }
                    SessionCommand::ReadDpfProbe { reply, .. } => {
                        let _ = reply.send(Err("DPF probe test request not scripted".into()));
                    }
                    SessionCommand::ReadEcuIdentification { reply, .. } => {
                        let _ =
                            reply.send(Err("ECU identification test request not scripted".into()));
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
                    SessionCommand::Capability { reply } => {
                        let _ = reply.send(HardwareCapability::conservative_default());
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

    #[tokio::test]
    async fn session_client_reads_actor_owned_hardware_capability() {
        let (sender, mut commands) = mpsc::channel(1);
        let actor = tokio::spawn(async move {
            if let Some(SessionCommand::Capability { reply }) = commands.recv().await {
                let _ = reply.send(
                    HardwareCapability::new(
                        3,
                        Duration::from_millis(280),
                        CapabilityProvenance::MeasuredFromCapture,
                    )
                    .unwrap(),
                );
            }
        });
        let client = SessionClient { sender };

        let capability = client.hardware_capability().await.unwrap();

        assert_eq!(capability.request_budget_per_second(), 3);
        assert_eq!(
            capability.representative_read_latency(),
            Duration::from_millis(280)
        );
        assert_eq!(
            capability.provenance(),
            CapabilityProvenance::MeasuredFromCapture
        );
        actor.await.unwrap();
    }

    #[test]
    fn request_service_estimator_uses_slowest_completed_attempt() {
        let mut estimator = RequestServiceEstimator::default();
        assert_eq!(
            estimator.capability(),
            HardwareCapability::conservative_default()
        );

        estimator.observe(Duration::from_millis(280));
        estimator.observe(Duration::from_millis(220));
        let capability = estimator.capability();
        assert_eq!(capability.request_budget_per_second(), 3);
        assert_eq!(
            capability.representative_read_latency(),
            Duration::from_millis(280)
        );
        assert_eq!(
            capability.provenance(),
            CapabilityProvenance::MeasuredFromCapture
        );

        estimator.observe(Duration::from_secs(2));
        assert_eq!(estimator.capability().request_budget_per_second(), 1);
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
    fn accepts_carly_targeted_rpm_frame_with_length_prefixed_55_padding() {
        let responses =
            normalize_mode01_responses("7E8 04 41 0C 00 00 55 55 55\r>", 0x0c, 2).unwrap();

        assert_eq!(responses.as_slice().len(), 1);
        assert_eq!(responses.as_slice()[0].payload, [0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
    }

    #[test]
    fn rejects_55_padding_without_length_prefix() {
        let error = normalize_mode01_responses("7E8 41 0C 00 00 55\r>", 0x0c, 2).unwrap_err();

        assert!(error.contains("unexpected bytes after OBD-II response"));
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
    fn normalizes_compact_hardware_no_dtc_responses_per_responder() {
        let responses = normalize_mode03_responses(
            "7E8 02 43 00 00 00 00 00 00\r7E9 02 43 00 AA AA AA AA AA\r>",
        )
        .unwrap();

        assert!(responses.errors().is_empty());
        assert_eq!(
            responses
                .as_slice()
                .iter()
                .map(|response| (response.responder.clone(), response.payload.clone()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some(ResponderIdentity::ElmHeader("7E8".into())),
                    vec![0x43, 0x00]
                ),
                (
                    Some(ResponderIdentity::ElmHeader("7E9".into())),
                    vec![0x43, 0x00]
                ),
            ]
        );
    }

    #[test]
    fn reassembles_mode03_iso_tp_frames_per_responder() {
        let responses = normalize_mode03_responses(
            "7E8 10 0B 43 01 0C 02 0D 00\r7E9 10 0B 43 00 00 00 00 00\r7E8 21 00 00 00 00 00 00 00\r7E9 21 00 00 00 00 00 00 00\r>",
        )
        .unwrap();

        assert_eq!(responses.errors(), &[]);
        assert_eq!(responses.as_slice().len(), 2);
        assert_eq!(
            responses.as_slice()[0],
            DiagnosticResponse {
                responder: Some(ResponderIdentity::ElmHeader("7E8".into())),
                payload: vec![0x43, 0x01, 0x0c, 0x02, 0x0d, 0, 0, 0, 0, 0, 0],
            }
        );
        assert_eq!(
            responses.as_slice()[1],
            DiagnosticResponse {
                responder: Some(ResponderIdentity::ElmHeader("7E9".into())),
                payload: vec![0x43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            }
        );
    }

    #[test]
    fn rejects_mode03_iso_tp_sequence_and_truncation_errors() {
        let sequence =
            normalize_mode03_responses("7E8 10 0B 43 01 0C 02 0D 00\r7E8 22 00 00\r>").unwrap();
        assert!(sequence.is_empty());
        assert_eq!(sequence.errors().len(), 1);
        assert!(sequence.errors()[0].error.contains("sequence mismatch"));

        let truncated = normalize_mode03_responses("7E8 10 0B 43 01 0C 02 0D 00\r>").unwrap();
        assert!(truncated.is_empty());
        assert_eq!(truncated.errors().len(), 1);
        assert!(truncated.errors()[0].error.contains("truncated"));
    }

    #[test]
    fn does_not_mix_interleaved_mode03_responders() {
        let responses = normalize_mode03_responses(
            "7E8 10 0B 43 01 0C 02 0D 00\r7E9 21 00 00 00 00 00 00 00\r7E8 21 00 00 00 00 00 00 00\r>",
        )
        .unwrap();

        assert_eq!(responses.as_slice().len(), 1);
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
        assert_eq!(responses.errors().len(), 1);
        assert_eq!(
            responses.errors()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E9".into()))
        );
        assert!(responses.errors()[0].error.contains("without first frame"));
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

    #[tokio::test]
    async fn protocol_negotiation_keeps_hardware_0100_evidence_out_of_mode03() {
        let mut exchange = ScriptedExchange::captured(vec![
            "SEARCHING...\r7E8 06 41 00 98 3B A0 13 00\r7E9 06 41 00 98 18 00 01 AA\r>".into(),
            "7E8 03 43 00 00\r7E9 03 43 01 0C\r>".into(),
        ]);

        let negotiation = establish_elm_protocol(&mut exchange).await.unwrap();
        let responses = read_elm_mode03_responses(&mut exchange).await.unwrap();

        assert_eq!(exchange.commands, ["0100\r", "03\r"]);
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
    async fn identity_negotiates_protocol_before_mode09_pid02() {
        let mut exchange = ScriptedExchange::captured(vec![
            "7E8 06 41 00 98 3B A0 13 00\r>".into(),
            "7E8 10 14 49 02 01 57 56 57\r7E8 21 5A 5A 5A 31 4A 5A 58\r7E8 22 57 30 30 30 30 30 31\r>".into(),
        ]);

        let identity = read_elm_identity(&mut exchange).await.unwrap();

        assert_eq!(identity.vin().as_str(), "WVWZZZ1JZXW000001");
        assert_eq!(exchange.commands, ["0100\r", "0902\r"]);
    }

    #[test]
    fn mode09_isotp_rejects_truncated_or_out_of_sequence_frames() {
        assert_eq!(
            normalize_mode09_segments("7E8 10 14 49 02 01 57 56 57\r>"),
            Err("truncated ISO-TP Mode 09 response".into())
        );
        assert_eq!(
            normalize_mode09_segments(
                "7E8 10 14 49 02 01 57 56 57\r7E8 22 5A 5A 5A 31 4A 5A 58\r>",
            ),
            Err("malformed ISO-TP Mode 09 consecutive frame".into())
        );
    }

    #[tokio::test]
    async fn failed_protocol_negotiation_never_dispatches_mode03() {
        let mut exchange = ScriptedExchange::captured(vec!["NO DATA\r>".into()]);

        assert!(establish_elm_protocol(&mut exchange).await.is_err());
        assert_eq!(exchange.commands, ["0100\r"]);
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
        assert!(require_response("NOT-EXPECTED\r>", "EXPECTED", false, "identity").is_err());
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
