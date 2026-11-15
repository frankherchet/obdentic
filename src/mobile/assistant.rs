//! REQ-041: the semantic tools executed on the one session owner.
//!
//! `semantic_tools` decides what a request may say; this module decides what the session can do right now
//! and sends nothing that the closed core actions would not send: a live tool reuses `read_outcome` and
//! `scan_dtcs`, the same code the user's own reads and scans run through. Every refusal and typed
//! outcome is produced before the first adapter byte of that call.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Mutex, TryLockError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

use super::{report, uds, Exchange, MobileCore};
use crate::{
    semantic_tools::{
        self as tools, Call, CancelHandle, Diagnosis, Evidence, Outcome, Provenance, Reason,
        Refusal, SessionIds, Tool,
    },
    signal_read::{OutcomeState, SignalRoute},
};

/// A request longer than this cannot be a tool call.
const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Longest error text handed to a consumer.
const MAX_DETAIL_CHARS: usize = 200;

#[derive(Default)]
pub(super) struct AssistantState {
    ids: SessionIds,
    pub(super) diagnosis: Option<Diagnosis>,
    cancel: CancelHandle,
    /// Chosen by the user through the platform, never by a tool argument.
    recording: Option<PathBuf>,
    /// Availability measured by the last `knowledge.availability` job of this session, by signal key.
    pub(super) measured: BTreeMap<String, String>,
}

impl AssistantState {
    /// A new physical session or vehicle: ids and measurements of the old one are void.
    pub(super) fn session_changed(&mut self) {
        self.ids.invalidate();
        self.measured.clear();
    }

    /// Called after every core action: whatever changed which signals exist or are readable ends the ids.
    pub(super) fn after_action(&mut self, action: &str, uds_changed: bool) {
        match action {
            "initialize" | "inventory" | "ecu.discover" => self.session_changed(),
            "uds.enable" | "uds.disable" | "knowledge.availability" => self.ids.invalidate(),
            _ if uds_changed => self.ids.invalidate(),
            _ => {}
        }
    }

    pub(super) fn record_availability(&mut self, entries: &[Value]) {
        self.measured = entries
            .iter()
            .filter_map(|entry| {
                Some((
                    entry["key"].as_str()?.to_owned(),
                    entry["status"].as_str()?.to_owned(),
                ))
            })
            .collect();
    }
}

/// One signal the session can read right now, before it gets an id.
struct Listed {
    key: String,
    semantic: String,
    info: Value,
}

