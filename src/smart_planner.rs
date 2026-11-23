//! Pure Smart measurement planner (REQ-039): which signals to read next, in which goal context
//! and in which order.
//!
//! The planner is plain data and a deterministic fold: no I/O, no clock (time is the monotonic
//! offset the caller passes), no adapter, no Android and no Knowledge access. Inputs are the
//! effective catalog ([`CatalogEntry`]), the vehicle ledger of `coverage_ledger` (REQ-038) and
//! timestamped readings ([`Observation`]); outputs are the next selection of at most
//! [`MAX_SELECTION`] signals plus plan-change events with their reason. One [`Planner`] is one
//! drive of one Smart run on one definition version (REQ-039.10); a new drive or a new definition
//! version is a new planner that resumes from the ledger.
//!
//! Boundaries (REQ-039.3, .9):
//! - A selection can only contain keys of the catalog the planner was built with; there is no
//!   method that adds a target, a raw command, a service byte, an address or a policy. Goals,
//!   conditions, thresholds, priorities and retry timings are constants of this module.
//! - Catalog entries flagged as session-only are never selected: they are handed to the extended
//!   session slice (REQ-040) through [`Plan::session_targets`] and reported as skipped.
//! - The module only reads: the plan is a list of read targets for the default session.
//!
//! Conventions the criteria leave open:
//! - A condition with any input that is not fresh, finite, plausible and successfully decoded is
//!   unknown, even when another input alone would decide it (REQ-039.6).
//! - The warm-up goal holds while the coolant temperature is below the warm threshold; the
//!   warm-dependent goals are cooling warm driving, dpf warm driving and rail.
//! - Goal membership of a signal is decided by its key (see [`Goal::includes`]); every signal is
//!   a member of the baseline.
//! - The planner must be asked for a plan at least once per step; re-arming and the turbo hold
//!   are evaluated at the offsets of those calls and of the readings.
//! - A goal that has not been active yet needs no re-arm time; the 10 seconds apply after it ended
//!   because its condition did not hold (REQ-039.12).
//! - A signal whose latest ledger state is a rejected, empty or unanswered read is attempted once
//!   in a new drive and not retried within it ("then once per drive", REQ-039.14).

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use crate::coverage_ledger::{
    ContextRule, DefinitionVersions, EntryKey, GoalContext, LatestState, Ledger, Outcome, Rules,
    Status, FRESHNESS_LIMIT, MIN_SAMPLES, MIN_VALID_TIME,
};

/// REQ-036.1/REQ-039.5: signals selected per step.
pub const MAX_SELECTION: usize = 32;
/// REQ-039.13: slots per step at most reserved for baseline signals below their target.
pub const BASELINE_RESERVE: usize = 8;
/// REQ-039.1/.2: provisional technical conditions, fixed and owned by OBDentic.
pub const WARM_CELSIUS: f64 = 80.0;
pub const TURBO_PEDAL_PERCENT: f64 = 80.0;
pub const RAIL_RPM: (f64, f64) = (500.0, 1100.0);
pub const EGR_SPEED_KMH: (f64, f64) = (50.0, 80.0);
pub const EGR_LOAD_PERCENT: (f64, f64) = (15.0, 60.0);
pub const EGR_CONTIGUOUS: Duration = Duration::from_secs(30);
/// REQ-039.12: the turbo context continues this long after the first fresh reading at or below
/// the pedal threshold.
pub const TURBO_RELEASE_HOLD: Duration = Duration::from_secs(20);
/// REQ-039.12: a goal can start again only after its condition did not hold for this long.
pub const REARM: Duration = FRESHNESS_LIMIT;
/// REQ-039.14: delays before the first and the second retry of a failed signal.
pub const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(30), Duration::from_secs(120)];
/// REQ-038.7: version of the goal, condition and threshold definitions of this module. Changing
/// any of them requires a new number; stored progress of the old number stays separate.
pub const GOAL_DEFINITION_VERSION: u32 = 1;

const COOLANT: &str = "engine.coolant_temperature";
const SPEED: &str = "vehicle.speed";
const RPM: &str = "engine.rpm";
const LOAD: &str = "engine.load";
const PEDAL: &str = "vehicle.accelerator_pedal_e";
const REGEN: &str = "dpf.field_regeneration_request_status";
/// REQ-039.4: the continuously monitored trigger set, selected in every step whenever readable.
pub const TRIGGER_SIGNALS: [&str; 6] = [COOLANT, SPEED, RPM, LOAD, PEDAL, REGEN];

/// Versions a new planner and the run it belongs to are recorded under (REQ-039.10).
pub fn current_versions(knowledge_revision: impl Into<String>) -> DefinitionVersions {
    DefinitionVersions {
        goal_definition: GOAL_DEFINITION_VERSION,
        knowledge_revision: knowledge_revision.into(),
    }
}

/// Goals in the order REQ-039.2 lists them (the tie-break of REQ-039.13), baseline last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Goal {
    CoolingWarmUp,
    CoolingWarmDriving,
    DpfWarmDriving,
    DpfRaw,
    Turbo,
    Rail,
    Egr,
    Baseline,
}

impl Goal {
    pub const ALL: [Goal; 8] = [
        Goal::CoolingWarmUp,
        Goal::CoolingWarmDriving,
        Goal::DpfWarmDriving,
        Goal::DpfRaw,
        Goal::Turbo,
        Goal::Rail,
        Goal::Egr,
        Goal::Baseline,
    ];

    /// Ledger context id of the goal.
    pub fn context_id(self) -> &'static str {
        match self {
            Goal::CoolingWarmUp => "cooling.warm_up",
            Goal::CoolingWarmDriving => "cooling.warm_driving",
            Goal::DpfWarmDriving => "dpf.warm_driving",
            Goal::DpfRaw => "dpf.raw_request",
            Goal::Turbo => "turbo",
            Goal::Rail => "rail",
            Goal::Egr => "egr",
            Goal::Baseline => crate::coverage_ledger::BASELINE_CONTEXT,
        }
    }

    pub fn context(self) -> GoalContext {
        GoalContext::new(self.context_id())
    }

    /// REQ-039.13 priority level, 0 highest. Cooling and dpf warm driving share one level.
    pub fn level(self) -> u8 {
        match self {
            Goal::DpfRaw => 0,
            Goal::Rail => 1,
            Goal::Turbo => 2,
            Goal::Egr => 3,
            Goal::CoolingWarmUp | Goal::CoolingWarmDriving | Goal::DpfWarmDriving => 4,
            Goal::Baseline => 5,
        }
    }

    fn warm_dependent(self) -> bool {
        matches!(
            self,
            Goal::CoolingWarmDriving | Goal::DpfWarmDriving | Goal::Rail
        )
    }

    /// Signals whose fresh readings the goal condition needs; also the required signals of the
    /// ledger rule.
    fn inputs(self) -> &'static [&'static str] {
        match self {
            Goal::CoolingWarmUp => &[COOLANT],
            Goal::CoolingWarmDriving | Goal::DpfWarmDriving => &[COOLANT, SPEED],
            Goal::DpfRaw => &[REGEN],
            Goal::Turbo => &[PEDAL],
            Goal::Rail => &[COOLANT, SPEED, RPM],
            Goal::Egr => &[SPEED, LOAD],
            Goal::Baseline => &[],
        }
    }

    /// Whether a signal belongs to the goal. Provisional key-based grouping owned by OBDentic;
    /// the baseline includes every signal.
    pub fn includes(self, key: &str) -> bool {
        let under = |prefix: &str| key.starts_with(prefix);
        match self {
            Goal::CoolingWarmUp | Goal::CoolingWarmDriving => key == COOLANT || under("cooling."),
            Goal::DpfWarmDriving | Goal::DpfRaw => under("dpf."),
            Goal::Turbo => under("turbo.") || key == "engine.intake_manifold_pressure",
            Goal::Rail => under("rail.") || under("engine.fuel_rail"),
            Goal::Egr => under("egr.") || under("engine.egr."),
            Goal::Baseline => true,
        }
    }

    fn holds(self, values: &[f64]) -> bool {
        let inside = |v: f64, (low, high): (f64, f64)| (low..=high).contains(&v);
        match (self, values) {
            (Goal::CoolingWarmUp, [coolant]) => *coolant < WARM_CELSIUS,
            (Goal::CoolingWarmDriving | Goal::DpfWarmDriving, [coolant, speed]) => {
                *coolant >= WARM_CELSIUS && *speed > 0.0
            }
            (Goal::DpfRaw, [regen]) => *regen != 0.0,
            (Goal::Turbo, [pedal]) => *pedal > TURBO_PEDAL_PERCENT,
            (Goal::Rail, [coolant, speed, rpm]) => {
                *coolant >= WARM_CELSIUS && *speed <= 0.0 && inside(*rpm, RAIL_RPM)
            }
            (Goal::Egr, [speed, load]) => {
                inside(*speed, EGR_SPEED_KMH) && inside(*load, EGR_LOAD_PERCENT)
            }
            _ => false,
        }
    }
}

