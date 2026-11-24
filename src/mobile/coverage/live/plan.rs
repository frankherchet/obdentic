//! The Smart planner of a live run (REQ-039.1, REQ-039.8, REQ-039.9): `smart.plan.*` actions.
//!
//! The planner core ([`crate::smart_planner`]) is pure; this module is the only place that drives
//! it for the mobile core. The catalog is built from the core's own signal catalog, so the platform
//! can name no target: it only reports which standard PIDs the vehicle does not support. The plan
//! (selection, events, goal states) goes back as data; the platform selects the keys and records
//! the events. Nothing here talks to the adapter, reads a clock or shows a value: readings enter
//! through the classified `coverage.fact` data plus the decoded number, and stay in memory.
use std::time::Duration;

use serde_json::{json, Value};

use super::{parse_argument, Live};
use crate::coverage_ledger::{Decoded, Ledger, Outcome};
use crate::mobile::coverage::reading_of;
use crate::smart_planner::{
    CatalogEntry, ConditionInput, EventKind, Goal, Observation, Phase, Plan, PlanEvent, Planner,
    Reason, Source, Warm,
};

/// The planner of the running Smart run.
pub(in crate::mobile) struct PlanRun {
    planner: Planner,
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn reason_token(reason: Reason) -> &'static str {
    match reason {
        Reason::NoReading => "no_reading",
        Reason::Stale => "stale",
        Reason::Gap => "gap",
        Reason::Unsupported => "unsupported",
        Reason::Rejected => "rejected",
        Reason::Empty => "empty",
        Reason::Unanswered => "unanswered",
        Reason::ReadError => "read_error",
        Reason::SessionRestricted => "session_restricted",
        Reason::Implausible => "implausible",
        Reason::NonFinite => "non_finite",
        Reason::ConditionEnded => "condition_ended",
        Reason::WarmNotYetReached => "warm_not_yet_reached",
        Reason::Rearming => "rearming",
        Reason::HigherPriority => "higher_priority",
        Reason::NoSignalsInCatalog => "no_signals_in_catalog",
        Reason::AllSignalsUnavailable => "all_signals_unavailable",
        Reason::ConditionInputUnavailable => "condition_input_unavailable",
        Reason::UserStop => "user_stop",
    }
}

fn goal_json(goal: Option<Goal>) -> Value {
    goal.map_or(Value::Null, |goal| json!(goal.context_id()))
}

fn input_json(input: &ConditionInput) -> Value {
    match &input.value {
        Ok(value) => json!({"signal":input.signal_key,"value":value,"reason":null}),
        Err(reason) => {
            json!({"signal":input.signal_key,"value":null,"reason":reason_token(*reason)})
        }
    }
}

/// One plan change for the recording and the display (REQ-039.8): the goal, the reason and the
/// condition readings behind it, under the definition versions in force. `stop` replaces the
/// planner's reason of a stop, which only knows the user's.
fn event_json(event: &PlanEvent, stop: Option<&str>) -> Value {
    let mut data = json!({
        "at_us":micros(event.at),"goal":goal_json(event.goal),
        "reason":event.reason.map(reason_token),
        "inputs":event.inputs.iter().map(input_json).collect::<Vec<_>>(),
        "goal_definition":event.versions.goal_definition,
        "knowledge_revision":event.versions.knowledge_revision,
    });
    let kind = match &event.kind {
        EventKind::Started => "started",
        EventKind::Interrupted { by } => {
            data["by"] = json!(by.context_id());
            "interrupted"
        }
        EventKind::Resumed => "resumed",
        EventKind::Completed => "completed",
        EventKind::ContextEnded => "context_ended",
        EventKind::Expired => "expired",
        EventKind::NotReachable => "not_reachable",
        EventKind::Stopped => {
            if let Some(reason) = stop {
                data["reason"] = json!(reason);
            }
            "stopped"
        }
        EventKind::SignalSkipped {
            signal_key,
            retry_at,
        } => {
            data["signal"] = json!(signal_key);
            data["retry_in_ms"] = json!(retry_at.map(|at| millis(at.saturating_sub(event.at))));
            "skipped"
        }
    };
    data["kind"] = json!(kind);
    data
}

