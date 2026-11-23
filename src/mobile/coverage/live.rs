//! The live coverage ledger of the mobile core (REQ-038.6, REQ-038.8, REQ-038.10).
//!
//! A live Smart run folds the same `coverage.*` records that are written into the recording, through
//! the same [`parse`] and [`SmartRun::apply`] the offline derivation uses, so live and replay cannot
//! count differently. The vehicle ledger is attached by importing a stored ledger and is credited by
//! `SmartRun::apply` only for facts with a confirmed identity. Everything here is pure: no transport,
//! no clock, no values, VINs or raw responses.
//!
//! A record that cannot be folded is never skipped: a skipped boundary or failed read could let two
//! readings around it count as one continuous segment. It marks the live ledger invalid, and an
//! invalid ledger shows no figures.
use std::{collections::BTreeSet, time::Duration};

use serde_json::{json, Map, Value};

use super::{parse, summarize, text, token, Item, Parsed, REASONS};
use crate::coverage_ledger::{
    DefinitionVersions, EntryKey, EvidenceRef, Fact, FactBody, GoalContext, IdentityKey,
    IdentityObservation, Latest, LatestState, Ledger, Rules, RunId, SessionKind, SessionState,
    SmartRun, BASELINE_LABEL,
};

/// Version of the exported vehicle ledger.
const LEDGER_VERSION: u64 = 1;
/// Upper bound of entries or identity observations of one imported ledger.
const MAX_ITEMS: usize = 100_000;
const MAX_EVIDENCE_LEN: usize = 256;

/// Why a stored ledger was not imported: a fixed vocabulary, never a value of the ledger.
const UNREADABLE: &str = "unreadable";
const UNKNOWN_VERSION: &str = "unknown_version";
const MALFORMED: &str = "malformed";
const TOO_LARGE: &str = "too_large";

struct Active {
    smart: SmartRun,
    versions: DefinitionVersions,
    last_offset: Duration,
    ended: bool,
}

/// State of the live ledger. Replaced by every `coverage.live.start`.
#[derive(Default)]
pub(in crate::mobile) struct Live {
    run: Option<Active>,
    vehicle: Option<Ledger>,
    invalid: bool,
}

impl Live {
    /// Executes one `coverage.live.*` or `coverage.ledger.summary` action.
    pub(in crate::mobile) fn act(&mut self, action: &str, argument: &str) -> Result<Value, String> {
        match action {
            "coverage.live.start" => self.start(&parse_argument(argument)?),
            "coverage.live.record" => self.record(&parse_argument(argument)?),
            "coverage.live.snapshot" => Ok(self.snapshot()),
            "coverage.live.export" => Ok(self.vehicle.as_ref().map_or(Value::Null, export)),
            "coverage.live.detach_vehicle" => {
                self.vehicle = None;
                Ok(json!({"vehicle_available":false}))
            }
            "coverage.live.end" => {
                if let Some(run) = &mut self.run {
                    run.ended = true;
                }
                Ok(json!({"active":false}))
            }
            "coverage.ledger.summary" => {
                let argument = parse_argument(argument)?;
                let ledger = import(argument.get("ledger").unwrap_or(&Value::Null))
                    .map_err(|reason| reason.to_owned())?;
                Ok(summary(&ledger))
            }
            _ => Err("unknown coverage action".into()),
        }
    }

    fn start(&mut self, argument: &Value) -> Result<Value, String> {
        let run = token(argument.get("run")).ok_or("run is missing or not a token")?;
        let goal_definition = argument
            .get("goal_definition")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or("goal_definition is missing")?;
        let knowledge_revision = argument
            .get("knowledge_revision")
            .and_then(text)
            .ok_or("knowledge_revision is missing")?;
        let (vehicle, error) = match argument.get("vehicle") {
            None | Some(Value::Null) => (None, None),
            Some(stored) => match import(stored) {
                Ok(ledger) => (Some(ledger), None),
                Err(reason) => (None, Some(reason)),
            },
        };
        let available = vehicle.is_some();
        self.run = Some(Active {
            smart: SmartRun::new(
                Rules::generic_baseline(),
                RunId(run.to_owned()),
                SessionKind::Live,
            ),
            versions: DefinitionVersions {
                goal_definition,
                knowledge_revision: knowledge_revision.to_owned(),
            },
            last_offset: Duration::ZERO,
            ended: false,
        });
        self.vehicle = vehicle;
        self.invalid = false;
        Ok(json!({"vehicle_available":available,"vehicle_error":error}))
    }

