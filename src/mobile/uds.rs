use std::time::{Duration, Instant};

use crate::{
    elm::{
        ElmExchange, ElmSession, ReadEvidence, ResponderIdentity, ResponseObservation,
        UdsSessionCommand,
    },
    knowledge_db::KnowledgeDefinition,
    runtime_state::Activity,
    safety::{ExtendedSessionReadRequest, OperationRequest, SafetyPolicy},
    topology::{AddressingContext, Protocol, RequestTarget},
};

const TESTER_PRESENT_INTERVAL: Duration = Duration::from_secs(2);
const EXTENDED_SESSION_LIMIT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum UdsSessionStatus {
    #[default]
    Inactive,
    Active,
    Indeterminate,
}

impl UdsSessionStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Indeterminate => "indeterminate",
        }
    }
}

#[derive(Default)]
pub(crate) struct UdsSessionState {
    status: UdsSessionStatus,
    target: Option<RequestTarget>,
    responder: Option<ResponderIdentity>,
    authorization: Option<ExtendedSessionReadRequest>,
    candidate_ids: Vec<String>,
    activated_at: Option<Instant>,
    last_tester_present: Option<Instant>,
}

pub(crate) struct UdsSessionOutcome {
    pub(crate) status: UdsSessionStatus,
    pub(crate) active: bool,
    pub(crate) target: Option<String>,
    pub(crate) responder: Option<String>,
    pub(crate) signals: Vec<String>,
    pub(crate) observations: Vec<ResponseObservation>,
    pub(crate) error: Option<String>,
}

pub(crate) struct UdsSessionReadResult {
    pub(crate) status: UdsSessionStatus,
    pub(crate) evidence: Option<ReadEvidence>,
    pub(crate) read_observations: Vec<ResponseObservation>,
    pub(crate) observations: Vec<ResponseObservation>,
    pub(crate) error: Option<String>,
}

impl UdsSessionState {
    pub(crate) fn is_active(&self) -> bool {
        self.status == UdsSessionStatus::Active
    }

    pub(crate) fn candidate_ids(&self) -> &[String] {
        if self.is_active() {
            &self.candidate_ids
        } else {
            &[]
        }
    }

    pub(crate) fn candidate_count(&self) -> usize {
        self.candidate_ids().len()
    }

    pub(crate) fn remaining_ms(&self, now: Instant) -> Option<u64> {
        self.activated_at
            .filter(|_| self.is_active())
            .map(|activated| {
                EXTENDED_SESSION_LIMIT
                    .saturating_sub(now.saturating_duration_since(activated))
                    .as_millis() as u64
            })
    }

    pub(crate) fn snapshot(&self) -> UdsSessionOutcome {
        self.outcome(Vec::new(), None)
    }

    pub(crate) fn can_read(&self, definition: &KnowledgeDefinition) -> bool {
        self.is_active()
            && definition.requires_extended_session()
            && self.candidate_ids.iter().any(|id| id == definition.id())
    }

    pub(crate) async fn enable<E: ElmExchange>(
        &mut self,
        session: &mut ElmSession<E>,
        target: RequestTarget,
        responder: ResponderIdentity,
        candidates: &[&KnowledgeDefinition],
        now: Instant,
    ) -> UdsSessionOutcome {
        if self.status == UdsSessionStatus::Indeterminate {
            return self.outcome(
                Vec::new(),
                Some("UDS session state is indeterminate; reconnect before diagnostics".into()),
            );
        }
        let mut candidate_ids = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if !candidate.requires_extended_session()
                || candidate.operation().ecu_role() != Some("engine")
            {
                return self.outcome(
                    Vec::new(),
                    Some("only engine Knowledge session candidates are allowed".into()),
                );
            }
            candidate_ids.push(candidate.id().to_owned());
        }
        candidate_ids.sort();
        candidate_ids.dedup();
        let Some(first_candidate) = candidates.first().copied() else {
            return self.outcome(
                Vec::new(),
                Some("no applicable session-flagged Knowledge DIDs".into()),
            );
        };
        if !candidate_ids.iter().any(|id| id == first_candidate.id()) {
            return self.outcome(Vec::new(), Some("invalid UDS candidate list".into()));
        }
        if target.context().protocol() != &Protocol::Obd2
            || target.context().addressing() != &AddressingContext::Physical
            || target.address().is_none()
            || responder.as_str().is_empty()
        {
            return self.outcome(Vec::new(), Some("invalid validated ECU target".into()));
        }

