//! Versioned, closed semantic tool contract and its safety vocabulary (REQ-041).
//!
//! A tool consumer (the Android assistant today, a local MCP server later) names one of five read-only
//! tools with typed arguments. [`parse_request`] is the only place that turns such a request into a
//! [`Call`]; anything else is a typed [`Refusal`] before any adapter traffic. The module has no
//! transport, no session and no model: executing a [`Call`] belongs to the session owner
//! (`mobile::assistant`), and the budget of one diagnosis lives here so every consumer shares it.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use serde_json::{json, Map, Value};

/// Version of the tool names, argument schemas and result vocabulary below.
pub const INTERFACE_VERSION: u32 = 1;
/// REQ-041.6: live signal reads one diagnosis may start.
pub const MAX_SIGNAL_READS: u32 = 20;
/// REQ-041.6: DTC scans (stored and pending together) one diagnosis may start.
pub const MAX_DTC_SCANS: u32 = 1;
/// REQ-041.6: no new live work starts this long after the diagnosis began.
pub const DIAGNOSIS_LIMIT: Duration = Duration::from_secs(60);
/// Age from which a known value is reported as stale, the same 10 s the Live tab uses.
pub const STALE_AFTER: Duration = Duration::from_secs(10);

const MAX_SIGNAL_ID_LENGTH: usize = 64;

/// The closed tool list (REQ-041.1). Nothing else is ever executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    VehicleInfo,
    SignalList,
    SignalRead,
    DtcScan,
    RecordingInspect,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::VehicleInfo,
        Tool::SignalList,
        Tool::SignalRead,
        Tool::DtcScan,
        Tool::RecordingInspect,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::VehicleInfo => "vehicle_info",
            Tool::SignalList => "signal_list",
            Tool::SignalRead => "signal_read",
            Tool::DtcScan => "dtc_scan",
            Tool::RecordingInspect => "recording_inspect",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Tool::VehicleInfo => {
                "Known facts of the current vehicle session. Sends no vehicle traffic and never reads the VIN."
            }
            Tool::SignalList => {
                "Signals that are available in the current session, each with an opaque id for signal_read. Sends no vehicle traffic."
            }
            Tool::SignalRead => {
                "One live read of a signal by an id from signal_list. Counts against the read budget of the diagnosis."
            }
            Tool::DtcScan => {
                "One scan of stored and pending trouble codes. At most one per diagnosis."
            }
            Tool::RecordingInspect => {
                "Findings of the recording the user selected, derived offline without Bluetooth."
            }
        }
    }

    /// Live tools send vehicle traffic and draw on the budget of a diagnosis.
    pub fn is_live(self) -> bool {
        matches!(self, Tool::SignalRead | Tool::DtcScan)
    }

    fn of(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.name() == name)
    }
}

/// The tool definitions of this interface version, for a model client or an MCP listing.
pub fn definitions() -> Value {
    let tools: Vec<Value> = Tool::ALL
        .into_iter()
        .map(|tool| {
            let (properties, required) = match tool {
                Tool::SignalRead => (
                    json!({"signal":{"type":"string","description":"id issued by signal_list in this session"}}),
                    json!(["signal"]),
                ),
                _ => (json!({}), json!([])),
            };
            json!({"name":tool.name(),"description":tool.description(),"live":tool.is_live(),
                "parameters":{"type":"object","properties":properties,"required":required,
                    "additionalProperties":false}})
        })
        .collect();
    json!({"version":INTERFACE_VERSION,"tools":tools})
}

/// A validated tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    VehicleInfo,
    SignalList,
    SignalRead { signal: String },
    DtcScan,
    RecordingInspect,
}

impl Call {
    pub fn tool(&self) -> Tool {
        match self {
            Call::VehicleInfo => Tool::VehicleInfo,
            Call::SignalList => Tool::SignalList,
            Call::SignalRead { .. } => Tool::SignalRead,
            Call::DtcScan => Tool::DtcScan,
            Call::RecordingInspect => Tool::RecordingInspect,
        }
    }
}