    fn record(&mut self, record: &Value) -> Result<Value, String> {
        if self.invalid {
            return Err("coverage ledger is invalid".into());
        }
        let active = match &mut self.run {
            Some(active) if !active.ended => active,
            _ => return Err("no active coverage run".into()),
        };
        // The record is folded into copies and committed only when every step succeeded.
        let outcome = (|| {
            let parsed = parse(record).map_err(|invalid| invalid.reason)?;
            if parsed.run != active.smart.id().0 {
                return Err("record belongs to another run".to_owned());
            }
            if parsed.offset < active.last_offset {
                return Err("run_offset_us goes backwards".to_owned());
            }
            let fact = match parsed.parsed {
                Parsed::Run { .. } => return Ok(None),
                Parsed::Item(Item::Read(fact)) => fact,
                Parsed::Item(Item::Break {
                    boundary,
                    offset,
                    at,
                }) => Fact {
                    run: active.smart.id().clone(),
                    wall_clock_unix_ms: at,
                    offset,
                    versions: active.versions.clone(),
                    identity_confirmed: false,
                    body: FactBody::Boundary(boundary),
                },
            };
            let mut smart = active.smart.clone();
            let mut vehicle = self.vehicle.clone();
            smart
                .apply(vehicle.as_mut(), &fact)
                .map_err(|error| error.to_string())?;
            Ok(Some((smart, vehicle, fact.offset)))
        })();
        match outcome {
            Ok(None) => Ok(json!({"counted":false})),
            Ok(Some((smart, vehicle, offset))) => {
                active.smart = smart;
                active.last_offset = offset;
                self.vehicle = vehicle;
                Ok(json!({"counted":true}))
            }
            Err(reason) => {
                self.invalid = true;
                Err(reason)
            }
        }
    }

    fn snapshot(&self) -> Value {
        let shown = !self.invalid;
        let run = self.run.as_ref().filter(|_| shown);
        json!({
            "active":self.run.as_ref().is_some_and(|run| !run.ended),
            "run":run.map(|run| summary(run.smart.ledger())),
            "vehicle":self.vehicle.as_ref().filter(|_| shown).map(summary),
            "vehicle_available":self.vehicle.is_some(),
            "session_only":run.is_some_and(|run| run.smart.is_session_only()),
            "invalid":self.invalid,
            "label":BASELINE_LABEL,
        })
    }
}

fn parse_argument(argument: &str) -> Result<Value, String> {
    if argument.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(argument).map_err(|_| "argument is not JSON".to_owned())
}

/// Per-entry and per-context figures of a ledger under the generic baseline rules.
fn summary(ledger: &Ledger) -> Value {
    let (entries, contexts) = summarize(ledger, &Rules::generic_baseline());
    json!({"contexts":contexts,"entries":entries})
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn session_name(session: SessionState) -> &'static str {
    match session {
        SessionState::Default => "default",
        SessionState::Extended => "extended",
    }
}

fn latest_state(state: &LatestState) -> (&'static str, Option<u8>, Option<&str>) {
    match state {
        LatestState::NotYetObserved => ("not_yet_observed", None, None),
        LatestState::Observed => ("observed", None, None),
        LatestState::Unsupported => ("unsupported", None, None),
        LatestState::Rejected { response_code } => ("rejected", Some(*response_code), None),
        LatestState::Empty => ("empty", None, None),
        LatestState::Error { reason } => ("error", None, Some(reason)),
        LatestState::NoAnswer { reason } => ("no_answer", None, Some(reason)),
        LatestState::Gap => ("gap", None, None),
        LatestState::SessionRestricted { reason } => ("session_restricted", None, Some(reason)),
        LatestState::Implausible => ("implausible", None, None),
        LatestState::ContextNotFresh => ("context_not_fresh", None, None),
    }
}

fn export_latest(latest: &Latest) -> Value {
    let (state, code, reason) = latest_state(&latest.state);
    json!({"state":state,"response_code":code,"reason":reason,"run":latest.run.0,
        "at":latest.wall_clock_unix_ms,"offset_us":micros(latest.offset),
        "session":session_name(latest.session),"evidence":latest.evidence.0})
}