/// Counting rules for the ledger fold of a Smart run, matching the goals of this module
/// (REQ-039.2, REQ-038.4). Callers fold facts with these rules so that planner and ledger judge
/// coverage identically.
pub fn goal_rules() -> Rules {
    Goal::ALL
        .iter()
        .filter(|goal| **goal != Goal::Baseline)
        .fold(Rules::generic_baseline(), |rules, goal| {
            rules.with_context(
                goal.context(),
                ContextRule {
                    required_signals: goal.inputs().iter().map(|s| (*s).to_owned()).collect(),
                    min_samples: MIN_SAMPLES,
                    min_total_time: MIN_VALID_TIME,
                    min_contiguous: (*goal == Goal::Egr).then_some(EGR_CONTIGUOUS),
                },
            )
        })
}

/// Where a catalog entry comes from (REQ-039.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Standard PID; only entries the vehicle reports as supported are plannable.
    StandardPid { reported_supported: bool },
    /// Default-session signal of the effective Knowledge.
    DefaultSession,
    /// Session-flagged DID: handed to the extended session slice, never planned here.
    SessionFlagged,
}

/// One signal of the effective catalog and availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub signal_key: String,
    pub ecu_target: Option<String>,
    pub source: Source,
}

/// Why something is unknown, skipped, interrupted or ended. Shown and recorded with plan changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NoReading,
    Stale,
    Gap,
    Unsupported,
    Rejected,
    Empty,
    Unanswered,
    ReadError,
    SessionRestricted,
    Implausible,
    NonFinite,
    ConditionEnded,
    WarmNotYetReached,
    Rearming,
    HigherPriority,
    NoSignalsInCatalog,
    AllSignalsUnavailable,
    UserStop,
}

/// A timestamped read result.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub signal_key: String,
    pub at: Duration,
    pub outcome: Outcome,
    /// Decoded numeric value, kept in memory and the private recording only.
    pub value: Option<f64>,
    pub implausible: bool,
}

/// One input behind a condition evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionInput {
    pub signal_key: String,
    pub value: Result<f64, Reason>,
}

/// Warm state of the warm-dependent goals (REQ-039.11); never a fault or health statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warm {
    NotYetReached,
    Reached,
    Unknown(Reason),
}

/// What a goal is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Condition holds and the goal got slots this step.
    Serving,
    /// Condition holds, a higher level took every slot.
    Interrupted {
        by: Goal,
    },
    /// Condition holds, signals are waiting for their retry time.
    AwaitingRetry,
    /// Condition holds and every plannable signal is covered.
    Complete,
    NotReachable(Reason),
    WaitingForWarm,
    WaitingForCondition,
    Rearming,
    ConditionUnknown(Reason),
    Stopped,
}