fn phase_json(phase: Phase) -> Value {
    let (name, by, reason) = match phase {
        Phase::Serving => ("serving", None, None),
        Phase::Interrupted { by } => ("interrupted", Some(by), None),
        Phase::AwaitingRetry => ("awaiting_retry", None, None),
        Phase::Complete => ("complete", None, None),
        Phase::NotReachable(reason) => ("not_reachable", None, Some(reason)),
        Phase::WaitingForWarm => ("waiting_for_warm", None, None),
        Phase::WaitingForCondition => ("waiting_for_condition", None, None),
        Phase::Rearming => ("rearming", None, None),
        Phase::ConditionUnknown(reason) => ("condition_unknown", None, Some(reason)),
        Phase::Stopped => ("stopped", None, None),
    };
    json!({"phase":name,"by":by.map(Goal::context_id),"reason":reason.map(reason_token)})
}

fn warm_json(warm: Option<Warm>) -> Value {
    match warm {
        None => Value::Null,
        Some(Warm::NotYetReached) => json!("not_yet_reached"),
        Some(Warm::Reached) => json!("reached"),
        Some(Warm::Unknown(reason)) => json!(format!("unknown:{}", reason_token(reason))),
    }
}

fn plan_json(plan: &Plan) -> Value {
    json!({
        "selection":plan.selection.iter().map(|selected| json!({
            "key":selected.signal_key,
            "contexts":selected.contexts.iter().map(|context| context.id()).collect::<Vec<_>>(),
            "trigger":selected.trigger,
        })).collect::<Vec<_>>(),
        "events":plan.events.iter().map(|event| event_json(event, None)).collect::<Vec<_>>(),
        "goals":plan.goals.iter().map(|status| {
            let mut goal = phase_json(status.phase);
            goal["goal"] = json!(status.goal.context_id());
            goal["share"] = json!(status.share);
            goal["warm"] = warm_json(status.warm);
            // REQ-039.21: hold and re-arm have different clocks; do not extrapolate
            // re-arm time between fresh condition readings.
            goal["contiguous_ms"] = json!(status.contiguous.map(millis));
            goal["contiguous_missing_ms"] = json!(status.contiguous.map(|contiguous| {
                millis(crate::smart_planner::EGR_CONTIGUOUS.saturating_sub(contiguous))
            }));
            goal["hold_remaining_ms"] = json!(status.hold_remaining.map(millis));
            goal["rearm_remaining_ms"] = json!(status.rearm_remaining.map(millis));
            goal
        }).collect::<Vec<_>>(),
        "session_targets":plan.session_targets.len(),
    })
}

/// The catalog of the planner, from the core's own catalog (REQ-039.3): standard PIDs, default
/// session Knowledge signals and, handed to the extended session slice only, session-flagged ones.
pub(in crate::mobile) fn catalog_of(catalog: &Value, unsupported: &[String]) -> Vec<CatalogEntry> {
    let mut entries: Vec<CatalogEntry> = Vec::new();
    let mut add = |entry: CatalogEntry| {
        if !entries
            .iter()
            .any(|existing| existing.signal_key == entry.signal_key)
        {
            entries.push(entry);
        }
    };
    let signals = catalog["signals"].as_array().into_iter().flatten();
    let candidates = catalog["session_candidates"]
        .as_array()
        .into_iter()
        .flatten();
    for signal in signals.chain(candidates) {
        let Some(semantic) = signal["semantic"].as_str() else {
            continue;
        };
        let session = signal["extended_session_required"] == true;
        match signal["key"].as_str() {
            Some(key) => add(CatalogEntry {
                signal_key: key.to_owned(),
                ecu_target: signal["target_id"].as_str().map(str::to_owned),
                source: if session {
                    Source::SessionFlagged
                } else {
                    Source::DefaultSession
                },
            }),
            None => add(CatalogEntry {
                signal_key: semantic.to_owned(),
                ecu_target: None,
                source: Source::StandardPid {
                    reported_supported: !unsupported.iter().any(|key| key == semantic),
                },
            }),
        }
    }
    entries
}