/// The versioned, lossless export of a vehicle ledger: counters, latest availability and the
/// provenance of static identities, never a value, VIN or raw response.
fn export(ledger: &Ledger) -> Value {
    let entries: Vec<Value> = ledger
        .entries
        .iter()
        .map(|(key, entry)| {
            json!({"goal_definition":key.versions.goal_definition,
                "knowledge_revision":key.versions.knowledge_revision,
                "signal_key":key.signal_key,"context":key.context.id(),
                "qualifying_samples":entry.qualifying_samples,
                "valid_time_us":micros(entry.valid_time),
                "longest_segment_us":micros(entry.longest_segment),
                "latest":entry.latest.as_ref().map(export_latest)})
        })
        .collect();
    let identity: Vec<Value> = ledger
        .identity
        .iter()
        .map(|(key, observation)| {
            json!({"goal_definition":key.versions.goal_definition,
                "knowledge_revision":key.versions.knowledge_revision,
                "signal_key":key.signal_key,"ecu_target":key.ecu_target,
                "run":observation.run.0,"at":observation.wall_clock_unix_ms,
                "evidence":observation.evidence.0})
        })
        .collect();
    json!({"v":LEDGER_VERSION,"entries":entries,"identity":identity})
}

fn string_of(object: &Map<String, Value>, name: &str) -> Option<String> {
    object.get(name).and_then(text).map(str::to_owned)
}

fn evidence_of(object: &Map<String, Value>) -> Option<EvidenceRef> {
    string_of(object, "evidence")
        .filter(|evidence| evidence.len() <= MAX_EVIDENCE_LEN)
        .map(EvidenceRef)
}

fn versions_of(object: &Map<String, Value>) -> Option<DefinitionVersions> {
    Some(DefinitionVersions {
        goal_definition: u32::try_from(object.get("goal_definition")?.as_u64()?).ok()?,
        knowledge_revision: string_of(object, "knowledge_revision")?,
    })
}

fn run_of(object: &Map<String, Value>) -> Option<RunId> {
    token(object.get("run")).map(|run| RunId(run.to_owned()))
}

fn import_latest(value: &Value) -> Option<Latest> {
    let object = value.as_object()?;
    let reason = || string_of(object, "reason").filter(|reason| REASONS.contains(&reason.as_str()));
    let state = match object.get("state")?.as_str()? {
        "not_yet_observed" => LatestState::NotYetObserved,
        "observed" => LatestState::Observed,
        "unsupported" => LatestState::Unsupported,
        "rejected" => LatestState::Rejected {
            response_code: u8::try_from(object.get("response_code")?.as_u64()?).ok()?,
        },
        "empty" => LatestState::Empty,
        "error" => LatestState::Error { reason: reason()? },
        "no_answer" => LatestState::NoAnswer { reason: reason()? },
        "gap" => LatestState::Gap,
        "session_restricted" => LatestState::SessionRestricted { reason: reason()? },
        "implausible" => LatestState::Implausible,
        "context_not_fresh" => LatestState::ContextNotFresh,
        _ => return None,
    };
    Some(Latest {
        state,
        run: run_of(object)?,
        wall_clock_unix_ms: object.get("at")?.as_i64()?,
        offset: Duration::from_micros(object.get("offset_us")?.as_u64()?),
        session: match object.get("session")?.as_str()? {
            "default" => SessionState::Default,
            "extended" => SessionState::Extended,
            _ => return None,
        },
        evidence: evidence_of(object)?,
    })
}

fn import_entry(value: &Value) -> Option<(EntryKey, crate::coverage_ledger::Entry)> {
    let object = value.as_object()?;
    let key = EntryKey {
        versions: versions_of(object)?,
        signal_key: string_of(object, "signal_key")?,
        context: GoalContext::new(string_of(object, "context")?),
    };
    let entry = crate::coverage_ledger::Entry {
        qualifying_samples: u32::try_from(object.get("qualifying_samples")?.as_u64()?).ok()?,
        valid_time: Duration::from_micros(object.get("valid_time_us")?.as_u64()?),
        longest_segment: Duration::from_micros(object.get("longest_segment_us")?.as_u64()?),
        latest: match object.get("latest")? {
            Value::Null => None,
            latest => Some(import_latest(latest)?),
        },
    };
    Some((key, entry))
}

fn import_identity(value: &Value) -> Option<(IdentityKey, IdentityObservation)> {
    let object = value.as_object()?;
    let key = IdentityKey {
        versions: versions_of(object)?,
        signal_key: string_of(object, "signal_key")?,
        ecu_target: match object.get("ecu_target")? {
            Value::Null => None,
            target => Some(text(target)?.to_owned()),
        },
    };
    let observation = IdentityObservation {
        run: run_of(object)?,
        wall_clock_unix_ms: object.get("at")?.as_i64()?,
        evidence: evidence_of(object)?,
    };
    Some((key, observation))
}

