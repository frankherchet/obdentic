//! Coverage facts in the recording and the offline re-derivation of the run ledger (REQ-038.9).
//!
//! A Smart run writes four action records into the analysis recording: `coverage.run` (`start`,
//! `resume`), `coverage.fact` (one read attempt, never a value; one per goal context it counts
//! toward), `coverage.context_end` (a goal context's contiguous intervals end, REQ-039.12) and
//! `coverage.boundary` (`gap`, `disconnect`, `reconnect`, `run_end`). A long run is a chain of
//! parts, one per rotated file.
//! [`derive`] is the only place that validates, orders and folds those records: the desktop report
//! and the Android replay both call it, so they cannot disagree. It only reads recorded facts
//! (never the raw reads next to them), takes time only from their monotonic offsets and folds them
//! through [`SmartRun::fold`] with [`SessionKind::Replay`], which can never credit a vehicle
//! ledger.
//!
//! A record that cannot be trusted is never skipped: a skipped boundary or failed read could let
//! two readings around it count as one continuous segment. It invalidates the run it may affect
//! (every run of the set when its run cannot be read).
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use serde_json::{json, Value};

use crate::coverage_ledger::{
    Boundary, Decoded, DefinitionVersions, EvidenceRef, Fact, FactBody, GoalContext, Latest,
    LatestState, Ledger, Outcome, Reading, Rules, RunId, SessionKind, SessionState, SmartRun,
    Status, BASELINE_CONTEXT, BASELINE_LABEL,
};
use crate::knowledge_db::KnowledgeCatalog;

pub(in crate::mobile) mod live;

/// The most parts one run may have; more invalidates it.
pub const MAX_PARTS: usize = 64;
/// Goal-definition version of the facts: the planner's goal catalogue (REQ-039.10).
pub const GOAL_DEFINITION: u32 = crate::smart_planner::GOAL_DEFINITION_VERSION;

/// The raw `coverage.*` records of one recording file in file order, never pre-filtered, and how
/// the file ended: the `session_end` reason, or `None` when it was interrupted or cut at an
/// unreadable line.
#[derive(Debug, Clone, Default)]
pub struct Part {
    pub records: Vec<Value>,
    pub ended: Option<String>,
}

/// Whether a recorded action belongs to the coverage facts.
pub fn is_coverage_action(action: &str) -> bool {
    action.starts_with("coverage.")
}

struct Invalid {
    /// The run the record names, when it can be read; `None` invalidates every run of the set.
    run: Option<String>,
    reason: String,
}

enum Item {
    Fact(Fact),
    Break {
        boundary: Boundary,
        offset: Duration,
        at: i64,
    },
}

enum Marker {
    Start,
    Resume { previous_part_records: u64 },
}

enum Parsed {
    Run {
        marker: Marker,
        part: u64,
        versions: DefinitionVersions,
    },
    Item(Item),
}

struct Record {
    run: String,
    offset: Duration,
    at: i64,
    parsed: Parsed,
}

/// One file's contribution to one run.
struct Piece {
    part: u64,
    marker: Marker,
    versions: DefinitionVersions,
    first_offset: Duration,
    first_at: i64,
    last_offset: Duration,
    items: Vec<Item>,
    run_end: bool,
    /// Number of `coverage.*` records of the whole file: what the next part must have counted.
    file_records: usize,
    ended: Option<String>,
}

fn token(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|text| {
        !text.is_empty()
            && text.len() <= 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

fn parse(record: &Value) -> Result<Record, Invalid> {
    let action = record["action"].as_str().unwrap_or("");
    if !matches!(
        action,
        "coverage.fact" | "coverage.boundary" | "coverage.run" | "coverage.context_end"
    ) {
        return Err(Invalid {
            run: None,
            reason: format!("{action}: unknown coverage action"),
        });
    }
    let hint = token(record["result"]["data"].get("run")).map(str::to_owned);
    let invalid = |reason: &str| Invalid {
        run: hint.clone(),
        reason: format!("{action}: {reason}"),
    };
    if record["type"] != "action" {
        return Err(invalid("record is not an action"));
    }
    if record["result"]["ok"] != true {
        return Err(invalid("ok is not true"));
    }
    let data = record["result"]["data"]
        .as_object()
        .ok_or_else(|| invalid("data is not an object"))?;
    let run = hint
        .clone()
        .ok_or_else(|| invalid("run is missing or not a token"))?;
    let offset = Duration::from_micros(
        data.get("run_offset_us")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("run_offset_us is missing"))?,
    );
    let at = record["at"]
        .as_i64()
        .ok_or_else(|| invalid("at is missing"))?;
    let versions = || -> Option<DefinitionVersions> {
        Some(DefinitionVersions {
            goal_definition: u32::try_from(data.get("goal_definition")?.as_u64()?).ok()?,
            knowledge_revision: text(data.get("knowledge_revision")?)?.to_owned(),
        })
    };
    let parsed = match action {
        "coverage.run" => {
            let part = data
                .get("part")
                .and_then(Value::as_u64)
                .filter(|part| *part >= 1)
                .ok_or_else(|| invalid("part is missing or not a positive integer"))?;
            let versions = versions().ok_or_else(|| invalid("definition versions are missing"))?;
            let marker = match data.get("action").and_then(Value::as_str) {
                Some("start") => {
                    if part != 1 {
                        return Err(invalid("start is not part 1"));
                    }
                    Marker::Start
                }
                Some("resume") => {
                    if part < 2 {
                        return Err(invalid("resume is lower than part 2"));
                    }
                    let previous_part_records = data
                        .get("previous_part_records")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| invalid("previous_part_records is missing"))?;
                    Marker::Resume {
                        previous_part_records,
                    }
                }
                _ => return Err(invalid("unknown run action")),
            };
            Parsed::Run {
                marker,
                part,
                versions,
            }
        }
        "coverage.boundary" => {
            let boundary = match data.get("class").and_then(Value::as_str) {
                Some("gap") => Boundary::Gap,
                Some("disconnect") => Boundary::Disconnect,
                Some("reconnect") => Boundary::Reconnect,
                Some("run_end") => Boundary::RunEnd,
                _ => return Err(invalid("unknown boundary class")),
            };
            Parsed::Item(Item::Break {
                boundary,
                offset,
                at,
            })
        }
        "coverage.context_end" => {
            let versions = versions().ok_or_else(|| invalid("definition versions are missing"))?;
            let context = data
                .get("context")
                .and_then(text)
                .ok_or_else(|| invalid("context is missing"))?;
            Parsed::Item(Item::Fact(Fact {
                run: RunId(run.clone()),
                wall_clock_unix_ms: at,
                offset,
                versions,
                identity_confirmed: false,
                body: FactBody::ContextEnd(GoalContext::new(context)),
            }))
        }
        _ => {
            let versions = versions().ok_or_else(|| invalid("definition versions are missing"))?;
            let signal_key = data
                .get("signal_key")
                .and_then(text)
                .ok_or_else(|| invalid("signal_key is missing"))?
                .to_owned();
            let context = data
                .get("context")
                .and_then(text)
                .ok_or_else(|| invalid("context is missing"))?
                .to_owned();
            let ecu_target = match data.get("ecu_target") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    text(value)
                        .ok_or_else(|| invalid("ecu_target is not text"))?
                        .to_owned(),
                ),
            };
            let session = match data.get("session").and_then(Value::as_str) {
                Some("default") => SessionState::Default,
                Some("extended") => SessionState::Extended,
                _ => return Err(invalid("unknown session state")),
            };
            let flag = |name: &str| data.get(name).and_then(Value::as_bool);
            let implausible =
                flag("implausible").ok_or_else(|| invalid("implausible is missing"))?;
            let static_identity =
                flag("static_identity").ok_or_else(|| invalid("static_identity is missing"))?;
            let identity_confirmed = flag("identity_confirmed")
                .ok_or_else(|| invalid("identity_confirmed is missing"))?;
            let reason = || {
                let reason = data
                    .get("reason")
                    .and_then(text)
                    .ok_or_else(|| invalid("reason is missing"))?;
                if REASONS.contains(&reason) {
                    Ok(reason.to_owned())
                } else {
                    Err(invalid("reason is not in the closed set"))
                }
            };
            let outcome = match data.get("outcome").and_then(Value::as_str) {
                Some("decoded") => match data.get("value_kind").and_then(Value::as_str) {
                    Some("numeric") => Outcome::Decoded(Decoded::Numeric {
                        finite: flag("finite").ok_or_else(|| invalid("finite is missing"))?,
                    }),
                    Some("text") => Outcome::Decoded(Decoded::Text {
                        nonempty: flag("nonempty").ok_or_else(|| invalid("nonempty is missing"))?,
                    }),
                    _ => return Err(invalid("unknown value kind")),
                },
                Some("unsupported") => Outcome::Unsupported,
                Some("rejected") => Outcome::Rejected {
                    response_code: data
                        .get("response_code")
                        .and_then(Value::as_u64)
                        .and_then(|code| u8::try_from(code).ok())
                        .ok_or_else(|| invalid("response_code is missing"))?,
                },
                Some("empty") => Outcome::Empty,
                Some("error") => Outcome::Error { reason: reason()? },
                Some("no_answer") => Outcome::NoAnswer { reason: reason()? },
                Some("session_restricted") => Outcome::SessionRestricted { reason: reason()? },
                _ => return Err(invalid("unknown outcome class")),
            };
            let evidence = data
                .get("evidence")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid("evidence reference is missing"))?;
            let evidence_at = evidence
                .get("at")
                .filter(|at| at.is_number() || at.as_str().is_some_and(|at| !at.is_empty()))
                .ok_or_else(|| invalid("evidence reference has no at"))?;
            let evidence_offset = evidence
                .get("offset_us")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("evidence reference has no offset_us"))?;
            let previous_file = match evidence.get("in_previous_file") {
                None => false,
                Some(Value::Bool(flag)) => *flag,
                Some(_) => return Err(invalid("in_previous_file is not a boolean")),
            };
            let evidence = EvidenceRef(format!(
                "{}@{evidence_offset}{}",
                evidence_at.to_string().trim_matches('"'),
                if previous_file { ":previous_file" } else { "" }
            ));
            Parsed::Item(Item::Fact(Fact {
                run: RunId(run.clone()),
                wall_clock_unix_ms: at,
                offset,
                versions: versions.clone(),
                identity_confirmed,
                body: FactBody::Reading(Reading {
                    signal_key,
                    context: GoalContext::new(context),
                    ecu_target,
                    session,
                    outcome,
                    implausible,
                    static_identity,
                    evidence,
                }),
            }))
        }
    };
    Ok(Record {
        run,
        offset,
        at,
        parsed,
    })
}

