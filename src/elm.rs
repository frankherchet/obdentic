use crate::{
    topology::{AddressingContext, Protocol, RequestTarget},
    ReadRequest,
};
use std::time::Duration;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MODE03_COMMAND: &str = "03\r";
const STALE_MODE01_RESPONSE_PREFIX: &str = "stale unrelated OBD-II Mode 01 response from responder";
const IGNORED_MODE01_RESPONSE_PREFIX: &str =
    "ignored unrelated OBD-II Mode 01 response from responder";

/// The small protocol seam shared by ELM dialect users and transport
/// backends.  The backend owns the actual byte exchange; this module owns
/// only ELM command sequencing and response validation.
pub(crate) trait ElmExchange {
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String>;
}

/// Verify the generic ELM327 identity before any dialect-specific setup.
pub(crate) async fn verify_elm327<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("ATI\r", Duration::from_secs(3)).await?;
    require_response(
        &response,
        "ELM327",
        false,
        "ATI did not identify an ELM327 adapter",
    )
}

/// Complete generic ELM initialization after a backend has performed its
/// adapter-specific identity check while keeping the established wire order unchanged.
pub(crate) async fn initialize_elm<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
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

pub(crate) fn require_response(
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PidSupport {
    pub(crate) pages: Vec<u32>,
    pub(crate) discovery: Vec<SupportDiscovery>,
}

impl PidSupport {
    pub(crate) fn supports_pid(&self, pid: u8) -> bool {
        self.status(pid) == SignalSupportStatus::Supported
    }

    pub(crate) fn status(&self, pid: u8) -> SignalSupportStatus {
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

/// The closed EA189 DPF UDS probe.  The profile fixes the DID; callers can
/// provide only independently validated physical routing evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetedDpfProbeRequest {
    operation: crate::protocol::ReadOperation,
    target: RequestTarget,
    expected_responder: ResponderIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetedEcuIdentificationRequest {
    candidate: crate::ecu_identification::IdentificationCandidate,
    operation: crate::protocol::ReadOperation,
    target: RequestTarget,
    expected_responder: ResponderIdentity,
}

impl TargetedDpfProbeRequest {
    /// Construct a closed EA189 probe from validated engine target evidence.
    pub fn from_mapping(
        probe: crate::ea189::Ea189DpfProbe,
        mapping: &crate::vehicle_knowledge::EcuTargetMapping,
    ) -> Result<Self, String> {
        if mapping.role().role() != &crate::topology::EcuRole::Engine {
            return Err("EA189 DPF probes require validated engine target evidence".into());
        }
        let target = mapping.target().target().clone();
        if mapping.expected_responder().context() != target.context() {
            return Err("EA189 DPF probe target and responder contexts differ".into());
        }
        let expected_responder = mapping
            .expected_responder()
            .value()
            .ok_or_else(|| "EA189 DPF probes require an expected responder".to_string())?;
        Self::new(
            probe,
            target,
            ResponderIdentity::ElmHeader(expected_responder.to_owned()),
        )
    }

    fn new(
        probe: crate::ea189::Ea189DpfProbe,
        target: RequestTarget,
        expected_responder: ResponderIdentity,
    ) -> Result<Self, String> {
        validate_request_target(&target)?;
        validate_elm_header(&expected_responder, "expected responder")?;
        Ok(Self {
            operation: crate::protocol::ReadOperation::uds_read_data_by_identifier(probe.id()),
            target,
            expected_responder: ResponderIdentity::ElmHeader(
                expected_responder.as_str().to_ascii_uppercase(),
            ),
        })
    }

    pub fn did(&self) -> u16 {
        self.operation.did()
    }

    pub fn request_bytes(&self) -> [u8; 3] {
        self.operation.request_bytes()
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn expected_responder(&self) -> &ResponderIdentity {
        &self.expected_responder
    }
}

impl TargetedEcuIdentificationRequest {
    pub fn from_evidence(
        candidate: &crate::ecu_identification::IdentificationCandidate,
        target: &crate::topology::RequestTargetEvidence,
        expected_responder: &crate::topology::ResponderIdentity,
    ) -> Result<Self, String> {
        if expected_responder.context() != target.target().context() {
            return Err("ECU identification target and responder contexts differ".into());
        }
        let responder = expected_responder.value().ok_or_else(|| {
            "ECU identification requires an evidenced expected responder".to_string()
        })?;
        Self::new(
            candidate.clone(),
            target.target().clone(),
            ResponderIdentity::ElmHeader(responder.to_owned()),
        )
    }

    fn new(
        candidate: crate::ecu_identification::IdentificationCandidate,
        target: RequestTarget,
        expected_responder: ResponderIdentity,
    ) -> Result<Self, String> {
        validate_request_target(&target)?;
        validate_elm_header(&expected_responder, "expected responder")?;
        let operation = candidate.operation();
        if operation.request_bytes() != candidate.request_bytes() {
            return Err("ECU identification candidate did not resolve deterministically".into());
        }
        Ok(Self {
            candidate,
            operation,
            target,
            expected_responder: ResponderIdentity::ElmHeader(
                expected_responder.as_str().to_ascii_uppercase(),
            ),
        })
    }

    pub fn candidate(&self) -> &crate::ecu_identification::IdentificationCandidate {
        &self.candidate
    }

    pub fn did(&self) -> u16 {
        self.operation.did()
    }

    pub fn request_bytes(&self) -> [u8; 3] {
        self.operation.request_bytes()
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn expected_responder(&self) -> &ResponderIdentity {
        &self.expected_responder
    }
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

pub(crate) fn validate_request_target(target: &RequestTarget) -> Result<(), String> {
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

pub(crate) fn validate_elm_header(identity: &ResponderIdentity, label: &str) -> Result<(), String> {
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

    pub(crate) fn observation(&self, selection_error: Option<String>) -> ResponseObservation {
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

    pub(crate) fn unambiguous_payload(&self, pid: u8) -> Result<Vec<u8>, String> {
        let first = self
            .responses
            .first()
            .ok_or_else(|| format!("01{pid:02X} response not found"))?;
        if !first.payload.starts_with(&[0x41, pid]) {
            return Err(format!("01{pid:02X} response not found"));
        }
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

pub(crate) async fn discover_pid_support<E>(
    exchange: &mut E,
    highest_page: u8,
) -> Result<PidSupport, String>
where
    E: ElmExchange,
{
    let mut pages = Vec::new();
    let mut discovery = Vec::new();
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

pub(crate) async fn validate_functional_support_exchange<E>(
    exchange: &mut E,
) -> Result<Vec<SupportDiscovery>, String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("0100\r", COMMAND_TIMEOUT).await?;
    normalize_pid_support_page_with_evidence(&response, 0x00).map(|(_, observations)| observations)
}

/// Force `ATSP0` auto-selection to finish before a semantic diagnostic job.
/// `01 00` is a fixed standards-based read-only probe already used elsewhere
/// for functional support validation; no caller-supplied payload is accepted.
pub(crate) async fn establish_elm_protocol<E>(
    exchange: &mut E,
) -> Result<ProtocolNegotiation, String>
where
    E: ElmExchange,
{
    let observations = validate_functional_support_exchange(exchange).await?;
    Ok(ProtocolNegotiation { observations })
}

pub(crate) async fn read_elm_identity<E>(
    exchange: &mut E,
) -> Result<crate::identity::VehicleIdentity, String>
where
    E: ElmExchange,
{
    establish_elm_protocol(exchange).await?;
    let response = exchange.exchange("0902\r", COMMAND_TIMEOUT).await?;
    let segments = normalize_mode09_segments(&response)?;
    crate::identity::decode_mode09_pid02(&segments).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) async fn read_elm<E>(exchange: &mut E, request: ReadRequest) -> Result<Vec<u8>, String>
where
    E: ElmExchange,
{
    read_elm_with_evidence(exchange, request)
        .await
        .map(|read| read.payload)
        .map_err(|error| error.error)
}

#[derive(Debug)]
pub(crate) struct ReadEvidence {
    pub(crate) payload: Vec<u8>,
    pub(crate) observations: Vec<ResponseObservation>,
}

#[derive(Debug)]
pub(crate) struct ReadEvidenceError {
    pub(crate) error: String,
    pub(crate) observations: Vec<ResponseObservation>,
}

#[derive(Debug)]
pub(crate) struct DpfProbeReadEvidence {
    pub(crate) responses: DiagnosticResponses,
    pub(crate) observations: Vec<ResponseObservation>,
}

#[derive(Debug)]
pub(crate) struct EcuIdentificationReadEvidence {
    pub(crate) responses: DiagnosticResponses,
    pub(crate) observations: Vec<ResponseObservation>,
}

/// A reusable, closed ELM session over any adapter exchange.
///
/// The exchange is deliberately private: this type exposes only the
/// allowlisted semantic operations and never a caller-supplied command path.
pub(crate) struct ElmSession<E> {
    exchange: E,
    supported: PidSupport,
}

impl<E> ElmSession<E>
where
    E: ElmExchange,
{
    pub(crate) fn new(exchange: E) -> Self {
        Self {
            exchange,
            supported: PidSupport::default(),
        }
    }

    pub(crate) fn supported(&self) -> &PidSupport {
        &self.supported
    }

    pub(crate) fn into_exchange(self) -> E {
        self.exchange
    }

    pub(crate) async fn discover_support(&mut self, highest_page: u8) -> Result<(), String> {
        self.supported = discover_pid_support(&mut self.exchange, highest_page).await?;
        Ok(())
    }

    pub(crate) async fn establish_protocol(&mut self) -> Result<ProtocolNegotiation, String> {
        establish_elm_protocol(&mut self.exchange).await
    }

    pub(crate) async fn identify(&mut self) -> Result<crate::identity::VehicleIdentity, String> {
        read_elm_identity(&mut self.exchange).await
    }

    pub(crate) async fn validate_functional_support(
        &mut self,
    ) -> Result<Vec<SupportDiscovery>, String> {
        validate_functional_support_exchange(&mut self.exchange).await
    }

    pub(crate) async fn read_with_evidence(
        &mut self,
        request: ReadRequest,
    ) -> Result<ReadEvidence, ReadEvidenceError> {
        if !self.supported.supports_pid(request.pid()) {
            return Err(ReadEvidenceError {
                error: format!(
                    "vehicle does not advertise support for {}",
                    crate::hex(&request.bytes())
                ),
                observations: Vec::new(),
            });
        }
        read_elm_with_evidence(&mut self.exchange, request).await
    }

    pub(crate) async fn read_targeted_with_evidence(
        &mut self,
        request: &TargetedReadRequest,
    ) -> Result<ReadEvidence, ReadEvidenceError> {
        read_elm_targeted_with_evidence(&mut self.exchange, request).await
    }

    pub(crate) async fn read_stored_dtcs(&mut self) -> Result<DiagnosticResponses, String> {
        read_elm_mode03_responses(&mut self.exchange).await
    }

    pub(crate) async fn read_dpf_probe_with_evidence(
        &mut self,
        request: &TargetedDpfProbeRequest,
    ) -> Result<DpfProbeReadEvidence, ReadEvidenceError> {
        configure_target(
            &mut self.exchange,
            request.target(),
            request.expected_responder(),
        )
        .await
        .map_err(|error| ReadEvidenceError {
            error,
            observations: Vec::new(),
        })?;

        let responses = read_elm_uds_responses(&mut self.exchange, request)
            .await
            .map_err(|error| ReadEvidenceError {
                error,
                observations: Vec::new(),
            })?;
        Ok(DpfProbeReadEvidence {
            observations: vec![responses.observation(None)],
            responses,
        })
    }

    pub(crate) async fn read_ecu_identification_with_evidence(
        &mut self,
        request: &TargetedEcuIdentificationRequest,
    ) -> Result<EcuIdentificationReadEvidence, ReadEvidenceError> {
        if let Err(error) = configure_target(
            &mut self.exchange,
            request.target(),
            request.expected_responder(),
        )
        .await
        {
            let restore = restore_functional(&mut self.exchange).await;
            return Err(ReadEvidenceError {
                error: combine_setup_errors(error, restore),
                observations: Vec::new(),
            });
        }

        let read = match read_elm_ecu_identification_responses(&mut self.exchange, request).await {
            Ok(first) if should_retry_stale_uds_response(&first) => {
                match read_elm_ecu_identification_responses(&mut self.exchange, request).await {
                    Ok(retry) => {
                        let responses = merge_response_attempts(first, retry);
                        let selection_error =
                            targeted_payload(&responses, request.expected_responder()).err();
                        Ok(EcuIdentificationReadEvidence {
                            observations: vec![responses.observation(selection_error)],
                            responses,
                        })
                    }
                    Err(retry_error) => {
                        let first_error = format!(
                            "ECU identification retry failed: {retry_error}; first ELM response={}",
                            first.raw_response().escape_default()
                        );
                        Err(ReadEvidenceError {
                            error: first_error.clone(),
                            observations: vec![first.observation(Some(first_error))],
                        })
                    }
                }
            }
            Ok(first) => {
                let selection_error = targeted_payload(&first, request.expected_responder()).err();
                Ok(EcuIdentificationReadEvidence {
                    observations: vec![first.observation(selection_error)],
                    responses: first,
                })
            }
            Err(error) => Err(ReadEvidenceError {
                error,
                observations: Vec::new(),
            }),
        };
        let restore = restore_functional(&mut self.exchange).await;
        match (read, restore) {
            (Ok(read), Ok(())) => Ok(read),
            (Err(error), Ok(())) => Err(error),
            (Ok(read), Err(error)) => Err(ReadEvidenceError {
                error: format!(
                    "ECU identification read succeeded; restoring functional addressing failed: {error}"
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
}

fn should_retry_stale_uds_response(responses: &DiagnosticResponses) -> bool {
    responses.responses.is_empty()
        && !responses.errors.is_empty()
        && responses.errors.iter().all(is_ignored_mode01_response)
}

fn should_retry_stale_mode01_response(responses: &DiagnosticResponses, pid: u8) -> bool {
    !responses.responses.is_empty()
        && responses.responses.len() == responses.errors.len()
        && responses.responses.iter().all(|response| {
            response.payload.first() == Some(&0x41)
                && response
                    .payload
                    .get(1)
                    .is_some_and(|observed| *observed != pid)
        })
        && responses
            .errors
            .iter()
            .all(|error| error.error.starts_with(STALE_MODE01_RESPONSE_PREFIX))
}

fn is_ignored_mode01_response(error: &DiagnosticResponseError) -> bool {
    error.responder.is_none() && error.error.starts_with(IGNORED_MODE01_RESPONSE_PREFIX)
}

fn merge_response_attempts(
    mut first: DiagnosticResponses,
    retry: DiagnosticResponses,
) -> DiagnosticResponses {
    first.responses.extend(retry.responses);
    first.errors.extend(retry.errors);
    first.raw_response.push('\n');
    first.raw_response.push_str(&retry.raw_response);
    first
}

pub(crate) async fn read_elm_with_evidence<E>(
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
        Err(error) if should_retry_stale_mode01_response(&first, request.pid()) => {
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

pub(crate) async fn read_elm_targeted_with_evidence<E>(
    exchange: &mut E,
    request: &TargetedReadRequest,
) -> Result<ReadEvidence, ReadEvidenceError>
where
    E: ElmExchange,
{
    if let Err(error) =
        configure_target(exchange, request.target(), request.expected_responder()).await
    {
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

pub(crate) async fn configure_target<E>(
    exchange: &mut E,
    target: &RequestTarget,
    expected_responder: &ResponderIdentity,
) -> Result<(), String>
where
    E: ElmExchange,
{
    let address = target
        .address()
        .expect("validated targeted request has a concrete address");
    let target_header = address.value().to_ascii_uppercase();
    let expected_header = expected_responder.as_str();
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

pub(crate) async fn restore_functional<E>(exchange: &mut E) -> Result<(), String>
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
    match normalize_mode01_responses(&response, request.pid(), request.data_len()) {
        Ok(responses) => Ok(responses),
        Err(error) => match normalize_stale_mode01_responses(&response, request.pid()) {
            Ok(responses) => Ok(responses),
            Err(_) => Err(error),
        },
    }
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

pub(crate) async fn read_elm_uds_responses<E>(
    exchange: &mut E,
    request: &TargetedDpfProbeRequest,
) -> Result<DiagnosticResponses, String>
where
    E: ElmExchange,
{
    let command = uds_command(request.operation);
    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;
    normalize_uds_responses(&response, request.did())
}

pub(crate) async fn read_elm_ecu_identification_responses<E>(
    exchange: &mut E,
    request: &TargetedEcuIdentificationRequest,
) -> Result<DiagnosticResponses, String>
where
    E: ElmExchange,
{
    let command = uds_command(request.operation);
    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;
    normalize_uds_responses(&response, request.did())
}

#[cfg(test)]
pub(crate) fn supports_pid(support: &PidSupport, pid: u8) -> bool {
    support.supports_pid(pid)
}

fn bitmap_supports_pid(bitmap: u32, pid: u8) -> bool {
    let offset = pid & 0x1f;
    let shift = if offset == 0 { 0 } else { 32 - offset };
    bitmap & (1 << shift) != 0
}

fn obd_command(request: ReadRequest) -> String {
    let mut command = String::with_capacity(request.bytes().len() * 2 + 1);
    for byte in request.bytes() {
        command.push_str(&format!("{byte:02X}"));
    }
    command.push('\r');
    command
}

pub(crate) fn uds_command(operation: crate::protocol::ReadOperation) -> String {
    let [service, high, low] = operation.request_bytes();
    format!("{service:02X}{high:02X}{low:02X}\r")
}

#[cfg(test)]
pub(crate) fn normalize_mode01(
    response: &str,
    pid: u8,
    data_len: usize,
) -> Result<Vec<u8>, String> {
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

/// Preserve a complete, length-prefixed response to a different Mode 01 PID
/// so the caller can retry its already-authorized request once. Anything less
/// definite remains the original normalization error.
fn normalize_stale_mode01_responses(
    response: &str,
    requested_pid: u8,
) -> Result<DiagnosticResponses, String> {
    let mut responses = Vec::new();
    let mut errors = Vec::new();

    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let compact = upper.split_ascii_whitespace().collect::<String>();
        if compact == format!("01{requested_pid:02X}")
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
            return Err(format!("ELM327 rejected 01{requested_pid:02X}: {line}"));
        }

        let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
        let header = tokens.first().filter(|token| token.len() == 3).copied();
        if header.is_some_and(|value| !value.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err(format!("malformed ELM327 responder header: {line:?}"));
        }
        let data = if header.is_some() {
            &tokens[1..]
        } else {
            tokens.as_slice()
        };
        let mut bytes = Vec::new();
        for token in data {
            if token.is_empty()
                || token.len() % 2 != 0
                || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("malformed ELM327 response line: {line:?}"));
            }
            for pair in token.as_bytes().as_chunks::<2>().0 {
                let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
                bytes.push(u8::from_str_radix(pair, 16).map_err(|error| error.to_string())?);
            }
        }
        let Some((&declared_len, payload_and_padding)) = bytes.split_first() else {
            return Err(format!(
                "01{requested_pid:02X} response not found in {response:?}"
            ));
        };
        let declared_len = declared_len as usize;
        if declared_len < 2
            || payload_and_padding.len() < declared_len
            || payload_and_padding[0] != 0x41
            || payload_and_padding[1] == requested_pid
            || payload_and_padding[declared_len..]
                .iter()
                .any(|byte| !matches!(byte, 0x00 | 0x55 | 0xaa))
        {
            return Err(format!(
                "01{requested_pid:02X} response not found in {response:?}"
            ));
        }
        let responder =
            header.map(|value| ResponderIdentity::ElmHeader(value.to_ascii_uppercase()));
        let payload = payload_and_padding[..declared_len].to_vec();
        let observed = responder.as_ref().map_or_else(
            || "unknown".to_owned(),
            |responder| responder.as_str().to_owned(),
        );
        responses.push(DiagnosticResponse {
            responder: responder.clone(),
            payload,
        });
        errors.push(DiagnosticResponseError {
            responder,
            error: format!(
                "stale unrelated OBD-II Mode 01 response from responder {observed} while awaiting 01{requested_pid:02X}: {line:?}"
            ),
        });
    }

    (!responses.is_empty())
        .then_some(DiagnosticResponses::with_errors(
            responses, response, errors,
        ))
        .ok_or_else(|| format!("01{requested_pid:02X} response not found in {response:?}"))
}

#[derive(Debug)]
struct Mode03IsoTpAssembly {
    declared_len: usize,
    payload: Vec<u8>,
    next_sequence: u8,
}

fn push_mode03_payload(
    matches: &mut Vec<DiagnosticResponse>,
    errors: &mut Vec<DiagnosticResponseError>,
    responder: Option<ResponderIdentity>,
    bytes: &[u8],
    line: &str,
    strict: bool,
) {
    let start = if strict {
        (bytes.first() == Some(&0x43)).then_some(0)
    } else {
        bytes.iter().position(|byte| *byte == 0x43)
    };
    let Some(start) = start else {
        if !strict && !bytes.is_empty() {
            matches.push(DiagnosticResponse {
                responder: responder.clone(),
                payload: bytes.to_vec(),
            });
        }
        errors.push(DiagnosticResponseError {
            responder,
            error: format!("Mode 03 positive response not found: {line:?}"),
        });
        return;
    };
    let payload = bytes[start..].to_vec();
    if payload != [0x43, 0x00] && (payload.len() < 3 || !(payload.len() - 1).is_multiple_of(2)) {
        errors.push(DiagnosticResponseError {
            responder: responder.clone(),
            error: format!("malformed normalized Mode 03 payload: {line:?}"),
        });
        if strict {
            return;
        }
    }
    matches.push(DiagnosticResponse { responder, payload });
}

pub(crate) fn normalize_mode03_responses(response: &str) -> Result<DiagnosticResponses, String> {
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    let mut assemblies: Vec<(Option<ResponderIdentity>, Mode03IsoTpAssembly)> = Vec::new();

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
            if matches!(bytes.first().map(|byte| byte >> 4), Some(1 | 2)) {
                if let Some(index) = assemblies
                    .iter()
                    .position(|(identity, _)| *identity == responder)
                {
                    assemblies.remove(index);
                }
            } else if bytes.first().map(|byte| byte >> 4) == Some(0) {
                let declared_len = (bytes[0] & 0x0f) as usize;
                let payload = &bytes[1..];
                if let Some(start) = payload.iter().position(|byte| *byte == 0x43) {
                    matches.push(DiagnosticResponse {
                        responder: responder.clone(),
                        payload: payload[start..payload.len().min(start + declared_len)].to_vec(),
                    });
                }
            } else if let Some(start) = bytes.iter().position(|byte| *byte == 0x43) {
                matches.push(DiagnosticResponse {
                    responder: responder.clone(),
                    payload: bytes[start..].to_vec(),
                });
            }
            errors.push(DiagnosticResponseError { responder, error });
            continue;
        }

        match bytes.first().map(|byte| byte >> 4) {
            Some(0) => {
                let declared_len = (bytes[0] & 0x0f) as usize;
                let payload = &bytes[1..];
                if declared_len == 0 {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed Mode 03 single frame: {line:?}"),
                    });
                    continue;
                }
                if payload.len() < declared_len {
                    if let Some(start) = payload.iter().position(|byte| *byte == 0x43) {
                        matches.push(DiagnosticResponse {
                            responder: responder.clone(),
                            payload: payload[start..].to_vec(),
                        });
                    }
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "truncated Mode 03 single frame: declared {declared_len} bytes, received {}",
                            payload.len()
                        ),
                    });
                    continue;
                }
                if payload[declared_len..]
                    .iter()
                    .any(|byte| !matches!(byte, 0x00 | 0xaa))
                {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("unexpected bytes after Mode 03 single frame: {line:?}"),
                    });
                    continue;
                }
                push_mode03_payload(
                    &mut matches,
                    &mut errors,
                    responder,
                    &payload[..declared_len],
                    line,
                    false,
                );
            }
            Some(1) => {
                if bytes.len() < 3 {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed Mode 03 first frame: {line:?}"),
                    });
                    continue;
                }
                let declared_len = (((bytes[0] & 0x0f) as usize) << 8) | bytes[1] as usize;
                let payload = &bytes[2..];
                if declared_len < 3 || payload.len() >= declared_len {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "malformed Mode 03 first frame: declared {declared_len} bytes, received {}",
                            payload.len()
                        ),
                    });
                    continue;
                }
                if assemblies
                    .iter()
                    .any(|(identity, _)| *identity == responder)
                {
                    assemblies.retain(|(identity, _)| *identity != responder);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("duplicate Mode 03 first frame: {line:?}"),
                    });
                    continue;
                }
                assemblies.push((
                    responder,
                    Mode03IsoTpAssembly {
                        declared_len,
                        payload: payload.to_vec(),
                        next_sequence: 1,
                    },
                ));
            }
            Some(2) => {
                let Some(index) = assemblies
                    .iter()
                    .position(|(identity, _)| *identity == responder)
                else {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("Mode 03 consecutive frame without first frame: {line:?}"),
                    });
                    continue;
                };
                if bytes.len() < 2 {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed Mode 03 consecutive frame: {line:?}"),
                    });
                    continue;
                }
                let sequence = bytes[0] & 0x0f;
                let expected = assemblies[index].1.next_sequence;
                if sequence != expected {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "Mode 03 sequence mismatch: expected {expected}, got {sequence}"
                        ),
                    });
                    continue;
                }
                let data = &bytes[1..];
                let (declared_len, received_len) = {
                    let assembly = &assemblies[index].1;
                    (assembly.declared_len, assembly.payload.len())
                };
                if received_len >= declared_len {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("Mode 03 ISO-TP payload exceeded declared length: {line:?}"),
                    });
                    continue;
                }
                let remaining = declared_len - received_len;
                if data.len() > remaining
                    && data[remaining..]
                        .iter()
                        .any(|byte| !matches!(byte, 0x00 | 0xaa))
                {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("Mode 03 ISO-TP payload exceeded declared length: {line:?}"),
                    });
                    continue;
                }
                let complete = {
                    let assembly = &mut assemblies[index].1;
                    assembly
                        .payload
                        .extend_from_slice(&data[..data.len().min(remaining)]);
                    assembly.next_sequence = (sequence + 1) & 0x0f;
                    assembly.payload.len() == declared_len
                };
                if complete {
                    let (_, assembly) = assemblies.remove(index);
                    push_mode03_payload(
                        &mut matches,
                        &mut errors,
                        responder,
                        &assembly.payload,
                        line,
                        true,
                    );
                }
            }
            Some(3) => {
                if let Some(index) = assemblies
                    .iter()
                    .position(|(identity, _)| *identity == responder)
                {
                    assemblies.remove(index);
                }
                errors.push(DiagnosticResponseError {
                    responder,
                    error: format!("unexpected Mode 03 flow-control frame: {line:?}"),
                });
            }
            _ => {
                if assemblies
                    .iter()
                    .any(|(identity, _)| *identity == responder)
                {
                    assemblies.retain(|(identity, _)| *identity != responder);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("interleaved Mode 03 response frame: {line:?}"),
                    });
                    continue;
                }
                push_mode03_payload(&mut matches, &mut errors, responder, &bytes, line, false);
            }
        }
    }

    for (responder, assembly) in assemblies {
        errors.push(DiagnosticResponseError {
            responder,
            error: format!(
                "truncated Mode 03 ISO-TP response: declared {} bytes, received {}",
                assembly.declared_len,
                assembly.payload.len()
            ),
        });
    }

    if matches.is_empty() && errors.is_empty() {
        errors.push(DiagnosticResponseError {
            responder: None,
            error: "Mode 03 response not found".into(),
        });
    }
    Ok(DiagnosticResponses::with_errors(matches, response, errors))
}