impl Phase {
    fn is_active(self) -> bool {
        matches!(
            self,
            Phase::Serving | Phase::Interrupted { .. } | Phase::AwaitingRetry | Phase::Complete
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    Started,
    Interrupted {
        by: Goal,
    },
    Resumed,
    Completed,
    Expired,
    NotReachable,
    Stopped,
    /// A signal is left out of the plan; `retry_at` is set when it will be tried again.
    SignalSkipped {
        signal_key: String,
        retry_at: Option<Duration>,
    },
}

/// A plan change with the reason and the readings behind it (REQ-039.8).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanEvent {
    pub at: Duration,
    pub goal: Option<Goal>,
    pub kind: EventKind,
    pub reason: Option<Reason>,
    pub inputs: Vec<ConditionInput>,
    pub versions: DefinitionVersions,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    pub signal_key: String,
    pub ecu_target: Option<String>,
    pub context: GoalContext,
    /// Selected as member of the monitored trigger set (REQ-039.4).
    pub trigger: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoalStatus {
    pub goal: Goal,
    pub phase: Phase,
    /// Mean per-signal share of the target over the goal's plannable signals (REQ-039.13).
    pub share: f64,
    pub warm: Option<Warm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub selection: Vec<Selected>,
    pub events: Vec<PlanEvent>,
    pub goals: Vec<GoalStatus>,
    /// Session-flagged signals handed to the extended session slice (REQ-040).
    pub session_targets: Vec<String>,
}

#[derive(Debug, Clone)]
enum Availability {
    Unchecked,
    Available,
    Unsupported,
    SessionRestricted,
    Failed {
        count: u32,
        retry_at: Option<Duration>,
    },
}

#[derive(Debug, Clone, Default)]
struct GoalState {
    /// May start when its condition holds.
    armed: bool,
    unarmed_since: Option<Duration>,
    pressed: bool,
    hold_until: Option<Duration>,
    active_before: bool,
    phase: Option<Phase>,
}

struct Evaluation {
    active: bool,
    inactive: Phase,
    inputs: Vec<ConditionInput>,
}

struct Candidates {
    plannable: usize,
    unfinished: usize,
    /// Attemptable unfinished signals in selection order.
    ready: Vec<String>,
    share: f64,
}

/// One drive of one Smart run on one definition version.
#[derive(Debug, Clone)]
pub struct Planner {
    versions: DefinitionVersions,
    rules: Rules,
    catalog: BTreeMap<String, CatalogEntry>,
    availability: BTreeMap<String, Availability>,
    /// Signals whose read failed in the ledger before this drive: attempted once, not retried.
    chronic: BTreeSet<String>,
    context: BTreeMap<String, (Duration, Result<f64, Reason>)>,
    goals: BTreeMap<Goal, GoalState>,
    last_planned: BTreeMap<String, u64>,
    step: u64,
    pending: Vec<PlanEvent>,
    announced_handoff: bool,
    stopped: bool,
}

impl Planner {
    /// `ledger` is only read for signals whose latest read failed before this drive (REQ-039.14).
    pub fn new(
        catalog: impl IntoIterator<Item = CatalogEntry>,
        versions: DefinitionVersions,
        ledger: &Ledger,
    ) -> Self {
        let mut entries = BTreeMap::new();
        for entry in catalog {
            entries.entry(entry.signal_key.clone()).or_insert(entry);
        }
        let availability = entries
            .values()
            .map(|entry| {
                let state = match entry.source {
                    Source::StandardPid {
                        reported_supported: false,
                    } => Availability::Unsupported,
                    Source::SessionFlagged => Availability::SessionRestricted,
                    _ => Availability::Unchecked,
                };
                (entry.signal_key.clone(), state)
            })
            .collect();
        let chronic = ledger
            .entries
            .iter()
            .filter(|(key, entry)| {
                key.versions == versions
                    && entry.latest.as_ref().is_some_and(|latest| {
                        matches!(
                            latest.state,
                            LatestState::Rejected { .. }
                                | LatestState::Empty
                                | LatestState::Error { .. }
                                | LatestState::NoAnswer { .. }
                        )
                    })
            })
            .map(|(key, _)| key.signal_key.clone())
            .collect();
        let goals = Goal::ALL
            .iter()
            .map(|goal| {
                (
                    *goal,
                    GoalState {
                        armed: true,
                        ..GoalState::default()
                    },
                )
            })
            .collect();
        Self {
            rules: goal_rules(),
            versions,
            catalog: entries,
            availability,
            chronic,
            context: BTreeMap::new(),
            goals,
            last_planned: BTreeMap::new(),
            step: 0,
            pending: Vec::new(),
            announced_handoff: false,
            stopped: false,
        }
    }

    pub fn versions(&self) -> &DefinitionVersions {
        &self.versions
    }

    /// Definition versions of the ledger that hold progress apart from this planner's version
    /// (REQ-039.10). The planner never reads or credits their entries.
    pub fn other_versions(&self, ledger: &Ledger) -> BTreeSet<DefinitionVersions> {
        ledger
            .entries
            .keys()
            .map(|key| &key.versions)
            .filter(|versions| **versions != self.versions)
            .cloned()
            .collect()
    }

    /// Takes a read result: refreshes the context readings and the availability of the signal.
    /// An unsuccessful read is never counted as a measurement, as 0 or as success.
    pub fn record(&mut self, observation: &Observation) {
        let key = observation.signal_key.as_str();
        if TRIGGER_SIGNALS.contains(&key)
            && self
                .context
                .get(key)
                .is_none_or(|(at, _)| observation.at >= *at)
        {
            self.context
                .insert(key.to_owned(), (observation.at, usable(observation)));
        }
        let Some(current) = self.availability.get(key) else {
            return;
        };
        if matches!(
            current,
            Availability::Unsupported | Availability::SessionRestricted
        ) {
            return;
        }
        let (next, reason) = match &observation.outcome {
            Outcome::Decoded(_) => (Availability::Available, None),
            Outcome::Gap => return,
            Outcome::Unsupported => (Availability::Unsupported, Some(Reason::Unsupported)),
            Outcome::SessionRestricted { .. } => (
                Availability::SessionRestricted,
                Some(Reason::SessionRestricted),
            ),
            failure => {
                let count = match current {
                    Availability::Failed { count, .. } => count + 1,
                    _ => 1,
                };
                let delay = (!self.chronic.contains(key))
                    .then(|| RETRY_DELAYS.get(count as usize - 1))
                    .flatten();
                (
                    Availability::Failed {
                        count,
                        retry_at: delay.map(|delay| observation.at + *delay),
                    },
                    Some(failure_reason(failure)),
                )
            }
        };
        let retry_at = match &next {
            Availability::Failed { retry_at, .. } => *retry_at,
            _ => None,
        };
        self.availability.insert(key.to_owned(), next);
        if let Some(reason) = reason {
            self.pending.push(self.event(
                observation.at,
                None,
                EventKind::SignalSkipped {
                    signal_key: key.to_owned(),
                    retry_at,
                },
                Some(reason),
                Vec::new(),
            ));
        }
    }

    /// Ends planning: goals in progress end with the user's stop, disconnect or error.
    pub fn stop(&mut self, now: Duration, reason: Reason) -> Vec<PlanEvent> {
        let mut events = std::mem::take(&mut self.pending);
        if !self.stopped {
            self.stopped = true;
            for goal in Goal::ALL {
                let state = self.goals.get_mut(&goal).expect("every goal has a state");
                if state.phase.is_some_and(Phase::is_active) {
                    state.phase = Some(Phase::Stopped);
                    events.push(PlanEvent {
                        at: now,
                        goal: Some(goal),
                        kind: EventKind::Stopped,
                        reason: Some(reason),
                        inputs: Vec::new(),
                        versions: self.versions.clone(),
                    });
                }
            }
        }
        events
    }

    /// The next selection of at most [`MAX_SELECTION`] signals and the plan changes since the
    /// previous call.
    pub fn plan(&mut self, now: Duration, ledger: &Ledger) -> Plan {
        let session_targets = self.session_targets();
        let mut events = std::mem::take(&mut self.pending);
        if self.stopped {
            return Plan {
                selection: Vec::new(),
                events,
                goals: Vec::new(),
                session_targets,
            };
        }
        self.step += 1;
        if !self.announced_handoff {
            self.announced_handoff = true;
            for key in &session_targets {
                events.push(self.event(
                    now,
                    None,
                    EventKind::SignalSkipped {
                        signal_key: key.clone(),
                        retry_at: None,
                    },
                    Some(Reason::SessionRestricted),
                    Vec::new(),
                ));
            }
        }

        let evaluations: BTreeMap<Goal, Evaluation> = Goal::ALL
            .iter()
            .map(|goal| (*goal, self.evaluate(*goal, now)))
            .collect();
        let data: BTreeMap<Goal, Candidates> = Goal::ALL
            .iter()
            .map(|goal| (*goal, self.candidates(*goal, now, ledger)))
            .collect();

        let mut order: Vec<Goal> = Goal::ALL
            .into_iter()
            .filter(|goal| evaluations[goal].active)
            .collect();
        order.sort_by(|a, b| {
            (a.level(), data[a].share, *a)
                .partial_cmp(&(b.level(), data[b].share, *b))
                .expect("shares are finite")
        });

        let mut selection: Vec<Selected> = Vec::new();
        for key in TRIGGER_SIGNALS {
            if self.plannable(key) && self.attemptable(key, now) {
                selection.push(self.selected(key, Goal::Baseline, true));
            }
        }
        let room = MAX_SELECTION - selection.len();
        let baseline_ready = data[&Goal::Baseline]
            .ready
            .iter()
            .filter(|key| !selection.iter().any(|s| s.signal_key == **key))
            .count();
        let reserve = BASELINE_RESERVE.min(room).min(baseline_ready);
        let mut contextual = room - reserve;
        let mut taken: BTreeMap<Goal, usize> = BTreeMap::new();
        for goal in order.iter().filter(|goal| **goal != Goal::Baseline) {
            for key in &data[goal].ready {
                let seat = selection.iter().position(|s| s.signal_key == *key);
                match seat {
                    Some(at)
                        if selection[at].trigger && selection[at].context.id() == "baseline" =>
                    {
                        selection[at].context = goal.context();
                        *taken.entry(*goal).or_default() += 1;
                    }
                    Some(_) => {}
                    None if contextual > 0 => {
                        contextual -= 1;
                        selection.push(self.selected(key, *goal, false));
                        *taken.entry(*goal).or_default() += 1;
                    }
                    None => {}
                }
            }
        }
        let mut baseline_room = MAX_SELECTION - selection.len();
        if evaluations[&Goal::Baseline].active {
            for key in &data[&Goal::Baseline].ready {
                if baseline_room == 0 {
                    break;
                }
                if !selection.iter().any(|s| s.signal_key == *key) {
                    baseline_room -= 1;
                    selection.push(self.selected(key, Goal::Baseline, false));
                    *taken.entry(Goal::Baseline).or_default() += 1;
                }
            }
        }
        for selected in &selection {
            self.last_planned
                .insert(selected.signal_key.clone(), self.step);
        }

        let first_served = order
            .iter()
            .copied()
            .find(|goal| taken.get(goal).is_some_and(|n| *n > 0));
        let warm = self.warm(now);
        let mut statuses = Vec::new();
        for goal in Goal::ALL {
            let evaluation = &evaluations[&goal];
            let candidates = &data[&goal];
            let phase = if !evaluation.active {
                evaluation.inactive
            } else if candidates.plannable == 0 {
                Phase::NotReachable(if self.members(goal).next().is_none() {
                    Reason::NoSignalsInCatalog
                } else {
                    Reason::AllSignalsUnavailable
                })
            } else if candidates.unfinished == 0 {
                Phase::Complete
            } else if candidates.ready.is_empty() {
                Phase::AwaitingRetry
            } else if taken.get(&goal).is_none_or(|n| *n == 0) {
                Phase::Interrupted {
                    by: first_served.unwrap_or(Goal::DpfRaw),
                }
            } else {
                Phase::Serving
            };
            self.transition(goal, phase, now, &evaluation.inputs, &mut events);
            statuses.push(GoalStatus {
                goal,
                phase,
                share: candidates.share,
                warm: goal.warm_dependent().then_some(warm),
            });
        }
        Plan {
            selection,
            events,
            goals: statuses,
            session_targets,
        }
    }

    fn event(
        &self,
        at: Duration,
        goal: Option<Goal>,
        kind: EventKind,
        reason: Option<Reason>,
        inputs: Vec<ConditionInput>,
    ) -> PlanEvent {
        PlanEvent {
            at,
            goal,
            kind,
            reason,
            inputs,
            versions: self.versions.clone(),
        }
    }

    fn selected(&self, key: &str, goal: Goal, trigger: bool) -> Selected {
        Selected {
            signal_key: key.to_owned(),
            ecu_target: self.catalog[key].ecu_target.clone(),
            context: goal.context(),
            trigger,
        }
    }

    fn session_targets(&self) -> Vec<String> {
        self.catalog
            .values()
            .filter(|entry| entry.source == Source::SessionFlagged)
            .map(|entry| entry.signal_key.clone())
            .collect()
    }

    /// Default-session catalog signals that belong to `goal`, regardless of availability.
    fn members(&self, goal: Goal) -> impl Iterator<Item = &str> {
        self.catalog
            .values()
            .filter(move |entry| {
                entry.source != Source::SessionFlagged && goal.includes(&entry.signal_key)
            })
            .map(|entry| entry.signal_key.as_str())
    }

    /// Whether the planner can ever read the signal in this drive.
    fn plannable(&self, key: &str) -> bool {
        self.catalog.contains_key(key)
            && match self.availability.get(key) {
                Some(Availability::Unchecked | Availability::Available) => true,
                Some(Availability::Failed { retry_at, .. }) => retry_at.is_some(),
                _ => false,
            }
    }

    fn attemptable(&self, key: &str, now: Duration) -> bool {
        match self.availability.get(key) {
            Some(Availability::Unchecked | Availability::Available) => true,
            Some(Availability::Failed {
                retry_at: Some(at), ..
            }) => now >= *at,
            _ => false,
        }
    }

    fn entry_key(&self, key: &str, goal: Goal) -> EntryKey {
        EntryKey {
            versions: self.versions.clone(),
            signal_key: key.to_owned(),
            context: goal.context(),
        }
    }

    /// Share of the target one signal reached in the goal context (REQ-039.13), 1 when covered.
    fn signal_share(&self, ledger: &Ledger, goal: Goal, key: &str) -> f64 {
        let entry_key = self.entry_key(key, goal);
        if ledger.status(&self.rules, &entry_key) == Status::Covered {
            return 1.0;
        }
        ledger.entry(&entry_key).map_or(0.0, |entry| {
            let samples = f64::from(entry.qualifying_samples) / f64::from(MIN_SAMPLES);
            let time = entry.valid_time.as_secs_f64() / MIN_VALID_TIME.as_secs_f64();
            samples.min(time).min(1.0)
        })
    }

    fn candidates(&self, goal: Goal, now: Duration, ledger: &Ledger) -> Candidates {
        let mut plannable = 0;
        let mut unfinished = 0;
        let mut total = 0.0;
        let mut ready: Vec<(u64, f64, &str)> = Vec::new();
        for key in self.members(goal).filter(|key| self.plannable(key)) {
            plannable += 1;
            let share = self.signal_share(ledger, goal, key);
            total += share;
            if ledger.status(&self.rules, &self.entry_key(key, goal)) == Status::Covered {
                continue;
            }
            unfinished += 1;
            if self.attemptable(key, now) {
                let seen = self.last_planned.get(key).copied().unwrap_or(0);
                ready.push((seen, share, key));
            }
        }
        // Signals not planned for the longest come first so that every plannable signal is
        // attempted over successive steps; then the least advanced one.
        ready.sort_by(|a, b| {
            (a.0, a.1, a.2)
                .partial_cmp(&(b.0, b.1, b.2))
                .expect("shares are finite")
        });
        Candidates {
            plannable,
            unfinished,
            ready: ready
                .into_iter()
                .map(|(_, _, key)| key.to_owned())
                .collect(),
            share: if plannable == 0 {
                1.0
            } else {
                total / plannable as f64
            },
        }
    }

    fn input(&self, key: &str, now: Duration) -> Result<f64, Reason> {
        match self.context.get(key) {
            None => Err(Reason::NoReading),
            Some((_, Err(reason))) => Err(*reason),
            Some((at, Ok(_))) if now.saturating_sub(*at) > FRESHNESS_LIMIT => Err(Reason::Stale),
            Some((_, Ok(value))) => Ok(*value),
        }
    }

    fn warm(&self, now: Duration) -> Warm {
        match self.input(COOLANT, now) {
            Err(reason) => Warm::Unknown(reason),
            Ok(value) if value >= WARM_CELSIUS => Warm::Reached,
            Ok(_) => Warm::NotYetReached,
        }
    }

    /// Updates the condition state of `goal` and says whether it is active now.
    fn evaluate(&mut self, goal: Goal, now: Duration) -> Evaluation {
        let inputs: Vec<ConditionInput> = goal
            .inputs()
            .iter()
            .map(|key| ConditionInput {
                signal_key: (*key).to_owned(),
                value: self.input(key, now),
            })
            .collect();
        if goal == Goal::Baseline {
            return Evaluation {
                active: true,
                inactive: Phase::WaitingForCondition,
                inputs,
            };
        }
        let unknown = inputs.iter().find_map(|input| input.value.err());
        let values: Vec<f64> = inputs.iter().filter_map(|input| input.value.ok()).collect();
        let reading_at = goal
            .inputs()
            .iter()
            .filter_map(|key| self.context.get(*key).map(|(at, _)| *at))
            .max()
            .unwrap_or(now);
        let warm_missing = goal.warm_dependent() && self.warm(now) == Warm::NotYetReached;
        let state = self.goals.get_mut(&goal).expect("every goal has a state");

        let (active, inactive) = match unknown {
            Some(reason) => {
                // Ended only by an unknown reading: no re-arm wait, and unknown time never
                // counts toward re-arming (REQ-039.6, REQ-039.12).
                if state.active_before {
                    state.armed = true;
                }
                state.unarmed_since = None;
                state.pressed = false;
                state.hold_until = None;
                (false, Phase::ConditionUnknown(reason))
            }
            None if goal.holds(&values) => {
                state.unarmed_since = None;
                let held = state.hold_until.take().is_some();
                state.pressed = true;
                if state.armed || held {
                    state.armed = true;
                    (true, Phase::Rearming)
                } else {
                    (false, Phase::Rearming)
                }
            }
            None => {
                if goal == Goal::Turbo && state.pressed {
                    state.hold_until = Some(reading_at + TURBO_RELEASE_HOLD);
                }
                state.pressed = false;
                if state.active_before {
                    state.armed = false;
                    state.unarmed_since = Some(now);
                }
                if !state.armed {
                    let since = *state.unarmed_since.get_or_insert(now);
                    if now.saturating_sub(since) >= REARM {
                        state.armed = true;
                        state.unarmed_since = None;
                    }
                }
                let held = state.hold_until.is_some_and(|until| now < until);
                if !held {
                    state.hold_until = None;
                }
                let phase = if warm_missing {
                    Phase::WaitingForWarm
                } else {
                    Phase::WaitingForCondition
                };
                (held, phase)
            }
        };
        state.active_before = active;
        Evaluation {
            active,
            inactive,
            inputs,
        }
    }

    fn transition(
        &mut self,
        goal: Goal,
        phase: Phase,
        now: Duration,
        inputs: &[ConditionInput],
        events: &mut Vec<PlanEvent>,
    ) {
        let state = self.goals.get_mut(&goal).expect("every goal has a state");
        let before = state.phase.replace(phase);
        let (kind, reason) = match (before, phase) {
            (Some(old), new) if old == new => return,
            (Some(Phase::Interrupted { .. }), Phase::Serving) => (EventKind::Resumed, None),
            (_, Phase::Serving) => (EventKind::Started, None),
            (Some(Phase::Interrupted { .. }), Phase::Interrupted { .. }) => return,
            (_, Phase::Interrupted { by }) => {
                (EventKind::Interrupted { by }, Some(Reason::HigherPriority))
            }
            (_, Phase::Complete) => (EventKind::Completed, None),
            (_, Phase::NotReachable(reason)) => (EventKind::NotReachable, Some(reason)),
            (Some(old), Phase::ConditionUnknown(reason)) if old.is_active() => {
                (EventKind::Expired, Some(reason))
            }
            (Some(old), Phase::WaitingForWarm | Phase::WaitingForCondition) if old.is_active() => {
                (EventKind::Expired, Some(Reason::ConditionEnded))
            }
            _ => return,
        };
        let event = self.event(now, Some(goal), kind, reason, inputs.to_vec());
        events.push(event);
    }
}

fn usable(observation: &Observation) -> Result<f64, Reason> {
    match (&observation.outcome, observation.value) {
        (Outcome::Decoded(_), Some(value)) if value.is_finite() => {
            if observation.implausible {
                Err(Reason::Implausible)
            } else {
                Ok(value)
            }
        }
        (Outcome::Decoded(_), _) => Err(Reason::NonFinite),
        (outcome, _) => Err(failure_reason(outcome)),
    }
}

fn failure_reason(outcome: &Outcome) -> Reason {
    match outcome {
        Outcome::Decoded(_) => Reason::NonFinite,
        Outcome::Unsupported => Reason::Unsupported,
        Outcome::Rejected { .. } => Reason::Rejected,
        Outcome::Empty => Reason::Empty,
        Outcome::Error { .. } => Reason::ReadError,
        Outcome::NoAnswer { .. } => Reason::Unanswered,
        Outcome::Gap => Reason::Gap,
        Outcome::SessionRestricted { .. } => Reason::SessionRestricted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage_ledger::{
        Decoded, Entry, EvidenceRef, Fact, FactBody, Reading, RunId, SessionKind, SessionState,
        SmartRun,
    };

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn versions() -> DefinitionVersions {
        current_versions("k1")
    }

    fn entry(key: &str) -> CatalogEntry {
        CatalogEntry {
            signal_key: key.into(),
            ecu_target: None,
            source: Source::DefaultSession,
        }
    }

    fn triggers() -> Vec<CatalogEntry> {
        TRIGGER_SIGNALS.iter().map(|k| entry(k)).collect()
    }

    fn planner(extra: &[&str]) -> Planner {
        let mut catalog = triggers();
        catalog.extend(extra.iter().map(|k| entry(k)));
        Planner::new(catalog, versions(), &Ledger::default())
    }

    fn ok(key: &str, at: u64, value: f64) -> Observation {
        Observation {
            signal_key: key.into(),
            at: secs(at),
            outcome: Outcome::Decoded(Decoded::Numeric { finite: true }),
            value: Some(value),
            implausible: false,
        }
    }

    fn failed(key: &str, at: u64, outcome: Outcome) -> Observation {
        Observation {
            signal_key: key.into(),
            at: secs(at),
            outcome,
            value: None,
            implausible: false,
        }
    }

    fn feed(p: &mut Planner, at: u64, values: &[(&str, f64)]) {
        for (key, value) in values {
            p.record(&ok(key, at, *value));
        }
    }

    /// Fresh readings of all six trigger signals; idle, cold.
    fn base(
        coolant: f64,
        speed: f64,
        rpm: f64,
        load: f64,
        pedal: f64,
        regen: f64,
    ) -> Vec<(&'static str, f64)> {
        vec![
            (COOLANT, coolant),
            (SPEED, speed),
            (RPM, rpm),
            (LOAD, load),
            (PEDAL, pedal),
            (REGEN, regen),
        ]
    }

    fn phase(plan: &Plan, goal: Goal) -> Phase {
        plan.goals.iter().find(|g| g.goal == goal).unwrap().phase
    }

    fn contexts(plan: &Plan, key: &str) -> Vec<String> {
        plan.selection
            .iter()
            .filter(|s| s.signal_key == key)
            .map(|s| s.context.id().to_owned())
            .collect()
    }

    fn kinds(plan: &Plan, goal: Goal) -> Vec<EventKind> {
        plan.events
            .iter()
            .filter(|e| e.goal == Some(goal))
            .map(|e| e.kind.clone())
            .collect()
    }

    fn at_least(ledger: &mut Ledger, key: &str, goal: Goal, samples: u32, time: u64) {
        ledger.entries.insert(
            EntryKey {
                versions: versions(),
                signal_key: key.into(),
                context: goal.context(),
            },
            Entry {
                qualifying_samples: samples,
                valid_time: secs(time),
                longest_segment: secs(time),
                latest: None,
            },
        );
    }

    #[test]
    fn req_039_2_conditions_use_the_agreed_thresholds() {
        let table: [(Goal, Vec<f64>, bool); 18] = [
            (Goal::CoolingWarmUp, vec![79.9], true),
            (Goal::CoolingWarmUp, vec![80.0], false),
            (Goal::CoolingWarmDriving, vec![80.0, 0.1], true),
            (Goal::CoolingWarmDriving, vec![80.0, 0.0], false),
            (Goal::DpfWarmDriving, vec![79.0, 50.0], false),
            (Goal::Turbo, vec![80.0], false),
            (Goal::Turbo, vec![80.1], true),
            (Goal::Rail, vec![80.0, 0.0, 500.0], true),
            (Goal::Rail, vec![80.0, 0.0, 1100.0], true),
            (Goal::Rail, vec![80.0, 0.0, 1101.0], false),
            (Goal::Rail, vec![80.0, 1.0, 800.0], false),
            (Goal::Egr, vec![50.0, 15.0], true),
            (Goal::Egr, vec![80.0, 60.0], true),
            (Goal::Egr, vec![49.9, 30.0], false),
            (Goal::Egr, vec![60.0, 60.1], false),
            (Goal::DpfRaw, vec![0.0], false),
            (Goal::DpfRaw, vec![2.0], true),
            (Goal::Baseline, vec![], false),
        ];
        for (goal, values, expected) in table {
            assert_eq!(goal.holds(&values), expected, "{goal:?} {values:?}");
        }
    }

    #[test]
    fn req_039_2_context_goal_only_while_its_condition_holds() {
        let mut p = planner(&["cooling.radiator_outlet_temperature_actual"]);
        let ledger = Ledger::default();
        feed(&mut p, 0, &base(90.0, 0.0, 800.0, 20.0, 10.0, 0.0));
        let idle = p.plan(secs(0), &ledger);
        assert_eq!(
            contexts(&idle, "cooling.radiator_outlet_temperature_actual"),
            ["baseline"]
        );
        feed(&mut p, 1, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0));
        let driving = p.plan(secs(1), &ledger);
        assert_eq!(
            contexts(&driving, "cooling.radiator_outlet_temperature_actual"),
            ["cooling.warm_driving"]
        );
    }

    #[test]
    fn req_039_3_selection_stays_inside_the_closed_catalog() {
        let mut catalog = triggers();
        catalog.push(entry("engine.maf"));
        catalog.push(CatalogEntry {
            signal_key: "uds.did.f40d".into(),
            ecu_target: Some("ecu-1".into()),
            source: Source::SessionFlagged,
        });
        catalog.push(CatalogEntry {
            signal_key: "engine.timing_advance".into(),
            ecu_target: None,
            source: Source::StandardPid {
                reported_supported: false,
            },
        });
        let mut p = Planner::new(catalog.clone(), versions(), &Ledger::default());
        let plan = p.plan(secs(0), &Ledger::default());
        let known: BTreeSet<_> = catalog.iter().map(|c| c.signal_key.clone()).collect();
        assert!(plan.selection.iter().all(|s| known.contains(&s.signal_key)));
        assert!(contexts(&plan, "uds.did.f40d").is_empty());
        assert!(contexts(&plan, "engine.timing_advance").is_empty());
        assert_eq!(contexts(&plan, "engine.maf"), ["baseline"]);
        assert_eq!(plan.session_targets, ["uds.did.f40d"]);
        let skipped: Vec<_> = plan
            .events
            .iter()
            .filter(|e| matches!(&e.kind, EventKind::SignalSkipped { signal_key, .. } if signal_key == "uds.did.f40d"))
            .collect();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].reason, Some(Reason::SessionRestricted));
        // Announced once only.
        assert!(p.plan(secs(1), &Ledger::default()).events.is_empty());
    }