/// Run an assistant action on a core another thread may be driving. A busy session owner answers with the
/// typed `busy` outcome at once instead of queueing behind its job, and sends nothing (REQ-041.7).
pub fn execute_nonblocking<E: Exchange>(
    core: &Mutex<MobileCore<E>>,
    action: &str,
    argument: &str,
) -> String {
    match core.try_lock() {
        Ok(mut core) => core.execute(action, argument),
        Err(TryLockError::WouldBlock) => {
            let tool = serde_json::from_str::<Value>(argument)
                .ok()
                .and_then(|request| tools::named_tool(&request));
            json!({"ok":true,"data":tools::failure(tool, Outcome::Busy, "session_busy",
                "the session owner is running another operation")})
            .to_string()
        }
        Err(TryLockError::Poisoned(_)) => {
            json!({"ok":false,"error":"native session poisoned"}).to_string()
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn detail(text: &str) -> String {
    text.chars().take(MAX_DETAIL_CHARS).collect()
}

impl<E: Exchange> MobileCore<E> {
    /// The handle that cancels the running diagnosis without waiting for the session owner.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.assistant.cancel.clone()
    }

    pub(super) fn assistant_action(
        &mut self,
        action: &str,
        argument: &str,
    ) -> Result<Value, String> {
        let closed = |argument: &str| {
            if argument.is_empty() {
                Ok(())
            } else {
                Err("this closed action accepts no argument".to_owned())
            }
        };
        match action {
            "tools" => {
                closed(argument)?;
                Ok(tools::definitions())
            }
            "begin" => {
                closed(argument)?;
                self.assistant.cancel.reset();
                let diagnosis = Diagnosis::start(Instant::now());
                let budget = diagnosis.budget();
                self.assistant.diagnosis = Some(diagnosis);
                Ok(json!({"version":tools::INTERFACE_VERSION,"budget":budget}))
            }
            "recording.select" => {
                // The platform passes the file the user picked; an empty argument clears the selection.
                if argument.is_empty() {
                    self.assistant.recording = None;
                    return Ok(json!({"selected":false}));
                }
                let path = PathBuf::from(argument);
                if !path.is_file() {
                    return Err("the selected recording is not a readable file".into());
                }
                self.assistant.recording = Some(path);
                Ok(json!({"selected":true}))
            }
            "tool" => Ok(self.run_tool(argument)),
            _ => Err(format!(
                "read-only mobile core rejected action: assistant.{action}"
            )),
        }
    }

    fn run_tool(&mut self, argument: &str) -> Value {
        if argument.len() > MAX_REQUEST_BYTES {
            let refusal = Refusal::new(Reason::MalformedArguments, "the request is too long");
            return tools::refused(None, &refusal);
        }
        let Ok(request) = serde_json::from_str::<Value>(argument) else {
            let refusal = Refusal::new(Reason::MalformedArguments, "the request is not valid JSON");
            return tools::refused(None, &refusal);
        };
        match tools::parse_request(&request) {
            Err(refusal) => tools::refused(tools::named_tool(&request), &refusal),
            Ok(Call::VehicleInfo) => self.vehicle_info(),
            Ok(Call::SignalList) => self.signal_list(),
            Ok(Call::SignalRead { signal }) => self.signal_read(&signal),
            Ok(Call::DtcScan) => self.dtc_scan_tool(),
            Ok(Call::RecordingInspect) => self.recording_inspect(),
        }
    }

    /// No ready session, no information and no live work (REQ-041.7).
    fn session_gate(&self, tool: Tool) -> Option<Value> {
        if !self.initialized {
            return Some(no_session(tool));
        }
        if self.uds.snapshot().status == uds::UdsSessionStatus::Indeterminate {
            return Some(tools::failure(
                Some(tool),
                Outcome::Unavailable,
                "session_indeterminate",
                "the diagnostic session state is unknown; reconnect first",
            ));
        }
        None
    }

    fn vehicle_info(&self) -> Value {
        if let Some(gate) = self.session_gate(Tool::VehicleInfo) {
            return gate;
        }
        let Some(session) = self.session.as_ref() else {
            return no_session(Tool::VehicleInfo);
        };
        let support = session.supported();
        let responders: BTreeSet<&str> = support
            .discovery
            .iter()
            .filter_map(|discovery| discovery.responder.as_ref().map(|r| r.as_str()))
            .collect();
        let advertised = crate::supported_signals()
            .iter()
            .filter(|signal| support.supports_pid(signal.metadata().request[1]))
            .count();
        let knowledge = match &self.knowledge {
            Some(knowledge) => {
                let roles: BTreeSet<&str> = knowledge
                    .present_targets()
                    .map(|target| target.candidate.role.as_str())
                    .collect();
                json!({"identified_ecu_roles":roles,"evidence":"observed",
                    "repository":knowledge.catalog.pin().repository(),
                    "revision":knowledge.catalog.pin().revision()})
            }
            None => json!({"identified_ecu_roles":[],"evidence":"unknown"}),
        };
        let manufacturer = match &self.vehicle_manufacturer {
            Some(name) => json!({"value":name,"evidence":"inferred","basis":"VIN make hint"}),
            None => json!({"value":null,"evidence":"unknown"}),
        };
        let uds = self.uds.snapshot();
        tools::ok(
            Tool::VehicleInfo,
            json!({
                "session":{"state":"ready","evidence":"observed",
                    "extended_session_active":uds.active,"uds_state":uds.status.as_str()},
                "manufacturer":manufacturer,
                "engine_target_validated":{"value":self.engine.is_some(),"evidence":"observed"},
                "responders":{"value":responders,"evidence":"observed"},
                "advertised_standard_signals":{"value":advertised,"evidence":"observed"},
                "knowledge":knowledge,
                "withheld":["vin","ecu_serials","adapter_identifier"],
                "provenance":live_provenance(Evidence::Observed, None, None).to_json()
            }),
        )
    }

    /// The signals this session can read now: standard PIDs the vehicle advertises and Knowledge signals of
    /// identified ECUs, minus those the last availability measurement found rejected or silent. Extended
    /// session signals are listed only while that session is active.
    fn eligible_signals(&self) -> Vec<Listed> {
        let mut listed = Vec::new();
        let Some(session) = self.session.as_ref() else {
            return listed;
        };
        let support = session.supported();
        let measured = |key: &str| self.assistant.measured.get(key).map(String::as_str);
        for signal in crate::supported_signals() {
            let meta = signal.metadata();
            if !support.supports_pid(meta.request[1])
                || matches!(measured(meta.semantic), Some("rejected" | "not_answered"))
            {
                continue;
            }
            listed.push(Listed {
                key: meta.semantic.to_owned(),
                semantic: meta.semantic.to_owned(),
                info: json!({"semantic":meta.semantic,"title":meta.description,"unit":meta.unit,
                    "source":"standard","ecu_role":null,"session":"default",
                    "availability":availability(measured(meta.semantic),"advertised"),
                    "validation":meta.hardware_validation}),
            });
        }
        let Some(knowledge) = &self.knowledge else {
            return listed;
        };
        let signals = knowledge
            .signal_catalog(|target, definition| self.uds_can_read(target, definition))
            .unwrap_or_default();
        let identification: BTreeSet<&str> = knowledge
            .plan
            .candidates()
            .iter()
            .map(|candidate| candidate.semantic())
            .collect();
        for signal in signals {
            let (Some(key), Some(semantic)) = (signal["key"].as_str(), signal["semantic"].as_str())
            else {
                continue;
            };
            // ECU identification (serials, part numbers, software numbers) is withheld by default (REQ-041.9).
            if identification.contains(semantic)
                || matches!(measured(key), Some("rejected" | "not_answered"))
            {
                continue;
            }
            listed.push(Listed {
                key: key.to_owned(),
                semantic: semantic.to_owned(),
                info: json!({"semantic":semantic,"title":signal["title"],"unit":signal["unit"],
                    "source":"knowledge","ecu_role":signal["ecu"],
                    "session":if signal["extended_session_required"] == true {"extended"} else {"default"},
                    "availability":availability(measured(key),"identified_ecu"),
                    "validation":signal["validation"]}),
            });
        }
        listed
    }

    fn signal_list(&mut self) -> Value {
        if let Some(gate) = self.session_gate(Tool::SignalList) {
            return gate;
        }
        let listing = self.eligible_signals();
        let ids = self
            .assistant
            .ids
            .issue(listing.iter().map(|listed| listed.key.clone()));
        let signals: Vec<Value> = listing
            .iter()
            .zip(ids)
            .map(|(listed, id)| {
                let mut info = listed.info.clone();
                info["id"] = json!(id);
                info
            })
            .collect();
        tools::ok(
            Tool::SignalList,
            json!({"count":signals.len(),"signals":signals,
                "ids_valid_until":"the session changes or reconnects",
                "provenance":live_provenance(Evidence::Observed, None, None).to_json()}),
        )
    }

    /// Would reading this key end the active restricted (extended) session? Then the tool refuses before
    /// any traffic (REQ-041.5): only reads that the session itself carries may continue inside it.
    fn read_would_change_session(&self, key: &str) -> Result<bool, String> {
        if !self.uds.is_active() {
            return Ok(false);
        }
        let route = SignalRoute::of(key);
        if let SignalRoute::Standard(_) = route {
            return Ok(!self.uds_matches_validated_engine());
        }
        let knowledge = self
            .knowledge
            .as_ref()
            .ok_or("Knowledge is not available in this session")?;
        let resolved = knowledge.select_signal(key, &route, |target, definition| {
            self.uds_can_read(target, definition)
        })?;
        Ok(!resolved.definition.requires_extended_session()
            && !self.uds_target_matches(&resolved.candidate))
    }

    fn signal_read(&mut self, signal: &str) -> Value {
        let tool = Tool::SignalRead;
        if let Some(gate) = self.session_gate(tool) {
            return gate;
        }
        if self.assistant.diagnosis.is_none() {
            return tools::refused(
                Some(tool),
                &Refusal::new(Reason::NoDiagnosis, "start a diagnosis before live reads"),
            );
        }
        let listing = self.eligible_signals();
        let key = match self
            .assistant
            .ids
            .resolve(signal, listing.iter().map(|listed| listed.semantic.clone()))
        {
            Ok(key) => key.to_owned(),
            Err(refusal) => return tools::refused(Some(tool), &refusal),
        };
        // An issued id never becomes another signal: when its signal stopped being readable it is unavailable.
        let Some(listed) = listing.iter().find(|listed| listed.key == key) else {
            return tools::failure(
                Some(tool),
                Outcome::Unavailable,
                "no_longer_available",
                "the signal is not available in this session any more",
            );
        };
        match self.read_would_change_session(&listed.key) {
            Ok(false) => {}
            Ok(true) => {
                return tools::refused(
                    Some(tool),
                    &Refusal::new(
                        Reason::SessionChange,
                        "this read would end the active extended session",
                    ),
                )
            }
            Err(error) => {
                return tools::failure(
                    Some(tool),
                    Outcome::Unavailable,
                    "unavailable",
                    &detail(&error),
                )
            }
        }
        if let Some(stop) = self.admit(|diagnosis, now, cancel| diagnosis.admit_read(now, cancel)) {
            return stop_outcome(tool, stop);
        }
        match self.read_outcome(&listed.key) {
            Err(error) => {
                // Safety cleanup after an error stays what it is for the user's own reads (REQ-041.5).
                if self.uds.is_active() {
                    let _ = self.disable_uds_outcome();
                }
                let reason = if crate::scheduler::is_fatal_runtime_error(&error) {
                    "transport"
                } else {
                    "read_error"
                };
                tools::failure(Some(tool), Outcome::Failed, reason, &detail(&error))
            }
            Ok((outcome, extras)) => {
                let signal = json!({"id":signal,"semantic":listed.semantic,
                    "source":listed.info["source"],"ecu_role":listed.info["ecu_role"]});
                match &outcome.state {
                    OutcomeState::Value { value, implausible } => {
                        let responder = extras
                            .get("observations")
                            .and_then(Value::as_array)
                            .and_then(|observations| {
                                observations.iter().rev().find_map(|observation| {
                                    observation["selected_responder"]
                                        .as_str()
                                        .map(str::to_owned)
                                })
                            });
                        let timestamp = outcome
                            .transaction
                            .as_ref()
                            .map_or_else(now_ms, |t| t.timestamp_ms() as u64);
                        let range = implausible
                            .map(|(minimum, maximum)| json!({"minimum":minimum,"maximum":maximum}));
                        tools::ok(
                            Tool::SignalRead,
                            json!({"signal":signal,"value":value,"unit":outcome.unit,
                                "plausible":implausible.is_none(),"plausible_range":range,
                                "validation":listed.info["validation"],
                                "provenance":live_provenance(Evidence::Decoded, responder, Some(timestamp)).to_json()}),
                        )
                    }
                    OutcomeState::Rejected { nrc } => tools::failure(
                        Some(tool),
                        Outcome::Failed,
                        "rejected",
                        &format!("the ECU rejected the read with NRC 0x{nrc:02X}"),
                    ),
                    OutcomeState::NotAnswered { reason } => tools::failure(
                        Some(tool),
                        Outcome::Failed,
                        "not_answered",
                        &detail(reason.as_deref().unwrap_or("no usable answer")),
                    ),
                }
            }
        }
    }

    /// Ask the diagnosis whether one more live step may start; counts it when it may.
    fn admit(
        &mut self,
        admit: impl FnOnce(&mut Diagnosis, Instant, &CancelHandle) -> Result<(), tools::Stop>,
    ) -> Option<tools::Stop> {
        let diagnosis = self.assistant.diagnosis.as_mut()?;
        admit(diagnosis, Instant::now(), &self.assistant.cancel).err()
    }

    fn dtc_scan_tool(&mut self) -> Value {
        let tool = Tool::DtcScan;
        if let Some(gate) = self.session_gate(tool) {
            return gate;
        }
        let Some(diagnosis) = self.assistant.diagnosis.clone() else {
            return tools::refused(
                Some(tool),
                &Refusal::new(Reason::NoDiagnosis, "start a diagnosis before live scans"),
            );
        };
        // A scan first leaves any extended session, which is a session change a tool never makes.
        if self.uds.is_active() {
            return tools::refused(
                Some(tool),
                &Refusal::new(
                    Reason::SessionChange,
                    "a scan would end the active extended session",
                ),
            );
        }
        if let Some(stop) =
            self.admit(|diagnosis, now, cancel| diagnosis.admit_dtc_scan(now, cancel))
        {
            return stop_outcome(tool, stop);
        }
        let cancel = self.assistant.cancel.clone();
        let proceed = move || diagnosis.check(Instant::now(), &cancel).err();
        match self.scan_dtcs(proceed) {
            Err(error) => {
                if self.uds.is_active() {
                    let _ = self.disable_uds_outcome();
                }
                tools::failure(Some(tool), Outcome::Failed, "scan_error", &detail(&error))
            }
            Ok(scan) => {
                let view = |section: &Value| {
                    if section.get("observations").is_some() {
                        json!({"observations":section["observations"],"errors":section["errors"]})
                    } else {
                        section.clone()
                    }
                };
                tools::ok(
                    Tool::DtcScan,
                    json!({"stored":view(&scan),"pending":view(&scan["pending"]),
                        "provenance":live_provenance(Evidence::Decoded, None, Some(now_ms())).to_json()}),
                )
            }
        }
    }

    /// REQ-041.10: the selected recording is read offline through the same report the desktop uses; a
    /// Knowledge pin that differs from the embedded one stops the inspection.
    fn recording_inspect(&self) -> Value {
        let tool = Tool::RecordingInspect;
        let Some(path) = &self.assistant.recording else {
            return tools::failure(
                Some(tool),
                Outcome::Unavailable,
                "no_recording_selected",
                "the user has not selected a recording",
            );
        };
        let report = match report::report(path) {
            Ok(report) => report,
            Err(error) => {
                return tools::failure(
                    Some(tool),
                    Outcome::Failed,
                    "recording_unreadable",
                    &detail(&error),
                )
            }
        };
        if report["knowledge_pin_mismatch"] == true {
            return tools::failure(
                Some(tool),
                Outcome::Failed,
                "knowledge_pin_mismatch",
                &format!(
                    "the recording's Knowledge pin {}",
                    super::knowledge::PIN_MISMATCH
                ),
            );
        }
        tools::ok(Tool::RecordingInspect, recording_view(&report))
    }
}

/// How sure the listing is that a signal answers: measured by the availability job, else the weaker
/// evidence of how it was listed.
fn availability(measured: Option<&str>, otherwise: &'static str) -> &'static str {
    if measured == Some("answered") {
        "answered"
    } else {
        otherwise
    }
}

fn live_provenance(
    evidence: Evidence,
    responder: Option<String>,
    timestamp_ms: Option<u64>,
) -> Provenance {
    Provenance {
        evidence,
        source: "live",
        responder,
        timestamp_ms,
        age: timestamp_ms.map(|_| Duration::ZERO),
        capture: Value::Null,
    }
}

fn no_session(tool: Tool) -> Value {
    tools::failure(
        Some(tool),
        Outcome::Disconnected,
        "no_session",
        "there is no ready adapter session",
    )
}

fn stop_outcome(tool: Tool, stop: tools::Stop) -> Value {
    let detail = match stop {
        tools::Stop::Cancelled => "the diagnosis was cancelled; no further live work starts",
        tools::Stop::TimeLimit => "the 60 s limit of the diagnosis has passed",
        tools::Stop::ReadLimit => "the diagnosis used its 20 signal reads",
        tools::Stop::DtcLimit => "the diagnosis used its DTC scan",
    };
    tools::failure(Some(tool), stop.outcome(), stop.code(), detail)
}

/// What a consumer may see of a recording report: decoded values, the last DTC scan and availability
/// statuses with their recorded provenance. Raw responses, identifiers and discovery data stay in the file.
fn recording_view(report: &Value) -> Value {
    let end = report["recording_end_offset_us"].as_u64();
    let signals: Vec<Value> = report["last_values"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, data)| {
            let recorded = &report["value_provenance"][key];
            let offset = recorded["offset_us"].as_u64();
            let responder = data["observations"].as_array().and_then(|observations| {
                observations.iter().rev().find_map(|observation| {
                    observation["selected_responder"].as_str().map(str::to_owned)
                })
            });
            let provenance = Provenance {
                evidence: Evidence::Decoded,
                source: "recording",
                responder,
                timestamp_ms: None,
                age: end
                    .zip(offset)
                    .map(|(end, offset)| Duration::from_micros(end.saturating_sub(offset))),
                capture: json!({"recorded_at":recorded["at"],"offset_us":recorded["offset_us"]}),
            };
            json!({"key":key,"semantic":data["semantic"],"value":data.get("value").unwrap_or(&Value::Null),
                "unit":data["unit"],"status":data["status"],"reason":data["error"],
                "provenance":provenance.to_json()})
        })
        .collect();
    let last_job = |action: &str| {
        report["diagnostic_jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .find(|job| job["action"] == action)
    };
    let dtc = last_job("dtc.scan").map(|job| {
        let data = &job["data"];
        json!({"recorded_at":job["at"],"stored":data["observations"],
            "pending":data["pending"].get("observations").unwrap_or(&Value::Null)})
    });
    let availability = last_job("knowledge.availability")
        .map(|job| json!({"recorded_at":job["at"],"entries":job["table"],"counts":job["counts"]}));
    json!({
        "recording":{"mode":report["mode"],"profile":report["profile"],"complete":report["complete"],
            "event_count":report["event_count"],"outcome":report["outcome"]},
        "signals":signals,
        "dtc_scan":dtc,
        "availability":availability,
        "measurement_gaps":report["measurement_gaps"],
        "issues":report["issues"],
        "withheld":["raw_responses","vin","ecu_serials","adapter_identifier"]
    })
}