/// Validates the records of one file and groups them into one piece per run.
fn pieces_of(
    file: &Part,
    pieces: &mut BTreeMap<String, Vec<Piece>>,
    invalid: &mut Vec<Invalid>,
    known: &mut BTreeSet<String>,
) {
    let mut open: BTreeMap<String, Piece> = BTreeMap::new();
    let mut done: Vec<(String, Piece)> = Vec::new();
    for record in &file.records {
        if let Some(run) = token(record["result"]["data"].get("run")) {
            known.insert(run.to_owned());
        }
        let parsed = match parse(record) {
            Ok(parsed) => parsed,
            Err(error) => {
                invalid.push(error);
                continue;
            }
        };
        let fail = |reason: &str| Invalid {
            run: Some(parsed.run.clone()),
            reason: reason.to_owned(),
        };
        match parsed.parsed {
            Parsed::Run {
                marker,
                part,
                versions,
            } => {
                if let Some(existing) = open.get(&parsed.run) {
                    invalid.push(fail(match (&marker, &existing.marker) {
                        (Marker::Start, _) => "coverage.run: second start of the run in one part",
                        _ => "coverage.run: part numbers are not increasing within one file",
                    }));
                    continue;
                }
                if done.iter().any(|(run, _)| *run == parsed.run) {
                    invalid.push(fail("coverage.run: record after run_end"));
                    continue;
                }
                open.insert(
                    parsed.run.clone(),
                    Piece {
                        part,
                        marker,
                        versions,
                        first_offset: parsed.offset,
                        first_at: parsed.at,
                        last_offset: parsed.offset,
                        items: Vec::new(),
                        run_end: false,
                        file_records: file.records.len(),
                        ended: file.ended.clone(),
                    },
                );
            }
            Parsed::Item(item) => {
                let kind = match item {
                    Item::Fact(Fact {
                        body: FactBody::ContextEnd(_),
                        ..
                    }) => "coverage.context_end",
                    Item::Fact(_) => "coverage.fact",
                    Item::Break { .. } => "coverage.boundary",
                };
                let Some(piece) = open.get_mut(&parsed.run) else {
                    invalid.push(fail(&if done.iter().any(|(run, _)| *run == parsed.run) {
                        format!("{kind}: record after run_end")
                    } else {
                        format!("{kind}: no start or resume of the run before it in this part")
                    }));
                    continue;
                };
                if parsed.offset < piece.last_offset {
                    invalid.push(fail(&format!("{kind}: run_offset_us goes backwards")));
                    continue;
                }
                piece.last_offset = parsed.offset;
                let run_end = matches!(
                    item,
                    Item::Break {
                        boundary: Boundary::RunEnd,
                        ..
                    }
                );
                piece.items.push(item);
                if run_end {
                    piece.run_end = true;
                    if let Some(piece) = open.remove(&parsed.run) {
                        done.push((parsed.run.clone(), piece));
                    }
                }
            }
        }
    }
    for (run, piece) in done.into_iter().chain(open) {
        pieces.entry(run).or_default().push(piece);
    }
}

/// How the join of two consecutive supplied pieces of a run stands.
enum Join {
    Verified,
    /// A part between them is not supplied.
    Missing(Vec<u64>),
    /// The earlier part did not end `rotated`.
    Unverified,
}