pub(crate) fn normalize_mode09_segments(response: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut segments = Vec::new();
    let mut isotp: Option<(Option<String>, usize, Vec<u8>, u8)> = None;
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
        let header = tokens
            .next()
            .filter(|token| token.len() == 3 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(str::to_ascii_uppercase);
        if header.is_none() {
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
        match bytes.first().map(|byte| byte >> 4) {
            Some(1) => {
                if bytes.len() < 3 || isotp.is_some() {
                    return Err("malformed ISO-TP Mode 09 first frame".into());
                }
                let declared_len = ((bytes[0] as usize & 0x0f) << 8) | bytes[1] as usize;
                let payload = bytes[2..].to_vec();
                if declared_len < 3 || payload.len() >= declared_len {
                    return Err("malformed ISO-TP Mode 09 first frame".into());
                }
                isotp = Some((header, declared_len, payload, 1));
            }
            Some(2) => {
                let Some((expected_header, declared_len, payload, expected_sequence)) =
                    isotp.as_mut()
                else {
                    return Err("Mode 09 consecutive frame without first frame".into());
                };
                if *expected_header != header || bytes[0] & 0x0f != *expected_sequence {
                    return Err("malformed ISO-TP Mode 09 consecutive frame".into());
                }
                let remaining = *declared_len - payload.len();
                let frame_payload = &bytes[1..];
                if frame_payload.len() > remaining
                    && frame_payload[remaining..]
                        .iter()
                        .any(|byte| !matches!(byte, 0x00 | 0xaa))
                {
                    return Err("unexpected bytes after ISO-TP Mode 09 response".into());
                }
                payload.extend_from_slice(&frame_payload[..frame_payload.len().min(remaining)]);
                *expected_sequence = (*expected_sequence + 1) & 0x0f;
                if payload.len() == *declared_len {
                    segments.push(std::mem::take(payload));
                    isotp = None;
                }
            }
            _ if !bytes.is_empty() => segments.push(bytes),
            _ => {}
        }
    }
    if isotp.is_some() {
        return Err("truncated ISO-TP Mode 09 response".into());
    }
    (!segments.is_empty())
        .then_some(segments)
        .ok_or_else(|| "0902 response not found".into())
}

#[cfg(test)]
pub(crate) fn normalize_pid_support_page(response: &str, page: u8) -> Result<[u8; 6], String> {
    normalize_pid_support_page_with_evidence(response, page).map(|(normalized, _)| normalized)
}

pub(crate) fn normalize_pid_support_page_with_evidence(
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
            let has_length_prefix =
                payload_start > 0 && bytes[payload_start - 1] == expected_len as u8;
            let frame_start = if has_length_prefix {
                payload_start - 1
            } else if payload_start == 0 {
                payload_start
            } else {
                search_from = payload_end;
                continue;
            };
            if bytes[payload_end..]
                .iter()
                .any(|byte| !matches!(byte, 0x00 | 0xaa) && !(has_length_prefix && *byte == 0x55))
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

#[derive(Debug)]
struct UdsIsoTpAssembly {
    declared_len: usize,
    payload: Vec<u8>,
    next_sequence: u8,
}

fn push_uds_payload(
    matches: &mut Vec<DiagnosticResponse>,
    responder: Option<ResponderIdentity>,
    payload: &[u8],
) {
    if !payload.is_empty() {
        matches.push(DiagnosticResponse {
            responder,
            payload: payload.to_vec(),
        });
    }
}

/// Normalize one ELM response to UDS application payloads while retaining
/// every responder and the complete raw adapter text in `DiagnosticResponses`.
pub(crate) fn normalize_uds_responses(
    response: &str,
    did: u16,
) -> Result<DiagnosticResponses, String> {
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    let mut assemblies: Vec<(Option<ResponderIdentity>, UdsIsoTpAssembly)> = Vec::new();
    let request_echo = format!("22{did:04X}");

    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let compact = upper.split_ascii_whitespace().collect::<String>();
        if compact == request_echo
            || upper.starts_with("SEARCHING")
            || (upper.starts_with("BUS INIT") && !upper.contains("ERROR"))
        {
            continue;
        }
        if ["?", "NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
            .iter()
            .any(|status| upper == *status || upper.contains(status))
        {
            errors.push(DiagnosticResponseError {
                responder: None,
                error: format!("ELM327 rejected UDS 22 response: {line}"),
            });
            continue;
        }

        let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
        let header = tokens.first().filter(|token| token.len() == 3).copied();
        let responder = if let Some(header) = header {
            if !header.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                errors.push(DiagnosticResponseError {
                    responder: None,
                    error: format!("malformed ELM327 UDS responder header: {line:?}"),
                });
                continue;
            }
            Some(ResponderIdentity::ElmHeader(header.to_ascii_uppercase()))
        } else {
            None
        };
        let data = if header.is_some() {
            &tokens[1..]
        } else {
            tokens.as_slice()
        };
        let mut bytes = Vec::new();
        let mut malformed = None;
        for token in data {
            if token.is_empty()
                || token.len() % 2 != 0
                || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                malformed = Some(format!("malformed ELM327 UDS response line: {line:?}"));
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
            errors.push(DiagnosticResponseError { responder, error });
            continue;
        }

        // A targeted UDS request can still receive a stale functional Mode 01
        // frame from the adapter. It is not a malformed UDS response and must
        // not compete with the requested 62 response during selection. Keep
        // the raw line in `raw_response` and expose the ignored frame as an
        // explicit parser issue without associating it with the expected
        // responder; callers can therefore still accept a valid 62 response.
        let is_mode01_response = bytes.first() == Some(&0x41)
            || (bytes.first().is_some_and(|byte| byte >> 4 == 0) && bytes.get(1) == Some(&0x41));
        if is_mode01_response {
            let observed = responder
                .as_ref()
                .map_or("unknown", ResponderIdentity::as_str);
            errors.push(DiagnosticResponseError {
                responder: None,
                error: format!(
                    "ignored unrelated OBD-II Mode 01 response from responder {observed} while awaiting UDS 22 response: {line:?}"
                ),
            });
            continue;
        }

        match bytes.first().map(|byte| byte >> 4) {
            Some(0) => {
                let declared_len = (bytes[0] & 0x0f) as usize;
                let payload = &bytes[1..];
                if declared_len == 0 {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed UDS single frame: {line:?}"),
                    });
                    continue;
                }
                if payload.len() < declared_len {
                    push_uds_payload(&mut matches, responder.clone(), payload);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "truncated UDS single frame: declared {declared_len} bytes, received {}",
                            payload.len()
                        ),
                    });
                    continue;
                }
                if payload[declared_len..]
                    .iter()
                    .any(|byte| !matches!(byte, 0x00 | 0x55 | 0xaa))
                {
                    push_uds_payload(&mut matches, responder.clone(), &payload[..declared_len]);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("unexpected bytes after UDS single frame: {line:?}"),
                    });
                    continue;
                }
                push_uds_payload(&mut matches, responder, &payload[..declared_len]);
            }
            Some(1) => {
                if bytes.len() < 3 {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed UDS first frame: {line:?}"),
                    });
                    continue;
                }
                let declared_len = (((bytes[0] & 0x0f) as usize) << 8) | bytes[1] as usize;
                let payload = &bytes[2..];
                if declared_len < 3 || payload.len() >= declared_len {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "malformed UDS first frame: declared {declared_len} bytes, received {}",
                            payload.len()
                        ),
                    });
                    continue;
                }
                if assemblies
                    .iter()
                    .any(|(identity, _)| *identity == responder)
                {
                    assemblies.retain(|(identity, _)| *identity != responder);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("duplicate UDS first frame: {line:?}"),
                    });
                    continue;
                }
                assemblies.push((
                    responder,
                    UdsIsoTpAssembly {
                        declared_len,
                        payload: payload.to_vec(),
                        next_sequence: 1,
                    },
                ));
            }
            Some(2) => {
                let Some(index) = assemblies
                    .iter()
                    .position(|(identity, _)| *identity == responder)
                else {
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("UDS consecutive frame without first frame: {line:?}"),
                    });
                    continue;
                };
                if bytes.len() < 2 {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("malformed UDS consecutive frame: {line:?}"),
                    });
                    continue;
                }
                let sequence = bytes[0] & 0x0f;
                let expected = assemblies[index].1.next_sequence;
                if sequence != expected {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!(
                            "UDS sequence mismatch: expected {expected}, got {sequence}"
                        ),
                    });
                    continue;
                }
                let data = &bytes[1..];
                let (declared_len, received_len) = {
                    let assembly = &assemblies[index].1;
                    (assembly.declared_len, assembly.payload.len())
                };
                if received_len >= declared_len {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("UDS payload exceeded declared length: {line:?}"),
                    });
                    continue;
                }
                let remaining = declared_len - received_len;
                if data.len() > remaining
                    && data[remaining..]
                        .iter()
                        .any(|byte| !matches!(byte, 0x00 | 0x55 | 0xaa))
                {
                    assemblies.remove(index);
                    errors.push(DiagnosticResponseError {
                        responder,
                        error: format!("UDS payload exceeded declared length: {line:?}"),
                    });
                    continue;
                }
                let complete = {
                    let assembly = &mut assemblies[index].1;
                    assembly
                        .payload
                        .extend_from_slice(&data[..data.len().min(remaining)]);
                    assembly.next_sequence = (sequence + 1) & 0x0f;
                    assembly.payload.len() == declared_len
                };
                if complete {
                    let (_, assembly) = assemblies.remove(index);
                    push_uds_payload(&mut matches, responder, &assembly.payload);
                }
            }
            Some(3) => {
                errors.push(DiagnosticResponseError {
                    responder,
                    error: format!("unexpected UDS flow-control frame: {line:?}"),
                });
            }
            _ => push_uds_payload(&mut matches, responder, &bytes),
        }
    }

    for (responder, assembly) in assemblies {
        push_uds_payload(&mut matches, responder.clone(), &assembly.payload);
        errors.push(DiagnosticResponseError {
            responder,
            error: format!(
                "truncated UDS ISO-TP response: declared {} bytes, received {}",
                assembly.declared_len,
                assembly.payload.len()
            ),
        });
    }
    if matches.is_empty() && errors.is_empty() {
        errors.push(DiagnosticResponseError {
            responder: None,
            error: "UDS 22 response not found".into(),
        });
    }
    Ok(DiagnosticResponses::with_errors(matches, response, errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedExchange {
        responses: VecDeque<String>,
        commands: Vec<String>,
    }

    impl ScriptedExchange {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: responses.into_iter().map(str::to_owned).collect(),
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
            self.commands.push(command.to_owned());
            self.responses
                .pop_front()
                .ok_or_else(|| "script ended before adapter response".to_string())
        }
    }

    fn canonical_ecu_identification_request() -> TargetedEcuIdentificationRequest {
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
        TargetedEcuIdentificationRequest::from_evidence(candidate, &target, &responder).unwrap()
    }

    #[tokio::test]
    async fn generic_initialization_preserves_backend_identity_boundary() {
        let mut exchange = ScriptedExchange::new([
            "ELM327 v1.4 v100\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);

        verify_elm327(&mut exchange).await.unwrap();
        initialize_elm(&mut exchange).await.unwrap();

        assert_eq!(
            exchange.commands,
            ["ATI\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"]
        );
    }

    #[tokio::test]
    async fn generic_mode01_exchange_normalizes_without_adapter_backend() {
        let mut exchange = ScriptedExchange::new(["7E8 04 41 0C 1A F8\r>"]);
        let request = crate::prepare_read("engine.rpm").unwrap();
        let responses = read_elm_responses(&mut exchange, request).await.unwrap();
        assert_eq!(exchange.commands, ["010C\r"]);
        assert_eq!(responses.as_slice()[0].payload, [0x41, 0x0c, 0x1a, 0xf8]);
        assert_eq!(
            responses.as_slice()[0].responder.as_ref().unwrap().as_str(),
            "7E8"
        );
    }

    #[tokio::test]
    async fn generic_mode01_retries_one_stale_different_pid_response() {
        let mut exchange = ScriptedExchange::new([
            "7E9 06 41 00 98 18 00 01 AA\r>",
            "7E9 03 41 0D 00 00 00 00\r>",
        ]);
        let request = crate::prepare_read("vehicle.speed").unwrap();

        let read = read_elm_with_evidence(&mut exchange, request)
            .await
            .unwrap();

        assert_eq!(read.payload, [0x41, 0x0d, 0x00]);
        assert_eq!(read.observations.len(), 2);
        assert!(read.observations[0]
            .selection_error
            .as_deref()
            .is_some_and(|error| error == "010D response not found"));
        assert_eq!(exchange.commands, ["010D\r", "010D\r"]);
    }

    #[tokio::test]
    async fn generic_mode01_stops_after_one_stale_different_pid_retry() {
        let stale = "7E9 06 41 00 98 18 00 01 AA\r>";
        let mut exchange = ScriptedExchange::new([stale, stale]);
        let request = crate::prepare_read("vehicle.speed").unwrap();

        let error = read_elm_with_evidence(&mut exchange, request)
            .await
            .unwrap_err();

        assert!(error.error.contains("first ELM response"));
        assert_eq!(error.observations.len(), 2);
        assert_eq!(exchange.commands, ["010D\r", "010D\r"]);
    }

    #[tokio::test]
    async fn generic_mode01_does_not_retry_adapter_rejection() {
        let mut exchange = ScriptedExchange::new(["NO DATA\r>"]);
        let request = crate::prepare_read("vehicle.speed").unwrap();

        let error = read_elm_with_evidence(&mut exchange, request)
            .await
            .unwrap_err();

        assert!(error.error.contains("ELM327 rejected"));
        assert_eq!(exchange.commands, ["010D\r"]);
    }

    #[tokio::test]
    async fn generic_session_executes_closed_mode01_read_without_adapter_backend() {
        let exchange = ScriptedExchange::new(["410000100000\r>", "7E8 04 41 0C 1A F8\r>"]);
        let mut session = ElmSession::new(exchange);

        session.discover_support(0).await.unwrap();
        let request = crate::prepare_read("engine.rpm").unwrap();
        let read = session.read_with_evidence(request).await.unwrap();

        assert_eq!(read.payload, [0x41, 0x0c, 0x1a, 0xf8]);
        assert_eq!(session.into_exchange().commands, ["0100\r", "010C\r"]);
    }

    #[tokio::test]
    async fn generic_session_executes_only_a_canonical_ecu_identification_candidate() {
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
            "7E8 05 62 F1 89 31 2E 55 55\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);
        let mut session = ElmSession::new(exchange);

        let read = session
            .read_ecu_identification_with_evidence(&request)
            .await
            .unwrap();

        assert_eq!(
            read.responses.as_slice()[0].payload,
            [0x62, 0xf1, 0x89, 0x31, 0x2e]
        );
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

    #[tokio::test]
    async fn ecu_identification_retries_only_stale_mode01_and_accepts_uds_response() {
        let request = canonical_ecu_identification_request();
        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 06 41 00 98 3B A0 13 00\r>",
            "7E8 05 62 F1 89 31 2E 55 55\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);
        let mut session = ElmSession::new(exchange);

        let read = session
            .read_ecu_identification_with_evidence(&request)
            .await
            .unwrap();

        assert_eq!(
            read.responses.as_slice()[0].payload,
            [0x62, 0xF1, 0x89, 0x31, 0x2E]
        );
        assert_eq!(read.responses.errors().len(), 1);
        assert!(read.responses.raw_response().contains("41 00 98 3B A0 13"));
        assert!(read.responses.raw_response().contains("62 F1 89 31 2E"));
        assert_eq!(
            session.into_exchange().commands,
            [
                "ATSH 7E0\r",
                "ATCRA 7E8\r",
                "22F189\r",
                "22F189\r",
                "ATSP0\r",
                "ATSH 7DF\r",
                "ATCRA\r",
            ]
        );
    }

    #[tokio::test]
    async fn ecu_identification_stops_after_one_stale_mode01_retry() {
        let request = canonical_ecu_identification_request();
        let stale = "7E8 06 41 00 98 3B A0 13 00\r>";
        let exchange =
            ScriptedExchange::new(["OK\r>", "OK\r>", stale, stale, "OK\r>", "OK\r>", "OK\r>"]);
        let mut session = ElmSession::new(exchange);

        let read = session
            .read_ecu_identification_with_evidence(&request)
            .await
            .unwrap();

        assert!(read.responses.as_slice().is_empty());
        assert_eq!(read.responses.errors().len(), 2);
        assert!(read.observations[0]
            .selection_error
            .as_deref()
            .is_some_and(|error| error.contains("did not answer")));
        assert_eq!(
            session.into_exchange().commands,
            [
                "ATSH 7E0\r",
                "ATCRA 7E8\r",
                "22F189\r",
                "22F189\r",
                "ATSP0\r",
                "ATSH 7DF\r",
                "ATCRA\r",
            ]
        );
    }

    #[test]
    fn ignores_stale_mode01_support_before_expected_uds_response() {
        let responses = normalize_uds_responses(
            "7E8 06 41 00 98 3B A0 13 00\r7E8 05 62 F1 89 31 2E 55 55\r>",
            0xF189,
        )
        .unwrap();

        assert_eq!(
            responses.as_slice()[0].payload,
            [0x62, 0xF1, 0x89, 0x31, 0x2E]
        );
        assert_eq!(
            responses.as_slice()[0].responder,
            Some(ResponderIdentity::ElmHeader("7E8".into()))
        );
        assert_eq!(responses.errors().len(), 1);
        assert!(responses.errors()[0]
            .error
            .contains("ignored unrelated OBD-II Mode 01 response from responder 7E8"));
        assert!(responses.raw_response().contains("41 00 98 3B A0 13"));
    }

    #[test]
    fn persistent_mode01_response_remains_an_explicit_uds_failure() {
        let responses = normalize_uds_responses("7E8 06 41 00 98 3B A0 13 00\r>", 0xF189).unwrap();

        assert!(responses.as_slice().is_empty());
        assert_eq!(responses.errors().len(), 1);
        assert!(responses.errors()[0]
            .error
            .contains("ignored unrelated OBD-II Mode 01 response"));
        assert!(responses.raw_response().contains("41 00 98 3B A0 13"));
    }
}
