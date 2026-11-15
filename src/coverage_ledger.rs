//! Pure measurement-coverage ledger and counting rules (REQ-038).
//!
//! The ledger says what was measured, for how long, and what could not be measured and why, per
//! signal key and goal context. It is a statement about measurement evidence and never about
//! vehicle health or diagnostic completeness. Two ledgers exist: the run ledger of one Smart start
//! ([`SmartRun`]) and the vehicle ledger ([`Ledger`]) that accumulates over runs and drives.
//!
//! The module is plain data and a fold over recorded [`Fact`]s: no I/O, no clock (time comes only
//! from the facts' monotonic offsets), no adapter and no Knowledge access. Writing facts into a
//! recording, reading them back and persisting the vehicle ledger belong to later slices, so the
//! types carry no serialization format yet. The desktop report and Android replay call the same
//! [`SmartRun::fold`].
//!
//! Counting conventions the criteria leave open:
//! - Facts are folded in recording order. A context reading older than one already folded cannot
//!   extend freshness; a reading of the same signal and context whose offset is not later than the
//!   previous one still counts as a sample but adds no time and does not move the segment.
//! - A context signal is fresh from its latest finite, plausible reading for [`FRESHNESS_LIMIT`],
//!   whatever goal context that reading was taken in, and only for results under the same
//!   definition versions. A reading counts toward the freshness it is judged by when its own
//!   signal is one of its context's required signals. A failed, implausible or non-finite reading
//!   of the signal, and a boundary, end the freshness of readings up to its offset (and the
//!   contiguous segments that depend on it); a later reading renewed the signal after it and
//!   keeps it fresh.
//! - Such an event is an explicit break: a reading with the event's own offset that is folded
//!   after it renewed the signal after the break and never merges with the freshness the event
//!   ended, and that ended freshness no longer holds at the event's offset for a reading folded
//!   after the event. A contiguous segment never continues from a reading whose freshness the
//!   event ended into a reading at the event's offset: the time up to the event counts and the
//!   later reading starts a new segment. An event that splits an already ended freshness span
//!   leaves both parts ended. A reading stamped before an event that was already folded cannot renew
//!   freshness at all (its horizon would need the facts reordered); it is rejected, which can
//!   only lower coverage.
//! - A static identity observation is kept once per vehicle ledger, signal key, ECU target and
//!   definition versions; the ledger stores provenance only, never the value. Its entry is covered
//!   by that one observation and stays covered when a later read fails.

use std::{collections::BTreeMap, error::Error, fmt, time::Duration};

/// REQ-038.2/.4: a context reading is fresh up to this age; two readings of one signal are
/// adjacent up to this distance.
pub const FRESHNESS_LIMIT: Duration = Duration::from_secs(10);
/// REQ-038.3/.4: qualifying samples every goal and the baseline need.
pub const MIN_SAMPLES: u32 = 3;
/// REQ-038.3/.4: total valid measured time every goal and the baseline need.
pub const MIN_VALID_TIME: Duration = Duration::from_secs(30);
/// Id of the generic technical baseline context (REQ-038.3).
pub const BASELINE_CONTEXT: &str = "baseline";
/// REQ-038.3: user-visible name of the baseline. It is a technical baseline, never a statement of
/// health, completeness or a finished diagnosis.
pub const BASELINE_LABEL: &str = "Technische Basisabdeckung";

/// Goal context a signal was measured in. Goal contexts are data (REQ-039 defines the catalogue);
/// any id not given a rule is an undefined context and is judged as the generic baseline.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GoalContext(String);

impl GoalContext {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn baseline() -> Self {
        Self::new(BASELINE_CONTEXT)
    }

    pub fn id(&self) -> &str {
        &self.0
    }
}

/// One Smart start.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

/// Reference to the raw request and response evidence in the recording.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceRef(pub String);

/// REQ-038.7: the definitions a result was measured under. Progress under different versions is
/// kept apart and never credited across versions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefinitionVersions {
    pub goal_definition: u32,
    pub knowledge_revision: String,
}

/// Where the facts come from. Only a live session can credit a vehicle ledger (REQ-038.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Live,
    Demo,
    Replay,
}

/// Diagnostic session a reading was taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Default,
    Extended,
}

/// Decoded value class of a successful read. The value itself stays in the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    Numeric { finite: bool },
    Text { nonempty: bool },
}

/// Result of one read (REQ-038.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Decoded(Decoded),
    Unsupported,
    Rejected { response_code: u8 },
    Empty,
    Error { reason: String },
    NoAnswer { reason: String },
    Gap,
    SessionRestricted { reason: String },
}

/// One signal reading or read attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub signal_key: String,
    pub context: GoalContext,
    pub ecu_target: Option<String>,
    pub session: SessionState,
    pub outcome: Outcome,
    /// REQ-030.8: flagged implausible; visible, but never counted.
    pub implausible: bool,
    /// Knowledge fact "member of `uds.standard.ecu_identification`" (REQ-038.2). The module does
    /// not load Knowledge.
    pub static_identity: bool,
    pub evidence: EvidenceRef,
}

/// Events that end every open contiguous segment of the run (REQ-038.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    Gap,
    Disconnect,
    Reconnect,
    RunEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactBody {
    Reading(Reading),
    Boundary(Boundary),
}

/// One recorded fact the ledgers are folded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub run: RunId,
    pub wall_clock_unix_ms: i64,
    /// Monotonic offset from the start of the run; the only time source of the counting rules.
    pub offset: Duration,
    pub versions: DefinitionVersions,
    /// A format-valid VIN of this live session selected or created the vehicle record (REQ-038.1).
    pub identity_confirmed: bool,
    pub body: FactBody,
}

/// Counting rule of one goal context (REQ-038.3/.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRule {
    /// Signals whose fresh, qualifying readings the context needs (age at most
    /// [`FRESHNESS_LIMIT`]).
    pub required_signals: Vec<String>,
    pub min_samples: u32,
    pub min_total_time: Duration,
    /// `egr` only: valid time that must lie in one contiguous segment.
    pub min_contiguous: Option<Duration>,
}

impl ContextRule {
    pub fn baseline() -> Self {
        Self {
            required_signals: Vec::new(),
            min_samples: MIN_SAMPLES,
            min_total_time: MIN_VALID_TIME,
            min_contiguous: None,
        }
    }
}

/// Rules per goal context. Everything without an explicit rule is judged as the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    contexts: BTreeMap<GoalContext, ContextRule>,
    baseline: ContextRule,
}

impl Rules {
    /// The only built-in context: the generic technical baseline.
    pub fn generic_baseline() -> Self {
        Self {
            contexts: BTreeMap::new(),
            baseline: ContextRule::baseline(),
        }
    }

    pub fn with_context(mut self, context: GoalContext, rule: ContextRule) -> Self {
        self.contexts.insert(context, rule);
        self
    }

    pub fn rule_for(&self, context: &GoalContext) -> &ContextRule {
        self.contexts.get(context).unwrap_or(&self.baseline)
    }

    /// The technical-baseline label for the baseline and any undefined context; goal contexts are
    /// named by the goal catalogue.
    pub fn label(&self, context: &GoalContext) -> Option<&'static str> {
        (!self.contexts.contains_key(context)).then_some(BASELINE_LABEL)
    }
}

/// Key of one ledger entry: signal and context under one set of definition versions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryKey {
    pub versions: DefinitionVersions,
    pub signal_key: String,
    pub context: GoalContext,
}

/// Key of one static identity observation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdentityKey {
    pub versions: DefinitionVersions,
    pub signal_key: String,
    pub ecu_target: Option<String>,
}

/// Provenance of a static identity observation. The possibly sensitive value is not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityObservation {
    pub run: RunId,
    pub wall_clock_unix_ms: i64,
    pub evidence: EvidenceRef,
}

/// Latest availability of a signal in a context (REQ-038.5), kept apart from the accumulated
/// coverage: a failure never erases earlier valid coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatestState {
    NotYetObserved,
    Observed,
    Unsupported,
    Rejected {
        response_code: u8,
    },
    Empty,
    Error {
        reason: String,
    },
    NoAnswer {
        reason: String,
    },
    Gap,
    SessionRestricted {
        reason: String,
    },
    /// Decoded but flagged implausible: shown with a warning, not counted.
    Implausible,
    /// Decoded while a required context reading was not fresh: not counted.
    ContextNotFresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub state: LatestState,
    pub run: RunId,
    pub wall_clock_unix_ms: i64,
    pub offset: Duration,
    pub session: SessionState,
    pub evidence: EvidenceRef,
}

/// Accumulated coverage of one signal in one context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub qualifying_samples: u32,
    pub valid_time: Duration,
    pub longest_segment: Duration,
    pub latest: Option<Latest>,
}

/// Coverage of a goal context or the baseline for one signal. Never a health claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotCovered,
    Partial,
    Covered,
}

impl ContextRule {
    pub fn status(&self, entry: &Entry) -> Status {
        let covered = entry.qualifying_samples >= self.min_samples
            && entry.valid_time >= self.min_total_time
            && self
                .min_contiguous
                .is_none_or(|needed| entry.longest_segment >= needed);
        if covered {
            Status::Covered
        } else if entry.qualifying_samples > 0 || !entry.valid_time.is_zero() {
            Status::Partial
        } else {
            Status::NotCovered
        }
    }
}

/// A coverage ledger: the vehicle ledger, or the result of one [`SmartRun`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    pub entries: BTreeMap<EntryKey, Entry>,
    pub identity: BTreeMap<IdentityKey, IdentityObservation>,
}

impl Ledger {
    pub fn entry(&self, key: &EntryKey) -> Option<&Entry> {
        self.entries.get(key)
    }

    pub fn latest_state(&self, key: &EntryKey) -> LatestState {
        self.entry(key)
            .and_then(|entry| entry.latest.as_ref())
            .map_or(LatestState::NotYetObserved, |latest| latest.state.clone())
    }

    /// Coverage of an entry. A static identity entry is covered by one observation of its signal
    /// under its definition versions, at any ECU target, and stays covered when a later read
    /// fails; the dynamic thresholds apply to every other entry.
    pub fn status(&self, rules: &Rules, key: &EntryKey) -> Status {
        if self.identity_observed(key) {
            return Status::Covered;
        }
        self.entry(key).map_or(Status::NotCovered, |entry| {
            rules.rule_for(&key.context).status(entry)
        })
    }

    fn identity_observed(&self, key: &EntryKey) -> bool {
        let first = IdentityKey {
            versions: key.versions.clone(),
            signal_key: key.signal_key.clone(),
            ecu_target: None,
        };
        self.identity
            .range(first..)
            .next()
            .is_some_and(|(found, _)| {
                found.versions == key.versions && found.signal_key == key.signal_key
            })
    }