/// Why a request was refused before any adapter traffic (REQ-041.3, REQ-041.5, REQ-041.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    UnknownTool,
    MalformedArguments,
    UnexpectedArgument,
    UnknownSignal,
    AmbiguousSignal,
    ExpiredSignal,
    Unsupported,
    Unsafe,
    SessionChange,
    NoDiagnosis,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::UnknownTool => "unknown_tool",
            Reason::MalformedArguments => "malformed_arguments",
            Reason::UnexpectedArgument => "unexpected_argument",
            Reason::UnknownSignal => "unknown_signal",
            Reason::AmbiguousSignal => "ambiguous_signal",
            Reason::ExpiredSignal => "expired_signal",
            Reason::Unsupported => "unsupported",
            Reason::Unsafe => "unsafe",
            Reason::SessionChange => "session_change",
            Reason::NoDiagnosis => "no_diagnosis",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: Reason,
    pub detail: String,
}

impl Refusal {
    pub fn new(reason: Reason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// Words that name a raw, mutating or session-changing operation. A request that uses one is `unsafe`
/// rather than merely unknown, whatever else it says.
fn is_unsafe_word(text: &str) -> bool {
    const FRAGMENTS: [&str; 14] = [
        "raw",
        "write",
        "clear",
        "erase",
        "reset",
        "securityaccess",
        "routine",
        "coding",
        "adaptation",
        "actuator",
        "flash",
        "program",
        "payload",
        "address",
    ];
    const TOKENS: [&str; 18] = [
        "can", "uds", "elm", "did", "pid", "at", "obd", "security", "session", "command", "send",
        "request", "header", "service", "rate", "interval", "period", "hz",
    ];
    let lower = text.to_ascii_lowercase();
    FRAGMENTS.iter().any(|word| lower.contains(word))
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| TOKENS.contains(&token))
}

/// REQ-041.1/2/3/11: the one gate from a consumer's request to a [`Call`].
///
/// A request is `{"tool":<name>,"arguments":{…},"version":1}`; `arguments` and `version` are optional.
/// The envelope and every tool's arguments are closed: an extra key is refused, never ignored.
pub fn parse_request(request: &Value) -> Result<Call, Refusal> {
    let request = request.as_object().ok_or_else(|| {
        Refusal::new(
            Reason::MalformedArguments,
            "the request is not a JSON object",
        )
    })?;
    if let Some(key) = request
        .keys()
        .find(|key| !matches!(key.as_str(), "tool" | "arguments" | "version"))
    {
        return Err(unexpected(key));
    }
    if let Some(version) = request.get("version") {
        if version.as_u64() != Some(u64::from(INTERFACE_VERSION)) {
            return Err(Refusal::new(
                Reason::Unsupported,
                format!("tool interface version {INTERFACE_VERSION} only"),
            ));
        }
    }
    let name = request
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::new(Reason::MalformedArguments, "tool must be a string"))?;
    let tool = Tool::of(name).ok_or_else(|| {
        if is_unsafe_word(name) {
            Refusal::new(Reason::Unsafe, "no raw, write or session operation exists")
        } else {
            Refusal::new(
                Reason::UnknownTool,
                "the tool is not part of this interface",
            )
        }
    })?;
    let empty = Map::new();
    let arguments = match request.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => {
            return Err(Refusal::new(
                Reason::MalformedArguments,
                "arguments must be an object",
            ))
        }
    };
    let allowed: &[&str] = if tool == Tool::SignalRead {
        &["signal"]
    } else {
        &[]
    };
    if let Some(key) = arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(unexpected(key));
    }
    Ok(match tool {
        Tool::VehicleInfo => Call::VehicleInfo,
        Tool::SignalList => Call::SignalList,
        Tool::DtcScan => Call::DtcScan,
        Tool::RecordingInspect => Call::RecordingInspect,
        Tool::SignalRead => {
            let signal = arguments
                .get("signal")
                .and_then(Value::as_str)
                .filter(|signal| !signal.is_empty() && signal.len() <= MAX_SIGNAL_ID_LENGTH)
                .ok_or_else(|| {
                    Refusal::new(
                        Reason::MalformedArguments,
                        "signal must be a non-empty id string",
                    )
                })?;
            Call::SignalRead {
                signal: signal.to_owned(),
            }
        }
    })
}