fn derive_run(run: &str, mut pieces: Vec<Piece>) -> Result<Value, Vec<String>> {
    let mut issues = Vec::new();
    if pieces.len() > MAX_PARTS {
        return Err(vec!["coverage.run: too_many_parts".into()]);
    }
    pieces.sort_by_key(|piece| piece.part);
    // A run has at most MAX_PARTS parts, so no part number can exceed it. Checked before any join
    // is built: a gap between two supplied parts is never expanded into a list of missing parts
    // unless both numbers are in range (a part 1 next to a part u64::MAX must not allocate).
    if let Some(piece) = pieces.iter().find(|piece| piece.part > MAX_PARTS as u64) {
        return Err(vec![format!(
            "coverage.run: part number {} exceeds the limit of {MAX_PARTS} parts",
            piece.part
        )]);
    }
    for pair in pieces.windows(2) {
        if pair[0].part == pair[1].part {
            issues.push(format!(
                "coverage.run: two parts of the run have the same part number {}",
                pair[0].part
            ));
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let mut joins = Vec::new();
    for pair in pieces.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        if earlier.run_end {
            issues.push("coverage.run: record after run_end".into());
            continue;
        }
        if later.first_offset < earlier.last_offset {
            issues.push(format!(
                "coverage.run: run_offset_us goes backwards across the join of parts {} and {}",
                earlier.part, later.part
            ));
            continue;
        }
        let join = if later.part > earlier.part + 1 {
            Join::Missing((earlier.part + 1..later.part).collect())
        } else if earlier.ended.as_deref() != Some("rotated") {
            Join::Unverified
        } else {
            let Marker::Resume {
                previous_part_records,
            } = later.marker
            else {
                issues.push("coverage.run: a start follows an earlier part".into());
                continue;
            };
            if previous_part_records != earlier.file_records as u64 {
                issues.push(format!(
                    "coverage.run: previous_part_records of part {} does not match the records of part {}",
                    later.part, earlier.part
                ));
                continue;
            }
            Join::Verified
        };
        joins.push((earlier.part, later.part, join));
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    // One fact per read attempt, across every part of the run: its evidence reference names the raw read
    // it classifies, in its own file or (`in_previous_file`) in the part before it.
    let mut classified: BTreeSet<(&str, &str, u64, &str)> = BTreeSet::new();
    for piece in &pieces {
        for item in &piece.items {
            let Item::Fact(Fact {
                body: FactBody::Reading(reading),
                ..
            }) = item
            else {
                continue;
            };
            let (raw, owner) = match reading.evidence.0.strip_suffix(":previous_file") {
                Some(raw) => (raw, piece.part.saturating_sub(1)),
                None => (reading.evidence.0.as_str(), piece.part),
            };
            if !classified.insert((
                reading.signal_key.as_str(),
                reading.context.id(),
                owner,
                raw,
            )) {
                issues.push("coverage.fact: a second fact for the same read of a signal".into());
            }
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let mut facts = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 && !matches!(joins[index - 1].2, Join::Verified) {
            facts.push(Fact {
                run: RunId(run.to_owned()),
                wall_clock_unix_ms: piece.first_at,
                offset: piece.first_offset,
                versions: piece.versions.clone(),
                identity_confirmed: false,
                body: FactBody::Boundary(Boundary::Gap),
            });
        }
        for item in &piece.items {
            facts.push(match item {
                Item::Fact(fact) => fact.clone(),
                Item::Break {
                    boundary,
                    offset,
                    at,
                } => Fact {
                    run: RunId(run.to_owned()),
                    wall_clock_unix_ms: *at,
                    offset: *offset,
                    versions: piece.versions.clone(),
                    identity_confirmed: false,
                    body: FactBody::Boundary(*boundary),
                },
            });
        }
    }
    let rules = crate::smart_planner::goal_rules();
    let ledger = SmartRun::fold(
        rules.clone(),
        RunId(run.to_owned()),
        SessionKind::Replay,
        &facts,
    )
    .map_err(|error| vec![format!("coverage.run: {error}")])?;

    let missing_start = matches!(pieces[0].marker, Marker::Resume { .. });
    let missing_parts: Vec<u64> = joins
        .iter()
        .flat_map(|(_, _, join)| match join {
            Join::Missing(parts) => parts.clone(),
            _ => Vec::new(),
        })
        .collect();
    let unverified_join = joins
        .iter()
        .any(|(_, _, join)| matches!(join, Join::Unverified));
    let no_run_end = !pieces.last().is_some_and(|piece| piece.run_end);
    let mut reasons = Vec::new();
    for (reason, applies) in [
        ("missing_start", missing_start),
        ("missing_parts", !missing_parts.is_empty()),
        ("unverified_join", unverified_join),
        ("no_run_end", no_run_end),
    ] {
        if applies {
            reasons.push(reason);
        }
    }
    let complete = reasons.is_empty();

    let (entries, contexts) = summarize(ledger.ledger(), &rules);
    let parts: Vec<Value> = pieces
        .iter()
        .map(|piece| json!({"part":piece.part,"records":piece.file_records,"ended":piece.ended}))
        .collect();
    let joins: Vec<Value> = joins
        .iter()
        .map(|(from, to, join)| {
            json!({"from":from,"to":to,"verified":matches!(join, Join::Verified),
                "missing_parts":match join {Join::Missing(parts) => parts.clone(), _ => Vec::new()}})
        })
        .collect();
    Ok(
        json!({"run":run,"coverage_invalid":false,"run_complete":complete,
        "partial":!complete,"partial_reasons":reasons,"missing_parts":missing_parts,
        "figures":if complete {"exact"} else {"supplied_parts_only"},
        "parts":parts,"joins":joins,"label":BASELINE_LABEL,
        "contexts":contexts,"entries":entries,"issues":Vec::<String>::new(),
        "first_at":pieces[0].first_at}),
    )
}

/// The per-entry and per-context figures of a ledger, shared by the offline derivation and the live
/// ledger so that both count and name them the same way.
fn summarize(ledger: &Ledger, rules: &Rules) -> (Vec<Value>, Vec<Value>) {
    let mut entries = Vec::new();
    let mut contexts: BTreeMap<String, [u32; 3]> = BTreeMap::new();
    for (key, entry) in &ledger.entries {
        let status = ledger.status(rules, key);
        let counts = contexts.entry(key.context.id().to_owned()).or_default();
        counts[match status {
            Status::Covered => 0,
            Status::Partial => 1,
            Status::NotCovered => 2,
        }] += 1;
        entries.push(json!({
            "signal_key":key.signal_key,"context":key.context.id(),
            "label":rules.label(&key.context),
            "goal_definition":key.versions.goal_definition,
            "knowledge_revision":key.versions.knowledge_revision,
            "qualifying_samples":entry.qualifying_samples,
            "valid_time_us":duration_us(entry.valid_time),
            "valid_time_ms":entry.valid_time.as_millis() as u64,
            "longest_segment_us":duration_us(entry.longest_segment),
            "longest_segment_ms":entry.longest_segment.as_millis() as u64,
            "status":status_name(status),
            "latest":latest_json(entry.latest.as_ref()),
        }));
    }
    let contexts = contexts
        .into_iter()
        .map(|(context, counts)| {
            json!({"context":context,"covered":counts[0],"partial":counts[1],
            "not_covered":counts[2]})
        })
        .collect();
    (entries, contexts)
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Covered => "covered",
        Status::Partial => "partial",
        Status::NotCovered => "not_covered",
    }
}

fn latest_json(latest: Option<&Latest>) -> Value {
    let Some(latest) = latest else {
        return Value::Null;
    };
    let (state, code, reason) = match &latest.state {
        LatestState::NotYetObserved => ("not_yet_observed", None, None),
        LatestState::Observed => ("observed", None, None),
        LatestState::Unsupported => ("unsupported", None, None),
        LatestState::Rejected { response_code } => ("rejected", Some(*response_code), None),
        LatestState::Empty => ("empty", None, None),
        LatestState::Error { reason } => ("error", None, Some(reason.as_str())),
        LatestState::NoAnswer { reason } => ("no_answer", None, Some(reason.as_str())),
        LatestState::Gap => ("gap", None, None),
        LatestState::SessionRestricted { reason } => {
            ("session_restricted", None, Some(reason.as_str()))
        }
        LatestState::Implausible => ("implausible", None, None),
        LatestState::ContextNotFresh => ("context_not_fresh", None, None),
    };
    json!({"state":state,"response_code":code,"reason":reason,
        "at":latest.wall_clock_unix_ms,"offset_us":duration_us(latest.offset),
        "session":match latest.session {SessionState::Default => "default",
            SessionState::Extended => "extended"},
        "evidence":latest.evidence.0})
}

/// The Knowledge revision this build embeds, loaded once.
fn pin_revision() -> &'static str {
    static PIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PIN.get_or_init(|| {
        KnowledgeCatalog::embedded_pinned()
            .map(|catalog| catalog.pin().revision().to_owned())
            .unwrap_or_default()
    })
}

/// The closed set of reasons a fact may carry (REQ-038.9). A fact holds classes and references, never the
/// text of an error: that text can quote a response, a VIN, an ECU serial number or an adapter identifier.
const REASON_TIMEOUT: &str = "timeout";
const REASON_NO_DATA: &str = "no_data";
const REASON_SESSION: &str = "session_state";
const REASON_MALFORMED: &str = "malformed_response";
const REASON_FAILED: &str = "read_failed";
const REASON_NO_USABLE_ANSWER: &str = "no_usable_answer";
const REASON_UNDECODABLE_VALUE: &str = "undecodable_value";
/// Written by Android when the core could not classify a read; the fact still ends the signal's segments.
const REASON_CLASSIFICATION_FAILED: &str = "classification_failed";
/// Every reason a recorded fact may carry; replay rejects any other text, so a hand-made or foreign
/// recording cannot put free text into the ledger.
const REASONS: [&str; 8] = [
    REASON_TIMEOUT,
    REASON_NO_DATA,
    REASON_SESSION,
    REASON_MALFORMED,
    REASON_FAILED,
    REASON_NO_USABLE_ANSWER,
    REASON_UNDECODABLE_VALUE,
    REASON_CLASSIFICATION_FAILED,
];

/// The pure classification of one live `read` result (REQ-038.5) into the outcome fields of a
/// fact. The value of the read is never copied: only its class survives.
fn classify(read: &Value, catalog: Option<&KnowledgeCatalog>) -> serde_json::Map<String, Value> {
    let data = &read["data"];
    let mut fields = serde_json::Map::new();
    let mut set = |name: &str, value: Value| {
        fields.insert(name.to_owned(), value);
    };
    let mut implausible = false;
    let mut static_identity = false;
    let revision = data["knowledge_revision"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| pin_revision().to_owned());
    let definition = data["definition_id"]
        .as_str()
        .and_then(|id| catalog.and_then(|catalog| catalog.definition(id)));
    if let (Some(definition), Some(catalog)) = (definition, catalog) {
        static_identity = catalog
            .set(crate::knowledge_db::STANDARD_UDS_ECU_IDENTIFICATION_SET)
            .is_some_and(|set| {
                set.members()
                    .iter()
                    .any(|member| member == definition.semantic())
            });
        implausible = match (data["value"].as_f64(), definition.plausible_range()) {
            (Some(number), Some(range)) => number < range.minimum() || number > range.maximum(),
            _ => false,
        };
    }
    let decoded_or = |value: &Value| -> Option<(&'static str, bool)> {
        match value {
            Value::Number(number) => Some(("numeric", number.as_f64().is_some_and(f64::is_finite))),
            Value::String(text) => Some(("text", !text.is_empty())),
            _ => None,
        }
    };
    if read["ok"] == true {
        let status = data["status"].as_str();
        if matches!(status, None | Some("available")) {
            match &data["value"] {
                Value::Null => set("outcome", json!("empty")),
                Value::String(text) if text.is_empty() => set("outcome", json!("empty")),
                value => match decoded_or(value) {
                    Some((kind, usable)) => {
                        set("outcome", json!("decoded"));
                        set("value_kind", json!(kind));
                        set(
                            if kind == "numeric" {
                                "finite"
                            } else {
                                "nonempty"
                            },
                            json!(usable),
                        );
                    }
                    None => {
                        set("outcome", json!("error"));
                        set("reason", json!(REASON_UNDECODABLE_VALUE));
                    }
                },
            }
        } else if let Some(nrc) = data["nrc"].as_u64() {
            set("outcome", json!("rejected"));
            set("response_code", json!(nrc));
        } else {
            // The Knowledge decoder's text may quote the response; the fact keeps the class only.
            set("outcome", json!("no_answer"));
            set("reason", json!(REASON_NO_USABLE_ANSWER));
        }
    } else if let Some(nrc) = read["negative_response"]["nrc"]
        .as_u64()
        .filter(|nrc| u8::try_from(*nrc).is_ok())
    {
        // A standard negative response arrives as structure; its error text is never read for it.
        set("outcome", json!("rejected"));
        set("response_code", json!(nrc));
    } else {
        let lower = read["error"].as_str().unwrap_or("").to_lowercase();
        if lower.contains("does not advertise support") || lower.contains("rejected unsupported") {
            set("outcome", json!("unsupported"));
        } else if lower.contains("session") || lower.contains("not sent") {
            set("outcome", json!("session_restricted"));
            set("reason", json!(REASON_SESSION));
        } else if lower.contains("timeout") || lower.contains("timed out") {
            set("outcome", json!("no_answer"));
            set("reason", json!(REASON_TIMEOUT));
        } else if ["no data", "no response", "no answer"]
            .iter()
            .any(|needle| lower.contains(needle))
        {
            set("outcome", json!("no_answer"));
            set("reason", json!(REASON_NO_DATA));
        } else {
            set("outcome", json!("error"));
            let malformed = ["malformed", "unexpected", "conflicting", "truncated"]
                .iter()
                .any(|needle| lower.contains(needle));
            set(
                "reason",
                json!(if malformed {
                    REASON_MALFORMED
                } else {
                    REASON_FAILED
                }),
            );
        }
    }
    fields.insert("implausible".into(), json!(implausible));
    fields.insert("static_identity".into(), json!(static_identity));
    fields.insert("knowledge_revision".into(), json!(revision));
    fields.insert("ecu_target".into(), data["target_id"].clone());
    fields
}

/// The core action `coverage.fact`: the `data` of one `coverage.fact` record for one live read.
/// Android supplies what only it owns (run, run clock, session state, identity flag, the evidence
/// reference of the raw read record) next to the read result. Pure: no transport, no clock.
pub(super) fn fact_data(
    argument: &str,
    catalog: Option<&KnowledgeCatalog>,
) -> Result<Value, String> {
    let argument: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let run = token(argument.get("run")).ok_or("run is missing or not a token")?;
    let key = argument["key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or("key is missing")?;
    let offset = argument["run_offset_us"]
        .as_u64()
        .ok_or("run_offset_us is missing")?;
    let session = match argument["session"].as_str() {
        Some(session @ ("default" | "extended")) => session,
        _ => return Err("unknown session state".into()),
    };
    let confirmed = argument["identity_confirmed"]
        .as_bool()
        .ok_or("identity_confirmed is missing")?;
    let evidence = &argument["evidence"];
    if !(evidence["at"].is_number() && evidence["offset_us"].is_u64()) {
        return Err("evidence reference is missing".into());
    }
    let read = argument
        .get("read")
        .filter(|read| read.is_object())
        .ok_or("read result is missing")?;
    let embedded;
    let catalog = match catalog {
        Some(catalog) => Some(catalog),
        None if read["data"]["definition_id"].is_string() => {
            embedded = KnowledgeCatalog::embedded_pinned().map_err(|error| error.to_string())?;
            Some(&embedded)
        }
        None => None,
    };
    let mut data = classify(read, catalog);
    let mut put = |name: &str, value: Value| {
        data.insert(name.to_owned(), value);
    };
    put("run", json!(run));
    put("signal_key", json!(key));
    put("context", json!(BASELINE_CONTEXT));
    put("session", json!(session));
    put("identity_confirmed", json!(confirmed));
    put("goal_definition", json!(GOAL_DEFINITION));
    put("run_offset_us", json!(offset));
    let mut reference = json!({"at":evidence["at"],"offset_us":evidence["offset_us"]});
    if evidence["in_previous_file"] == true {
        reference["in_previous_file"] = json!(true);
    }
    put("evidence", reference);
    Ok(Value::Object(data))
}

/// The classified reading inside the `data` of a `coverage.fact` (as the core returns it) and its
/// run offset, read through the same [`parse`] the ledger folds with.
fn reading_of(data: &Value) -> Result<(Duration, Reading), String> {
    let record = json!({"type":"action","action":"coverage.fact","at":0,
        "result":{"ok":true,"data":data}});
    match parse(&record).map_err(|invalid| invalid.reason)?.parsed {
        Parsed::Item(Item::Fact(Fact {
            offset,
            body: FactBody::Reading(reading),
            ..
        })) => Ok((offset, reading)),
        _ => Err("not a classified reading".into()),
    }
}

/// The core action `coverage.run`: the `data` of one `coverage.run` record. Android supplies the run,
/// the part counter and the run clock; the definition versions are the core's own.
pub(super) fn run_data(argument: &str) -> Result<Value, String> {
    let argument: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let run = token(argument.get("run")).ok_or("run is missing or not a token")?;
    let offset = argument["run_offset_us"]
        .as_u64()
        .ok_or("run_offset_us is missing")?;
    let part = argument["part"]
        .as_u64()
        .filter(|part| *part >= 1)
        .ok_or("part is missing or not a positive integer")?;
    let mut data = json!({"run":run,"part":part,"goal_definition":GOAL_DEFINITION,
        "knowledge_revision":pin_revision(),"run_offset_us":offset});
    match (argument["action"].as_str(), part) {
        (Some("start"), 1) => data["action"] = json!("start"),
        (Some("resume"), 2..) => {
            data["action"] = json!("resume");
            data["previous_part_records"] = json!(argument["previous_part_records"]
                .as_u64()
                .ok_or("previous_part_records is missing")?);
        }
        _ => return Err("unknown run action for this part".into()),
    }
    Ok(data)
}

/// The core action `replay.coverage.part`: hands one file over as a part (its raw `coverage.*`
/// records in file order and how it ended). One more than [`MAX_PARTS`] is kept so that the
/// derivation can reject a run of too many parts; nothing beyond that is accepted.
pub(super) fn accept_part(parts: &mut Vec<Part>, argument: &str) -> Result<Value, String> {
    let argument: Value = serde_json::from_str(argument).map_err(|error| error.to_string())?;
    let records = argument["records"]
        .as_array()
        .ok_or("records is not a list")?;
    if records.len() > 200_000 {
        return Err("coverage part exceeds the record limit".into());
    }
    let ended = match &argument["ended"] {
        Value::Null => None,
        Value::String(reason) => Some(reason.clone()),
        _ => return Err("ended is neither null nor text".into()),
    };
    if parts.len() > MAX_PARTS {
        return Err("too many coverage parts handed over".into());
    }
    parts.push(Part {
        records: records.clone(),
        ended,
    });
    Ok(json!({"parts":parts.len()}))
}

/// The core action `replay.coverage`: derives the parts handed over so far and forgets them. A
/// Knowledge-pin mismatch stops the replay with the error `replay.read` gives.
pub(super) fn derive_parts(parts: Vec<Part>) -> Result<Value, String> {
    let derived = derive(&parts);
    if derived["knowledge_pin_mismatch"] == true {
        return Err(format!(
            "recording Knowledge revision {}",
            super::knowledge::PIN_MISMATCH
        ));
    }
    Ok(derived)
}

/// Derives the run ledgers of every Smart run named by the supplied parts, with the Knowledge
/// pin of this build. `Value::Null` when no part has a `coverage.*` record (REQ-038.9: older
/// recordings contribute nothing).
pub fn derive(parts: &[Part]) -> Value {
    derive_with_pin(parts, pin_revision())
}

pub fn derive_with_pin(parts: &[Part], pin: &str) -> Value {
    if parts.iter().all(|part| part.records.is_empty()) {
        return Value::Null;
    }
    let mut pieces: BTreeMap<String, Vec<Piece>> = BTreeMap::new();
    let mut invalid = Vec::new();
    let mut known = BTreeSet::new();
    for part in parts {
        pieces_of(part, &mut pieces, &mut invalid, &mut known);
    }
    let mismatch = parts
        .iter()
        .flat_map(|part| &part.records)
        .filter_map(|record| {
            record["result"]["data"]
                .get("knowledge_revision")
                .and_then(Value::as_str)
        })
        .any(|revision| revision != pin);
    if mismatch {
        return json!({"runs":Vec::<Value>::new(),"knowledge_pin_mismatch":true,
            "issues":[format!("coverage: recording Knowledge revision {}",
                super::knowledge::PIN_MISMATCH)],"parts":parts.len()});
    }
    let mut issues: Vec<String> = Vec::new();
    let mut by_run: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut global = false;
    for error in invalid {
        match error.run {
            Some(run) => by_run.entry(run).or_default().push(error.reason),
            None => {
                global = true;
                issues.push(error.reason);
            }
        }
    }
    let mut runs = Vec::new();
    let names: BTreeSet<String> = known.iter().chain(pieces.keys()).cloned().collect();
    for run in &names {
        let mut reasons = by_run.remove(run).unwrap_or_default();
        if global {
            reasons.push("coverage: a coverage record of the set cannot be read".into());
        }
        let derived = if reasons.is_empty() {
            match derive_run(run, pieces.remove(run).unwrap_or_default()) {
                Ok(derived) => Some(derived),
                Err(found) => {
                    reasons = found;
                    None
                }
            }
        } else {
            None
        };
        runs.push(
            derived.unwrap_or_else(|| json!({"run":run,"coverage_invalid":true,"issues":reasons})),
        );
    }
    runs.sort_by(|a, b| {
        (a["first_at"].as_i64(), a["run"].as_str())
            .cmp(&(b["first_at"].as_i64(), b["run"].as_str()))
    });
    for run in &mut runs {
        if let Some(object) = run.as_object_mut() {
            object.remove("first_at");
        }
    }
    json!({"runs":runs,"knowledge_pin_mismatch":false,"issues":issues,"parts":parts.len()})
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub(crate) const PIN: &str = "rev-a";
    pub(crate) const RUN: &str = "run-aaaa";

    pub(crate) fn at(offset_us: u64) -> i64 {
        1_700_000_000_000 + (offset_us / 1000) as i64
    }

    pub(crate) fn secs(seconds: u64) -> u64 {
        seconds * 1_000_000
    }

    pub(crate) fn record(action: &str, offset_us: u64, data: Value) -> Value {
        json!({"type":"action","action":action,"argument":"","at":at(offset_us),
            "offset_us":offset_us,"result":{"ok":true,"data":data}})
    }

    pub(crate) fn start(run: &str, offset_us: u64) -> Value {
        record(
            "coverage.run",
            offset_us,
            json!({"run":run,"action":"start","part":1,"goal_definition":1,
                "knowledge_revision":PIN,"run_offset_us":offset_us}),
        )
    }

    pub(crate) fn resume(run: &str, part: u64, previous: u64, offset_us: u64) -> Value {
        record(
            "coverage.run",
            offset_us,
            json!({"run":run,"action":"resume","part":part,"previous_part_records":previous,
                "goal_definition":1,"knowledge_revision":PIN,"run_offset_us":offset_us}),
        )
    }

    pub(crate) fn fact_with(run: &str, signal: &str, offset_us: u64, outcome: Value) -> Value {
        let mut data = json!({"run":run,"signal_key":signal,"context":"baseline",
            "ecu_target":null,"session":"default","implausible":false,"static_identity":false,
            "identity_confirmed":false,"goal_definition":1,"knowledge_revision":PIN,
            "run_offset_us":offset_us,
            "evidence":{"at":at(offset_us),"offset_us":offset_us}});
        for (name, value) in outcome.as_object().unwrap() {
            data[name] = value.clone();
        }
        record("coverage.fact", offset_us, data)
    }

    pub(crate) fn read(signal: &str, seconds: u64) -> Value {
        fact_with(
            RUN,
            signal,
            secs(seconds),
            json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
        )
    }

    pub(crate) fn failed_read(signal: &str, seconds: u64) -> Value {
        fact_with(
            RUN,
            signal,
            secs(seconds),
            json!({"outcome":"error","reason":"classification_failed"}),
        )
    }

    pub(crate) fn boundary(run: &str, class: &str, offset_us: u64) -> Value {
        record(
            "coverage.boundary",
            offset_us,
            json!({"run":run,"class":class,"run_offset_us":offset_us}),
        )
    }

    pub(crate) fn part(records: Vec<Value>, ended: Option<&str>) -> Part {
        Part {
            records,
            ended: ended.map(str::to_owned),
        }
    }

    pub(crate) fn reads(signal: &str, from: u64, to: u64) -> Vec<Value> {
        (from..=to).step_by(2).map(|s| read(signal, s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    /// The same facts: one file, or three rotated parts of a 60 s run.
    fn whole_run_in_one_file() -> Part {
        let mut records = vec![start(RUN, 0)];
        records.extend(reads("engine.rpm", 0, 60));
        records.push(boundary(RUN, "run_end", secs(60)));
        part(records, Some("disconnect"))
    }

    fn three_parts() -> Vec<Part> {
        let mut first = vec![start(RUN, 0)];
        first.extend(reads("engine.rpm", 0, 20));
        let first_count = first.len() as u64;
        let mut second = vec![resume(RUN, 2, first_count, secs(21))];
        second.extend(reads("engine.rpm", 22, 40));
        let second_count = second.len() as u64;
        let mut third = vec![resume(RUN, 3, second_count, secs(41))];
        third.extend(reads("engine.rpm", 42, 60));
        third.push(boundary(RUN, "run_end", secs(60)));
        vec![
            part(first, Some("rotated")),
            part(second, Some("rotated")),
            part(third, Some("disconnect")),
        ]
    }

    fn run_of(set: &Value) -> &Value {
        &set["runs"][0]
    }

    fn entry<'a>(run: &'a Value, signal: &str) -> &'a Value {
        run["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["signal_key"] == signal)
            .unwrap()
    }

    fn figures(run: &Value) -> Value {
        json!({"entries":run["entries"],"contexts":run["contexts"]})
    }

    #[test]
    fn req_038_9_whole_run_over_three_parts_equals_the_one_file_run_in_any_order() {
        let single = derive_with_pin(&[whole_run_in_one_file()], PIN);
        let expected = figures(run_of(&single));
        let engine = entry(run_of(&single), "engine.rpm");
        assert_eq!(engine["valid_time_ms"], 60_000);
        assert_eq!(engine["longest_segment_ms"], 60_000);
        assert_eq!(engine["status"], "covered");
        assert_eq!(run_of(&single)["run_complete"], true);
        for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0], [2, 1, 0]] {
            let parts = three_parts();
            let shuffled: Vec<Part> = order.iter().map(|&i| parts[i].clone()).collect();
            let set = derive_with_pin(&shuffled, PIN);
            let run = run_of(&set);
            assert_eq!(figures(run), expected, "order {order:?}");
            assert_eq!(run["run_complete"], true);
            assert_eq!(run["partial"], false);
            assert_eq!(run["figures"], "exact");
            assert_eq!(run["partial_reasons"], json!([]));
            assert_eq!(run["parts"].as_array().unwrap().len(), 3);
            assert_eq!(run["joins"][0]["verified"], true);
            assert_eq!(run["joins"][1]["verified"], true);
        }
        let single_run = run_of(&single);
        assert_eq!(single_run["figures"], "exact");
        assert_eq!(single_run["parts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn req_038_9_derivation_reads_facts_only_and_never_credits_a_vehicle_ledger() {
        let mut records = whole_run_in_one_file().records;
        records.insert(
            2,
            json!({"type":"action","action":"read","argument":"engine.rpm","at":1,
                "offset_us":1,"result":{"ok":true,"data":{"key":"engine.rpm","value":900}}}),
        );
        records.retain(|record| is_coverage_action(record["action"].as_str().unwrap()));
        let set = derive_with_pin(&[part(records, None)], PIN);
        assert_eq!(entry(run_of(&set), "engine.rpm")["qualifying_samples"], 31);
        // A fact that claims a confirmed identity still only fills the run ledger.
        assert_eq!(run_of(&set)["coverage_invalid"], false);
    }

    #[test]
    fn req_038_9_older_recordings_without_coverage_records_derive_nothing() {
        assert_eq!(derive_with_pin(&[part(Vec::new(), None)], PIN), Value::Null);
        assert_eq!(derive_with_pin(&[], PIN), Value::Null);
    }

    #[test]
    fn req_038_9_missing_middle_part_is_partial_and_bridges_nothing() {
        let parts = three_parts();
        let set = derive_with_pin(&[parts[0].clone(), parts[2].clone()], PIN);
        let run = run_of(&set);
        assert_eq!(run["partial"], true);
        assert_eq!(run["run_complete"], false);
        assert_eq!(run["figures"], "supplied_parts_only");
        assert_eq!(run["missing_parts"], json!([2]));
        assert_eq!(run["partial_reasons"], json!(["missing_parts"]));
        // 0..20 s and 42..60 s are two segments: the 22 s between 20 s and 42 s add nothing.
        let engine = entry(run, "engine.rpm");
        assert_eq!(engine["valid_time_ms"], 20_000 + 18_000);
        assert_eq!(engine["longest_segment_ms"], 20_000);
        assert_eq!(run["joins"][0]["verified"], false);
    }

    #[test]
    fn req_038_9_two_readings_two_seconds_apart_over_a_missing_part_add_no_time() {
        let mut first = vec![start(RUN, 0)];
        first.extend(reads("engine.rpm", 0, 20));
        let mut last = vec![resume(RUN, 3, 99, secs(21))];
        last.extend(reads("engine.rpm", 22, 40));
        last.push(boundary(RUN, "run_end", secs(40)));
        let set = derive_with_pin(
            &[part(first, Some("rotated")), part(last, Some("disconnect"))],
            PIN,
        );
        assert_eq!(
            entry(run_of(&set), "engine.rpm")["valid_time_ms"],
            20_000 + 18_000
        );
    }

    #[test]
    fn req_038_9_first_part_missing_is_partial_with_missing_start() {
        let parts = three_parts();
        let set = derive_with_pin(&[parts[1].clone(), parts[2].clone()], PIN);
        let run = run_of(&set);
        assert_eq!(run["partial"], true);
        assert_eq!(run["partial_reasons"], json!(["missing_start"]));
        assert_eq!(run["figures"], "supplied_parts_only");
        assert_eq!(run["joins"][0]["verified"], true);
        assert_eq!(entry(run, "engine.rpm")["valid_time_ms"], 38_000);
    }

    #[test]
    fn req_038_9_last_part_missing_or_without_run_end_is_partial() {
        let parts = three_parts();
        let set = derive_with_pin(&[parts[0].clone(), parts[1].clone()], PIN);
        assert_eq!(run_of(&set)["partial_reasons"], json!(["no_run_end"]));
        assert_eq!(entry(run_of(&set), "engine.rpm")["valid_time_ms"], 40_000);
        let set = derive_with_pin(&[parts[0].clone()], PIN);
        assert_eq!(run_of(&set)["partial"], true);
        assert_eq!(run_of(&set)["run_complete"], false);
        assert_eq!(run_of(&set)["figures"], "supplied_parts_only");
        // A rotated file reports a complete recording; the run is not complete by that.
        let mut interrupted = parts[2].clone();
        interrupted.records.pop();
        interrupted.ended = None;
        let set = derive_with_pin(&[parts[0].clone(), parts[1].clone(), interrupted], PIN);
        assert_eq!(run_of(&set)["partial_reasons"], json!(["no_run_end"]));
    }

    #[test]
    fn req_038_9_part_not_ended_rotated_before_its_next_part_is_an_unverified_join() {
        let mut parts = three_parts();
        parts[0].ended = None;
        let set = derive_with_pin(&parts, PIN);
        let run = run_of(&set);
        assert_eq!(run["partial"], true);
        assert_eq!(run["partial_reasons"], json!(["unverified_join"]));
        assert_eq!(run["joins"][0]["verified"], false);
        assert_eq!(run["joins"][1]["verified"], true);
        // The derived break at the first offset of part 2: 20 s -> 22 s is not bridged.
        let engine = entry(run, "engine.rpm");
        assert_eq!(engine["valid_time_ms"], 58_000);
        assert_eq!(engine["longest_segment_ms"], 38_000);
    }

    #[test]
    fn req_038_9_a_run_that_fits_one_file_is_complete_from_that_file_alone() {
        let set = derive_with_pin(&[whole_run_in_one_file()], PIN);
        assert_eq!(run_of(&set)["run_complete"], true);
        assert_eq!(run_of(&set)["partial"], false);
    }

    #[test]
    fn req_038_9_failure_fact_breaks_the_segment_and_adds_no_time() {
        let mut records = vec![start(RUN, 0), read("engine.rpm", 0), read("engine.rpm", 5)];
        records.push(failed_read("engine.rpm", 7));
        records.extend([read("engine.rpm", 9), read("engine.rpm", 14)]);
        records.push(boundary(RUN, "run_end", secs(14)));
        let set = derive_with_pin(&[part(records, None)], PIN);
        let engine = entry(run_of(&set), "engine.rpm");
        // 0->5 and 9->14 count; the failure at 7 s lies inside 5->9 and ends both.
        assert_eq!(engine["valid_time_ms"], 10_000);
        assert_eq!(engine["longest_segment_ms"], 5_000);
        assert_eq!(engine["latest"]["state"], "observed");
    }

    #[test]
    fn req_038_9_records_hold_references_and_classes_never_values() {
        let set = derive_with_pin(&[whole_run_in_one_file()], PIN);
        let text = set.to_string();
        assert!(!text.contains("\"value\""));
        assert!(entry(run_of(&set), "engine.rpm")["latest"]["evidence"]
            .as_str()
            .unwrap()
            .contains('@'));
    }

    // Fail-closed negatives. A skipped record would credit 5 s between readings at 10 s and 15 s.

    fn around(middle: Value) -> Part {
        part(
            vec![
                start(RUN, 0),
                read("engine.rpm", 10),
                middle,
                read("engine.rpm", 15),
                boundary(RUN, "run_end", secs(15)),
            ],
            Some("disconnect"),
        )
    }

    fn assert_invalid(set: &Value, run: &str, fragment: &str) {
        let entry = set["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["run"] == run)
            .unwrap_or_else(|| panic!("run {run} missing in {set}"));
        assert_eq!(entry["coverage_invalid"], true, "{set}");
        assert!(
            entry["issues"].to_string().contains(fragment),
            "{} should contain {fragment}",
            entry["issues"]
        );
        for field in ["entries", "contexts", "run_complete", "partial", "figures"] {
            assert!(entry.get(field).is_none(), "{field} must not be reported");
        }
    }

    fn bad_boundary(change: impl Fn(&mut Value)) -> Part {
        let mut boundary = boundary(RUN, "gap", secs(12));
        change(&mut boundary);
        around(boundary)
    }

    #[test]
    fn req_038_9_untrusted_boundary_or_failed_read_invalidates_its_run_and_credits_nothing() {
        let cases: Vec<(&str, Part, &str)> = vec![
            (
                "unknown boundary class",
                bad_boundary(|r| r["result"]["data"]["class"] = json!("pause")),
                "unknown boundary class",
            ),
            (
                "ok false",
                bad_boundary(|r| r["result"]["ok"] = json!(false)),
                "ok is not true",
            ),
            (
                "run_offset_us missing",
                bad_boundary(|r| {
                    r["result"]["data"]
                        .as_object_mut()
                        .unwrap()
                        .remove("run_offset_us");
                }),
                "run_offset_us is missing",
            ),
            (
                "run_offset_us not numeric",
                bad_boundary(|r| r["result"]["data"]["run_offset_us"] = json!("12")),
                "run_offset_us is missing",
            ),
            (
                "backwards boundary",
                bad_boundary(|r| r["result"]["data"]["run_offset_us"] = json!(secs(3))),
                "goes backwards",
            ),
            (
                "failed read with unknown outcome class",
                around(fact_with(
                    RUN,
                    "engine.rpm",
                    secs(12),
                    json!({"outcome":"exploded"}),
                )),
                "unknown outcome class",
            ),
            (
                "failed read with unknown session state",
                around(fact_with(
                    RUN,
                    "engine.rpm",
                    secs(12),
                    json!({"outcome":"error","reason":"read_failed","session":"programming"}),
                )),
                "unknown session state",
            ),
            (
                "failed read without reason",
                around(fact_with(
                    RUN,
                    "engine.rpm",
                    secs(12),
                    json!({"outcome":"error"}),
                )),
                "reason is missing",
            ),
            (
                "failed read backwards",
                around(fact_with(
                    RUN,
                    "engine.rpm",
                    secs(2),
                    json!({"outcome":"error","reason":"read_failed"}),
                )),
                "goes backwards",
            ),
            (
                "fact without evidence",
                {
                    let mut fact = fact_with(
                        RUN,
                        "engine.rpm",
                        secs(12),
                        json!({"outcome":"error","reason":"read_failed"}),
                    );
                    fact["result"]["data"]
                        .as_object_mut()
                        .unwrap()
                        .remove("evidence");
                    around(fact)
                },
                "evidence reference is missing",
            ),
        ];
        for (name, part, fragment) in cases {
            let set = derive_with_pin(&[part], PIN);
            assert_invalid(&set, RUN, fragment);
            assert!(!set.to_string().contains("\"valid_time_ms\""), "{name}");
        }
    }

    #[test]
    fn req_038_9_unreadable_record_invalidates_every_run_of_the_set() {
        let other = "run-bbbb";
        let mut records = vec![start(other, 0)];
        records.extend((0..=10).step_by(2).map(|s| {
            fact_with(
                other,
                "vehicle.speed",
                secs(s),
                json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
            )
        }));
        let unrelated = part(records, None);
        let mut broken = boundary(RUN, "gap", secs(12));
        for change in [
            |record: &mut Value| record["result"]["data"] = json!("not an object"),
            |record: &mut Value| {
                record["result"]["data"]
                    .as_object_mut()
                    .unwrap()
                    .remove("run");
            },
            |record: &mut Value| record["result"]["data"]["run"] = json!("not a token!"),
            |record: &mut Value| record["action"] = json!("coverage.pause"),
        ] {
            change(&mut broken);
            let set = derive_with_pin(&[around(broken.clone()), unrelated.clone()], PIN);
            for run in [RUN, other] {
                assert_invalid(&set, run, "");
            }
            broken = boundary(RUN, "gap", secs(12));
        }
    }

    #[test]
    fn req_038_9_a_readable_invalid_run_leaves_an_unrelated_run_unaffected() {
        let other = "run-bbbb";
        let mut records = vec![start(other, 0)];
        records.extend((0..=40).step_by(2).map(|s| {
            fact_with(
                other,
                "vehicle.speed",
                secs(s),
                json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
            )
        }));
        records.push(boundary(other, "run_end", secs(40)));
        let set = derive_with_pin(
            &[
                bad_boundary(|r| r["result"]["data"]["class"] = json!("pause")),
                part(records, Some("disconnect")),
            ],
            PIN,
        );
        assert_invalid(&set, RUN, "unknown boundary class");
        let unrelated = set["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["run"] == other)
            .unwrap();
        assert_eq!(unrelated["coverage_invalid"], false);
        assert_eq!(unrelated["run_complete"], true);
        assert_eq!(entry(unrelated, "vehicle.speed")["valid_time_ms"], 40_000);
    }

    #[test]
    fn req_038_9_a_fact_repeated_in_the_next_part_invalidates_the_run() {
        // The same raw read classified in part 1 and again in part 2 through `in_previous_file`.
        let reference = |signal: &str, previous: bool, offset: u64| {
            let mut fact = fact_with(
                RUN,
                signal,
                secs(3),
                json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
            );
            fact["result"]["data"]["evidence"] = if previous {
                json!({"at":at(secs(2)),"offset_us":offset,"in_previous_file":true})
            } else {
                json!({"at":at(secs(2)),"offset_us":offset})
            };
            fact
        };
        let parts = |second: Value| {
            vec![
                part(vec![start(RUN, 0), read("engine.rpm", 2)], Some("rotated")),
                part(
                    vec![
                        resume(RUN, 2, 2, secs(3)),
                        second,
                        boundary(RUN, "run_end", secs(4)),
                    ],
                    None,
                ),
            ]
        };
        let set = derive_with_pin(&parts(reference("engine.rpm", true, secs(2))), PIN);
        assert_invalid(&set, RUN, "second fact for the same read");
        // another signal, or a read of the second file that merely shares the numbers, is no duplicate
        for second in [
            reference("vehicle.speed", true, secs(2)),
            reference("engine.rpm", false, secs(2)),
        ] {
            let set = derive_with_pin(&parts(second), PIN);
            assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
        }
    }

    #[test]
    fn req_038_9_a_recorded_reason_outside_the_closed_set_invalidates_the_run() {
        for reason in ["link failure for WVWZZZ1JZXW000001", "TIMEOUT", ""] {
            let set = derive_with_pin(
                &[part(
                    vec![
                        start(RUN, 0),
                        fact_with(
                            RUN,
                            "engine.rpm",
                            secs(2),
                            json!({"outcome":"error","reason":reason}),
                        ),
                        boundary(RUN, "run_end", secs(4)),
                    ],
                    None,
                )],
                PIN,
            );
            assert_invalid(&set, RUN, "reason");
            assert!(!set.to_string().contains("WVWZZZ"), "{set}");
        }
        // the fallback Android writes for an unclassifiable read is part of the closed set
        let set = derive_with_pin(
            &[part(
                vec![
                    start(RUN, 0),
                    failed_read("engine.rpm", 2),
                    boundary(RUN, "run_end", secs(4)),
                ],
                None,
            )],
            PIN,
        );
        assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
    }

    #[test]
    fn req_038_9_a_second_fact_for_one_read_invalidates_the_run() {
        // #339: one fact per read attempt; a repeated record must not count as a second observation.
        let set = derive_with_pin(
            &[part(
                vec![
                    start(RUN, 0),
                    read("engine.rpm", 2),
                    read("engine.rpm", 2),
                    boundary(RUN, "run_end", secs(4)),
                ],
                None,
            )],
            PIN,
        );
        assert_invalid(&set, RUN, "second fact for the same read");
        // another signal at the same time, or the same signal at another time, is no duplicate
        let set = derive_with_pin(
            &[part(
                vec![
                    start(RUN, 0),
                    read("engine.rpm", 2),
                    read("vehicle.speed", 2),
                    read("engine.rpm", 3),
                    boundary(RUN, "run_end", secs(4)),
                ],
                None,
            )],
            PIN,
        );
        assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
    }

    #[test]
    fn req_038_9_structural_violations_invalidate_the_run() {
        let parts = three_parts();
        let with_part = |part_no: Value| {
            let mut second = parts[1].clone();
            second.records[0]["result"]["data"]["part"] = part_no;
            vec![parts[0].clone(), second, parts[2].clone()]
        };
        let mut cases: Vec<(String, Vec<Part>, &str)> = Vec::new();
        for value in [json!(0), json!(-1), json!("2"), json!(null)] {
            cases.push((
                format!("part {value}"),
                with_part(value),
                "part is missing or not a positive integer",
            ));
        }
        let mut count_low = parts.clone();
        count_low[1].records[0]["result"]["data"]["previous_part_records"] = json!(11);
        cases.push(("count one lower".into(), count_low, "previous_part_records"));
        let mut count_high = parts.clone();
        count_high[1].records[0]["result"]["data"]["previous_part_records"] = json!(13);
        cases.push((
            "count one higher".into(),
            count_high,
            "previous_part_records",
        ));
        let mut missing_count = parts.clone();
        missing_count[1].records[0]["result"]["data"]
            .as_object_mut()
            .unwrap()
            .remove("previous_part_records");
        cases.push((
            "count missing".into(),
            missing_count,
            "previous_part_records",
        ));
        cases.push((
            "duplicate part".into(),
            vec![
                parts[0].clone(),
                parts[1].clone(),
                parts[1].clone(),
                parts[2].clone(),
            ],
            "same part number",
        ));
        cases.push((
            "same file twice".into(),
            vec![parts[0].clone(), parts[0].clone()],
            "same part number",
        ));
        let mut start_part_two = parts.clone();
        start_part_two[0].records[0]["result"]["data"]["part"] = json!(2);
        cases.push((
            "start is part 2".into(),
            start_part_two,
            "start is not part 1",
        ));
        let mut resume_part_one = parts.clone();
        resume_part_one[1].records[0]["result"]["data"]["part"] = json!(1);
        cases.push((
            "resume is part 1".into(),
            resume_part_one,
            "lower than part 2",
        ));
        let mut backwards = parts.clone();
        backwards[1].records[0]["result"]["data"]["run_offset_us"] = json!(secs(5));
        cases.push((
            "offsets backwards across the join".into(),
            backwards,
            "goes backwards",
        ));
        let mut after_end = parts.clone();
        let end = boundary(RUN, "run_end", secs(20));
        after_end[0].records.push(end);
        // The count of part 1 changed too; the run_end rule must win.
        cases.push(("resume after run_end".into(), after_end, "after run_end"));
        let mut orphan = parts.clone();
        orphan[0].records.remove(0);
        cases.push(("orphan fact".into(), orphan, "no start or resume"));
        let mut second_start = parts.clone();
        second_start[0].records.insert(1, start(RUN, 0));
        cases.push(("second start".into(), second_start, "second start"));
        let mut twice_in_one_file = parts.clone();
        twice_in_one_file[1]
            .records
            .insert(1, resume(RUN, 3, 5, secs(21)));
        cases.push((
            "resume twice in one file".into(),
            twice_in_one_file,
            "not increasing",
        ));
        let mut many = Vec::new();
        for number in 1..=65u64 {
            let mut records = if number == 1 {
                vec![start(RUN, 0)]
            } else {
                vec![resume(RUN, number, 1, secs(number))]
            };
            records.push(read("engine.rpm", number));
            many.push(part(records, Some("rotated")));
        }
        cases.push(("65 parts".into(), many, "too_many_parts"));
        for (name, parts, fragment) in cases {
            let set = derive_with_pin(&parts, PIN);
            assert_invalid(&set, RUN, fragment);
            assert!(!set.to_string().contains("\"valid_time_ms\""), "{name}");
        }
    }

    #[test]
    fn req_038_9_sixty_four_parts_are_still_derived() {
        let mut many = Vec::new();
        for number in 1..=64u64 {
            let mut records = if number == 1 {
                vec![start(RUN, 0)]
            } else {
                vec![resume(RUN, number, 2, secs(number * 2))]
            };
            records.push(read("engine.rpm", number * 2));
            many.push(part(records, Some("rotated")));
        }
        let set = derive_with_pin(&many, PIN);
        assert_eq!(run_of(&set)["coverage_invalid"], false);
        assert_eq!(run_of(&set)["partial_reasons"], json!(["no_run_end"]));
    }

    // The review of #344: a part number far beyond the 64-part limit must invalidate the run before
    // any list of "missing parts" is built (a gap of u64::MAX would exhaust memory).
    #[test]
    fn req_038_9_a_huge_part_number_gap_invalidates_the_run_without_building_the_missing_parts() {
        for number in [65, 1 << 40, u64::MAX - 1, u64::MAX] {
            let parts = vec![
                part(vec![start(RUN, 0), read("engine.rpm", 1)], Some("rotated")),
                part(
                    vec![resume(RUN, number, 2, secs(2)), read("engine.rpm", 3)],
                    Some("rotated"),
                ),
            ];
            let set = derive_with_pin(&parts, PIN);
            assert_invalid(&set, RUN, "exceeds the limit of 64 parts");
            let reversed = vec![parts[1].clone(), parts[0].clone()];
            assert_invalid(&derive_with_pin(&reversed, PIN), RUN, "exceeds the limit");
        }
        // The limit itself is still a valid number: part 1 and part 64 leave 62 parts missing.
        let parts = vec![
            part(vec![start(RUN, 0), read("engine.rpm", 1)], Some("rotated")),
            part(
                vec![resume(RUN, 64, 2, secs(2)), read("engine.rpm", 3)],
                Some("rotated"),
            ),
        ];
        let set = derive_with_pin(&parts, PIN);
        assert_eq!(run_of(&set)["coverage_invalid"], false);
        assert_eq!(run_of(&set)["missing_parts"].as_array().unwrap().len(), 62);
    }

    #[test]
    fn req_038_9_knowledge_pin_mismatch_in_any_part_derives_no_coverage() {
        let mut parts = three_parts();
        parts[2].records[1]["result"]["data"]["knowledge_revision"] = json!("rev-other");
        let set = derive_with_pin(&parts, PIN);
        assert_eq!(set["knowledge_pin_mismatch"], true);
        assert_eq!(set["runs"], json!([]));
        assert!(set["issues"][0]
            .as_str()
            .unwrap()
            .ends_with(super::super::knowledge::PIN_MISMATCH));
    }

    // Classification of one live read (REQ-038.5, REQ-038.9).

    struct Offline;
    impl crate::mobile::Exchange for Offline {
        fn exchange(&mut self, _: &str, _: Duration) -> Result<String, String> {
            Err("no adapter in a classification test".into())
        }
    }

    fn classify_read(read: Value, catalog: Option<&KnowledgeCatalog>) -> Value {
        fact_data(
            &json!({"run":RUN,"key":"engine.rpm","run_offset_us":secs(3),"session":"default",
                "identity_confirmed":true,"evidence":{"at":at(secs(3)),"offset_us":777},
                "read":read})
            .to_string(),
            catalog,
        )
        .unwrap()
    }

    #[test]
    fn req_038_9_classification_maps_every_read_outcome_without_copying_the_value() {
        let table = [
            (
                json!({"ok":true,"data":{"key":"engine.rpm","value":812.5}}),
                json!({"outcome":"decoded","value_kind":"numeric","finite":true}),
            ),
            (
                json!({"ok":true,"data":{"key":"x","value":"WVWZZZ1JZXW000001"}}),
                json!({"outcome":"decoded","value_kind":"text","nonempty":true}),
            ),
            (
                json!({"ok":true,"data":{"key":"x","value":""}}),
                json!({"outcome":"empty"}),
            ),
            (
                json!({"ok":true,"data":{"key":"x","status":"unavailable","nrc":49,
                    "error":"negative response"}}),
                json!({"outcome":"rejected","response_code":49}),
            ),
            (
                json!({"ok":true,"data":{"key":"x","status":"unavailable","nrc":null,
                    "error":"Knowledge DID unavailable"}}),
                json!({"outcome":"no_answer","reason":"no_usable_answer"}),
            ),
            (
                json!({"ok":false,"error":"vehicle does not advertise support for engine.rpm"}),
                json!({"outcome":"unsupported"}),
            ),
            (
                json!({"ok":false,"error":"Knowledge DID is not enabled for this ECU session"}),
                json!({"outcome":"session_restricted","reason":"session_state"}),
            ),
            (
                json!({"ok":false,"error":"adapter timeout after 2s"}),
                json!({"outcome":"no_answer","reason":"timeout"}),
            ),
            (
                json!({"ok":false,"error":"ELM327 rejected 010C: NO DATA"}),
                json!({"outcome":"no_answer","reason":"no_data"}),
            ),
            (
                json!({"ok":false,"error":"malformed ELM327 response line: \"7E8 XY\""}),
                json!({"outcome":"error","reason":"malformed_response"}),
            ),
            (
                json!({"ok":false,"error":"link failure"}),
                json!({"outcome":"error","reason":"read_failed"}),
            ),
            (
                json!({"ok":true,"data":{"key":"x","value":{"nested":1}}}),
                json!({"outcome":"error","reason":"undecodable_value"}),
            ),
            (
                json!({"ok":false,"error":"negative OBD-II response: 7E8 03 7F 01 12",
                    "negative_response":{"service":1,"nrc":18}}),
                json!({"outcome":"rejected","response_code":18}),
            ),
        ];
        for (read, expected) in table {
            let fact = classify_read(read.clone(), None);
            for (name, value) in expected.as_object().unwrap() {
                assert_eq!(&fact[name], value, "{read}");
            }
            assert!(!fact.to_string().contains("812"), "no value in {fact}");
            assert!(!fact.to_string().contains("WVW"), "no value in {fact}");
        }
    }

    #[test]
    fn req_038_9_standard_negative_response_is_a_rejection_with_its_code_not_an_error() {
        // #339: structure decides; the text of the error is not consulted for a rejection and not kept.
        for (nrc, error) in [
            (0x12, "negative OBD-II response: 7E8 03 7F 01 12"),
            (0x31, "negative OBD-II response: 7E8 03 7F 01 31"),
            (0x78, "anything, even a timeout or a session"),
        ] {
            let fact = classify_read(
                json!({"ok":false,"error":error,"negative_response":{"service":1,"nrc":nrc}}),
                None,
            );
            assert_eq!(fact["outcome"], "rejected", "{fact}");
            assert_eq!(fact["response_code"], nrc, "{fact}");
            assert!(fact.get("reason").is_none(), "{fact}");
            assert!(!fact.to_string().contains("negative OBD"), "{fact}");
        }
        // the same read without the structure is only a failed read: the text is never parsed into a code
        let fact = classify_read(
            json!({"ok":false,"error":"negative OBD-II response: 7E8 03 7F 01 12"}),
            None,
        );
        assert_eq!(fact["outcome"], "error", "{fact}");
        assert!(fact.get("response_code").is_none(), "{fact}");
        // a code that is no byte is no rejection either
        let fact = classify_read(
            json!({"ok":false,"error":"x","negative_response":{"service":1,"nrc":300}}),
            None,
        );
        assert_eq!(fact["outcome"], "error", "{fact}");
        // the produced rejection is a record the derivation accepts, and it counts as rejected (never as a value)
        let fact = classify_read(
            json!({"ok":false,"error":"x","negative_response":{"service":1,"nrc":18}}),
            None,
        );
        let mut opening = start(RUN, 0);
        opening["result"]["data"]["knowledge_revision"] = json!(pin_revision());
        let set = derive(&[part(
            vec![
                opening,
                record("coverage.fact", secs(3), fact),
                boundary(RUN, "run_end", secs(4)),
            ],
            None,
        )]);
        assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
        assert_eq!(
            set["runs"][0]["entries"][0]["latest"]["state"], "rejected",
            "{set}"
        );
        assert_eq!(
            set["runs"][0]["entries"][0]["latest"]["response_code"], 18,
            "{set}"
        );
    }

    #[test]
    fn req_038_9_facts_never_copy_error_text_that_can_name_a_vehicle_or_adapter() {
        const SECRETS: [&str; 6] = [
            "WVWZZZ1JZXW000001",
            "SN-4711-0815",
            "AA:BB:CC:00:00:97",
            "123456 km",
            "ELM327 v1.5 Carly-Secret",
            "7E8 10 14 49 02 01 57 56 57",
        ];
        const CLOSED: [&str; 7] = [
            "timeout",
            "no_data",
            "session_state",
            "malformed_response",
            "read_failed",
            "no_usable_answer",
            "undecodable_value",
        ];
        let secret = SECRETS.join(" ");
        let errors = [
            format!("adapter timeout after 2s on {secret}"),
            format!("ELM327 rejected 010C: NO DATA {secret}"),
            format!("active UDS session does not match {secret}; Mode 01 read not sent"),
            format!("malformed ELM327 response line: {secret:?}"),
            format!("conflicting 010C responses (responders: {secret})"),
            format!("link failure {secret}"),
            secret.clone(),
            String::new(),
        ];
        let mut reads = Vec::new();
        for error in &errors {
            reads.push(json!({"ok":false,"error":error}));
            reads.push(
                json!({"ok":true,"data":{"key":"x","status":"unavailable","nrc":null,
                "error":error}}),
            );
        }
        reads.push(json!({"ok":true,"data":{"key":"x","value":{"vin":secret}}}));
        for read in reads {
            let fact = classify_read(read.clone(), None);
            let text = fact.to_string();
            for secret in SECRETS {
                assert!(
                    !text.contains(secret),
                    "{secret} copied from {read} into {fact}"
                );
            }
            assert!(!text.to_lowercase().contains("carly"), "{fact}");
            if let Some(reason) = fact.get("reason") {
                assert!(
                    CLOSED.contains(&reason.as_str().unwrap()),
                    "reason outside the closed set in {fact}"
                );
            }
            // the fact is a record the derivation accepts as it is
            let mut opening = start(RUN, 0);
            opening["result"]["data"]["knowledge_revision"] = json!(pin_revision());
            let set = derive(&[part(
                vec![
                    opening,
                    record("coverage.fact", secs(3), fact),
                    boundary(RUN, "run_end", secs(4)),
                ],
                None,
            )]);
            assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
            let ledger = set.to_string();
            for secret in SECRETS {
                assert!(
                    !ledger.contains(secret),
                    "{secret} reached the ledger of {read}"
                );
            }
        }
    }

    #[test]
    fn req_038_9_classification_adds_the_owned_fields_and_a_valid_record() {
        let fact = classify_read(
            json!({"ok":true,"data":{"key":"engine.rpm","value":900}}),
            None,
        );
        assert_eq!(fact["run"], RUN);
        assert_eq!(fact["signal_key"], "engine.rpm");
        assert_eq!(fact["context"], "baseline");
        assert_eq!(fact["session"], "default");
        assert_eq!(fact["identity_confirmed"], true);
        assert_eq!(fact["goal_definition"], 1);
        assert_eq!(fact["run_offset_us"], secs(3));
        assert_eq!(fact["knowledge_revision"], pin_revision());
        assert_eq!(fact["evidence"], json!({"at":at(secs(3)),"offset_us":777}));
        // The produced data is a record the derivation accepts as it is.
        let mut opening = start(RUN, 0);
        opening["result"]["data"]["knowledge_revision"] = json!(pin_revision());
        let records = vec![
            opening,
            record("coverage.fact", secs(3), fact),
            boundary(RUN, "run_end", secs(4)),
        ];
        let set = derive(&[part(records, None)]);
        assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
        assert_eq!(set["runs"][0]["entries"][0]["signal_key"], "engine.rpm");
    }

    #[test]
    fn req_038_9_classification_takes_plausibility_and_identity_from_knowledge() {
        let catalog = KnowledgeCatalog::embedded_pinned().unwrap();
        let ranged = catalog
            .definitions()
            .find(|definition| definition.plausible_range().is_some())
            .unwrap();
        let range = ranged.plausible_range().unwrap();
        let read = |value: f64| {
            json!({"ok":true,"data":{"key":"k","definition_id":ranged.id(),
                "target_id":"engine","status":"available","value":value}})
        };
        let inside = classify_read(
            read((range.minimum() + range.maximum()) / 2.0),
            Some(&catalog),
        );
        assert_eq!(inside["implausible"], false);
        assert_eq!(inside["ecu_target"], "engine");
        let outside = classify_read(read(range.maximum() + 1000.0), Some(&catalog));
        assert_eq!(outside["implausible"], true);
        assert_eq!(outside["outcome"], "decoded");

        let identity = catalog
            .set(crate::knowledge_db::STANDARD_UDS_ECU_IDENTIFICATION_SET)
            .unwrap()
            .members()
            .iter()
            .find_map(|member| catalog.semantic(member))
            .unwrap();
        let identity_read = json!({"ok":true,"data":{"key":"k","definition_id":identity.id(),
            "target_id":"engine","status":"available","value":"SERIAL-1"}});
        let fact = classify_read(identity_read.clone(), Some(&catalog));
        assert_eq!(fact["static_identity"], true);
        assert!(!fact.to_string().contains("SERIAL-1"));
        // Without a handed-over catalog the embedded pin answers the same.
        assert_eq!(classify_read(identity_read, None)["static_identity"], true);
        assert_eq!(fact["knowledge_revision"], catalog.pin().revision());
        assert_eq!(inside["static_identity"], false);
    }

    #[test]
    fn req_038_9_classification_rejects_arguments_it_cannot_trust() {
        let good = json!({"run":RUN,"key":"engine.rpm","run_offset_us":1,"session":"default",
            "identity_confirmed":false,"evidence":{"at":1,"offset_us":2},
            "read":{"ok":true,"data":{"value":1}}});
        assert!(fact_data(&good.to_string(), None).is_ok());
        for (field, value) in [
            ("run", json!("not a token!")),
            ("key", json!("")),
            ("run_offset_us", json!(-1)),
            ("session", json!("programming")),
            ("identity_confirmed", json!("yes")),
            ("evidence", json!({"at":1})),
            ("read", json!("ok")),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(fact_data(&bad.to_string(), None).is_err(), "{field}");
        }
        assert!(fact_data("not json", None).is_err());
    }

    #[test]
    fn req_038_9_core_action_classifies_without_any_adapter_traffic() {
        let mut core = crate::mobile::MobileCore::new(Offline).unwrap();
        let argument = json!({"run":RUN,"key":"engine.rpm","run_offset_us":5,"session":"extended",
            "identity_confirmed":false,"evidence":{"at":1,"offset_us":2,"in_previous_file":true},
            "read":{"ok":false,"error":"link failure"}});
        let result: Value =
            serde_json::from_str(&core.execute("coverage.fact", &argument.to_string())).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["data"]["outcome"], "error");
        assert_eq!(result["data"]["session"], "extended");
        assert_eq!(result["data"]["evidence"]["in_previous_file"], true);
        let rejected: Value = serde_json::from_str(&core.execute("coverage.fact", "{}")).unwrap();
        assert_eq!(rejected["ok"], false);
    }

    #[test]
    fn req_038_9_core_action_builds_run_records_the_derivation_accepts() {
        let mut core = crate::mobile::MobileCore::new(Offline).unwrap();
        let mut ask = |argument: Value| -> Value {
            serde_json::from_str(&core.execute("coverage.run", &argument.to_string())).unwrap()
        };
        let first = ask(json!({"run":RUN,"action":"start","part":1,"run_offset_us":0}));
        let second = ask(
            json!({"run":RUN,"action":"resume","part":2,"previous_part_records":2,"run_offset_us":5}),
        );
        assert_eq!(first["ok"], true, "{first}");
        assert_eq!(first["data"]["goal_definition"], 1);
        assert_eq!(first["data"]["knowledge_revision"], pin_revision());
        assert_eq!(second["data"]["previous_part_records"], 2);
        for bad in [
            json!({"run":RUN,"action":"start","part":2,"run_offset_us":0}),
            json!({"run":RUN,"action":"resume","part":1,"previous_part_records":0,"run_offset_us":0}),
            json!({"run":RUN,"action":"resume","part":2,"run_offset_us":0}),
            json!({"run":"bad token","action":"start","part":1,"run_offset_us":0}),
            json!({"run":RUN,"action":"pause","part":1,"run_offset_us":0}),
        ] {
            assert_eq!(ask(bad.clone())["ok"], false, "{bad}");
        }
        let records = vec![
            record("coverage.run", 0, first["data"].clone()),
            record("coverage.fact", 1, {
                let mut data = classify_read(
                    json!({"ok":true,"data":{"key":"engine.rpm","value":900}}),
                    None,
                );
                data["run_offset_us"] = json!(1);
                data
            }),
            record(
                "coverage.boundary",
                2,
                json!({"run":RUN,"class":"run_end","run_offset_us":2}),
            ),
        ];
        let set = derive(&[part(records, None)]);
        assert_eq!(set["runs"][0]["coverage_invalid"], false, "{set}");
    }

    #[test]
    fn req_038_9_core_actions_derive_the_parts_handed_over_one_by_one_and_then_forget_them() {
        let mut core = crate::mobile::MobileCore::new(Offline).unwrap();
        let mut ask = |action: &str, argument: Value| -> Value {
            serde_json::from_str(&core.execute(action, &argument.to_string())).unwrap()
        };
        let pin = pin_revision();
        let files: Vec<Part> = three_parts()
            .into_iter()
            .map(|mut file| {
                for record in &mut file.records {
                    if record["result"]["data"]["knowledge_revision"].is_string() {
                        record["result"]["data"]["knowledge_revision"] = json!(pin);
                    }
                }
                file
            })
            .collect();
        for index in [2, 0, 1] {
            let handed = ask(
                "replay.coverage.part",
                json!({"records":files[index].records,"ended":files[index].ended}),
            );
            assert_eq!(handed["ok"], true, "{handed}");
        }
        let derived = ask("replay.coverage", json!({}));
        assert_eq!(derived["ok"], true, "{derived}");
        let run = &derived["data"]["runs"][0];
        assert_eq!(run["run_complete"], true);
        assert_eq!(run["entries"][0]["valid_time_ms"], 60_000);
        // The accumulator is empty again: nothing is derived twice or mixed into the next file.
        assert_eq!(ask("replay.coverage", json!({}))["data"], Value::Null);
        for bad in [
            json!({"records":"x","ended":null}),
            json!({"records":[],"ended":5}),
        ] {
            assert_eq!(ask("replay.coverage.part", bad)["ok"], false);
        }
    }

    #[test]
    fn req_038_9_core_replay_stops_on_a_knowledge_pin_mismatch() {
        let mut core = crate::mobile::MobileCore::new(Offline).unwrap();
        let file = whole_run_in_one_file();
        let handed: Value = serde_json::from_str(&core.execute(
            "replay.coverage.part",
            &json!({"records":file.records,"ended":file.ended}).to_string(),
        ))
        .unwrap();
        assert_eq!(handed["ok"], true);
        let derived: Value = serde_json::from_str(&core.execute("replay.coverage", "{}")).unwrap();
        assert_eq!(derived["ok"], false);
        assert!(derived["error"]
            .as_str()
            .unwrap()
            .ends_with(super::super::knowledge::PIN_MISMATCH));
    }

    #[test]
    fn req_038_9_core_accepts_at_most_one_part_more_than_the_run_limit() {
        let mut core = crate::mobile::MobileCore::new(Offline).unwrap();
        let empty = json!({"records":[],"ended":null}).to_string();
        for _ in 0..=MAX_PARTS {
            let handed: Value =
                serde_json::from_str(&core.execute("replay.coverage.part", &empty)).unwrap();
            assert_eq!(handed["ok"], true);
        }
        let refused: Value =
            serde_json::from_str(&core.execute("replay.coverage.part", &empty)).unwrap();
        assert_eq!(refused["ok"], false);
    }
}