    fn record(&mut self, key: &EntryKey, update: &Update) {
        let entry = self.entries.entry(key.clone()).or_default();
        entry.qualifying_samples += u32::from(update.sample);
        entry.valid_time += update.time;
        if let Some(length) = update.segment_length {
            entry.longest_segment = entry.longest_segment.max(length);
        }
        entry.latest = Some(update.latest.clone());
    }

    fn observe_identity(&mut self, key: &IdentityKey, observation: &IdentityObservation) {
        self.identity
            .entry(key.clone())
            .or_insert_with(|| observation.clone());
    }
}

/// A fact belonged to another run than the one being folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignRun {
    pub expected: RunId,
    pub found: RunId,
}

impl fmt::Display for ForeignRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fact of run {} cannot be folded into run {}",
            self.found.0, self.expected.0
        )
    }
}

impl Error for ForeignRun {}

/// Continuous freshness of one context signal: the usable readings that renewed it and the instant
/// it holds until.
#[derive(Debug, Clone)]
struct Span {
    /// Offsets of the usable readings, ascending; the first one starts the span.
    renewals: Vec<Duration>,
    until: Duration,
    /// A failed reading or boundary cut the span: `until` is that event, not a renewal horizon.
    /// A reading with the same offset folded afterwards renewed the signal after the event and
    /// starts a span of its own, so the event stays a segment break.
    ended: bool,
}

impl Span {
    fn start(&self) -> Duration {
        self.renewals[0]
    }

    /// Whether `at` lies inside the freshness this span grants. An ended span grants nothing at
    /// the instant of its ending event: a reading folded after the event is after it.
    fn holds(&self, at: Duration) -> bool {
        self.start() <= at && (at < self.until || (!self.ended && at == self.until))
    }

    /// An event at `at` (failed reading or boundary) ends the freshness of the readings up to
    /// `at`. Readings after it renewed the signal again and stay in a span of their own.
    fn cut(self, at: Duration) -> (Self, Option<Self>) {
        let split = self.renewals.partition_point(|&renewal| renewal <= at);
        let mut kept = self.renewals;
        let later = kept.split_off(split);
        let until = self.until;
        let later = (!later.is_empty()).then_some(Self {
            renewals: later,
            until,
            // Splitting an ended span leaves the part after `at` ended by the same event.
            ended: self.ended,
        });
        let kept = Self {
            renewals: kept,
            until: until.min(at),
            ended: true,
        };
        (kept, later)
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenSegment {
    last: Duration,
    run_length: Duration,
    /// `None` until a reading with confirmed identity starts the segment for the vehicle ledger.
    vehicle_length: Option<Duration>,
}

struct Update {
    latest: Latest,
    sample: bool,
    time: Duration,
    segment_length: Option<Duration>,
}

/// The run ledger of one Smart start and the fold that also credits a vehicle ledger.
#[derive(Debug, Clone)]
pub struct SmartRun {
    rules: Rules,
    id: RunId,
    kind: SessionKind,
    ledger: Ledger,
    confirmed_since: Option<Duration>,
    open: BTreeMap<EntryKey, OpenSegment>,
    /// Per definition versions and context signal the spans of continuous freshness, ordered by
    /// start. A reading under other versions never renews or extends them.
    freshness: BTreeMap<(DefinitionVersions, String), Vec<Span>>,
    /// Latest failed reading per definition versions and context signal that was folded.
    failures: BTreeMap<(DefinitionVersions, String), Duration>,
    /// Latest boundary that was folded; it ends the freshness of every signal.
    last_boundary: Option<Duration>,
}

impl SmartRun {
    pub fn new(rules: Rules, id: RunId, kind: SessionKind) -> Self {
        Self {
            rules,
            id,
            kind,
            ledger: Ledger::default(),
            confirmed_since: None,
            open: BTreeMap::new(),
            freshness: BTreeMap::new(),
            failures: BTreeMap::new(),
            last_boundary: None,
        }
    }

    /// Re-derives a run ledger from recorded facts without any vehicle ledger (REQ-038.9 replay
    /// and report): the same counting as live.
    pub fn fold<'a>(
        rules: Rules,
        id: RunId,
        kind: SessionKind,
        facts: impl IntoIterator<Item = &'a Fact>,
    ) -> Result<Self, ForeignRun> {
        let mut run = Self::new(rules, id, kind);
        for fact in facts {
            run.apply(None, fact)?;
        }
        Ok(run)
    }

    pub fn id(&self) -> &RunId {
        &self.id
    }

    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// REQ-038.1: no observation of this run was attributed to a vehicle.
    pub fn is_session_only(&self) -> bool {
        self.confirmed_since.is_none()
    }

    /// Offset of the first live observation with confirmed identity; earlier ones stay
    /// session-only.
    pub fn confirmed_since(&self) -> Option<Duration> {
        self.confirmed_since
    }

    /// Folds one fact into the run ledger and, when the session is live and the fact carries a
    /// confirmed identity, into `vehicle`. Demo and replay never touch `vehicle` (REQ-038.1).
    pub fn apply(&mut self, vehicle: Option<&mut Ledger>, fact: &Fact) -> Result<(), ForeignRun> {
        if fact.run != self.id {
            return Err(ForeignRun {
                expected: self.id.clone(),
                found: fact.run.clone(),
            });
        }
        let credited = self.kind == SessionKind::Live && fact.identity_confirmed;
        if credited && self.confirmed_since.is_none() {
            self.confirmed_since = Some(fact.offset);
        }
        let vehicle = vehicle.filter(|_| credited);
        match &fact.body {
            FactBody::Boundary(_) => self.end_segments(fact.offset),
            FactBody::Reading(reading) => self.apply_reading(vehicle, fact, reading),
        }
        Ok(())
    }

    fn end_segments(&mut self, at: Duration) {
        self.last_boundary = self.last_boundary.max(Some(at));
        self.open.clear();
        for spans in self.freshness.values_mut() {
            Self::cut(spans, at);
        }
    }

    fn apply_reading(&mut self, mut vehicle: Option<&mut Ledger>, fact: &Fact, reading: &Reading) {
        let credited = vehicle.is_some();
        let offset = fact.offset;
        let key = EntryKey {
            versions: fact.versions.clone(),
            signal_key: reading.signal_key.clone(),
            context: reading.context.clone(),
        };
        let usable = !reading.implausible
            && match reading.outcome {
                Outcome::Decoded(Decoded::Numeric { finite }) => finite,
                Outcome::Decoded(Decoded::Text { nonempty }) => reading.static_identity && nonempty,
                _ => false,
            };
        // A usable reading renews its own signal first: when a context requires the signal it
        // belongs to, the reading counts toward the freshness it is judged by.
        if usable && !reading.static_identity {
            self.touch(&fact.versions, &reading.signal_key, offset);
        }
        let fresh = self
            .extent(&fact.versions, &reading.context, offset)
            .is_some();
        let qualifying = usable && !reading.static_identity && fresh;
        let latest = Latest {
            state: Self::state_of(reading, fresh),
            run: fact.run.clone(),
            wall_clock_unix_ms: fact.wall_clock_unix_ms,
            offset,
            session: reading.session,
            evidence: reading.evidence.clone(),
        };

        if reading.static_identity {
            if usable {
                let key = IdentityKey {
                    versions: fact.versions.clone(),
                    signal_key: reading.signal_key.clone(),
                    ecu_target: reading.ecu_target.clone(),
                };
                let observation = IdentityObservation {
                    run: fact.run.clone(),
                    wall_clock_unix_ms: fact.wall_clock_unix_ms,
                    evidence: reading.evidence.clone(),
                };
                self.ledger.observe_identity(&key, &observation);
                if let Some(vehicle) = vehicle.as_deref_mut() {
                    vehicle.observe_identity(&key, &observation);
                }
            }
            let update = Update {
                latest,
                sample: false,
                time: Duration::ZERO,
                segment_length: None,
            };
            self.ledger.record(&key, &update);
            if let Some(vehicle) = vehicle.as_deref_mut() {
                vehicle.record(&key, &update);
            }
            return;
        }

        let (run_update, vehicle_update) = if qualifying {
            self.advance_segment(&key, offset, credited, latest)
        } else {
            self.open.remove(&key);
            let update = |latest: Latest| Update {
                latest,
                sample: false,
                time: Duration::ZERO,
                segment_length: None,
            };
            (update(latest.clone()), update(latest))
        };
        self.ledger.record(&key, &run_update);
        if let Some(vehicle) = vehicle {
            vehicle.record(&key, &vehicle_update);
        }
        if !usable {
            self.invalidate(&fact.versions, &reading.signal_key, offset);
        }
    }

    /// Counts one qualifying reading and returns the updates for the run and the vehicle ledger.
    fn advance_segment(
        &mut self,
        key: &EntryKey,
        offset: Duration,
        credited: bool,
        latest: Latest,
    ) -> (Update, Update) {
        let start = OpenSegment {
            last: offset,
            run_length: Duration::ZERO,
            vehicle_length: credited.then_some(Duration::ZERO),
        };
        let mut next = start;
        let mut run = (Duration::ZERO, None);
        let mut vehicle = (Duration::ZERO, None);

        if let Some(previous) = self.open.get(key).copied() {
            if offset <= previous.last {
                // Not later than the previous reading: counted as a sample, adds no time and does
                // not move the segment.
                next = OpenSegment {
                    vehicle_length: previous.vehicle_length.or(start.vehicle_length),
                    ..previous
                };
            } else if offset - previous.last <= FRESHNESS_LIMIT {
                let interval = offset - previous.last;
                let confirmed = self
                    .extent(&key.versions, &key.context, previous.last)
                    .map_or(Duration::ZERO, |until| until.saturating_sub(previous.last));
                let counted = interval.min(confirmed);
                let run_length = previous.run_length + counted;
                run = (counted, Some(run_length));
                let vehicle_length = credited
                    .then(|| previous.vehicle_length.map(|length| length + counted))
                    .flatten();
                if let Some(length) = vehicle_length {
                    vehicle = (counted, Some(length));
                }
                if counted == interval
                    && self.holds_through(&key.versions, &key.context, previous.last, offset)
                {
                    next.run_length = run_length;
                    next.vehicle_length = vehicle_length.or(start.vehicle_length);
                }
                // Otherwise the segment ended inside or at the end of the interval: a required
                // context reading expired inside it (only the confirmed portion counted), or a
                // failure or boundary ended the freshness of the previous reading exactly at this
                // reading's offset, so a recovery at that offset is a new freshness. This reading
                // starts a new segment.
            }
        }
        self.open.insert(key.clone(), next);

        let update = |(time, segment_length): (Duration, Option<Duration>), latest| Update {
            latest,
            sample: true,
            time,
            segment_length,
        };
        (update(run, latest.clone()), update(vehicle, latest))
    }

    fn state_of(reading: &Reading, fresh: bool) -> LatestState {
        match &reading.outcome {
            Outcome::Decoded(Decoded::Numeric { finite: false }) => LatestState::Error {
                reason: "non_finite_value".into(),
            },
            Outcome::Decoded(Decoded::Text { nonempty: false }) => LatestState::Empty,
            Outcome::Decoded(Decoded::Text { .. }) if !reading.static_identity => {
                LatestState::Error {
                    reason: "not_numeric".into(),
                }
            }
            Outcome::Decoded(_) if reading.implausible => LatestState::Implausible,
            Outcome::Decoded(_) if !reading.static_identity && !fresh => {
                LatestState::ContextNotFresh
            }
            Outcome::Decoded(_) => LatestState::Observed,
            Outcome::Unsupported => LatestState::Unsupported,
            Outcome::Rejected { response_code } => LatestState::Rejected {
                response_code: *response_code,
            },
            Outcome::Empty => LatestState::Empty,
            Outcome::Error { reason } => LatestState::Error {
                reason: reason.clone(),
            },
            Outcome::NoAnswer { reason } => LatestState::NoAnswer {
                reason: reason.clone(),
            },
            Outcome::Gap => LatestState::Gap,
            Outcome::SessionRestricted { reason } => LatestState::SessionRestricted {
                reason: reason.clone(),
            },
        }
    }

    /// Instant until which every required context signal of `context` stays continuously fresh
    /// under `versions`, counted from `at`; `None` when one is not fresh at `at`.
    fn extent(
        &self,
        versions: &DefinitionVersions,
        context: &GoalContext,
        at: Duration,
    ) -> Option<Duration> {
        let mut until = Duration::MAX;
        for signal in &self.rules.rule_for(context).required_signals {
            let span = self
                .freshness
                .get(&(versions.clone(), signal.clone()))?
                .iter()
                .rev()
                .find(|span| span.holds(at))?;
            until = until.min(span.until);
        }
        Some(until)
    }

    /// Whether every required context signal of `context` is fresh at `to` through the very span
    /// that makes it fresh at `from`. A span that a failure or boundary ended at `to` does not
    /// reach `to`: what is fresh there afterwards is a new span, and a segment never joins two.
    fn holds_through(
        &self,
        versions: &DefinitionVersions,
        context: &GoalContext,
        from: Duration,
        to: Duration,
    ) -> bool {
        self.rules
            .rule_for(context)
            .required_signals
            .iter()
            .all(|signal| {
                self.freshness
                    .get(&(versions.clone(), signal.clone()))
                    .and_then(|spans| spans.iter().rev().find(|span| span.holds(from)))
                    .is_some_and(|span| span.holds(to))
            })
    }

    /// Latest failed reading of the signal or boundary that was folded.
    fn latest_break(&self, versions: &DefinitionVersions, signal: &str) -> Option<Duration> {
        let failure = self
            .failures
            .get(&(versions.clone(), signal.to_owned()))
            .copied();
        failure.max(self.last_boundary)
    }

    /// Records a finite, plausible reading of a context signal. A reading older than a failure or
    /// boundary that was already folded cannot renew anything: it belongs to freshness that the
    /// event ended, and its horizon is not known without reordering the facts. One at the
    /// event's own offset was folded after it and starts a span of its own.
    fn touch(&mut self, versions: &DefinitionVersions, signal: &str, at: Duration) {
        if self
            .latest_break(versions, signal)
            .is_some_and(|event| at < event)
        {
            return;
        }
        let spans = self
            .freshness
            .entry((versions.clone(), signal.to_owned()))
            .or_default();
        match spans.last_mut() {
            Some(last) if at < last.start() => {}
            Some(last) if !last.ended && at <= last.until => {
                let position = last.renewals.partition_point(|&renewal| renewal <= at);
                last.renewals.insert(position, at);
                last.until = last.until.max(at + FRESHNESS_LIMIT);
            }
            _ => spans.push(Span {
                renewals: vec![at],
                until: at + FRESHNESS_LIMIT,
                ended: false,
            }),
        }
    }

    /// A failed, unusable or implausible reading of a context signal ends the freshness of its
    /// earlier readings at that moment, whatever goal context the reading was taken in. Readings
    /// taken after it were not affected and keep the signal fresh. A dependent segment whose last
    /// reading has the failure's own offset has no confirmed time left to count and ends there,
    /// so a recovery with the same offset cannot bridge it; earlier ones keep the portion up to
    /// the failure that the ended span confirms.
    fn invalidate(&mut self, versions: &DefinitionVersions, signal: &str, at: Duration) {
        let key = (versions.clone(), signal.to_owned());
        self.failures
            .entry(key.clone())
            .and_modify(|latest| *latest = (*latest).max(at))
            .or_insert(at);
        if let Some(spans) = self.freshness.get_mut(&key) {
            Self::cut(spans, at);
        }
        let rules = &self.rules;
        self.open.retain(|entry, segment| {
            let depends = entry.versions == *versions
                && rules
                    .rule_for(&entry.context)
                    .required_signals
                    .iter()
                    .any(|required| required == signal);
            !(depends && segment.last == at)
        });
    }

    /// Applies an event at `at` to `spans`: spans that started after it stay as they are.
    fn cut(spans: &mut Vec<Span>, at: Duration) {
        let mut cut = Vec::with_capacity(spans.len());
        for span in spans.drain(..) {
            if span.start() > at {
                cut.push(span);
                continue;
            }
            let (kept, later) = span.cut(at);
            cut.push(kept);
            cut.extend(later);
        }
        *spans = cut;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "run-1";

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    fn versions(goal_definition: u32) -> DefinitionVersions {
        DefinitionVersions {
            goal_definition,
            knowledge_revision: "k1".into(),
        }
    }

    fn reading(signal: &str, context: &str, outcome: Outcome) -> Reading {
        Reading {
            signal_key: signal.into(),
            context: GoalContext::new(context),
            ecu_target: None,
            session: SessionState::Default,
            outcome,
            implausible: false,
            static_identity: false,
            evidence: EvidenceRef("evidence".into()),
        }
    }

    fn numeric() -> Outcome {
        Outcome::Decoded(Decoded::Numeric { finite: true })
    }

    fn fact(run: &str, at: u64, confirmed: bool, body: FactBody) -> Fact {
        Fact {
            run: RunId(run.into()),
            wall_clock_unix_ms: 1_000 * at as i64,
            offset: secs(at),
            versions: versions(1),
            identity_confirmed: confirmed,
            body,
        }
    }

    fn sample(signal: &str, context: &str, at: u64) -> Fact {
        fact(
            RUN,
            at,
            false,
            FactBody::Reading(reading(signal, context, numeric())),
        )
    }

    fn confirmed_sample(signal: &str, context: &str, at: u64) -> Fact {
        fact(
            RUN,
            at,
            true,
            FactBody::Reading(reading(signal, context, numeric())),
        )
    }

    fn outcome_fact(signal: &str, context: &str, at: u64, outcome: Outcome) -> Fact {
        fact(
            RUN,
            at,
            false,
            FactBody::Reading(reading(signal, context, outcome)),
        )
    }

    fn boundary(at: u64, boundary: Boundary) -> Fact {
        fact(RUN, at, false, FactBody::Boundary(boundary))
    }

    fn key(signal: &str, context: &str) -> EntryKey {
        EntryKey {
            versions: versions(1),
            signal_key: signal.into(),
            context: GoalContext::new(context),
        }
    }

    fn run_with(rules: Rules, kind: SessionKind, facts: &[Fact]) -> SmartRun {
        SmartRun::fold(rules, RunId(RUN.into()), kind, facts).unwrap()
    }

    fn baseline_run(facts: &[Fact]) -> SmartRun {
        run_with(Rules::generic_baseline(), SessionKind::Live, facts)
    }

    fn entry<'a>(ledger: &'a Ledger, signal: &str, context: &str) -> &'a Entry {
        ledger.entry(&key(signal, context)).unwrap()
    }