fn unexpected(key: &str) -> Refusal {
    if is_unsafe_word(key) {
        Refusal::new(
            Reason::Unsafe,
            "addresses, identifiers, raw requests, payloads, rates and session commands are not arguments",
        )
    } else {
        Refusal::new(
            Reason::UnexpectedArgument,
            "the argument is not part of the schema",
        )
    }
}

/// Process-wide source of session epochs, so an id issued by one session can never match another.
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// REQ-041.2/3: the opaque signal ids of one session. An id names the epoch it was issued in and the
/// position in the listing of that epoch, and the listing keeps the signal each id was issued for. Any
/// change of the session starts a new epoch, so older ids are refused as expired and are never mapped
/// onto a different signal.
#[derive(Debug)]
pub struct SessionIds {
    epoch: u64,
    keys: Vec<String>,
}

impl Default for SessionIds {
    fn default() -> Self {
        Self {
            epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
            keys: Vec::new(),
        }
    }
}

impl SessionIds {
    /// The session changed or reconnected: every issued id expires.
    pub fn invalidate(&mut self) {
        *self = Self::default();
    }

    /// Issue one id per signal key of the current listing, in order; a later listing replaces this one.
    pub fn issue(&mut self, keys: impl IntoIterator<Item = String>) -> Vec<String> {
        self.keys = keys.into_iter().collect();
        (0..self.keys.len())
            .map(|index| format!("s{}.{index}", self.epoch))
            .collect()
    }

    /// The signal key an id was issued for. `semantics` are the semantic names of the current listing: a
    /// bare semantic that several listed signals share is ambiguous, never guessed.
    pub fn resolve(
        &self,
        id: &str,
        semantics: impl Iterator<Item = String>,
    ) -> Result<&str, Refusal> {
        let parsed = id.strip_prefix('s').and_then(|rest| {
            let (epoch, index) = rest.split_once('.')?;
            Some((epoch.parse::<u64>().ok()?, index.parse::<usize>().ok()?))
        });
        let Some((epoch, index)) = parsed else {
            let matches = semantics.filter(|semantic| semantic == id).count();
            return Err(if matches > 1 {
                Refusal::new(
                    Reason::AmbiguousSignal,
                    "several signals share this semantic; use the id from signal_list",
                )
            } else {
                Refusal::new(
                    Reason::UnknownSignal,
                    "only ids issued by signal_list are accepted",
                )
            });
        };
        if epoch != self.epoch {
            return Err(Refusal::new(
                Reason::ExpiredSignal,
                "the session changed; list the signals again",
            ));
        }
        self.keys.get(index).map(String::as_str).ok_or_else(|| {
            Refusal::new(
                Reason::UnknownSignal,
                "no such id was issued in this session",
            )
        })
    }
}

/// Cancels the running diagnosis from any thread, without waiting for the session owner.
#[derive(Debug, Clone, Default)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(crate) fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why a diagnosis starts no further live work (REQ-041.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    TimeLimit,
    ReadLimit,
    DtcLimit,
}

impl Stop {
    pub fn code(self) -> &'static str {
        match self {
            Stop::Cancelled => "cancelled",
            Stop::TimeLimit => "time_limit",
            Stop::ReadLimit => "read_limit",
            Stop::DtcLimit => "dtc_limit",
        }
    }

    pub fn outcome(self) -> Outcome {
        match self {
            Stop::Cancelled => Outcome::Cancelled,
            _ => Outcome::BudgetExhausted,
        }
    }
}

/// The budget of one diagnosis. The clock is an argument, so tests do not sleep.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    started: Instant,
    reads: u32,
    dtc_scans: u32,
}

impl Diagnosis {
    pub fn start(now: Instant) -> Self {
        Self {
            started: now,
            reads: 0,
            dtc_scans: 0,
        }
    }