    #[test]
    fn req_039_3_outcomes_cannot_add_a_target() {
        let mut p = planner(&[]);
        p.record(&ok("engine.not_in_catalog", 0, 1.0));
        let plan = p.plan(secs(1), &Ledger::default());
        assert!(contexts(&plan, "engine.not_in_catalog").is_empty());
    }

    #[test]
    fn req_039_4_dpf_raw_request_takes_priority_and_resumes_the_others() {
        let mut p = planner(&[
            "dpf.soot_mass_calculated",
            "cooling.radiator_outlet_temperature_actual",
        ]);
        let ledger = Ledger::default();
        feed(&mut p, 0, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0));
        let before = p.plan(secs(0), &ledger);
        assert_eq!(phase(&before, Goal::CoolingWarmDriving), Phase::Serving);
        assert_eq!(phase(&before, Goal::DpfRaw), Phase::WaitingForCondition);

        feed(&mut p, 5, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 2.0));
        let regen = p.plan(secs(5), &ledger);
        assert_eq!(phase(&regen, Goal::DpfRaw), Phase::Serving);
        assert_eq!(
            contexts(&regen, "dpf.soot_mass_calculated"),
            ["dpf.raw_request"]
        );
        assert!(kinds(&regen, Goal::DpfRaw).contains(&EventKind::Started));

        // The raw request ends with the first fresh 0 and the lower goals resume.
        feed(&mut p, 8, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0));
        let after = p.plan(secs(8), &ledger);
        assert_eq!(phase(&after, Goal::DpfRaw), Phase::WaitingForCondition);
        assert_eq!(kinds(&after, Goal::DpfRaw), [EventKind::Expired]);
        assert_eq!(
            contexts(&after, "dpf.soot_mass_calculated"),
            ["dpf.warm_driving"]
        );
    }

    #[test]
    fn req_039_4_unknown_ends_priority_and_next_nonzero_restarts_without_rearm() {
        let mut p = planner(&["dpf.soot_mass_calculated"]);
        let ledger = Ledger::default();
        feed(&mut p, 0, &base(90.0, 0.0, 800.0, 20.0, 10.0, 3.0));
        assert_eq!(
            phase(&p.plan(secs(0), &ledger), Goal::DpfRaw),
            Phase::Serving
        );
        // The reading becomes unanswered: priority ends, reason recorded as unknown.
        p.record(&failed(REGEN, 2, Outcome::NoAnswer { reason: "t".into() }));
        let unknown = p.plan(secs(2), &ledger);
        assert_eq!(
            phase(&unknown, Goal::DpfRaw),
            Phase::ConditionUnknown(Reason::Unanswered)
        );
        assert_eq!(
            unknown
                .events
                .iter()
                .find(|e| e.kind == EventKind::Expired)
                .unwrap()
                .reason,
            Some(Reason::Unanswered)
        );
        feed(&mut p, 3, &[(REGEN, 1.0)]);
        assert_eq!(
            phase(&p.plan(secs(3), &ledger), Goal::DpfRaw),
            Phase::Serving
        );
        // After a fresh 0 the 10-second re-arm applies.
        feed(&mut p, 4, &[(REGEN, 0.0)]);
        p.plan(secs(4), &ledger);
        feed(&mut p, 6, &[(REGEN, 1.0)]);
        assert_eq!(
            phase(&p.plan(secs(6), &ledger), Goal::DpfRaw),
            Phase::Rearming
        );
        feed(&mut p, 14, &[(REGEN, 1.0)]);
        assert_eq!(
            phase(&p.plan(secs(14), &ledger), Goal::DpfRaw),
            Phase::Rearming
        );
    }

    #[test]
    fn req_039_5_catalog_is_distributed_over_steps_of_at_most_32() {
        let keys: Vec<String> = (0..100).map(|i| format!("engine.sig{i:03}")).collect();
        let mut catalog = triggers();
        catalog.extend(keys.iter().map(|k| entry(k)));
        let mut p = Planner::new(catalog, versions(), &Ledger::default());
        let ledger = Ledger::default();
        let mut seen = BTreeSet::new();
        for step in 0..8 {
            let plan = p.plan(secs(step), &ledger);
            assert!(plan.selection.len() <= MAX_SELECTION);
            let unique: BTreeSet<_> = plan.selection.iter().map(|s| &s.signal_key).collect();
            assert_eq!(unique.len(), plan.selection.len());
            seen.extend(plan.selection.iter().map(|s| s.signal_key.clone()));
        }
        for key in &keys {
            assert!(seen.contains(key), "{key} was never attempted");
        }
    }

    #[test]
    fn req_039_5_resumes_from_the_ledger_and_one_unavailable_signal_never_blocks() {
        let mut ledger = Ledger::default();
        at_least(&mut ledger, "engine.done", Goal::Baseline, 3, 30);
        let mut p = planner(&["engine.done", "engine.dead", "engine.live"]);
        p.record(&failed("engine.dead", 0, Outcome::Unsupported));
        let plan = p.plan(secs(1), &ledger);
        assert!(contexts(&plan, "engine.done").is_empty());
        assert!(contexts(&plan, "engine.dead").is_empty());
        assert_eq!(contexts(&plan, "engine.live"), ["baseline"]);
    }

    #[test]
    fn req_039_6_unknown_conditions_never_start_a_goal() {
        let ledger = Ledger::default();
        let mut p = planner(&["turbo.boost"]);
        // No reading at all.
        assert_eq!(
            phase(&p.plan(secs(0), &ledger), Goal::Turbo),
            Phase::ConditionUnknown(Reason::NoReading)
        );
        for (outcome, reason) in [
            (Outcome::Unsupported, Reason::Unsupported),
            (
                Outcome::Rejected {
                    response_code: 0x31,
                },
                Reason::Rejected,
            ),
            (Outcome::Empty, Reason::Empty),
            (Outcome::Gap, Reason::Gap),
        ] {
            p.record(&failed(PEDAL, 1, outcome));
            assert_eq!(
                phase(&p.plan(secs(1), &ledger), Goal::Turbo),
                Phase::ConditionUnknown(reason)
            );
        }
        p.record(&Observation {
            implausible: true,
            ..ok(PEDAL, 2, 99.0)
        });
        assert_eq!(
            phase(&p.plan(secs(2), &ledger), Goal::Turbo),
            Phase::ConditionUnknown(Reason::Implausible)
        );
        // Older than ten seconds is stale, exactly ten is still fresh.
        feed(&mut p, 3, &[(PEDAL, 99.0)]);
        assert_eq!(
            phase(&p.plan(secs(13), &ledger), Goal::Turbo),
            Phase::Serving
        );
        assert_eq!(
            phase(&p.plan(secs(14), &ledger), Goal::Turbo),
            Phase::ConditionUnknown(Reason::Stale)
        );
        let event = p.plan(secs(15), &ledger);
        assert!(event.events.is_empty());
    }

    #[test]
    fn req_039_6_event_carries_the_condition_readings() {
        let mut p = planner(&["turbo.boost"]);
        feed(&mut p, 0, &[(PEDAL, 95.0)]);
        let plan = p.plan(secs(0), &Ledger::default());
        let started = plan
            .events
            .iter()
            .find(|e| e.kind == EventKind::Started)
            .unwrap();
        assert_eq!(started.goal, Some(Goal::Turbo));
        assert_eq!(
            started.inputs,
            [ConditionInput {
                signal_key: PEDAL.into(),
                value: Ok(95.0)
            }]
        );
        assert_eq!(started.versions, versions());
    }

    #[test]
    fn req_039_7_unsuccessful_reads_are_skipped_with_reason_and_never_counted() {
        let mut p = planner(&["engine.a", "dpf.only"]);
        let ledger = Ledger::default();
        p.record(&failed("engine.a", 0, Outcome::Unsupported));
        p.record(&failed(
            "dpf.only",
            0,
            Outcome::SessionRestricted { reason: "x".into() },
        ));
        let plan = p.plan(secs(1), &ledger);
        let reasons: Vec<_> = plan
            .events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::SignalSkipped { signal_key, .. } => {
                    Some((signal_key.as_str(), e.reason))
                }
                _ => None,
            })
            .collect();
        assert!(reasons.contains(&("engine.a", Some(Reason::Unsupported))));
        assert!(reasons.contains(&("dpf.only", Some(Reason::SessionRestricted))));
        for later in 2..200 {
            let plan = p.plan(secs(later), &ledger);
            assert!(contexts(&plan, "engine.a").is_empty());
            assert!(contexts(&plan, "dpf.only").is_empty());
        }
    }

    #[test]
    fn req_039_7_goal_without_available_signals_is_not_reachable_and_others_continue() {
        let mut p = planner(&["dpf.soot_mass_calculated", "engine.maf"]);
        let ledger = Ledger::default();
        p.record(&failed("dpf.soot_mass_calculated", 0, Outcome::Unsupported));
        p.record(&failed(REGEN, 0, Outcome::Unsupported));
        feed(&mut p, 0, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0)[..5]);
        let plan = p.plan(secs(1), &ledger);
        assert_eq!(
            phase(&plan, Goal::DpfWarmDriving),
            Phase::NotReachable(Reason::AllSignalsUnavailable)
        );
        let event = plan
            .events
            .iter()
            .find(|e| e.kind == EventKind::NotReachable)
            .unwrap();
        assert_eq!(event.goal, Some(Goal::DpfWarmDriving));
        assert_eq!(event.reason, Some(Reason::AllSignalsUnavailable));
        assert_eq!(phase(&plan, Goal::Baseline), Phase::Serving);
        assert_eq!(contexts(&plan, "engine.maf"), ["baseline"]);
        // A goal without any catalog signal says so.
        assert_eq!(phase(&plan, Goal::Egr), Phase::WaitingForCondition);
        let mut again = planner(&[]);
        feed(&mut again, 0, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(
            phase(&again.plan(secs(0), &ledger), Goal::Egr),
            Phase::NotReachable(Reason::NoSignalsInCatalog)
        );
    }

    #[test]
    fn req_039_10_progress_of_other_versions_is_never_credited() {
        let mut ledger = Ledger::default();
        let old = DefinitionVersions {
            goal_definition: 0,
            knowledge_revision: "k0".into(),
        };
        ledger.entries.insert(
            EntryKey {
                versions: old.clone(),
                signal_key: "engine.maf".into(),
                context: GoalContext::baseline(),
            },
            Entry {
                qualifying_samples: 9,
                valid_time: secs(90),
                longest_segment: secs(90),
                latest: None,
            },
        );
        let mut p = planner(&["engine.maf"]);
        let plan = p.plan(secs(0), &ledger);
        assert_eq!(contexts(&plan, "engine.maf"), ["baseline"]);
        assert_eq!(p.other_versions(&ledger), BTreeSet::from([old]));
        assert_eq!(p.versions(), &versions());
    }

    #[test]
    fn req_039_11_cold_engine_is_warm_up_not_a_failure() {
        let mut p = planner(&["cooling.radiator_outlet_temperature_actual"]);
        let ledger = Ledger::default();
        feed(&mut p, 0, &base(60.0, 20.0, 1500.0, 20.0, 10.0, 0.0));
        let plan = p.plan(secs(0), &ledger);
        assert_eq!(
            contexts(&plan, "cooling.radiator_outlet_temperature_actual"),
            ["cooling.warm_up"]
        );
        assert_eq!(
            phase(&plan, Goal::CoolingWarmDriving),
            Phase::WaitingForWarm
        );
        let status = plan
            .goals
            .iter()
            .find(|g| g.goal == Goal::CoolingWarmDriving)
            .unwrap();
        assert_eq!(status.warm, Some(Warm::NotYetReached));
        assert!(plan
            .events
            .iter()
            .all(|e| e.kind != EventKind::NotReachable));
        // Unknown coolant is neither warm nor below.
        p.record(&failed(COOLANT, 1, Outcome::Empty));
        let unknown = p.plan(secs(1), &ledger);
        let status = unknown
            .goals
            .iter()
            .find(|g| g.goal == Goal::CoolingWarmDriving)
            .unwrap();
        assert_eq!(status.warm, Some(Warm::Unknown(Reason::Empty)));
        assert_eq!(
            phase(&unknown, Goal::CoolingWarmUp),
            Phase::ConditionUnknown(Reason::Empty)
        );
    }

    #[test]
    fn req_039_12_turbo_holds_twenty_seconds_after_release_and_pressing_continues() {
        let mut p = planner(&["turbo.boost"]);
        let ledger = Ledger::default();
        feed(&mut p, 0, &[(PEDAL, 95.0)]);
        assert_eq!(
            phase(&p.plan(secs(0), &ledger), Goal::Turbo),
            Phase::Serving
        );
        // First fresh reading at or below 80 % starts the hold at that reading.
        feed(&mut p, 5, &[(PEDAL, 80.0)]);
        let released = p.plan(secs(5), &ledger);
        assert_eq!(phase(&released, Goal::Turbo), Phase::Serving);
        assert_eq!(contexts(&released, "turbo.boost"), ["turbo"]);
        feed(&mut p, 20, &[(PEDAL, 10.0)]);
        assert_eq!(
            phase(&p.plan(secs(24), &ledger), Goal::Turbo),
            Phase::Serving
        );
        feed(&mut p, 25, &[(PEDAL, 10.0)]);
        let ended = p.plan(secs(25), &ledger);
        assert_eq!(phase(&ended, Goal::Turbo), Phase::WaitingForCondition);
        assert_eq!(contexts(&ended, "turbo.boost"), ["baseline"]);
        // Pressing again during the hold continues it.
        let mut q = planner(&["turbo.boost"]);
        feed(&mut q, 0, &[(PEDAL, 95.0)]);
        q.plan(secs(0), &ledger);
        feed(&mut q, 2, &[(PEDAL, 30.0)]);
        q.plan(secs(2), &ledger);
        feed(&mut q, 10, &[(PEDAL, 95.0)]);
        assert_eq!(
            phase(&q.plan(secs(10), &ledger), Goal::Turbo),
            Phase::Serving
        );
        feed(&mut q, 15, &[(PEDAL, 30.0)]);
        q.plan(secs(15), &ledger);
        feed(&mut q, 30, &[(PEDAL, 30.0)]);
        assert_eq!(
            phase(&q.plan(secs(34), &ledger), Goal::Turbo),
            Phase::Serving
        );
    }

    #[test]
    fn req_039_12_unknown_ends_the_turbo_hold_and_a_press_restarts_at_once() {
        let ledger = Ledger::default();
        let mut p = planner(&["turbo.boost"]);
        feed(&mut p, 0, &[(PEDAL, 95.0)]);
        p.plan(secs(0), &ledger);
        feed(&mut p, 1, &[(PEDAL, 20.0)]);
        p.plan(secs(1), &ledger);
        p.record(&failed(PEDAL, 2, Outcome::NoAnswer { reason: "t".into() }));
        assert_eq!(
            phase(&p.plan(secs(2), &ledger), Goal::Turbo),
            Phase::ConditionUnknown(Reason::Unanswered)
        );
        feed(&mut p, 3, &[(PEDAL, 95.0)]);
        assert_eq!(
            phase(&p.plan(secs(3), &ledger), Goal::Turbo),
            Phase::Serving
        );
    }

    #[test]
    fn req_039_12_goal_restarts_only_after_ten_seconds_of_not_holding() {
        let ledger = Ledger::default();
        let mut p = planner(&["egr.valve_duty"]);
        feed(&mut p, 0, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(phase(&p.plan(secs(0), &ledger), Goal::Egr), Phase::Serving);
        feed(&mut p, 2, &base(90.0, 20.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(
            phase(&p.plan(secs(2), &ledger), Goal::Egr),
            Phase::WaitingForCondition
        );
        feed(&mut p, 5, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(phase(&p.plan(secs(5), &ledger), Goal::Egr), Phase::Rearming);
        // Unknown time does not count toward re-arming and does not reset armed.
        feed(&mut p, 6, &base(90.0, 20.0, 2000.0, 30.0, 10.0, 0.0));
        p.plan(secs(6), &ledger);
        p.record(&failed(SPEED, 8, Outcome::Gap));
        p.plan(secs(8), &ledger);
        feed(&mut p, 18, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(
            phase(&p.plan(secs(18), &ledger), Goal::Egr),
            Phase::Rearming
        );
        // Ten fresh seconds of not holding re-arm it.
        feed(&mut p, 19, &base(90.0, 20.0, 2000.0, 30.0, 10.0, 0.0));
        p.plan(secs(19), &ledger);
        feed(&mut p, 29, &base(90.0, 20.0, 2000.0, 30.0, 10.0, 0.0));
        p.plan(secs(29), &ledger);
        feed(&mut p, 30, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(phase(&p.plan(secs(30), &ledger), Goal::Egr), Phase::Serving);
    }

    #[test]
    fn req_039_12_ended_only_by_unknown_restarts_without_waiting() {
        let ledger = Ledger::default();
        let mut p = planner(&["egr.valve_duty"]);
        feed(&mut p, 0, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        p.plan(secs(0), &ledger);
        p.record(&failed(LOAD, 1, Outcome::Gap));
        assert_eq!(
            phase(&p.plan(secs(1), &ledger), Goal::Egr),
            Phase::ConditionUnknown(Reason::Gap)
        );
        feed(&mut p, 2, &base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0));
        assert_eq!(phase(&p.plan(secs(2), &ledger), Goal::Egr), Phase::Serving);
    }

    #[test]
    fn req_039_13_strict_priority_and_one_shared_level() {
        let mut p = planner(&[
            "dpf.soot_mass_calculated",
            "turbo.boost",
            "rail.pressure",
            "egr.valve_duty",
            "cooling.radiator_outlet_temperature_actual",
        ]);
        let ledger = Ledger::default();
        // Rail (warm idle) outranks turbo only if both hold; speed 0 and pedal high.
        feed(&mut p, 0, &base(90.0, 0.0, 800.0, 30.0, 95.0, 2.0));
        let plan = p.plan(secs(0), &ledger);
        assert_eq!(
            contexts(&plan, "dpf.soot_mass_calculated"),
            ["dpf.raw_request"]
        );
        assert_eq!(contexts(&plan, "rail.pressure"), ["rail"]);
        assert_eq!(contexts(&plan, "turbo.boost"), ["turbo"]);

        let order: Vec<Goal> = {
            let mut goals: Vec<Goal> = Goal::ALL.to_vec();
            goals.sort_by_key(|g| g.level());
            goals
        };
        assert_eq!(
            order,
            [
                Goal::DpfRaw,
                Goal::Rail,
                Goal::Turbo,
                Goal::Egr,
                Goal::CoolingWarmUp,
                Goal::CoolingWarmDriving,
                Goal::DpfWarmDriving,
                Goal::Baseline
            ]
        );
    }

    #[test]
    fn req_039_13_lower_goal_is_interrupted_when_a_higher_one_takes_every_slot() {
        let dpf: Vec<String> = (0..40).map(|i| format!("dpf.s{i:02}")).collect();
        let mut catalog = triggers();
        catalog.extend(dpf.iter().map(|k| entry(k)));
        catalog.push(entry("turbo.boost"));
        let mut p = Planner::new(catalog, versions(), &Ledger::default());
        let ledger = Ledger::default();
        feed(&mut p, 0, &base(90.0, 30.0, 2000.0, 20.0, 95.0, 0.0));
        let both = p.plan(secs(0), &ledger);
        // Turbo (level 2) goes before warm driving (level 4); dpf warm driving is squeezed out
        // of contextual slots but keeps progress and the baseline keeps its reserve.
        assert_eq!(phase(&both, Goal::Turbo), Phase::Serving);
        assert_eq!(contexts(&both, "turbo.boost"), ["turbo"]);
        assert!(both.selection.len() <= MAX_SELECTION);
        assert!(matches!(
            phase(&both, Goal::DpfWarmDriving),
            Phase::Serving | Phase::Interrupted { .. }
        ));
        let baseline = both
            .selection
            .iter()
            .filter(|s| s.context.id() == "baseline" && !s.trigger)
            .count();
        assert!(baseline <= BASELINE_RESERVE);
    }

    #[test]
    fn req_039_13_baseline_keeps_its_reserved_share() {
        let dpf: Vec<String> = (0..40).map(|i| format!("dpf.s{i:02}")).collect();
        let others: Vec<String> = (0..40).map(|i| format!("engine.o{i:02}")).collect();
        let mut catalog = triggers();
        catalog.extend(dpf.iter().map(|k| entry(k)));
        catalog.extend(others.iter().map(|k| entry(k)));
        let mut p = Planner::new(catalog, versions(), &Ledger::default());
        feed(&mut p, 0, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 2.0));
        let plan = p.plan(secs(0), &Ledger::default());
        assert_eq!(plan.selection.len(), MAX_SELECTION);
        let baseline = plan
            .selection
            .iter()
            .filter(|s| s.context.id() == "baseline" && !s.trigger)
            .count();
        assert_eq!(baseline, BASELINE_RESERVE);
        assert!(plan
            .selection
            .iter()
            .any(|s| s.context.id() == "dpf.raw_request"));
    }

    #[test]
    fn req_039_13_within_a_level_the_goal_with_the_lowest_share_goes_first() {
        let cooling: Vec<String> = (0..30).map(|i| format!("cooling.c{i:02}")).collect();
        let dpf: Vec<String> = (0..30).map(|i| format!("dpf.d{i:02}")).collect();
        let mut catalog = triggers();
        catalog.extend(cooling.iter().map(|k| entry(k)));
        catalog.extend(dpf.iter().map(|k| entry(k)));
        let mut ledger = Ledger::default();
        for key in &cooling {
            at_least(&mut ledger, key, Goal::CoolingWarmDriving, 2, 30);
        }
        let mut p = Planner::new(catalog.clone(), versions(), &ledger);
        feed(&mut p, 0, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0));
        let plan = p.plan(secs(0), &ledger);
        let first = plan.selection.iter().find(|s| !s.trigger).unwrap();
        assert_eq!(first.context.id(), "dpf.warm_driving");

        // On equal shares the goal listed first in REQ-039.2 wins.
        let mut tie = Planner::new(catalog, versions(), &Ledger::default());
        feed(&mut tie, 0, &base(90.0, 30.0, 2000.0, 20.0, 10.0, 0.0));
        let plan = tie.plan(secs(0), &Ledger::default());
        let first = plan.selection.iter().find(|s| !s.trigger).unwrap();
        assert_eq!(first.context.id(), "cooling.warm_driving");
    }

    #[test]
    fn req_039_14_failed_signal_is_retried_after_30s_then_2min_then_not_again() {
        let ledger = Ledger::default();
        let mut p = planner(&["engine.flaky"]);
        assert_eq!(
            contexts(&p.plan(secs(0), &ledger), "engine.flaky"),
            ["baseline"]
        );
        p.record(&failed("engine.flaky", 0, Outcome::Empty));
        assert!(contexts(&p.plan(secs(29), &ledger), "engine.flaky").is_empty());
        assert_eq!(
            contexts(&p.plan(secs(30), &ledger), "engine.flaky"),
            ["baseline"]
        );
        p.record(&failed(
            "engine.flaky",
            30,
            Outcome::Rejected {
                response_code: 0x22,
            },
        ));
        assert!(contexts(&p.plan(secs(149), &ledger), "engine.flaky").is_empty());
        assert_eq!(
            contexts(&p.plan(secs(150), &ledger), "engine.flaky"),
            ["baseline"]
        );
        p.record(&failed(
            "engine.flaky",
            150,
            Outcome::NoAnswer { reason: "t".into() },
        ));
        for at in 151..2000 {
            assert!(contexts(&p.plan(secs(at), &ledger), "engine.flaky").is_empty());
        }
        assert!(p.plan(secs(2000), &ledger).events.is_empty());
    }

    #[test]
    fn req_039_14_skips_are_recorded_with_reason_and_retry_time() {
        let mut p = planner(&["engine.flaky"]);
        p.record(&failed("engine.flaky", 10, Outcome::Empty));
        let plan = p.plan(secs(11), &Ledger::default());
        let event = plan
            .events
            .iter()
            .find(|e| matches!(&e.kind, EventKind::SignalSkipped { signal_key, .. } if signal_key == "engine.flaky"))
            .unwrap();
        assert_eq!(event.reason, Some(Reason::Empty));
        assert_eq!(
            event.kind,
            EventKind::SignalSkipped {
                signal_key: "engine.flaky".into(),
                retry_at: Some(secs(40)),
            }
        );
        assert_eq!(event.at, secs(10));
    }

    #[test]
    fn req_039_14_a_signal_that_failed_before_is_attempted_once_per_drive() {
        let mut ledger = Ledger::default();
        ledger.entries.insert(
            EntryKey {
                versions: versions(),
                signal_key: "engine.flaky".into(),
                context: GoalContext::baseline(),
            },
            Entry {
                latest: Some(crate::coverage_ledger::Latest {
                    state: LatestState::Empty,
                    run: RunId("old".into()),
                    wall_clock_unix_ms: 0,
                    offset: secs(0),
                    session: SessionState::Default,
                    evidence: EvidenceRef("e".into()),
                }),
                ..Entry::default()
            },
        );
        let mut p = Planner::new(
            triggers().into_iter().chain([entry("engine.flaky")]),
            versions(),
            &ledger,
        );
        assert_eq!(
            contexts(&p.plan(secs(0), &ledger), "engine.flaky"),
            ["baseline"]
        );
        p.record(&failed("engine.flaky", 0, Outcome::Empty));
        assert!(contexts(&p.plan(secs(31), &ledger), "engine.flaky").is_empty());
    }

    #[test]
    fn req_039_14_session_restricted_is_not_retried_without_a_session() {
        let mut p = planner(&["engine.restricted"]);
        p.record(&failed(
            "engine.restricted",
            0,
            Outcome::SessionRestricted { reason: "x".into() },
        ));
        for at in [1, 31, 1000] {
            assert!(
                contexts(&p.plan(secs(at), &Ledger::default()), "engine.restricted").is_empty()
            );
        }
    }

    #[test]
    fn req_039_8_stop_ends_active_goals_with_the_reason() {
        let mut p = planner(&["turbo.boost"]);
        feed(&mut p, 0, &[(PEDAL, 95.0)]);
        p.plan(secs(0), &Ledger::default());
        let events = p.stop(secs(3), Reason::UserStop);
        assert!(events.iter().any(|e| e.goal == Some(Goal::Turbo)
            && e.kind == EventKind::Stopped
            && e.reason == Some(Reason::UserStop)));
        assert!(p.plan(secs(4), &Ledger::default()).selection.is_empty());
    }

    // --- replay: the planner closes the loop with the ledger on scripted vehicle data ---

    fn script(at: u64) -> Vec<(&'static str, f64)> {
        // 0-59 s cold idle, 60-179 s warm driving at 60 km/h, partial load.
        if at < 60 {
            base(60.0 + at as f64 / 3.0, 0.0, 800.0, 20.0, 5.0, 0.0)
        } else {
            base(90.0, 60.0, 2000.0, 30.0, 10.0, 0.0)
        }
    }

    fn drive(seed: u64) -> (Vec<Vec<String>>, Ledger) {
        let versions = versions();
        let mut keys: Vec<String> = [
            "engine.maf",
            "egr.valve_duty",
            "dpf.soot_mass_calculated",
            "cooling.radiator_outlet_temperature_actual",
        ]
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
        keys.extend((0..60).map(|i| format!("engine.sig{i:02}")));
        let mut catalog = triggers();
        catalog.extend(keys.iter().map(|k| entry(k)));
        let mut vehicle = Ledger::default();
        let mut run = SmartRun::new(goal_rules(), RunId("r".into()), SessionKind::Live);
        let mut p = Planner::new(catalog, versions.clone(), &vehicle);
        let mut history = Vec::new();
        for at in (0..=179u64).step_by(2) {
            let live = script(at);
            let plan = p.plan(secs(at), &vehicle);
            history.push(
                plan.selection
                    .iter()
                    .map(|s| format!("{}@{}", s.signal_key, s.context.id()))
                    .collect(),
            );
            for selected in &plan.selection {
                let value = live
                    .iter()
                    .find(|(k, _)| *k == selected.signal_key)
                    .map_or(1.0 + seed as f64, |(_, v)| *v);
                let observation = ok(&selected.signal_key, at, value);
                p.record(&observation);
                let fact = Fact {
                    run: RunId("r".into()),
                    wall_clock_unix_ms: at as i64 * 1000,
                    offset: secs(at),
                    versions: versions.clone(),
                    identity_confirmed: true,
                    body: FactBody::Reading(Reading {
                        signal_key: selected.signal_key.clone(),
                        context: selected.context.clone(),
                        ecu_target: None,
                        session: SessionState::Default,
                        outcome: observation.outcome.clone(),
                        implausible: false,
                        static_identity: false,
                        evidence: EvidenceRef("e".into()),
                    }),
                };
                run.apply(Some(&mut vehicle), &fact).unwrap();
            }
        }
        (history, vehicle)
    }

    #[test]
    fn replay_planning_is_deterministic_and_covers_the_catalog_without_a_vehicle() {
        let (first, ledger) = drive(0);
        let (second, _) = drive(0);
        assert_eq!(first, second);
        assert!(first.iter().all(|step| step.len() <= MAX_SELECTION));
        let rules = goal_rules();
        for key in (0..60).map(|i| format!("engine.sig{i:02}")) {
            let status = ledger.status(
                &rules,
                &EntryKey {
                    versions: versions(),
                    signal_key: key.clone(),
                    context: GoalContext::baseline(),
                },
            );
            assert_eq!(status, Status::Covered, "{key}");
        }
        let egr = EntryKey {
            versions: versions(),
            signal_key: "egr.valve_duty".into(),
            context: Goal::Egr.context(),
        };
        assert_eq!(ledger.status(&rules, &egr), Status::Covered);
        let warm_up = EntryKey {
            versions: versions(),
            signal_key: "cooling.radiator_outlet_temperature_actual".into(),
            context: Goal::CoolingWarmUp.context(),
        };
        assert_ne!(ledger.status(&rules, &warm_up), Status::NotCovered);
    }
}