    /// Samples of one signal at the given offsets.
    fn samples(signal: &str, context: &str, offsets: &[u64]) -> Vec<Fact> {
        offsets
            .iter()
            .map(|&at| sample(signal, context, at))
            .collect()
    }

    fn egr_rules() -> Rules {
        Rules::generic_baseline().with_context(
            GoalContext::new("egr"),
            ContextRule {
                min_contiguous: Some(secs(30)),
                ..ContextRule::baseline()
            },
        )
    }

    fn cooling_rules() -> Rules {
        Rules::generic_baseline().with_context(
            GoalContext::new("cooling"),
            ContextRule {
                required_signals: vec!["vehicle.speed".into()],
                ..ContextRule::baseline()
            },
        )
    }

    #[test]
    // REQ-038.1
    fn req_038_1_run_ledger_counts_its_own_readings() {
        let run = baseline_run(&samples("engine.rpm", "baseline", &[0, 10, 20, 30]));
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(entry.qualifying_samples, 4);
        assert_eq!(entry.valid_time, secs(30));
        assert!(run.is_session_only());
    }

    #[test]
    // REQ-038.1
    fn req_038_1_vehicle_ledger_accumulates_over_runs_without_bridging_them() {
        let mut vehicle = Ledger::default();
        for (name, offsets) in [("run-1", [0, 10, 20]), ("run-2", [5, 15, 25])] {
            let mut run = SmartRun::new(
                Rules::generic_baseline(),
                RunId(name.into()),
                SessionKind::Live,
            );
            for at in offsets {
                let fact = fact(
                    name,
                    at,
                    true,
                    FactBody::Reading(reading("engine.rpm", "baseline", numeric())),
                );
                run.apply(Some(&mut vehicle), &fact).unwrap();
            }
        }
        let entry = entry(&vehicle, "engine.rpm", "baseline");
        assert_eq!(entry.qualifying_samples, 6);
        // 20 s per run; the 5 s between the last reading of run 1 and the first of run 2 is not
        // bridged.
        assert_eq!(entry.valid_time, secs(40));
        assert_eq!(entry.longest_segment, secs(20));
    }