        if self.is_active() {
            if self.target.as_ref() == Some(&target)
                && self.responder.as_ref() == Some(&responder)
                && self.candidate_ids == candidate_ids
            {
                return self.outcome(Vec::new(), None);
            }
            return self.outcome(
                Vec::new(),
                Some("an extended session is already active for another ECU".into()),
            );
        }

        let authorization = match authorize_candidate(first_candidate) {
            Ok(authorization) => authorization,
            Err(error) => return self.outcome(Vec::new(), Some(error)),
        };
        self.target = Some(target);
        self.responder = Some(responder);
        self.authorization = Some(authorization);
        self.candidate_ids = candidate_ids;

        match session
            .uds_session_command(
                UdsSessionCommand::Enter,
                self.target.as_ref().unwrap(),
                self.responder.as_ref().unwrap(),
            )
            .await
        {
            Ok(evidence) if evidence.payload.starts_with(&[0x50, 0x03]) => {
                self.status = UdsSessionStatus::Active;
                self.activated_at = Some(now);
                self.last_tester_present = Some(now);
                self.outcome(evidence.observations, None)
            }
            Ok(evidence) => {
                self.cleanup_after_error(
                    session,
                    "unexpected positive response to UDS session activation".into(),
                    evidence.observations,
                )
                .await
            }
            Err(error) => {
                if has_expected_negative_session_response(
                    &error.observations,
                    self.responder.as_ref().unwrap(),
                ) {
                    self.mark_inactive();
                    self.outcome(error.observations, Some(error.error))
                } else {
                    self.cleanup_after_error(session, error.error, error.observations)
                        .await
                }
            }
        }
    }

    pub(crate) async fn disable<E: ElmExchange>(
        &mut self,
        session: &mut ElmSession<E>,
        _now: Instant,
    ) -> UdsSessionOutcome {
        match self.status {
            UdsSessionStatus::Inactive => self.outcome(Vec::new(), None),
            UdsSessionStatus::Indeterminate => self.outcome(
                Vec::new(),
                Some("UDS session state is indeterminate; reconnect before diagnostics".into()),
            ),
            UdsSessionStatus::Active => self.leave(session).await,
        }
    }

    pub(crate) async fn tick<E: ElmExchange>(
        &mut self,
        session: &mut ElmSession<E>,
        now: Instant,
    ) -> UdsSessionOutcome {
        if !self.is_active() {
            return self.outcome(Vec::new(), None);
        }
        if self.activated_at.is_some_and(|activated| {
            now.saturating_duration_since(activated) >= EXTENDED_SESSION_LIMIT
        }) {
            return self.leave(session).await;
        }
        if self
            .last_tester_present
            .is_some_and(|last| now.saturating_duration_since(last) < TESTER_PRESENT_INTERVAL)
        {
            return self.outcome(Vec::new(), None);
        }
        if let Err(error) = self.authorize_stored() {
            return self.cleanup_after_error(session, error, Vec::new()).await;
        }
        match session
            .uds_session_command(
                UdsSessionCommand::TesterPresent,
                self.target.as_ref().unwrap(),
                self.responder.as_ref().unwrap(),
            )
            .await
        {
            Ok(evidence) => {
                self.last_tester_present = Some(now);
                self.outcome(evidence.observations, None)
            }
            Err(error) => {
                self.cleanup_after_error(session, error.error, error.observations)
                    .await
            }
        }
    }

    pub(crate) async fn read<E: ElmExchange>(
        &mut self,
        session: &mut ElmSession<E>,
        definition: &KnowledgeDefinition,
        now: Instant,
    ) -> UdsSessionReadResult {
        if !self.can_read(definition) {
            return self.read_result(
                None,
                Vec::new(),
                Vec::new(),
                Some("Knowledge DID is not enabled for this ECU session".into()),
            );
        }
        if self.activated_at.is_some_and(|activated| {
            now.saturating_duration_since(activated) >= EXTENDED_SESSION_LIMIT
        }) {
            let outcome = self.leave(session).await;
            let error = outcome
                .error
                .clone()
                .unwrap_or_else(|| "UDS extended session expired".into());
            return self.read_result(None, Vec::new(), outcome.observations, Some(error));
        }
        let authorization = match authorize_candidate(definition) {
            Ok(authorization) => authorization,
            Err(error) => {
                let outcome = self
                    .cleanup_after_error(session, error.clone(), Vec::new())
                    .await;
                return self.read_result(None, Vec::new(), outcome.observations, Some(error));
            }
        };
        let policy = SafetyPolicy::read_only().authorize_activity(
            Activity::Diagnose,
            OperationRequest::ExtendedSessionRead(authorization),
        );
        if let Err(error) = policy {
            let error = error.to_string();
            let outcome = self
                .cleanup_after_error(session, error.clone(), Vec::new())
                .await;
            return self.read_result(None, Vec::new(), outcome.observations, outcome.error);
        }

        match session
            .read_knowledge_with_evidence(
                definition,
                self.target.as_ref().unwrap(),
                self.responder.as_ref().unwrap(),
            )
            .await
        {
            Ok(evidence) => match definition.validate_response(&evidence.payload) {
                Ok(_) => {
                    let observations = evidence.observations.clone();
                    self.read_result(Some(evidence), observations.clone(), observations, None)
                }
                Err(error) => {
                    let read_observations = evidence.observations.clone();
                    let outcome = self
                        .cleanup_after_error(session, error.clone(), read_observations.clone())
                        .await;
                    self.read_result(
                        Some(evidence),
                        read_observations,
                        outcome.observations,
                        Some(error),
                    )
                }
            },
            Err(error) => {
                let read_observations = error.observations.clone();
                let outcome = self
                    .cleanup_after_error(session, error.error.clone(), read_observations.clone())
                    .await;
                self.read_result(None, read_observations, outcome.observations, outcome.error)
            }
        }
    }

    pub(crate) fn on_disconnect(&mut self) {
        if self.is_active() {
            self.status = UdsSessionStatus::Indeterminate;
            self.candidate_ids.clear();
            self.authorization = None;
            self.activated_at = None;
            self.last_tester_present = None;
        }
    }

    pub(crate) fn on_reconnect(&mut self) {
        *self = Self::default();
    }

    async fn leave<E: ElmExchange>(&mut self, session: &mut ElmSession<E>) -> UdsSessionOutcome {
        if let Err(error) = self.authorize_stored() {
            return self.mark_indeterminate(error, Vec::new());
        }
        match session
            .uds_session_command(
                UdsSessionCommand::Leave,
                self.target.as_ref().unwrap(),
                self.responder.as_ref().unwrap(),
            )
            .await
        {
            Ok(evidence) if evidence.payload.starts_with(&[0x50, 0x01]) => {
                self.mark_inactive();
                self.outcome(evidence.observations, None)
            }
            Ok(evidence) => self.mark_indeterminate(
                "unexpected response while returning to the default session".into(),
                evidence.observations,
            ),
            Err(error) => self.mark_indeterminate(error.error, error.observations),
        }
    }

    async fn cleanup_after_error<E: ElmExchange>(
        &mut self,
        session: &mut ElmSession<E>,
        error: String,
        mut observations: Vec<ResponseObservation>,
    ) -> UdsSessionOutcome {
        if let Err(authorize_error) = self.authorize_stored() {
            return self.mark_indeterminate(
                format!("{error}; default-session cleanup blocked: {authorize_error}"),
                observations,
            );
        }
        match session
            .uds_session_command(
                UdsSessionCommand::Leave,
                self.target.as_ref().unwrap(),
                self.responder.as_ref().unwrap(),
            )
            .await
        {
            Ok(evidence) if evidence.payload.starts_with(&[0x50, 0x01]) => {
                observations.extend(evidence.observations);
                self.mark_inactive();
                self.outcome(observations, Some(error))
            }
            Ok(evidence) => {
                observations.extend(evidence.observations);
                self.mark_indeterminate(
                    format!("{error}; unexpected default-session cleanup response"),
                    observations,
                )
            }
            Err(cleanup_error) => {
                observations.extend(cleanup_error.observations);
                self.mark_indeterminate(
                    format!(
                        "{error}; returning to default session failed: {}",
                        cleanup_error.error
                    ),
                    observations,
                )
            }
        }
    }

    fn authorize_stored(&self) -> Result<(), String> {
        let authorization = self
            .authorization
            .clone()
            .ok_or_else(|| "extended-session authorization is missing".to_string())?;
        SafetyPolicy::read_only()
            .authorize_activity(
                Activity::Diagnose,
                OperationRequest::ExtendedSessionRead(authorization),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn mark_inactive(&mut self) {
        self.status = UdsSessionStatus::Inactive;
        self.authorization = None;
        self.candidate_ids.clear();
        self.activated_at = None;
        self.last_tester_present = None;
    }

    fn mark_indeterminate(
        &mut self,
        error: String,
        observations: Vec<ResponseObservation>,
    ) -> UdsSessionOutcome {
        self.status = UdsSessionStatus::Indeterminate;
        self.authorization = None;
        self.candidate_ids.clear();
        self.activated_at = None;
        self.last_tester_present = None;
        self.outcome(observations, Some(error))
    }

    fn outcome(
        &self,
        observations: Vec<ResponseObservation>,
        error: Option<String>,
    ) -> UdsSessionOutcome {
        UdsSessionOutcome {
            status: self.status,
            active: self.is_active(),
            target: self
                .target
                .as_ref()
                .and_then(RequestTarget::address)
                .map(|address| address.value().to_owned()),
            responder: self
                .responder
                .as_ref()
                .map(|responder| responder.as_str().to_owned()),
            signals: self.candidate_ids().to_vec(),
            observations,
            error,
        }
    }

    fn read_result(
        &self,
        evidence: Option<ReadEvidence>,
        read_observations: Vec<ResponseObservation>,
        observations: Vec<ResponseObservation>,
        error: Option<String>,
    ) -> UdsSessionReadResult {
        UdsSessionReadResult {
            status: self.status,
            evidence,
            read_observations,
            observations,
            error,
        }
    }
}

fn has_expected_negative_session_response(
    observations: &[ResponseObservation],
    expected: &ResponderIdentity,
) -> bool {
    observations.iter().any(|observation| {
        observation.responses.iter().any(|response| {
            response
                .responder
                .as_deref()
                .is_some_and(|responder| responder.eq_ignore_ascii_case(expected.as_str()))
                && matches!(response.payload.as_slice(), [0x7F, 0x10, _])
        })
    })
}

fn authorize_candidate(
    definition: &KnowledgeDefinition,
) -> Result<ExtendedSessionReadRequest, String> {
    let request =
        OperationRequest::extended_session_read(definition).map_err(|error| error.to_string())?;
    let OperationRequest::ExtendedSessionRead(authorization) = request else {
        unreachable!("extended-session helper returns only its typed request")
    };
    SafetyPolicy::read_only()
        .authorize_activity(
            Activity::Diagnose,
            OperationRequest::ExtendedSessionRead(authorization.clone()),
        )
        .map_err(|error| error.to_string())?;
    Ok(authorization)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        elm::{ElmSession, ResponderIdentity},
        test_support::ScriptedExchange,
        topology::{ProtocolContext, RequestAddress},
    };

    fn target_and_responder() -> (RequestTarget, ResponderIdentity) {
        (
            RequestTarget::concrete(
                ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical),
                RequestAddress::new("elm-header", "7E0"),
            ),
            ResponderIdentity::ElmHeader("7E8".into()),
        )
    }

    fn candidates(catalog: &crate::knowledge_db::KnowledgeCatalog) -> Vec<&KnowledgeDefinition> {
        catalog
            .definitions()
            .filter(|definition| definition.requires_extended_session())
            .collect()
    }

    #[tokio::test]
    async fn req_027_1_activation_requires_positive_response_before_becoming_active() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange = ScriptedExchange::new(["OK\r>", "OK\r>", "7E8 06 50 03 00 32 01 F4\r>"]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let now = Instant::now();

        let outcome = state
            .enable(&mut session, target, responder, &candidates, now)
            .await;

        assert_eq!(outcome.status, UdsSessionStatus::Active);
        assert_eq!(outcome.signals.len(), candidates.len());
        assert_eq!(spy.commands(), ["ATSH 7E0\r", "ATCRA 7E8\r", "1003\r"]);
        assert!(state.can_read(candidates[0]));
        assert!(!catalog
            .definitions()
            .find(|definition| !definition.requires_extended_session())
            .is_some_and(|definition| state.can_read(definition)));
    }

    #[tokio::test]
    async fn req_027_2_tick_sends_tester_present_at_most_every_two_seconds() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 06 50 03 00 32 01 F4\r>",
            "NO DATA\r>",
        ]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;
        state
            .tick(&mut session, start + Duration::from_millis(1500))
            .await;
        assert_eq!(spy.commands().len(), 3);
        state
            .tick(&mut session, start + Duration::from_secs(2))
            .await;
        state
            .tick(&mut session, start + Duration::from_secs(3))
            .await;
        assert_eq!(
            spy.commands(),
            ["ATSH 7E0\r", "ATCRA 7E8\r", "1003\r", "3E80\r"]
        );
    }

    #[tokio::test]
    async fn req_027_2_session_candidate_read_is_targeted_and_stays_active() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let definition = catalog
            .definitions()
            .find(|definition| {
                definition.requires_extended_session()
                    && definition
                        .response()
                        .exact_payload_length()
                        .is_some_and(|len| len <= 4)
            })
            .unwrap();
        let mut payload = vec![
            0x62,
            (definition.operation().did() >> 8) as u8,
            definition.operation().did() as u8,
        ];
        payload.resize(
            payload.len() + definition.response().exact_payload_length().unwrap(),
            0,
        );
        let mut response = format!("7E8 {:02X}", payload.len());
        for byte in payload {
            response.push_str(&format!(" {byte:02X}"));
        }
        response.push_str("\r>");
        let exchange = ScriptedExchange::new(vec![
            "OK\r>".to_owned(),
            "OK\r>".to_owned(),
            "7E8 06 50 03 00 32 01 F4\r>".to_owned(),
            response,
            "7E8 02 50 01\r>".to_owned(),
        ]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        let candidates = candidates(&catalog);
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;

        let result = state.read(&mut session, definition, start).await;

        assert_eq!(result.status, UdsSessionStatus::Active);
        assert!(result.error.is_none());
        assert!(result.evidence.is_some());
        assert_eq!(
            spy.commands(),
            [
                "ATSH 7E0\r".to_owned(),
                "ATCRA 7E8\r".to_owned(),
                "1003\r".to_owned(),
                format!("22{:04X}\r", definition.operation().did()),
            ]
        );
        let outcome = state.disable(&mut session, start).await;
        assert_eq!(outcome.status, UdsSessionStatus::Inactive);
        assert_eq!(spy.commands().last().unwrap(), "1001\r");
    }

    #[tokio::test]
    async fn req_027_5_rejected_session_read_keeps_raw_response_separate_from_cleanup() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let definition = catalog
            .definitions()
            .find(|definition| {
                definition.requires_extended_session()
                    && definition
                        .response()
                        .exact_payload_length()
                        .is_some_and(|len| len <= 4)
            })
            .unwrap();
        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 06 50 03 00 32 01 F4\r>",
            "7E8 03 7F 22 31\r>",
            "7E8 02 50 01\r>",
        ]);
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        let candidates = candidates(&catalog);
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;

        let result = state.read(&mut session, definition, start).await;

        assert_eq!(result.status, UdsSessionStatus::Inactive);
        assert_eq!(
            result.evidence.as_ref().unwrap().payload,
            [0x7F, 0x22, 0x31]
        );
        assert_eq!(result.read_observations.len(), 1);
        assert_eq!(
            result.read_observations[0].responses[0].payload,
            [0x7F, 0x22, 0x31]
        );
        assert_eq!(result.observations.len(), 2);
        assert_eq!(result.observations[1].responses[0].payload, [0x50, 0x01]);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn req_027_3_disable_and_timeout_return_to_default_and_remove_signals() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 06 50 03 00 32 01 F4\r>",
            "7E8 02 50 01\r>",
        ]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;
        let outcome = state
            .disable(&mut session, start + Duration::from_secs(1))
            .await;
        assert_eq!(outcome.status, UdsSessionStatus::Inactive);
        assert!(outcome.signals.is_empty());
        assert_eq!(spy.commands().last().unwrap(), "1001\r");

        let exchange = ScriptedExchange::new([
            "OK\r>",
            "OK\r>",
            "7E8 06 50 03 00 32 01 F4\r>",
            "7E8 02 50 01\r>",
        ]);
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;
        let outcome = state
            .tick(&mut session, start + EXTENDED_SESSION_LIMIT)
            .await;
        assert_eq!(outcome.status, UdsSessionStatus::Inactive);
        assert!(outcome.signals.is_empty());
    }

    #[tokio::test]
    async fn req_027_5_negative_activation_stays_off_without_unneeded_cleanup() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange =
            ScriptedExchange::new(["OK\r>", "OK\r>", "7E8 03 7F 10 22\r>", "7E8 02 50 01\r>"]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();

        let outcome = state
            .enable(&mut session, target, responder, &candidates, Instant::now())
            .await;

        assert_eq!(outcome.status, UdsSessionStatus::Inactive);
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("7F 10 22")));
        assert!(outcome.signals.is_empty());
        assert!(!spy.commands().iter().any(|command| command == "1001\r"));
        assert_eq!(spy.commands().last().unwrap(), "1003\r");
    }

    #[tokio::test]
    async fn req_027_3_failed_cleanup_makes_session_indeterminate_and_stops_traffic() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange =
            ScriptedExchange::new(["OK\r>", "OK\r>", "7E8 06 50 03 00 32 01 F4\r>", "ERROR\r>"]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;
        let outcome = state
            .disable(&mut session, start + Duration::from_secs(1))
            .await;
        assert_eq!(outcome.status, UdsSessionStatus::Indeterminate);
        state
            .tick(&mut session, start + Duration::from_secs(4))
            .await;
        assert_eq!(spy.commands().last().unwrap(), "1001\r");
        state.on_reconnect();
        assert_eq!(state.status, UdsSessionStatus::Inactive);
    }

    #[tokio::test]
    async fn req_027_3_disconnect_stops_keepalive_until_reconnect() {
        let catalog = crate::knowledge_db::KnowledgeCatalog::embedded_pinned().unwrap();
        let candidates = candidates(&catalog);
        let exchange = ScriptedExchange::new(["OK\r>", "OK\r>", "7E8 06 50 03 00 32 01 F4\r>"]);
        let spy = exchange.spy();
        let mut session = ElmSession::new(exchange);
        let (target, responder) = target_and_responder();
        let mut state = UdsSessionState::default();
        let start = Instant::now();
        state
            .enable(&mut session, target, responder, &candidates, start)
            .await;
        state.on_disconnect();
        let outcome = state
            .tick(&mut session, start + Duration::from_secs(3))
            .await;
        assert_eq!(outcome.status, UdsSessionStatus::Indeterminate);
        assert_eq!(state.candidate_count(), 0);
        assert_eq!(spy.commands().len(), 3);
        state.on_reconnect();
        assert_eq!(state.status, UdsSessionStatus::Inactive);
    }
}