/// Reads an exported vehicle ledger. Only version 1 is understood; anything else is refused with
/// a short fixed reason, never silently read and never echoed.
fn import(stored: &Value) -> Result<Ledger, &'static str> {
    let object = stored.as_object().ok_or(UNREADABLE)?;
    match object.get("v").and_then(Value::as_u64) {
        Some(LEDGER_VERSION) => {}
        Some(_) => return Err(UNKNOWN_VERSION),
        None => return Err(UNREADABLE),
    }
    let list = |name: &str| object.get(name).and_then(Value::as_array).ok_or(MALFORMED);
    let (entries, identity) = (list("entries")?, list("identity")?);
    if entries.len() > MAX_ITEMS || identity.len() > MAX_ITEMS {
        return Err(TOO_LARGE);
    }
    let mut ledger = Ledger::default();
    for value in entries {
        let (key, entry) = import_entry(value).ok_or(MALFORMED)?;
        if ledger.entries.insert(key, entry).is_some() {
            return Err(MALFORMED);
        }
    }
    let mut seen = BTreeSet::new();
    for value in identity {
        let (key, observation) = import_identity(value).ok_or(MALFORMED)?;
        if !seen.insert(key.clone()) {
            return Err(MALFORMED);
        }
        ledger.identity.insert(key, observation);
    }
    Ok(ledger)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::super::GOAL_DEFINITION;
    use super::*;

    const OTHER_RUN: &str = "run-bbbb";

    fn start_arg(run: &str, vehicle: &Value) -> String {
        json!({"run":run,"goal_definition":GOAL_DEFINITION,"knowledge_revision":PIN,
            "vehicle":vehicle})
        .to_string()
    }

    fn empty_vehicle() -> Value {
        json!({"v":1,"entries":[],"identity":[]})
    }

    fn confirmed(mut record: Value) -> Value {
        record["result"]["data"]["identity_confirmed"] = json!(true);
        record
    }

    fn feed(live: &mut Live, record: &Value) -> Result<Value, String> {
        live.act("coverage.live.record", &record.to_string())
    }

    fn rpm(run: &str, seconds: u64) -> Value {
        confirmed(fact_with(
            run,
            "engine.rpm",
            secs(seconds),
            json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
        ))
    }

    /// A first run with a static identity, a rejected read with code, a gap and a failed read.
    fn first_run(live: &mut Live) {
        let started = live
            .act("coverage.live.start", &start_arg(RUN, &empty_vehicle()))
            .unwrap();
        assert_eq!(
            started,
            json!({"vehicle_available":true,"vehicle_error":null})
        );
        feed(live, &start(RUN, 0)).unwrap();
        let mut identity = confirmed(fact_with(
            RUN,
            "vehicle.id",
            0,
            json!({"outcome":"decoded","value_kind":"text","nonempty":true}),
        ));
        identity["result"]["data"]["static_identity"] = json!(true);
        identity["result"]["data"]["ecu_target"] = json!("engine");
        feed(live, &identity).unwrap();
        for second in [0, 2, 4, 6] {
            feed(live, &rpm(RUN, second)).unwrap();
        }
        let rejected = confirmed(fact_with(
            RUN,
            "engine.load",
            secs(7),
            json!({"outcome":"rejected","response_code":18}),
        ));
        feed(live, &rejected).unwrap();
        feed(live, &boundary(RUN, "gap", secs(8))).unwrap();
        feed(live, &rpm(RUN, 10)).unwrap();
        feed(live, &confirmed(failed_read("engine.rpm", 12))).unwrap();
    }

    fn export_of(live: &mut Live) -> Value {
        live.act("coverage.live.export", "{}").unwrap()
    }

    fn entry<'a>(summary: &'a Value, signal: &str) -> &'a Value {
        summary["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["signal_key"] == signal)
            .unwrap_or_else(|| panic!("no entry for {signal}"))
    }

    #[test]
    fn req_038_6_export_import_export_is_identical_and_exports_a_versioned_ledger() {
        let mut live = Live::default();
        first_run(&mut live);
        let exported = export_of(&mut live);
        assert_eq!(exported["v"], 1);
        assert_eq!(exported["identity"].as_array().unwrap().len(), 1);
        let latest_states: Vec<&Value> = exported["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| &entry["latest"]["state"])
            .collect();
        assert!(latest_states.contains(&&json!("rejected")), "{exported}");
        assert!(latest_states.contains(&&json!("error")), "{exported}");
        let rejected = entry(&exported, "engine.load");
        assert_eq!(rejected["latest"]["response_code"], 18);
        assert_eq!(rejected["latest"]["run"], RUN);

        let mut second = Live::default();
        let started = second
            .act("coverage.live.start", &start_arg(OTHER_RUN, &exported))
            .unwrap();
        assert_eq!(started["vehicle_available"], true);
        assert_eq!(export_of(&mut second), exported);
    }

    #[test]
    fn req_038_6_the_vehicle_ledger_accumulates_across_two_runs() {
        let mut live = Live::default();
        first_run(&mut live);
        let first = export_of(&mut live);
        let before = entry(&first, "engine.rpm").clone();
        live.act("coverage.live.end", "{}").unwrap();
        // The ended run keeps its vehicle ledger exportable until the next start.
        assert_eq!(export_of(&mut live), first);

        live.act("coverage.live.start", &start_arg(OTHER_RUN, &first))
            .unwrap();
        for second in [0, 2, 4] {
            feed(&mut live, &rpm(OTHER_RUN, second)).unwrap();
        }
        let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
        let in_run = entry(&snapshot["run"], "engine.rpm");
        let in_vehicle = entry(&snapshot["vehicle"], "engine.rpm");
        assert_eq!(in_run["qualifying_samples"], 3);
        assert_eq!(
            in_vehicle["qualifying_samples"].as_u64().unwrap(),
            before["qualifying_samples"].as_u64().unwrap() + 3
        );
        assert_eq!(
            in_vehicle["valid_time_us"].as_u64().unwrap(),
            before["valid_time_us"].as_u64().unwrap() + in_run["valid_time_us"].as_u64().unwrap()
        );
    }

    #[test]
    fn req_038_8_an_unreadable_ledger_starts_the_run_without_vehicle_and_keeps_counting() {
        for (stored, reason) in [
            (json!({"v":2,"entries":[],"identity":[]}), "unknown_version"),
            (json!({"v":1,"entries":"x","identity":[]}), "malformed"),
            (
                json!({"v":1,"entries":[{"signal_key":"a"}],"identity":[]}),
                "malformed",
            ),
            (json!("text"), "unreadable"),
            (json!({"entries":[],"identity":[]}), "unreadable"),
        ] {
            let mut live = Live::default();
            let started = live
                .act("coverage.live.start", &start_arg(RUN, &stored))
                .unwrap();
            assert_eq!(
                started,
                json!({"vehicle_available":false,"vehicle_error":reason}),
                "{stored}"
            );
            feed(&mut live, &rpm(RUN, 0)).unwrap();
            let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
            assert_eq!(snapshot["vehicle"], Value::Null);
            assert_eq!(snapshot["vehicle_available"], false);
            assert_eq!(snapshot["invalid"], false);
            assert_eq!(
                entry(&snapshot["run"], "engine.rpm")["qualifying_samples"],
                1
            );
            assert_eq!(export_of(&mut live), Value::Null);
        }
        let mut live = Live::default();
        let started = live
            .act("coverage.live.start", &start_arg(RUN, &Value::Null))
            .unwrap();
        assert_eq!(
            started,
            json!({"vehicle_available":false,"vehicle_error":null})
        );
    }

    #[test]
    fn req_038_8_the_summary_of_a_malformed_or_unknown_ledger_is_an_error_without_content() {
        let mut live = Live::default();
        for ledger in [
            json!({"v":1,"entries":[{"signal_key":"secret-value"}],"identity":[]}),
            json!({"v":7,"entries":[],"identity":[]}),
            json!(null),
        ] {
            let result = live.act(
                "coverage.ledger.summary",
                &json!({"ledger":ledger}).to_string(),
            );
            let error = result.unwrap_err();
            assert!(
                ["malformed", "unknown_version", "unreadable"].contains(&error.as_str()),
                "{error}"
            );
        }
        let stored = export(&Ledger::default());
        let summary = live
            .act(
                "coverage.ledger.summary",
                &json!({"ledger":stored}).to_string(),
            )
            .unwrap();
        assert_eq!(summary, json!({"contexts":[],"entries":[]}));
    }

    #[test]
    fn req_038_10_snapshot_counts_per_context_and_keeps_unconfirmed_facts_in_the_run_only() {
        let mut live = Live::default();
        live.act("coverage.live.start", &start_arg(RUN, &empty_vehicle()))
            .unwrap();
        // Unconfirmed identity: counted in the run ledger, never in the vehicle ledger.
        for second in 0..=40 {
            feed(&mut live, &read("engine.rpm", second)).unwrap();
        }
        let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
        assert_eq!(snapshot["active"], true);
        assert_eq!(snapshot["label"], "Technische Basisabdeckung");
        assert_eq!(snapshot["session_only"], true);
        assert_eq!(snapshot["invalid"], false);
        assert_eq!(
            snapshot["run"]["contexts"],
            json!([{"context":"baseline","covered":1,"partial":0,"not_covered":0}])
        );
        assert_eq!(
            entry(&snapshot["run"], "engine.rpm")["label"],
            BASELINE_LABEL
        );
        assert_eq!(snapshot["vehicle_available"], true);
        assert_eq!(snapshot["vehicle"]["entries"], json!([]));

        feed(&mut live, &rpm(RUN, 41)).unwrap();
        let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
        assert_eq!(snapshot["session_only"], false);
        assert_eq!(
            entry(&snapshot["vehicle"], "engine.rpm")["qualifying_samples"],
            1
        );
        assert_eq!(
            entry(&snapshot["vehicle"], "engine.rpm")["status"],
            "partial"
        );
    }

    #[test]
    fn req_038_10_a_detached_vehicle_ledger_is_no_longer_credited_or_exported() {
        let mut live = Live::default();
        live.act("coverage.live.start", &start_arg(RUN, &empty_vehicle()))
            .unwrap();
        feed(&mut live, &rpm(RUN, 0)).unwrap();
        live.act("coverage.live.detach_vehicle", "{}").unwrap();
        feed(&mut live, &rpm(RUN, 2)).unwrap();
        let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
        assert_eq!(snapshot["vehicle"], Value::Null);
        assert_eq!(snapshot["vehicle_available"], false);
        assert_eq!(
            entry(&snapshot["run"], "engine.rpm")["qualifying_samples"],
            2
        );
        assert_eq!(snapshot["session_only"], false);
        assert_eq!(export_of(&mut live), Value::Null);
    }

    #[test]
    fn req_038_10_run_records_are_accepted_but_not_counted_and_an_ended_run_is_inactive() {
        let mut live = Live::default();
        live.act("coverage.live.start", &start_arg(RUN, &Value::Null))
            .unwrap();
        let marker = feed(&mut live, &start(RUN, 0)).unwrap();
        assert_eq!(marker["counted"], false);
        live.act("coverage.live.end", "{}").unwrap();
        let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
        assert_eq!(snapshot["active"], false);
        assert!(feed(&mut live, &rpm(RUN, 0)).is_err());
        assert_eq!(
            live.act("coverage.live.snapshot", "{}").unwrap()["invalid"],
            false
        );
    }

    #[test]
    fn req_038_10_an_invalid_or_foreign_record_marks_the_ledger_invalid_and_hides_figures() {
        for bad in [
            rpm(OTHER_RUN, 0),
            json!({"type":"action","action":"coverage.fact","result":{"ok":true,"data":{}}}),
            json!({"action":"read"}),
        ] {
            let mut live = Live::default();
            live.act("coverage.live.start", &start_arg(RUN, &empty_vehicle()))
                .unwrap();
            feed(&mut live, &rpm(RUN, 0)).unwrap();
            assert!(feed(&mut live, &bad).is_err());
            let snapshot = live.act("coverage.live.snapshot", "{}").unwrap();
            assert_eq!(snapshot["invalid"], true);
            assert_eq!(snapshot["run"], Value::Null);
            assert_eq!(snapshot["vehicle"], Value::Null);
            assert_eq!(snapshot["vehicle_available"], true);
            // Once invalid, nothing more is folded.
            assert!(feed(&mut live, &rpm(RUN, 2)).is_err());
        }
    }

    #[test]
    fn req_038_10_a_record_going_back_in_time_is_invalid() {
        let mut live = Live::default();
        live.act("coverage.live.start", &start_arg(RUN, &Value::Null))
            .unwrap();
        feed(&mut live, &rpm(RUN, 10)).unwrap();
        assert!(feed(&mut live, &rpm(RUN, 2)).is_err());
        assert_eq!(
            live.act("coverage.live.snapshot", "{}").unwrap()["invalid"],
            true
        );
    }

    #[test]
    fn req_038_8_the_export_holds_no_value_or_raw_response() {
        let mut live = Live::default();
        first_run(&mut live);
        let text = export_of(&mut live).to_string();
        for forbidden in ["value\"", "vin", "raw", "response\""] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "{forbidden}: {text}"
            );
        }
    }
}