    /// Whether new live work may start at all; cancellation wins over the time limit.
    pub fn check(&self, now: Instant, cancel: &CancelHandle) -> Result<(), Stop> {
        if cancel.is_cancelled() {
            Err(Stop::Cancelled)
        } else if now.saturating_duration_since(self.started) >= DIAGNOSIS_LIMIT {
            Err(Stop::TimeLimit)
        } else {
            Ok(())
        }
    }

    /// Admit one live signal read and count it.
    pub fn admit_read(&mut self, now: Instant, cancel: &CancelHandle) -> Result<(), Stop> {
        self.check(now, cancel)?;
        if self.reads >= MAX_SIGNAL_READS {
            return Err(Stop::ReadLimit);
        }
        self.reads += 1;
        Ok(())
    }

    /// Admit the diagnosis' one DTC scan and count it.
    pub fn admit_dtc_scan(&mut self, now: Instant, cancel: &CancelHandle) -> Result<(), Stop> {
        self.check(now, cancel)?;
        if self.dtc_scans >= MAX_DTC_SCANS {
            return Err(Stop::DtcLimit);
        }
        self.dtc_scans += 1;
        Ok(())
    }

    pub fn budget(&self) -> Value {
        json!({"signal_reads_used":self.reads,"signal_reads_max":MAX_SIGNAL_READS,
            "dtc_scans_used":self.dtc_scans,"dtc_scans_max":MAX_DTC_SCANS,
            "time_limit_s":DIAGNOSIS_LIMIT.as_secs()})
    }
}

/// The typed result of a tool call; `ok` carries data, every other value says why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Refused,
    Unavailable,
    Busy,
    Disconnected,
    Cancelled,
    BudgetExhausted,
    Failed,
}

impl Outcome {
    pub fn code(self) -> &'static str {
        match self {
            Outcome::Refused => "refused",
            Outcome::Unavailable => "unavailable",
            Outcome::Busy => "busy",
            Outcome::Disconnected => "disconnected",
            Outcome::Cancelled => "cancelled",
            Outcome::BudgetExhausted => "budget_exhausted",
            Outcome::Failed => "failed",
        }
    }
}

pub fn ok(tool: Tool, body: Value) -> Value {
    let mut result = json!({"version":INTERFACE_VERSION,"tool":tool.name(),"outcome":"ok"});
    if let (Some(result), Value::Object(body)) = (result.as_object_mut(), body) {
        result.extend(body);
    }
    result
}

/// A typed non-`ok` result. `tool` is `None` when the request named no usable tool.
pub fn failure(tool: Option<Tool>, outcome: Outcome, reason: &str, detail: &str) -> Value {
    json!({"version":INTERFACE_VERSION,"tool":tool.map(Tool::name),"outcome":outcome.code(),
        "reason":reason,"detail":detail})
}

pub fn refused(tool: Option<Tool>, refusal: &Refusal) -> Value {
    failure(
        tool,
        Outcome::Refused,
        refusal.reason.code(),
        &refusal.detail,
    )
}

/// The tool a request names, when it names a known one; used to label a refusal.
pub fn named_tool(request: &Value) -> Option<Tool> {
    request.get("tool")?.as_str().and_then(Tool::of)
}

/// What kind of knowledge a result item is (REQ-041.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// A fact the vehicle reported as such.
    Observed,
    /// A value decoded deterministically from an observed response.
    Decoded,
    /// A conclusion that is not an observation, for example a VIN-derived make hint.
    Inferred,
    Unknown,
}

impl Evidence {
    fn code(self) -> &'static str {
        match self {
            Evidence::Observed => "observed",
            Evidence::Decoded => "decoded",
            Evidence::Inferred => "inferred",
            Evidence::Unknown => "unknown",
        }
    }
}

/// Provenance of one result item. Unknown parts stay `null`; nothing is invented or defaulted.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub evidence: Evidence,
    /// `live` or `recording`.
    pub source: &'static str,
    pub responder: Option<String>,
    pub timestamp_ms: Option<u64>,
    /// Age of the value when the result was produced.
    pub age: Option<Duration>,
    /// Where in the capture the value was recorded; `null` for a live value.
    pub capture: Value,
}