    #[test]
    // REQ-038.1
    fn req_038_1_without_confirmed_identity_only_the_run_ledger_exists() {
        let mut vehicle = Ledger::default();
        let mut run = SmartRun::new(
            Rules::generic_baseline(),
            RunId(RUN.into()),
            SessionKind::Live,
        );
        for fact in samples("engine.rpm", "baseline", &[0, 10]) {
            run.apply(Some(&mut vehicle), &fact).unwrap();
        }
        assert_eq!(vehicle, Ledger::default());
        assert!(run.is_session_only());
        assert_eq!(
            entry(run.ledger(), "engine.rpm", "baseline").qualifying_samples,
            2
        );
    }

    #[test]
    // REQ-038.1
    fn req_038_1_identity_confirmed_mid_run_credits_only_later_readings() {
        let mut vehicle = Ledger::default();
        let mut run = SmartRun::new(
            Rules::generic_baseline(),
            RunId(RUN.into()),
            SessionKind::Live,
        );
        let facts = [
            sample("engine.rpm", "baseline", 0),
            sample("engine.rpm", "baseline", 10),
            confirmed_sample("engine.rpm", "baseline", 20),
            confirmed_sample("engine.rpm", "baseline", 30),
        ];
        for fact in &facts {
            run.apply(Some(&mut vehicle), fact).unwrap();
        }
        let in_run = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(in_run.qualifying_samples, 4);
        assert_eq!(in_run.valid_time, secs(30));
        // The interval 10 s..20 s starts at a session-only reading and is not credited.
        let credited = entry(&vehicle, "engine.rpm", "baseline");
        assert_eq!(credited.qualifying_samples, 2);
        assert_eq!(credited.valid_time, secs(10));
        assert_eq!(credited.longest_segment, secs(10));
        assert_eq!(run.confirmed_since(), Some(secs(20)));
        assert!(!run.is_session_only());
    }

    #[test]
    // REQ-038.1
    fn req_038_1_demo_and_replay_never_change_a_vehicle_ledger() {
        for kind in [SessionKind::Demo, SessionKind::Replay] {
            let mut vehicle = Ledger::default();
            let mut run = SmartRun::new(Rules::generic_baseline(), RunId(RUN.into()), kind);
            for at in [0, 10, 20] {
                run.apply(
                    Some(&mut vehicle),
                    &confirmed_sample("engine.rpm", "baseline", at),
                )
                .unwrap();
            }
            let identity = Reading {
                static_identity: true,
                outcome: Outcome::Decoded(Decoded::Text { nonempty: true }),
                ..reading("uds.ecu.hardware_number", "baseline", numeric())
            };
            run.apply(
                Some(&mut vehicle),
                &fact(RUN, 20, true, FactBody::Reading(identity)),
            )
            .unwrap();
            assert_eq!(vehicle, Ledger::default(), "{kind:?}");
            assert!(run.is_session_only(), "{kind:?}");
            assert_eq!(
                entry(run.ledger(), "engine.rpm", "baseline").qualifying_samples,
                3
            );
        }
    }

    #[test]
    // REQ-038.1
    fn req_038_1_replay_fold_is_the_same_counting_as_live() {
        let facts = samples("engine.rpm", "baseline", &[0, 10, 20]);
        let mut live = SmartRun::new(
            Rules::generic_baseline(),
            RunId(RUN.into()),
            SessionKind::Live,
        );
        for fact in &facts {
            live.apply(None, fact).unwrap();
        }
        let replay = run_with(Rules::generic_baseline(), SessionKind::Replay, &facts);
        assert_eq!(live.ledger(), replay.ledger());
    }

    #[test]
    // REQ-038.1
    fn req_038_1_facts_of_another_run_are_refused() {
        let mut run = SmartRun::new(
            Rules::generic_baseline(),
            RunId(RUN.into()),
            SessionKind::Live,
        );
        let foreign = fact(
            "run-2",
            0,
            false,
            FactBody::Reading(reading("engine.rpm", "baseline", numeric())),
        );
        assert_eq!(
            run.apply(None, &foreign),
            Err(ForeignRun {
                expected: RunId(RUN.into()),
                found: RunId("run-2".into())
            })
        );
        assert_eq!(*run.ledger(), Ledger::default());
    }

    #[test]
    // REQ-038.2
    fn req_038_2_the_same_signal_is_covered_per_context() {
        let rules = cooling_rules();
        let mut facts = samples("vehicle.speed", "baseline", &[0, 5, 10, 15, 20, 25, 30]);
        facts.extend(samples(
            "engine.coolant_temperature",
            "baseline",
            &[0, 10, 20, 30],
        ));
        facts.extend(samples("engine.coolant_temperature", "cooling", &[31]));
        let run = run_with(rules.clone(), SessionKind::Live, &facts);
        let ledger = run.ledger();
        let baseline = key("engine.coolant_temperature", "baseline");
        let cooling = key("engine.coolant_temperature", "cooling");
        assert_eq!(ledger.status(&rules, &baseline), Status::Covered);
        assert_eq!(ledger.status(&rules, &cooling), Status::Partial);
        assert_eq!(
            entry(ledger, "engine.coolant_temperature", "baseline").qualifying_samples,
            4
        );
        assert_eq!(
            entry(ledger, "engine.coolant_temperature", "cooling").qualifying_samples,
            1
        );
    }