impl Live {
    /// Builds the planner of the active run over the core's catalog. A second start replaces the
    /// first.
    pub(in crate::mobile) fn plan_start(
        &mut self,
        catalog: &Value,
        argument: &str,
    ) -> Result<Value, String> {
        let argument = parse_argument(argument)?;
        let unsupported: Vec<String> = argument["unsupported"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|key| key.as_str().map(str::to_owned))
            .collect();
        let versions = match &self.run {
            Some(active) if !active.ended => active.versions.clone(),
            _ => return Err("no active coverage run".into()),
        };
        let entries = catalog_of(catalog, &unsupported);
        let count = entries.len();
        let planner =
            Planner::new(entries, versions.clone()).map_err(|error| format!("{error:?}"))?;
        self.planner = Some(PlanRun { planner });
        // REQ-039.10: the run finishes on the definition versions it started with.
        Ok(
            json!({"catalog":count,"goal_definition":versions.goal_definition,
            "knowledge_revision":versions.knowledge_revision}),
        )
    }

    pub(in crate::mobile) fn plan_act(
        &mut self,
        action: &str,
        argument: &str,
    ) -> Result<Value, String> {
        let argument = parse_argument(argument)?;
        let run = self.planner.as_mut().ok_or("no Smart planner is running")?;
        match action {
            "smart.plan.observe" => {
                let data = argument.get("fact").ok_or("fact is missing")?;
                let (at, reading) = reading_of(data)?;
                let value = argument.get("value").and_then(Value::as_f64);
                let key = reading.signal_key.clone();
                run.planner.record(&Observation {
                    signal_key: key.clone(),
                    at,
                    outcome: if argument["gap"] == true
                        && !matches!(
                            reading.outcome,
                            Outcome::Decoded(
                                Decoded::Numeric { finite: true }
                                    | Decoded::Text { nonempty: true }
                            )
                        ) {
                        Outcome::Gap
                    } else {
                        reading.outcome
                    },
                    value,
                    implausible: reading.implausible,
                });
                let contexts: Vec<String> = run
                    .planner
                    .contexts_at(&key, at)
                    .iter()
                    .map(|context| context.id().to_owned())
                    .collect();
                Ok(json!({"contexts":contexts}))
            }
            "smart.plan.step" => {
                let now =
                    Duration::from_micros(argument["now_us"].as_u64().ok_or("now_us is missing")?);
                // The vehicle ledger when one is attached (progress over drives), else the run's.
                let empty = Ledger::default();
                let ledger = self
                    .vehicle
                    .as_ref()
                    .or_else(|| self.run.as_ref().map(|active| active.smart.ledger()))
                    .unwrap_or(&empty);
                Ok(plan_json(&run.planner.plan(now, ledger)))
            }
            "smart.plan.stop" => {
                let now =
                    Duration::from_micros(argument["now_us"].as_u64().ok_or("now_us is missing")?);
                let reason = match argument["reason"].as_str() {
                    Some(
                        reason @ ("user" | "disconnect" | "recording_lost" | "error"
                        | "conflicting_work"),
                    ) => reason.to_owned(),
                    _ => return Err("unknown stop reason".into()),
                };
                let events = run.planner.stop(now, Reason::UserStop);
                self.planner = None;
                Ok(json!({"events":events.iter()
                    .map(|event| event_json(event, Some(&reason))).collect::<Vec<_>>()}))
            }
            _ => Err("unknown smart plan action".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures::*;
    use super::super::super::{derive_with_pin, GOAL_DEFINITION};
    use super::*;

    fn catalog() -> Value {
        let standard = |semantic: &str| json!({"semantic":semantic});
        json!({
            "signals":[
                standard("engine.coolant_temperature"),standard("vehicle.speed"),
                standard("engine.rpm"),standard("engine.load"),
                standard("vehicle.accelerator_pedal_e"),
                standard("engine.oil_temperature"),standard("engine.maf"),
                {"semantic":"cooling.fan1_duty","key":"cooling.fan1_duty@engine",
                    "target_id":"engine","extended_session_required":false},
            ],
            "session_candidates":[
                {"semantic":"fuel.pressure_regulator_duty",
                    "key":"fuel.pressure_regulator_duty@engine","target_id":"engine",
                    "extended_session_required":true},
            ],
        })
    }

    fn started(unsupported: &[&str]) -> Live {
        let mut live = Live::default();
        live.act(
            "coverage.live.start",
            &json!({"run":RUN,"goal_definition":GOAL_DEFINITION,"knowledge_revision":PIN})
                .to_string(),
        )
        .unwrap();
        live.plan_start(&catalog(), &json!({"unsupported":unsupported}).to_string())
            .unwrap();
        live
    }

    fn observe(live: &mut Live, signal: &str, seconds: u64, value: f64) -> Value {
        let fact = fact_with(
            RUN,
            signal,
            secs(seconds),
            json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
        );
        live.plan_act(
            "smart.plan.observe",
            &json!({"fact":fact["result"]["data"],"value":value}).to_string(),
        )
        .unwrap()
    }

    fn step(live: &mut Live, seconds: u64) -> Value {
        live.plan_act(
            "smart.plan.step",
            &json!({"now_us":secs(seconds)}).to_string(),
        )
        .unwrap()
    }

    fn keys(plan: &Value) -> Vec<&str> {
        plan["selection"]
            .as_array()
            .unwrap()
            .iter()
            .map(|selected| selected["key"].as_str().unwrap())
            .collect()
    }

    fn events<'a>(plan: &'a Value, kind: &str) -> Vec<&'a Value> {
        plan["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == kind)
            .collect()
    }

    // REQ-039.21: the action JSON consumed by Live carries both clocks, including nulls.
    #[test]
    fn req_039_21_plan_action_reports_turbo_hold_and_rearm_remaining() {
        let mut live = started(&[]);
        let pedal = "vehicle.accelerator_pedal_e";
        observe(&mut live, pedal, 1, 95.0);
        let initial = step(&mut live, 1);
        let turbo = |plan: Value| {
            plan["goals"]
                .as_array()
                .unwrap()
                .iter()
                .find(|goal| goal["goal"] == "turbo")
                .unwrap()
                .clone()
        };
        let armed = turbo(initial);
        assert_eq!(armed.get("hold_remaining_ms"), Some(&Value::Null));
        assert_eq!(armed.get("rearm_remaining_ms"), Some(&Value::Null));

        observe(&mut live, pedal, 2, 20.0);
        let released = turbo(step(&mut live, 2));
        assert_eq!(released.get("hold_remaining_ms"), Some(&json!(20_000)));
        assert_eq!(released.get("rearm_remaining_ms"), Some(&json!(10_000)));

        observe(&mut live, pedal, 7, 20.0);
        let waiting = turbo(step(&mut live, 7));
        assert_eq!(waiting.get("hold_remaining_ms"), Some(&json!(15_000)));
        assert_eq!(waiting.get("rearm_remaining_ms"), Some(&json!(5_000)));

        observe(&mut live, pedal, 12, 20.0);
        let rearmed = turbo(step(&mut live, 12));
        assert_eq!(rearmed.get("hold_remaining_ms"), Some(&json!(10_000)));
        assert_eq!(rearmed.get("rearm_remaining_ms"), Some(&Value::Null));
    }

    #[test]
    fn req_039_14_gap_probes_do_not_consume_retries() {
        let mut live = started(&[]);
        step(&mut live, 0); // drain the catalog's session-restricted notice
        let failed = |seconds| {
            fact_with(
                RUN,
                "engine.rpm",
                secs(seconds),
                json!({"outcome":"rejected","response_code":18}),
            )
        };
        let ordinary = failed(1);
        live.plan_act(
            "smart.plan.observe",
            &json!({"fact":ordinary["result"]["data"]}).to_string(),
        )
        .unwrap();
        assert_eq!(events(&step(&mut live, 1), "skipped").len(), 1);
        for seconds in [5, 35, 155, 160] {
            let probe = failed(seconds);
            live.plan_act(
                "smart.plan.observe",
                &json!({"fact":probe["result"]["data"],"gap":true}).to_string(),
            )
            .unwrap();
            assert!(
                events(&step(&mut live, seconds), "skipped").is_empty(),
                "gap probes must not advance retries"
            );
        }
        assert!(
            keys(&step(&mut live, 161)).contains(&"engine.rpm"),
            "the original retry remains available"
        );
    }

    #[test]
    fn req_039_3_the_catalog_is_the_cores_own_and_the_caller_only_marks_unsupported_pids() {
        let entries = catalog_of(&catalog(), &["engine.maf".to_owned()]);
        let by_key = |key: &str| {
            entries
                .iter()
                .find(|entry| entry.signal_key == key)
                .unwrap()
        };
        assert_eq!(
            by_key("engine.maf").source,
            Source::StandardPid {
                reported_supported: false
            }
        );
        assert_eq!(
            by_key("engine.rpm").source,
            Source::StandardPid {
                reported_supported: true
            }
        );
        assert_eq!(
            by_key("cooling.fan1_duty@engine").ecu_target.as_deref(),
            Some("engine")
        );
        assert_eq!(
            by_key("fuel.pressure_regulator_duty@engine").source,
            Source::SessionFlagged
        );
        let mut live = started(&[]);
        let plan = step(&mut live, 0);
        // A caller key that is not in the core's catalog has no effect and is never selected.
        assert!(!keys(&plan).contains(&"engine.fake"));
        assert!(!keys(&plan).contains(&"fuel.pressure_regulator_duty@engine"));
        assert_eq!(plan["session_targets"], 1);
    }

    #[test]
    fn req_039_9_the_caller_cannot_change_goals_thresholds_versions_or_targets() {
        let mut plain = started(&[]);
        let mut live = Live::default();
        live.act(
            "coverage.live.start",
            &json!({"run":RUN,"goal_definition":GOAL_DEFINITION,"knowledge_revision":PIN})
                .to_string(),
        )
        .unwrap();
        let started = live
            .plan_start(
                &catalog(),
                &json!({"unsupported":[],"warm":50,"thresholds":{"turbo":10},"goal_definition":99,
                    "knowledge_revision":"other","targets":["engine.fake@engine"],
                    "goals":["egr"]})
                .to_string(),
            )
            .unwrap();
        assert_eq!(started["goal_definition"], GOAL_DEFINITION);
        assert_eq!(started["knowledge_revision"], PIN);
        assert_eq!(started["catalog"], 9);
        let with_extras = step(&mut live, 0);
        assert!(!keys(&with_extras).contains(&"engine.fake@engine"));
        assert_eq!(with_extras, step(&mut plain, 0));
    }

    #[test]
    fn req_039_1_a_plan_needs_the_running_run_and_without_one_no_action_works() {
        let mut live = Live::default();
        assert_eq!(
            live.plan_start(&catalog(), "{}").unwrap_err(),
            "no active coverage run"
        );
        assert_eq!(
            live.plan_act("smart.plan.step", &json!({"now_us":0}).to_string())
                .unwrap_err(),
            "no Smart planner is running"
        );
        let mut live = started(&[]);
        live.act("coverage.live.end", "{}").unwrap();
        assert!(live
            .plan_act("smart.plan.step", &json!({"now_us":0}).to_string())
            .is_err());
    }

    #[test]
    fn req_039_8_plan_changes_carry_goal_reason_condition_readings_and_versions() {
        let mut live = started(&["engine.maf"]);
        let first = step(&mut live, 0);
        let skipped = events(&first, "skipped");
        let maf = skipped
            .iter()
            .find(|event| event["signal"] == "engine.maf")
            .expect("an unsupported PID is reported");
        assert_eq!(maf["reason"], "unsupported");
        assert_eq!(maf["retry_in_ms"], Value::Null);
        assert!(keys(&first).contains(&"engine.coolant_temperature"));
        observe(&mut live, "engine.coolant_temperature", 1, 90.0);
        observe(&mut live, "vehicle.speed", 1, 50.0);
        let second = step(&mut live, 2);
        let started = events(&second, "started");
        let cooling = started
            .iter()
            .find(|event| event["goal"] == "cooling.warm_driving")
            .expect("warm driving starts while coolant is 90 and the vehicle moves");
        assert_eq!(cooling["at_us"], secs(2));
        assert_eq!(cooling["goal_definition"], GOAL_DEFINITION);
        assert_eq!(cooling["knowledge_revision"], PIN);
        assert_eq!(
            cooling["inputs"],
            json!([
                {"signal":"engine.coolant_temperature","value":90.0,"reason":null},
                {"signal":"vehicle.speed","value":50.0,"reason":null},
            ])
        );
        let stopped = live
            .plan_act(
                "smart.plan.stop",
                &json!({"now_us":secs(3),"reason":"disconnect"}).to_string(),
            )
            .unwrap();
        let ends: Vec<&Value> = stopped["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "stopped")
            .collect();
        assert!(ends
            .iter()
            .any(|event| event["goal"] == "cooling.warm_driving"));
        assert!(ends.iter().all(|event| event["reason"] == "disconnect"));
        assert_eq!(
            live.plan_act("smart.plan.step", &json!({"now_us":0}).to_string())
                .unwrap_err(),
            "no Smart planner is running"
        );
    }

    #[test]
    fn req_039_8_an_unknown_stop_reason_is_refused_and_keeps_the_planner() {
        let mut live = started(&[]);
        let refused = live.plan_act(
            "smart.plan.stop",
            &json!({"now_us":0,"reason":"because"}).to_string(),
        );
        assert_eq!(refused.unwrap_err(), "unknown stop reason");
        step(&mut live, 0);
    }

    #[test]
    fn req_039_2_a_reading_counts_toward_every_context_whose_condition_holds() {
        let mut live = started(&[]);
        step(&mut live, 0);
        observe(&mut live, "engine.coolant_temperature", 1, 90.0);
        observe(&mut live, "vehicle.speed", 1, 50.0);
        step(&mut live, 2);
        let contexts = observe(&mut live, "engine.oil_temperature", 2, 95.0);
        assert_eq!(
            contexts["contexts"],
            json!(["cooling.warm_driving", "baseline"])
        );
        let other = observe(&mut live, "engine.maf", 2, 4.0);
        assert_eq!(other["contexts"], json!(["baseline"]));
    }

    #[test]
    fn req_039_12_a_context_end_record_folds_into_the_live_ledger_in_order() {
        let mut live = started(&[]);
        let feed =
            |live: &mut Live, record: &Value| live.act("coverage.live.record", &record.to_string());
        assert_eq!(
            feed(&mut live, &start(RUN, 0)).unwrap(),
            json!({"counted":false})
        );
        let mut egr = fact_with(
            RUN,
            "engine.maf",
            secs(2),
            json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
        );
        egr["result"]["data"]["context"] = json!("egr");
        assert_eq!(feed(&mut live, &egr).unwrap(), json!({"counted":true}));
        let end = record(
            "coverage.context_end",
            secs(4),
            json!({"run":RUN,"context":"egr","goal_definition":1,"knowledge_revision":PIN,
                "run_offset_us":secs(4)}),
        );
        assert_eq!(feed(&mut live, &end).unwrap(), json!({"counted":true}));
        // The clock never goes backwards: an earlier context end invalidates the live ledger.
        let early = record(
            "coverage.context_end",
            secs(1),
            json!({"run":RUN,"context":"egr","goal_definition":1,"knowledge_revision":PIN,
                "run_offset_us":secs(1)}),
        );
        assert!(feed(&mut live, &early).is_err());
    }

    #[test]
    fn req_039_8_goal_context_facts_and_context_ends_derive_offline_without_adapter_traffic() {
        let mut egr = fact_with(
            RUN,
            "engine.maf",
            secs(2),
            json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
        );
        egr["result"]["data"]["context"] = json!("egr");
        let mut records = vec![start(RUN, 0)];
        for (signal, seconds) in [("engine.speed", 0), ("engine.load", 0)] {
            records.push(read(signal, seconds));
        }
        records.push(read("engine.maf", 2));
        records.push(egr);
        records.push(record(
            "coverage.context_end",
            secs(4),
            json!({"run":RUN,"context":"egr","goal_definition":1,"knowledge_revision":PIN,
                "run_offset_us":secs(4)}),
        ));
        records.push(boundary(RUN, "run_end", secs(5)));
        let derived = derive_with_pin(&[part(records, Some("stopped"))], PIN);
        let run = &derived["runs"][0];
        assert_eq!(run["coverage_invalid"], false, "{run}");
        let contexts: Vec<&str> = run["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|context| context["context"].as_str().unwrap())
            .collect();
        assert!(contexts.contains(&"egr") && contexts.contains(&"baseline"));
    }
}