impl Provenance {
    pub fn to_json(&self) -> Value {
        let freshness = match self.age {
            None => "unknown",
            Some(age) if age >= STALE_AFTER => "stale",
            Some(_) => "fresh",
        };
        json!({"evidence":self.evidence.code(),"source":self.source,"responder":self.responder,
            "timestamp_ms":self.timestamp_ms,"age_ms":self.age.map(|age| age.as_millis() as u64),
            "freshness":freshness,"capture":self.capture})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(request: Value) -> Result<Call, Reason> {
        parse_request(&request).map_err(|refusal| refusal.reason)
    }

    #[test]
    // REQ-041.1
    fn req_041_1_exactly_five_tools_with_closed_schemas() {
        let definitions = definitions();
        let names: Vec<_> = definitions["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "vehicle_info",
                "signal_list",
                "signal_read",
                "dtc_scan",
                "recording_inspect"
            ]
        );
        assert_eq!(definitions["version"], 1);
        assert!(definitions["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["parameters"]["additionalProperties"] == false));
        assert_eq!(parse(json!({"tool":"vehicle_info"})), Ok(Call::VehicleInfo));
        assert_eq!(
            parse(json!({"tool":"signal_read","arguments":{"signal":"s1.0"}})),
            Ok(Call::SignalRead {
                signal: "s1.0".into()
            })
        );
    }

    #[test]
    // REQ-041.3
    fn req_041_3_malformed_unknown_and_unexpected_requests_are_refused() {
        for (request, reason) in [
            (json!("vehicle_info"), Reason::MalformedArguments),
            (json!({"tool":7}), Reason::MalformedArguments),
            (json!({}), Reason::MalformedArguments),
            (json!({"tool":"horn"}), Reason::UnknownTool),
            (
                json!({"tool":"vehicle_info","arguments":[]}),
                Reason::MalformedArguments,
            ),
            (json!({"tool":"signal_read"}), Reason::MalformedArguments),
            (
                json!({"tool":"signal_read","arguments":{"signal":3}}),
                Reason::MalformedArguments,
            ),
            (
                json!({"tool":"signal_read","arguments":{"signal":""}}),
                Reason::MalformedArguments,
            ),
            (
                json!({"tool":"vehicle_info","arguments":{"verbose":true}}),
                Reason::UnexpectedArgument,
            ),
            (
                json!({"tool":"signal_read","arguments":{"signal":"s1.0","extra":1}}),
                Reason::UnexpectedArgument,
            ),
            (
                json!({"tool":"signal_list","note":"x"}),
                Reason::UnexpectedArgument,
            ),
            (
                json!({"tool":"signal_list","version":2}),
                Reason::Unsupported,
            ),
        ] {
            assert_eq!(parse(request.clone()), Err(reason), "{request}");
        }
    }

    #[test]
    // REQ-041.2, REQ-041.11
    fn req_041_2_11_raw_write_and_addressing_vocabulary_is_unsafe() {
        for request in [
            json!({"tool":"raw_command","arguments":{"command":"ATZ"}}),
            json!({"tool":"uds_request"}),
            json!({"tool":"clear_dtc"}),
            json!({"tool":"write_memory"}),
            json!({"tool":"security_access"}),
            json!({"tool":"routine_control"}),
            json!({"tool":"coding"}),
            json!({"tool":"actuator_test"}),
            json!({"tool":"send_elm"}),
            json!({"tool":"signal_read","arguments":{"signal":"s1.0","did":"F190"}}),
            json!({"tool":"signal_read","arguments":{"signal":"s1.0","address":"7E0"}}),
            json!({"tool":"signal_read","arguments":{"signal":"s1.0","payload":"22F190"}}),
            json!({"tool":"signal_read","arguments":{"signal":"s1.0","rate_ms":10}}),
            json!({"tool":"signal_read","arguments":{"signal":"s1.0","session":"extended"}}),
            json!({"tool":"dtc_scan","arguments":{"clear":true}}),
        ] {
            assert_eq!(parse(request.clone()), Err(Reason::Unsafe), "{request}");
        }
    }

    #[test]
    // REQ-041.2, REQ-041.3
    fn req_041_2_3_ids_are_session_bound_and_never_guessed() {
        let mut ids = SessionIds::default();
        let none = || std::iter::empty::<String>();
        let reason = |result: Result<&str, Refusal>| result.unwrap_err().reason;
        assert_eq!(reason(ids.resolve("s0.0", none())), Reason::ExpiredSignal);
        // Nothing is issued before a listing.
        let current = format!("s{}.0", ids.epoch);
        assert_eq!(reason(ids.resolve(&current, none())), Reason::UnknownSignal);
        let issued = ids.issue(["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        assert_eq!(ids.resolve(&issued[2], none()), Ok("c"));
        assert_eq!(
            reason(ids.resolve(&format!("s{}.3", ids.epoch), none())),
            Reason::UnknownSignal
        );
        // A later listing replaces the earlier one under the same epoch.
        let issued = ids.issue(["x".to_owned()]);
        assert_eq!(ids.resolve(&issued[0], none()), Ok("x"));
        let expired = issued[0].clone();
        ids.invalidate();
        assert_eq!(reason(ids.resolve(&expired, none())), Reason::ExpiredSignal);
        let twice = ["engine.rpm".to_owned(), "engine.rpm".to_owned()];
        assert_eq!(
            reason(ids.resolve("engine.rpm", twice.into_iter())),
            Reason::AmbiguousSignal
        );
        assert_eq!(
            reason(ids.resolve("engine.rpm", ["engine.rpm".to_owned()].into_iter())),
            Reason::UnknownSignal
        );
    }

    #[test]
    // REQ-041.6
    fn req_041_6_budget_allows_twenty_reads_one_scan_and_sixty_seconds() {
        let cancel = CancelHandle::default();
        let start = Instant::now();
        let mut diagnosis = Diagnosis::start(start);
        for _ in 0..MAX_SIGNAL_READS {
            assert_eq!(diagnosis.admit_read(start, &cancel), Ok(()));
        }
        assert_eq!(diagnosis.admit_read(start, &cancel), Err(Stop::ReadLimit));
        assert_eq!(diagnosis.admit_dtc_scan(start, &cancel), Ok(()));
        assert_eq!(
            diagnosis.admit_dtc_scan(start, &cancel),
            Err(Stop::DtcLimit)
        );

        let mut late = Diagnosis::start(start);
        assert_eq!(
            late.admit_read(start + Duration::from_millis(59_999), &cancel),
            Ok(())
        );
        assert_eq!(
            late.admit_read(start + DIAGNOSIS_LIMIT, &cancel),
            Err(Stop::TimeLimit)
        );
        cancel.cancel();
        assert_eq!(late.admit_dtc_scan(start, &cancel), Err(Stop::Cancelled));
        assert_eq!(Stop::Cancelled.outcome(), Outcome::Cancelled);
        assert_eq!(Stop::TimeLimit.outcome(), Outcome::BudgetExhausted);
    }

    #[test]
    // REQ-041.8
    fn req_041_8_unknown_parts_stay_null_and_old_values_are_stale() {
        let unknown = Provenance {
            evidence: Evidence::Unknown,
            source: "live",
            responder: None,
            timestamp_ms: None,
            age: None,
            capture: Value::Null,
        }
        .to_json();
        assert_eq!(unknown["freshness"], "unknown");
        assert!(unknown["responder"].is_null() && unknown["timestamp_ms"].is_null());
        let stale = Provenance {
            evidence: Evidence::Decoded,
            source: "recording",
            responder: Some("7E8".into()),
            timestamp_ms: Some(5),
            age: Some(STALE_AFTER),
            capture: json!({"offset_us":1}),
        }
        .to_json();
        assert_eq!(stale["freshness"], "stale");
        assert_eq!(stale["evidence"], "decoded");
    }
}