    #[test]
    // REQ-038.2
    fn req_038_2_a_reading_with_a_stale_required_context_does_not_qualify() {
        let rules = cooling_rules();
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("engine.coolant_temperature", "cooling", 10), // age 10 s: still fresh
            sample("engine.coolant_temperature", "cooling", 21), // age 21 s: stale
        ];
        let run = run_with(rules, SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 1);
        assert_eq!(
            entry.latest.as_ref().unwrap().state,
            LatestState::ContextNotFresh
        );
    }

    #[test]
    // REQ-038.2
    fn req_038_2_an_implausible_reading_is_visible_but_not_counted() {
        let implausible = Reading {
            implausible: true,
            ..reading("engine.rpm", "baseline", numeric())
        };
        let mut facts = samples("engine.rpm", "baseline", &[0, 10]);
        facts.push(fact(RUN, 20, false, FactBody::Reading(implausible)));
        facts.push(sample("engine.rpm", "baseline", 30));
        let run = baseline_run(&facts);
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        // The implausible reading ends the segment: 0..10 counts, 20..30 does not.
        assert_eq!(entry.qualifying_samples, 3);
        assert_eq!(entry.valid_time, secs(10));
        assert_eq!(entry.longest_segment, secs(10));
        assert_eq!(entry.latest.as_ref().unwrap().state, LatestState::Observed);
        let flagged = baseline_run(&facts[..3]);
        assert_eq!(
            flagged
                .ledger()
                .latest_state(&key("engine.rpm", "baseline")),
            LatestState::Implausible
        );
    }

    #[test]
    // REQ-038.2
    fn req_038_2_only_decoded_finite_numbers_qualify_dynamically() {
        for outcome in [
            Outcome::Decoded(Decoded::Numeric { finite: false }),
            Outcome::Decoded(Decoded::Text { nonempty: true }),
        ] {
            let run = baseline_run(&[outcome_fact("engine.rpm", "baseline", 0, outcome)]);
            let entry = entry(run.ledger(), "engine.rpm", "baseline");
            assert_eq!(entry.qualifying_samples, 0);
            assert!(matches!(
                entry.latest.as_ref().unwrap().state,
                LatestState::Error { .. }
            ));
        }
    }

    fn identity_fact(signal: &str, ecu: &str, run: &str, at: u64, outcome: Outcome) -> Fact {
        let identity = Reading {
            ecu_target: Some(ecu.into()),
            static_identity: true,
            ..reading(signal, "baseline", outcome)
        };
        fact(run, at, true, FactBody::Reading(identity))
    }

    #[test]
    // REQ-038.2
    fn req_038_2_static_identity_is_observed_once_per_vehicle_without_its_value() {
        let text = || Outcome::Decoded(Decoded::Text { nonempty: true });
        let mut vehicle = Ledger::default();
        for (name, at) in [("run-1", 5), ("run-2", 9)] {
            let mut run = SmartRun::new(
                Rules::generic_baseline(),
                RunId(name.into()),
                SessionKind::Live,
            );
            let fact = identity_fact("uds.ecu.hardware_number", "engine", name, at, text());
            run.apply(Some(&mut vehicle), &fact).unwrap();
        }
        let identity_key = IdentityKey {
            versions: versions(1),
            signal_key: "uds.ecu.hardware_number".into(),
            ecu_target: Some("engine".into()),
        };
        assert_eq!(vehicle.identity.len(), 1);
        let observation = &vehicle.identity[&identity_key];
        assert_eq!(observation.run, RunId("run-1".into()));
        assert_eq!(observation.wall_clock_unix_ms, 5_000);
        // Needs neither samples nor valid time.
        let entry = entry(&vehicle, "uds.ecu.hardware_number", "baseline");
        assert_eq!(entry.qualifying_samples, 0);
        assert_eq!(entry.valid_time, Duration::ZERO);
        // Another ECU target is a separate observation.
        let mut run = SmartRun::new(
            Rules::generic_baseline(),
            RunId("run-3".into()),
            SessionKind::Live,
        );
        run.apply(
            Some(&mut vehicle),
            &identity_fact("uds.ecu.hardware_number", "gateway", "run-3", 1, text()),
        )
        .unwrap();
        assert_eq!(vehicle.identity.len(), 2);
    }

    #[test]
    // REQ-038.2
    fn req_038_2_empty_or_implausible_identity_is_not_an_observation() {
        let empty = identity_fact(
            "uds.ecu.hardware_number",
            "engine",
            RUN,
            0,
            Outcome::Decoded(Decoded::Text { nonempty: false }),
        );
        let mut flagged = identity_fact(
            "uds.ecu.software_number",
            "engine",
            RUN,
            1,
            Outcome::Decoded(Decoded::Text { nonempty: true }),
        );
        if let FactBody::Reading(reading) = &mut flagged.body {
            reading.implausible = true;
        }
        let run = baseline_run(&[empty, flagged]);
        assert!(run.ledger().identity.is_empty());
    }

    #[test]
    // REQ-038.3
    fn req_038_3_a_static_identity_entry_is_covered_by_one_observation() {
        let text = || Outcome::Decoded(Decoded::Text { nonempty: true });
        let signal = "uds.ecu.hardware_number";
        let rules = Rules::generic_baseline();
        let identity_key = key(signal, "baseline");

        let run = baseline_run(&[identity_fact(signal, "engine", RUN, 0, text())]);
        assert_eq!(run.ledger().status(&rules, &identity_key), Status::Covered);

        // A later failed read keeps the earlier observation and its coverage.
        let failed = Outcome::NoAnswer {
            reason: "timeout".into(),
        };
        let run = baseline_run(&[
            identity_fact(signal, "engine", RUN, 0, text()),
            identity_fact(signal, "engine", RUN, 5, failed.clone()),
        ]);
        assert_eq!(run.ledger().status(&rules, &identity_key), Status::Covered);
        assert_eq!(
            run.ledger().latest_state(&identity_key),
            LatestState::NoAnswer {
                reason: "timeout".into()
            }
        );

        // Observed in a vehicle ledger over two runs, the second run only failing.
        let mut vehicle = Ledger::default();
        for (name, outcome) in [("run-1", text()), ("run-2", failed)] {
            let mut run = SmartRun::new(rules.clone(), RunId(name.into()), SessionKind::Live);
            run.apply(
                Some(&mut vehicle),
                &identity_fact(signal, "engine", name, 1, outcome),
            )
            .unwrap();
        }
        assert_eq!(vehicle.status(&rules, &identity_key), Status::Covered);

        // Without an observation nothing is covered, and another version is separate.
        let run = baseline_run(&[
            identity_fact(
                signal,
                "engine",
                RUN,
                0,
                Outcome::Decoded(Decoded::Text { nonempty: false }),
            ),
            versioned(2, identity_fact(signal, "engine", RUN, 1, text())),
        ]);
        assert_eq!(
            run.ledger().status(&rules, &identity_key),
            Status::NotCovered
        );
        assert_eq!(
            run.ledger()
                .status(&rules, &keyed(versions(2), signal, "baseline")),
            Status::Covered
        );
    }

    #[test]
    // REQ-038.2
    fn req_038_2_every_outcome_keeps_key_context_run_and_session() {
        let restricted = Reading {
            session: SessionState::Extended,
            ..reading(
                "engine.rail_pressure",
                "rail",
                Outcome::SessionRestricted {
                    reason: "extended_session".into(),
                },
            )
        };
        let run = baseline_run(&[fact(RUN, 7, false, FactBody::Reading(restricted))]);
        let latest = entry(run.ledger(), "engine.rail_pressure", "rail")
            .latest
            .clone()
            .unwrap();
        assert_eq!(latest.run, RunId(RUN.into()));
        assert_eq!(latest.offset, secs(7));
        assert_eq!(latest.session, SessionState::Extended);
        assert_eq!(
            latest.state,
            LatestState::SessionRestricted {
                reason: "extended_session".into()
            }
        );
        // Not merged into another context of the same signal.
        assert!(run
            .ledger()
            .entry(&key("engine.rail_pressure", "baseline"))
            .is_none());
    }

    fn baseline_status(offsets: &[u64]) -> Status {
        let run = baseline_run(&samples("engine.rpm", "baseline", offsets));
        run.ledger()
            .status(&Rules::generic_baseline(), &key("engine.rpm", "baseline"))
    }

    #[test]
    // REQ-038.3
    fn req_038_3_baseline_needs_three_samples_and_thirty_seconds() {
        assert_eq!(baseline_status(&[]), Status::NotCovered);
        assert_eq!(baseline_status(&[0, 10, 20]), Status::Partial); // 20 s
        assert_eq!(baseline_status(&[0, 30]), Status::Partial); // 2 samples, no 10 s steps: 0 s
        assert_eq!(baseline_status(&[0, 10, 20, 30]), Status::Covered);
    }

    #[test]
    // REQ-038.3
    fn req_038_3_baseline_does_not_need_continuity() {
        let mut facts = samples("engine.rpm", "baseline", &[0, 10, 15]);
        facts.push(boundary(16, Boundary::Disconnect));
        facts.extend(samples("engine.rpm", "baseline", &[100, 110, 120]));
        let run = baseline_run(&facts);
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(entry.valid_time, secs(35));
        assert_eq!(entry.longest_segment, secs(20));
        assert_eq!(
            run.ledger()
                .status(&Rules::generic_baseline(), &key("engine.rpm", "baseline")),
            Status::Covered
        );
    }

    #[test]
    // REQ-038.3
    fn req_038_3_gap_and_failed_readings_never_add_time() {
        for failure in [
            Outcome::Gap,
            Outcome::Unsupported,
            Outcome::Rejected {
                response_code: 0x31,
            },
            Outcome::Empty,
            Outcome::NoAnswer {
                reason: "timeout".into(),
            },
        ] {
            let mut facts = samples("engine.rpm", "baseline", &[0, 5]);
            facts.push(outcome_fact("engine.rpm", "baseline", 10, failure.clone()));
            facts.extend(samples("engine.rpm", "baseline", &[15, 20]));
            let run = baseline_run(&facts);
            let entry = entry(run.ledger(), "engine.rpm", "baseline");
            assert_eq!(entry.valid_time, secs(10), "{failure:?}");
            assert_eq!(entry.qualifying_samples, 4, "{failure:?}");
            assert_eq!(entry.longest_segment, secs(5), "{failure:?}");
        }
    }

    #[test]
    // REQ-038.3
    fn req_038_3_undefined_context_is_judged_as_the_technical_baseline() {
        let rules = egr_rules();
        let run = run_with(
            rules.clone(),
            SessionKind::Live,
            &samples("engine.rpm", "unknown-goal", &[0, 10, 20, 30]),
        );
        assert_eq!(
            run.ledger()
                .status(&rules, &key("engine.rpm", "unknown-goal")),
            Status::Covered
        );
        assert_eq!(
            rules.label(&GoalContext::new("unknown-goal")),
            Some(BASELINE_LABEL)
        );
        assert_eq!(rules.label(&GoalContext::baseline()), Some(BASELINE_LABEL));
        assert_eq!(rules.label(&GoalContext::new("egr")), None);
    }

    #[test]
    // REQ-038.3
    fn req_038_3_the_baseline_label_makes_no_health_claim() {
        assert!(BASELINE_LABEL.contains("Technische"));
        for claim in ["gesund", "vollständig", "fertig", "Diagnose", "ok"] {
            assert!(
                !BASELINE_LABEL
                    .to_lowercase()
                    .contains(&claim.to_lowercase()),
                "{claim}"
            );
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_adjacent_readings_count_up_to_ten_seconds_only() {
        let run = baseline_run(&samples("engine.rpm", "baseline", &[0, 10, 21, 31]));
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        // 0..10 counts, 10..21 (11 s) does not and ends the segment, 21..31 counts.
        assert_eq!(entry.valid_time, secs(20));
        assert_eq!(entry.longest_segment, secs(10));
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_single_reading_adds_no_time() {
        let run = baseline_run(&samples("engine.rpm", "baseline", &[0]));
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(entry.qualifying_samples, 1);
        assert_eq!(entry.valid_time, Duration::ZERO);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_non_advancing_offsets_add_no_time() {
        let run = baseline_run(&samples("engine.rpm", "baseline", &[10, 10, 5, 20]));
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(entry.qualifying_samples, 4);
        assert_eq!(entry.valid_time, secs(10));
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_context_reading_expiring_inside_an_interval_limits_the_counted_time() {
        let rules = cooling_rules();
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("engine.coolant_temperature", "cooling", 4),
            // Speed expires at 10 s; the interval 4..13 counts only until then.
            sample("engine.coolant_temperature", "cooling", 13),
            sample("vehicle.speed", "baseline", 14),
            sample("engine.coolant_temperature", "cooling", 20),
        ];
        let run = run_with(rules, SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        // 13 s reading is itself taken with a stale speed: not a sample and the segment ended.
        assert_eq!(entry.qualifying_samples, 2);
        assert_eq!(entry.valid_time, Duration::ZERO);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_the_confirmed_portion_counts_when_the_context_is_renewed_after_expiry() {
        let rules = cooling_rules();
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("engine.coolant_temperature", "cooling", 4),
            sample("vehicle.speed", "baseline", 13),
            // Interval 4..13 is 9 s long; speed was fresh until 10 s: only 6 s are confirmed.
            sample("engine.coolant_temperature", "cooling", 13),
            sample("engine.coolant_temperature", "cooling", 20),
        ];
        let run = run_with(rules, SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 3);
        assert_eq!(entry.valid_time, secs(6) + secs(7));
        // The segment ended at the expiry, so the longest one is 7 s.
        assert_eq!(entry.longest_segment, secs(7));
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_renewed_context_inside_the_interval_keeps_the_segment() {
        let rules = cooling_rules();
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("engine.coolant_temperature", "cooling", 4),
            sample("vehicle.speed", "baseline", 9),
            sample("engine.coolant_temperature", "cooling", 13),
            sample("vehicle.speed", "baseline", 18),
            sample("engine.coolant_temperature", "cooling", 22),
        ];
        let run = run_with(rules, SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.valid_time, secs(18));
        assert_eq!(entry.longest_segment, secs(18));
    }

    #[test]
    // REQ-038.4
    fn req_038_4_boundaries_end_the_segment_and_never_bridge() {
        for kind in [
            Boundary::Gap,
            Boundary::Disconnect,
            Boundary::Reconnect,
            Boundary::RunEnd,
        ] {
            let mut facts = samples("engine.rpm", "baseline", &[0, 5]);
            facts.push(boundary(6, kind));
            facts.extend(samples("engine.rpm", "baseline", &[8, 13]));
            let run = baseline_run(&facts);
            let entry = entry(run.ledger(), "engine.rpm", "baseline");
            assert_eq!(entry.valid_time, secs(10), "{kind:?}");
            assert_eq!(entry.longest_segment, secs(5), "{kind:?}");
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_boundary_ends_context_freshness_too() {
        let rules = cooling_rules();
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            boundary(2, Boundary::Disconnect),
            sample("engine.coolant_temperature", "cooling", 4),
        ];
        let run = run_with(rules, SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 0);
    }

    /// A failed, implausible or non-finite reading of a context signal at `at` seconds.
    fn invalid_speed_readings(at: u64) -> Vec<Fact> {
        let implausible = Reading {
            implausible: true,
            ..reading("vehicle.speed", "baseline", numeric())
        };
        vec![
            outcome_fact(
                "vehicle.speed",
                "baseline",
                at,
                Outcome::NoAnswer {
                    reason: "timeout".into(),
                },
            ),
            outcome_fact("vehicle.speed", "baseline", at, Outcome::Unsupported),
            outcome_fact("vehicle.speed", "baseline", at, Outcome::Gap),
            outcome_fact(
                "vehicle.speed",
                "baseline",
                at,
                Outcome::Decoded(Decoded::Numeric { finite: false }),
            ),
            fact(RUN, at, false, FactBody::Reading(implausible)),
            // Any goal context of the signal ends its freshness.
            outcome_fact(
                "vehicle.speed",
                "cooling",
                at,
                Outcome::Rejected {
                    response_code: 0x31,
                },
            ),
        ]
    }

    #[test]
    // REQ-038.4
    fn req_038_4_an_invalid_context_reading_ends_its_freshness_and_the_segment() {
        for invalid in invalid_speed_readings(3) {
            let facts = [
                sample("vehicle.speed", "baseline", 0),
                sample("engine.coolant_temperature", "cooling", 1),
                invalid.clone(),
                // Speed is measured again only at 7 s: 3 s..7 s was never confirmed.
                sample("vehicle.speed", "baseline", 7),
                sample("engine.coolant_temperature", "cooling", 8),
            ];
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 2, "{invalid:?}");
            // Only 1 s..3 s counts; the segment ended at the invalid reading.
            assert_eq!(entry.valid_time, secs(2), "{invalid:?}");
            assert_eq!(entry.longest_segment, secs(2), "{invalid:?}");
        }
    }

    #[test]
    // REQ-038.2
    fn req_038_2_a_reading_after_an_invalid_context_reading_is_not_fresh() {
        for invalid in invalid_speed_readings(3) {
            let facts = [
                sample("vehicle.speed", "baseline", 0),
                invalid.clone(),
                sample("engine.coolant_temperature", "cooling", 4),
            ];
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 0, "{invalid:?}");
            assert_eq!(
                entry.latest.as_ref().unwrap().state,
                LatestState::ContextNotFresh,
                "{invalid:?}"
            );
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_an_older_invalid_reading_cannot_end_later_freshness() {
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("vehicle.speed", "baseline", 20),
            outcome_fact("vehicle.speed", "baseline", 3, Outcome::Unsupported),
            sample("engine.coolant_temperature", "cooling", 21),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 1);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_later_renewal_survives_an_older_failed_reading_in_an_overlapping_span() {
        // Speed is measured at 0 s and 9 s; the failed reading stamped 3 s arrives afterwards. It
        // ends the freshness of the 0 s reading only: the 9 s renewal keeps speed fresh at 10 s.
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("vehicle.speed", "baseline", 9),
            outcome_fact("vehicle.speed", "baseline", 3, Outcome::Unsupported),
            sample("engine.coolant_temperature", "cooling", 10),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let renewed = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(renewed.qualifying_samples, 1);
        assert_eq!(
            renewed.latest.as_ref().unwrap().state,
            LatestState::Observed
        );

        // The failure still ends the older reading: speed was not fresh between 3 s and 9 s.
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("vehicle.speed", "baseline", 9),
            outcome_fact("vehicle.speed", "baseline", 3, Outcome::Unsupported),
            sample("engine.coolant_temperature", "cooling", 5),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 0);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_later_renewal_survives_an_older_boundary() {
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("vehicle.speed", "baseline", 9),
            boundary(3, Boundary::Gap),
            sample("engine.coolant_temperature", "cooling", 10),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 1);
    }

    /// Cooling needs 30 s of valid time in one segment while speed stays fresh.
    fn continuous_cooling_rules() -> Rules {
        Rules::generic_baseline().with_context(
            GoalContext::new("cooling"),
            ContextRule {
                required_signals: vec!["vehicle.speed".into()],
                min_contiguous: Some(secs(30)),
                ..ContextRule::baseline()
            },
        )
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_context_failure_and_recovery_at_one_timestamp_keep_separate_segments() {
        let cooling = key("engine.coolant_temperature", "cooling");
        for invalid in invalid_speed_readings(20) {
            let facts = [
                sample("vehicle.speed", "baseline", 0),
                sample("engine.coolant_temperature", "cooling", 1),
                sample("vehicle.speed", "baseline", 9),
                sample("engine.coolant_temperature", "cooling", 9),
                sample("vehicle.speed", "baseline", 18),
                sample("engine.coolant_temperature", "cooling", 19),
                // Speed fails and is measured again within the same second: two freshness spans.
                invalid.clone(),
                sample("vehicle.speed", "baseline", 20),
                // 19 s..26 s crosses the failure: only 19 s..20 s is confirmed.
                sample("engine.coolant_temperature", "cooling", 26),
                sample("vehicle.speed", "baseline", 28),
                sample("engine.coolant_temperature", "cooling", 32),
            ];
            let rules = continuous_cooling_rules();
            let run = run_with(rules.clone(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 5, "{invalid:?}");
            assert_eq!(entry.valid_time, secs(8 + 10 + 1 + 6), "{invalid:?}");
            assert_eq!(entry.longest_segment, secs(19), "{invalid:?}");
            assert_eq!(
                run.ledger().status(&rules, &cooling),
                Status::Partial,
                "{invalid:?}"
            );
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_failure_and_recovery_at_the_timestamp_of_the_last_reading_end_its_segment() {
        for invalid in invalid_speed_readings(20) {
            let facts = [
                sample("vehicle.speed", "baseline", 10),
                sample("engine.coolant_temperature", "cooling", 12),
                sample("engine.coolant_temperature", "cooling", 20),
                invalid.clone(),
                sample("vehicle.speed", "baseline", 20),
                // 20 s..26 s lies behind the failure: a new segment starts at 26 s.
                sample("engine.coolant_temperature", "cooling", 26),
                sample("engine.coolant_temperature", "cooling", 30),
            ];
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 4, "{invalid:?}");
            assert_eq!(entry.valid_time, secs(8 + 4), "{invalid:?}");
            assert_eq!(entry.longest_segment, secs(8), "{invalid:?}");
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_context_failure_at_an_interval_endpoint_does_not_bridge_the_segment() {
        let cooling = key("engine.coolant_temperature", "cooling");
        for invalid in invalid_speed_readings(6) {
            let facts = [
                sample("vehicle.speed", "baseline", 0),
                sample("engine.coolant_temperature", "cooling", 1),
                sample("engine.coolant_temperature", "cooling", 4),
                // Speed fails and is measured again at the second the next reading closes the
                // interval 4 s..6 s: that interval is confirmed up to the failure, the segment
                // ends there, and what follows is a segment of its own.
                invalid.clone(),
                sample("vehicle.speed", "baseline", 6),
                sample("engine.coolant_temperature", "cooling", 6),
                sample("engine.coolant_temperature", "cooling", 9),
                sample("engine.coolant_temperature", "cooling", 12),
            ];
            let rules = continuous_cooling_rules();
            let run = run_with(rules.clone(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 5, "{invalid:?}");
            assert_eq!(entry.valid_time, secs(3 + 2 + 3 + 3), "{invalid:?}");
            assert_eq!(entry.longest_segment, secs(6), "{invalid:?}");
            assert_eq!(
                run.ledger().status(&rules, &cooling),
                Status::Partial,
                "{invalid:?}"
            );
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_splitting_an_ended_span_keeps_its_end_marker() {
        let mut breaks = invalid_speed_readings(3);
        breaks.push(boundary(3, Boundary::Disconnect));
        for earlier in breaks {
            let facts = [
                sample("vehicle.speed", "baseline", 0),
                sample("vehicle.speed", "baseline", 5),
                // The failure at 8 s ends the span of both readings ...
                outcome_fact(
                    "vehicle.speed",
                    "baseline",
                    8,
                    Outcome::NoAnswer {
                        reason: "timeout".into(),
                    },
                ),
                // ... and an older event folded afterwards splits it: the part after 3 s is
                // still ended at 8 s.
                earlier.clone(),
                sample("engine.coolant_temperature", "cooling", 8),
            ];
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 0, "{earlier:?}");
            assert_eq!(
                entry.latest.as_ref().unwrap().state,
                LatestState::ContextNotFresh,
                "{earlier:?}"
            );
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_boundary_and_a_renewal_at_one_timestamp_keep_separate_segments() {
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            sample("engine.coolant_temperature", "cooling", 1),
            sample("engine.coolant_temperature", "cooling", 4),
            boundary(5, Boundary::Reconnect),
            sample("vehicle.speed", "baseline", 5),
            sample("engine.coolant_temperature", "cooling", 8),
            sample("engine.coolant_temperature", "cooling", 12),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.valid_time, secs(3 + 4));
        assert_eq!(entry.longest_segment, secs(4));
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_reading_at_the_timestamp_of_a_context_failure_folded_before_it_is_not_fresh() {
        for invalid in invalid_speed_readings(20) {
            let facts = [
                sample("vehicle.speed", "baseline", 15),
                invalid.clone(),
                sample("engine.coolant_temperature", "cooling", 20),
            ];
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 0, "{invalid:?}");
            assert_eq!(
                entry.latest.as_ref().unwrap().state,
                LatestState::ContextNotFresh,
                "{invalid:?}"
            );
        }

        // The recovery stamped with the failure's second keeps speed fresh, and a failure folded
        // after the recovery ends it again.
        let renewed = [
            sample("vehicle.speed", "baseline", 15),
            outcome_fact("vehicle.speed", "baseline", 20, Outcome::Unsupported),
            sample("vehicle.speed", "baseline", 20),
            sample("engine.coolant_temperature", "cooling", 22),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &renewed);
        let after_recovery = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(after_recovery.qualifying_samples, 1);

        let failed_again = [
            sample("vehicle.speed", "baseline", 20),
            outcome_fact("vehicle.speed", "baseline", 20, Outcome::Unsupported),
            sample("engine.coolant_temperature", "cooling", 22),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &failed_again);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 0);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_an_older_success_folded_after_a_failure_cannot_extend_freshness_past_it() {
        let cooling = |at| sample("engine.coolant_temperature", "cooling", at);
        let speed = |at| sample("vehicle.speed", "baseline", at);
        for invalid in invalid_speed_readings(8) {
            let orders = [
                (
                    "success folded first",
                    vec![speed(5), invalid.clone(), cooling(12)],
                ),
                (
                    "failure folded first",
                    vec![invalid.clone(), speed(5), cooling(12)],
                ),
                (
                    "failure folded before an older success of an open span",
                    vec![speed(0), invalid.clone(), speed(5), cooling(12)],
                ),
                (
                    "success folded first in an open span",
                    vec![speed(0), speed(5), invalid.clone(), cooling(12)],
                ),
            ];
            for (order, facts) in orders {
                let run = run_with(cooling_rules(), SessionKind::Live, &facts);
                let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
                assert_eq!(entry.qualifying_samples, 0, "{order}: {invalid:?}");
                assert_eq!(
                    entry.latest.as_ref().unwrap().state,
                    LatestState::ContextNotFresh,
                    "{order}: {invalid:?}"
                );
            }

            // A reading taken after the failure renews speed in either order.
            let orders = [
                (
                    "success folded first",
                    vec![speed(5), invalid.clone(), speed(9)],
                ),
                (
                    "failure folded first",
                    vec![invalid.clone(), speed(5), speed(9)],
                ),
            ];
            for (order, mut facts) in orders {
                facts.push(cooling(12));
                let run = run_with(cooling_rules(), SessionKind::Live, &facts);
                let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
                assert_eq!(entry.qualifying_samples, 1, "{order}: {invalid:?}");
                assert_eq!(
                    entry.latest.as_ref().unwrap().state,
                    LatestState::Observed,
                    "{order}: {invalid:?}"
                );
            }
        }
    }

    #[test]
    // REQ-038.4
    fn req_038_4_an_older_success_folded_after_a_boundary_cannot_extend_freshness_past_it() {
        let cooling = |at| sample("engine.coolant_temperature", "cooling", at);
        let speed = |at| sample("vehicle.speed", "baseline", at);
        let gap = || boundary(8, Boundary::Gap);
        let orders = [
            ("success folded first", vec![speed(5), gap(), cooling(12)]),
            ("boundary folded first", vec![gap(), speed(5), cooling(12)]),
            (
                "boundary folded before an older success of an open span",
                vec![speed(0), gap(), speed(5), cooling(12)],
            ),
        ];
        for (order, facts) in orders {
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
            assert_eq!(entry.qualifying_samples, 0, "{order}");
        }

        let renewed = [gap(), speed(5), speed(9), cooling(12)];
        let run = run_with(cooling_rules(), SessionKind::Live, &renewed);
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 1);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_a_signal_required_by_its_own_context_counts_its_current_reading() {
        let rules = Rules::generic_baseline().with_context(
            GoalContext::new("cooling"),
            ContextRule {
                required_signals: vec!["engine.coolant_temperature".into()],
                min_contiguous: Some(secs(30)),
                ..ContextRule::baseline()
            },
        );
        let run = run_with(
            rules.clone(),
            SessionKind::Live,
            &samples("engine.coolant_temperature", "cooling", &[0, 10, 20, 30]),
        );
        let own = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(own.qualifying_samples, 4);
        assert_eq!(own.valid_time, secs(30));
        assert_eq!(own.longest_segment, secs(30));
        assert_eq!(
            run.ledger()
                .status(&rules, &key("engine.coolant_temperature", "cooling")),
            Status::Covered
        );

        // An unusable own reading is never counted toward its own freshness.
        let run = run_with(
            rules,
            SessionKind::Live,
            &[outcome_fact(
                "engine.coolant_temperature",
                "cooling",
                0,
                Outcome::Decoded(Decoded::Numeric { finite: false }),
            )],
        );
        let entry = entry(run.ledger(), "engine.coolant_temperature", "cooling");
        assert_eq!(entry.qualifying_samples, 0);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_egr_needs_thirty_seconds_in_one_segment() {
        let rules = egr_rules();
        let egr = key("engine.egr_position", "egr");

        let mut split = samples("engine.egr_position", "egr", &[0, 10, 20]);
        split.push(boundary(21, Boundary::Gap));
        split.extend(samples("engine.egr_position", "egr", &[100, 110, 120]));
        let run = run_with(rules.clone(), SessionKind::Live, &split);
        assert_eq!(
            entry(run.ledger(), "engine.egr_position", "egr").valid_time,
            secs(40)
        );
        assert_eq!(run.ledger().status(&rules, &egr), Status::Partial);

        let run = run_with(
            rules.clone(),
            SessionKind::Live,
            &samples("engine.egr_position", "egr", &[0, 10, 20, 30]),
        );
        assert_eq!(run.ledger().status(&rules, &egr), Status::Covered);

        // The other contexts use cumulative time and need no continuity.
        let mut split_baseline = samples("engine.egr_position", "baseline", &[0, 10, 20]);
        split_baseline.push(boundary(21, Boundary::Gap));
        split_baseline.extend(samples("engine.egr_position", "baseline", &[100, 110, 120]));
        let run = run_with(rules.clone(), SessionKind::Live, &split_baseline);
        assert_eq!(
            run.ledger()
                .status(&rules, &key("engine.egr_position", "baseline")),
            Status::Covered
        );
    }

    #[test]
    // REQ-038.4
    fn req_038_4_egr_longest_segment_survives_in_the_vehicle_ledger() {
        let rules = egr_rules();
        let mut vehicle = Ledger::default();
        for (name, offsets) in [("run-1", [0, 10, 20, 30]), ("run-2", [0, 10, 20, 25])] {
            let mut run = SmartRun::new(rules.clone(), RunId(name.into()), SessionKind::Live);
            for at in offsets {
                let fact = fact(
                    name,
                    at,
                    true,
                    FactBody::Reading(reading("engine.egr_position", "egr", numeric())),
                );
                run.apply(Some(&mut vehicle), &fact).unwrap();
            }
        }
        let entry = entry(&vehicle, "engine.egr_position", "egr");
        assert_eq!(entry.valid_time, secs(55));
        assert_eq!(entry.longest_segment, secs(30));
    }

    #[test]
    // REQ-038.5
    fn req_038_5_each_availability_state_is_reported_separately() {
        let cases = [
            (Outcome::Unsupported, LatestState::Unsupported),
            (
                Outcome::Rejected {
                    response_code: 0x22,
                },
                LatestState::Rejected {
                    response_code: 0x22,
                },
            ),
            (Outcome::Empty, LatestState::Empty),
            (
                Outcome::Error {
                    reason: "decode".into(),
                },
                LatestState::Error {
                    reason: "decode".into(),
                },
            ),
            (
                Outcome::NoAnswer {
                    reason: "timeout".into(),
                },
                LatestState::NoAnswer {
                    reason: "timeout".into(),
                },
            ),
            (Outcome::Gap, LatestState::Gap),
            (
                Outcome::SessionRestricted {
                    reason: "needs_extended".into(),
                },
                LatestState::SessionRestricted {
                    reason: "needs_extended".into(),
                },
            ),
            (numeric(), LatestState::Observed),
        ];
        for (outcome, expected) in cases {
            let run = baseline_run(&[outcome_fact("engine.rpm", "baseline", 0, outcome)]);
            assert_eq!(
                run.ledger().latest_state(&key("engine.rpm", "baseline")),
                expected
            );
        }
        let run = baseline_run(&[]);
        assert_eq!(
            run.ledger().latest_state(&key("engine.rpm", "baseline")),
            LatestState::NotYetObserved
        );
    }

    #[test]
    // REQ-038.5
    fn req_038_5_a_failure_adds_nothing_and_keeps_earlier_coverage_visible() {
        let mut facts = samples("engine.rpm", "baseline", &[0, 10, 20, 30]);
        facts.push(outcome_fact(
            "engine.rpm",
            "baseline",
            40,
            Outcome::NoAnswer {
                reason: "timeout".into(),
            },
        ));
        let run = baseline_run(&facts);
        let entry = entry(run.ledger(), "engine.rpm", "baseline");
        assert_eq!(entry.qualifying_samples, 4);
        assert_eq!(entry.valid_time, secs(30));
        assert_eq!(
            run.ledger()
                .status(&Rules::generic_baseline(), &key("engine.rpm", "baseline")),
            Status::Covered
        );
        assert_eq!(
            entry.latest.as_ref().unwrap().state,
            LatestState::NoAnswer {
                reason: "timeout".into()
            }
        );
    }

    fn versioned(version: u32, mut fact: Fact) -> Fact {
        fact.versions = versions(version);
        fact
    }

    #[test]
    // REQ-038.7
    fn req_038_7_progress_under_an_older_definition_stays_separate() {
        let mut facts: Vec<Fact> = samples("engine.rpm", "baseline", &[0, 10, 20, 30]);
        facts.extend(
            samples("engine.rpm", "baseline", &[40, 50])
                .into_iter()
                .map(|fact| versioned(2, fact)),
        );
        let run = baseline_run(&facts);
        let old = key("engine.rpm", "baseline");
        let new = EntryKey {
            versions: versions(2),
            ..old.clone()
        };
        assert_eq!(
            run.ledger().status(&Rules::generic_baseline(), &old),
            Status::Covered
        );
        assert_eq!(
            run.ledger().status(&Rules::generic_baseline(), &new),
            Status::Partial
        );
        // The old progress is never credited to the new version.
        assert_eq!(run.ledger().entry(&old).unwrap().qualifying_samples, 4);
        assert_eq!(run.ledger().entry(&new).unwrap().qualifying_samples, 2);
        assert_eq!(run.ledger().entry(&new).unwrap().valid_time, secs(10));
    }

    #[test]
    // REQ-038.7
    fn req_038_7_a_new_knowledge_revision_starts_separate_progress() {
        let mut facts = samples("engine.rpm", "baseline", &[0, 10]);
        let mut newer = sample("engine.rpm", "baseline", 20);
        newer.versions.knowledge_revision = "k2".into();
        facts.push(newer);
        let run = baseline_run(&facts);
        let k1 = key("engine.rpm", "baseline");
        let k2 = EntryKey {
            versions: DefinitionVersions {
                goal_definition: 1,
                knowledge_revision: "k2".into(),
            },
            ..k1.clone()
        };
        assert_eq!(run.ledger().entry(&k1).unwrap().valid_time, secs(10));
        assert_eq!(run.ledger().entry(&k2).unwrap().qualifying_samples, 1);
        assert_eq!(run.ledger().entry(&k2).unwrap().valid_time, Duration::ZERO);
    }

    #[test]
    // REQ-038.7
    fn req_038_7_static_identity_is_observed_per_version() {
        let text = || Outcome::Decoded(Decoded::Text { nonempty: true });
        let first = identity_fact("uds.ecu.hardware_number", "engine", RUN, 0, text());
        let second = versioned(
            2,
            identity_fact("uds.ecu.hardware_number", "engine", RUN, 1, text()),
        );
        let run = baseline_run(&[first, second]);
        assert_eq!(run.ledger().identity.len(), 2);
    }

    fn knowledge(revision: &str, mut fact: Fact) -> Fact {
        fact.versions.knowledge_revision = revision.into();
        fact
    }

    fn keyed(versions: DefinitionVersions, signal: &str, context: &str) -> EntryKey {
        EntryKey {
            versions,
            signal_key: signal.into(),
            context: GoalContext::new(context),
        }
    }

    #[test]
    // REQ-038.7
    fn req_038_7_a_context_reading_of_an_older_goal_definition_is_not_fresh_for_a_new_one() {
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            versioned(2, sample("engine.coolant_temperature", "cooling", 4)),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let new = keyed(versions(2), "engine.coolant_temperature", "cooling");
        let entry = run.ledger().entry(&new).unwrap();
        assert_eq!(entry.qualifying_samples, 0);
        assert_eq!(
            entry.latest.as_ref().unwrap().state,
            LatestState::ContextNotFresh
        );
    }

    #[test]
    // REQ-038.7
    fn req_038_7_a_context_reading_of_an_older_knowledge_revision_is_not_fresh_for_a_new_one() {
        let facts = [
            sample("vehicle.speed", "baseline", 0),
            knowledge("k2", sample("engine.coolant_temperature", "cooling", 4)),
        ];
        let run = run_with(cooling_rules(), SessionKind::Live, &facts);
        let new = keyed(
            DefinitionVersions {
                goal_definition: 1,
                knowledge_revision: "k2".into(),
            },
            "engine.coolant_temperature",
            "cooling",
        );
        let entry = run.ledger().entry(&new).unwrap();
        assert_eq!(entry.qualifying_samples, 0);
        assert_eq!(
            entry.latest.as_ref().unwrap().state,
            LatestState::ContextNotFresh
        );
    }

    #[test]
    // REQ-038.7
    fn req_038_7_time_after_a_definition_change_counts_only_under_its_own_context_readings() {
        type Wrap = Box<dyn Fn(Fact) -> Fact>;
        let transitions: [(Wrap, DefinitionVersions); 2] = [
            (
                Box::new(|fact| versioned(2, fact)),
                DefinitionVersions {
                    goal_definition: 2,
                    knowledge_revision: "k1".into(),
                },
            ),
            (
                Box::new(|fact| knowledge("k2", fact)),
                DefinitionVersions {
                    goal_definition: 1,
                    knowledge_revision: "k2".into(),
                },
            ),
        ];
        for (wrap, new_versions) in transitions {
            let mut facts = vec![
                sample("vehicle.speed", "baseline", 0),
                sample("engine.coolant_temperature", "cooling", 1),
                sample("engine.coolant_temperature", "cooling", 5),
            ];
            // The new version has no context reading yet: these two do not qualify, however
            // fresh the old version's speed still is.
            facts.extend(
                [
                    sample("engine.coolant_temperature", "cooling", 6),
                    sample("engine.coolant_temperature", "cooling", 7),
                ]
                .map(&wrap),
            );
            // Its own speed reading renews freshness for the new version only.
            facts.extend(
                [
                    sample("vehicle.speed", "baseline", 8),
                    sample("engine.coolant_temperature", "cooling", 9),
                    sample("engine.coolant_temperature", "cooling", 12),
                ]
                .map(&wrap),
            );
            let run = run_with(cooling_rules(), SessionKind::Live, &facts);
            let old = key("engine.coolant_temperature", "cooling");
            let new = keyed(new_versions, "engine.coolant_temperature", "cooling");
            let old_entry = run.ledger().entry(&old).unwrap();
            assert_eq!(old_entry.valid_time, secs(4));
            assert_eq!(old_entry.qualifying_samples, 2);
            let new_entry = run.ledger().entry(&new).unwrap();
            assert_eq!(new_entry.qualifying_samples, 2);
            assert_eq!(new_entry.valid_time, secs(3));
            assert_eq!(new_entry.longest_segment, secs(3));
        }
    }

    /// One step of the exhaustive interval and segment invariants below: an `egr` reading that
    /// needs fresh speed, a speed reading, a failed speed reading or a boundary.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Step {
        Egr,
        Speed,
        SpeedFails,
        Boundary,
    }

    impl Step {
        const ALL: [Step; 4] = [Step::Egr, Step::Speed, Step::SpeedFails, Step::Boundary];

        fn breaks(self) -> bool {
            matches!(self, Step::SpeedFails | Step::Boundary)
        }

        fn fact(self, at: u64) -> Fact {
            match self {
                Step::Egr => sample("engine.egr_rate", "egr", at),
                Step::Speed => sample("vehicle.speed", "baseline", at),
                Step::SpeedFails => outcome_fact(
                    "vehicle.speed",
                    "baseline",
                    at,
                    Outcome::NoAnswer {
                        reason: "timeout".into(),
                    },
                ),
                Step::Boundary => boundary(at, Boundary::Gap),
            }
        }
    }

    type Sequence = Vec<(Step, u64)>;

    fn invariant_rules() -> Rules {
        Rules::generic_baseline().with_context(
            GoalContext::new("egr"),
            ContextRule {
                required_signals: vec!["vehicle.speed".into()],
                min_contiguous: Some(secs(30)),
                ..ContextRule::baseline()
            },
        )
    }

    /// Calls `visit` with every sequence of at most `max_len` steps over `offsets`; sorted ones
    /// only when `sorted` (non-decreasing offsets, the recording order of a live session).
    fn enumerate(
        offsets: &[u64],
        sorted: bool,
        max_len: usize,
        current: &mut Sequence,
        visit: &mut impl FnMut(&Sequence),
    ) {
        visit(current);
        if current.len() == max_len {
            return;
        }
        for &at in offsets {
            if sorted && current.last().is_some_and(|&(_, last)| at < last) {
                continue;
            }
            for step in Step::ALL {
                current.push((step, at));
                enumerate(offsets, sorted, max_len, current, visit);
                current.pop();
            }
        }
    }

    fn fold_sequence(sequence: &Sequence) -> SmartRun {
        let facts: Vec<Fact> = sequence.iter().map(|&(step, at)| step.fact(at)).collect();
        run_with(invariant_rules(), SessionKind::Live, &facts)
    }

    fn egr_entry(run: &SmartRun) -> Entry {
        run.ledger()
            .entry(&key("engine.egr_rate", "egr"))
            .cloned()
            .unwrap_or_default()
    }

    /// Upper bounds the counting rules must respect for recording-ordered facts: consecutive
    /// `egr` readings at most [`FRESHNESS_LIMIT`] apart add at most their distance; a segment
    /// continues across such a pair only when no failure or boundary was folded between them (a
    /// pair with one ends its segment there); a larger distance starts a new segment.
    fn segment_bounds(sequence: &Sequence) -> (Duration, Duration) {
        let limit = FRESHNESS_LIMIT.as_secs();
        let (mut valid, mut longest, mut current) = (0, 0, 0);
        let mut previous: Option<u64> = None;
        let mut broken = false;
        for &(step, at) in sequence {
            match step {
                Step::Egr => {
                    if let Some(before) = previous {
                        let gap = at - before;
                        if gap > limit {
                            current = 0;
                        } else if broken {
                            valid += gap;
                            longest = longest.max(current + gap);
                            current = 0;
                        } else {
                            valid += gap;
                            current += gap;
                            longest = longest.max(current);
                        }
                    }
                    previous = Some(at);
                    broken = false;
                }
                step if step.breaks() => broken = true,
                _ => {}
            }
        }
        (secs(valid), secs(longest))
    }

    #[test]
    // REQ-038.4
    fn req_038_4_invariant_segments_never_bridge_a_failure_or_boundary() {
        let mut checked = 0_u32;
        enumerate(
            &[0, 5, 10, 15, 20],
            true,
            6,
            &mut Vec::new(),
            &mut |sequence| {
                let entry = egr_entry(&fold_sequence(sequence));
                let (valid, longest) = segment_bounds(sequence);
                assert!(entry.longest_segment <= entry.valid_time, "{sequence:?}");
                assert!(entry.valid_time <= valid, "{sequence:?}: {entry:?}");
                assert!(entry.longest_segment <= longest, "{sequence:?}: {entry:?}");
                checked += 1;
            },
        );
        assert!(checked > 100_000);
    }

    /// Whether an `egr` reading at `at`, folded at position `reading`, may be fresh: a speed
    /// reading folded after every earlier break above its own offset renews the signal for
    /// [`FRESHNESS_LIMIT`], inclusive, unless a break at an offset in `[speed, at]` was folded
    /// after it. This is deliberately an upper bound.
    fn may_be_fresh(sequence: &Sequence, reading: usize, at: u64) -> bool {
        let limit = FRESHNESS_LIMIT.as_secs();
        sequence[..reading]
            .iter()
            .enumerate()
            .any(|(index, &(step, speed))| {
                step == Step::Speed
                    && speed <= at
                    && at <= speed + limit
                    && !sequence[..index]
                        .iter()
                        .any(|&(earlier, event)| earlier.breaks() && event > speed)
                    && !sequence[index + 1..reading]
                        .iter()
                        .any(|&(later, event)| later.breaks() && speed <= event && event <= at)
            })
    }

    #[test]
    // REQ-038.4
    fn req_038_4_invariant_a_failed_context_never_becomes_fresh_whatever_the_fold_order() {
        let mut checked = 0_u32;
        enumerate(
            &[0, 5, 10, 15],
            false,
            5,
            &mut Vec::new(),
            &mut |sequence| {
                let mut run =
                    SmartRun::new(invariant_rules(), RunId(RUN.into()), SessionKind::Live);
                for (position, &(step, at)) in sequence.iter().enumerate() {
                    run.apply(None, &step.fact(at)).unwrap();
                    if step == Step::Egr {
                        let state = egr_entry(&run).latest.unwrap().state;
                        assert!(
                            state != LatestState::Observed || may_be_fresh(sequence, position, at),
                            "{sequence:?} counted as fresh at step {position}"
                        );
                    }
                }
                checked += 1;
            },
        );
        assert!(checked > 100_000);
    }

    #[test]
    // REQ-038.4
    fn req_038_4_invariant_an_extra_failure_or_boundary_never_raises_coverage() {
        let mut checked = 0_u32;
        enumerate(&[0, 5, 10, 15], true, 4, &mut Vec::new(), &mut |sequence| {
            let base = egr_entry(&fold_sequence(sequence));
            for position in 0..=sequence.len() {
                let low = position
                    .checked_sub(1)
                    .map_or(0, |before| sequence[before].1);
                let high = sequence.get(position).map_or(15, |&(_, at)| at);
                for at in (low..=high).filter(|at| at % 5 == 0) {
                    for step in [Step::SpeedFails, Step::Boundary] {
                        let mut changed = sequence.clone();
                        changed.insert(position, (step, at));
                        let entry = egr_entry(&fold_sequence(&changed));
                        let context = format!("{changed:?} vs {sequence:?}");
                        assert!(entry.valid_time <= base.valid_time, "{context}");
                        assert!(entry.longest_segment <= base.longest_segment, "{context}");
                        assert!(
                            entry.qualifying_samples <= base.qualifying_samples,
                            "{context}"
                        );
                        checked += 1;
                    }
                }
            }
        });
        assert!(checked > 10_000);
    }
}
