//! The architecture proposed in the ADR: one serialized owner, per-target session ledger
//! (observed vs app-assumed state), a shared-transport classification, and a scheduler that
//! puts due maintenance first. Nothing here knows an ECU's real S3.

use crate::sim::{ExchangeError, Ms, Sim};

#[derive(Clone)]
pub struct Config {
    /// Tester present no more often than this per target (REQ-027.2).
    pub period_ms: Ms,
    /// App policy, not an ECU timeout: assumption decays to Unconfirmed after this long
    /// without maintenance.
    pub horizon_ms: Ms,
    pub backoff_base_ms: Ms,
    pub episode_attempts: u32,
    pub cooldown_ms: Ms,
    /// A frame is a benign late answer only within this time of the silent request.
    pub late_window_ms: Ms,
    pub max_sessions: usize,
    /// Share of each period that maintenance may use, per mille.
    pub max_util_pm: u64,
    pub burst_reads: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            period_ms: 2000,
            horizon_ms: 4000,
            backoff_base_ms: 2000,
            episode_attempts: 3,
            cooldown_ms: 60_000,
            late_window_ms: 2000,
            max_sessions: 3,
            max_util_pm: 500,
            burst_reads: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: &'static str,
    pub req: &'static str,
    pub resp: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Observed {
    Never,
    Default(Ms),
    Extended(Ms),
    NotExtended(Ms),
    Refused(u8, Ms),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Assumed {
    Default,
    Extended { since: Ms, maintained: Ms },
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Active,
    Retry { next_at: Ms },
    CoolDown { until: Ms },
    Refused(u8),
    Isolated(String),
}

#[derive(Clone, Debug)]
pub struct Session {
    pub target: Target,
    pub observed: Observed,
    pub assumed: Assumed,
    pub status: Status,
    pub attempts: u32,
    pub unanswered_reads: u32,
    pub canary: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Did(u16),
    Pid(u8),
}

#[derive(Clone, Debug)]
pub struct Want {
    pub ti: usize,
    pub item: Item,
    /// 0 is the highest priority (DPF raw request status).
    pub prio: u8,
    pub session_only: bool,
    pub period_ms: Ms,
    pub remaining: u32,
    pub due: Ms,
    pub served: u32,
}

impl Want {
    pub fn new(
        ti: usize,
        item: Item,
        prio: u8,
        session_only: bool,
        period_ms: Ms,
        remaining: u32,
    ) -> Self {
        Self {
            ti,
            item,
            prio,
            session_only,
            period_ms,
            remaining,
            due: 0,
            served: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub ti: usize,
    pub item: Item,
    pub data: Vec<u8>,
    pub at: Ms,
    pub observed: Observed,
    pub assumed: Assumed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    Activated(usize),
    Retry {
        ti: usize,
        attempt: u32,
        next_at: Ms,
    },
    CoolDown(usize),
    Refused(usize, u8),
    SessionLost(usize),
    Overdue(usize),
    Isolated(usize, String),
    Closed(usize),
    CapacityRefused(usize),
    LateDiscarded,
    Rejected {
        ti: usize,
        item: Item,
        nrc: u8,
    },
    Unanswered {
        ti: usize,
        item: Item,
    },
    StopAll(String),
}

// ---- NRC table (pure, so the ADR table is testable) --------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum ActEffect {
    /// Refused for this Smart run, no retry.
    Definitive,
    /// Final negative answer that says "not now": back off and retry.
    Transient,
    /// Not a final answer (busyRepeatRequest 0x21, like responsePending 0x78): the ECU may
    /// still have entered the session. Assumed unconfirmed, bounded `10 01` cleanup; back off
    /// only after a positive default return, otherwise isolate the target.
    Unconfirmed,
}

pub fn activation_effect(nrc: u8) -> ActEffect {
    match nrc {
        0x21 => ActEffect::Unconfirmed,
        0x22 | 0x37 | 0x92 | 0x93 => ActEffect::Transient,
        _ => ActEffect::Definitive,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadEffect {
    Unsupported,
    Restricted,
    RetryLater,
    Rejected,
    SessionLost,
}

pub fn read_effect(nrc: u8, session_only: bool) -> ReadEffect {
    match nrc {
        0x31 => ReadEffect::Unsupported,
        0x33 => ReadEffect::Restricted,
        0x21 | 0x22 | 0x78 | 0x92 | 0x93 => ReadEffect::RetryLater,
        0x7E | 0x7F if session_only => ReadEffect::SessionLost,
        _ => ReadEffect::Rejected,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeepEffect {
    /// The ECU cannot be maintained: release it.
    Refused,
    SessionLost,
}

pub fn maintenance_effect(nrc: u8) -> KeepEffect {
    match nrc {
        0x11 | 0x12 | 0x13 => KeepEffect::Refused,
        _ => KeepEffect::SessionLost,
    }
}

// ---- frames ------------------------------------------------------------------------------

#[derive(Default)]
struct Parsed {
    ok: bool,
    no_data: bool,
    adapter_error: Option<String>,
    payloads: Vec<(String, Vec<u8>)>,
    broken: Option<String>,
}

fn parse(text: &str) -> Parsed {
    let mut p = Parsed::default();
    // (header, total length, bytes so far, next sequence)
    let mut asm: Option<(String, usize, Vec<u8>, u8)> = None;
    for raw in text.split('\r') {
        let line = raw.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper == "OK" {
            p.ok = true;
            continue;
        }
        if upper == "NO DATA" {
            p.no_data = true;
            continue;
        }
        if upper.contains('?')
            || ["ERROR", "STOPPED", "UNABLE", "BUFFER FULL"]
                .iter()
                .any(|e| upper.contains(e))
        {
            p.adapter_error = Some(upper);
            continue;
        }
        let mut tokens = upper.split_ascii_whitespace();
        let hdr = tokens.next().unwrap_or_default().to_string();
        let bytes: Option<Vec<u8>> = tokens.map(|t| u8::from_str_radix(t, 16).ok()).collect();
        let bytes = match bytes {
            Some(b) if hdr.len() == 3 && !b.is_empty() => b,
            _ => {
                p.broken = Some(format!("malformed line {line:?}"));
                continue;
            }
        };
        match bytes[0] >> 4 {
            0 => {
                let n = (bytes[0] & 0xF) as usize;
                if n == 0 || bytes.len() < 1 + n || asm.is_some() {
                    p.broken = Some("bad single frame".into());
                } else {
                    p.payloads.push((hdr, bytes[1..=n].to_vec()));
                }
            }
            1 => {
                let total =
                    (((bytes[0] & 0xF) as usize) << 8) | *bytes.get(1).unwrap_or(&0) as usize;
                if asm.is_some() {
                    p.broken = Some("first frame inside a running assembly".into());
                }
                asm = Some((hdr, total, bytes.get(2..).unwrap_or_default().to_vec(), 1));
            }
            2 => match asm.take() {
                Some((h, total, mut buf, seq)) if h == hdr && bytes[0] & 0xF == seq % 16 => {
                    buf.extend(&bytes[1..]);
                    if buf.len() >= total {
                        buf.truncate(total);
                        p.payloads.push((h, buf));
                    } else {
                        asm = Some((h, total, buf, seq + 1));
                    }
                }
                _ => {
                    p.broken =
                        Some("consecutive frame without its first frame or out of sequence".into())
                }
            },
            _ => p.broken = Some("unknown frame type".into()),
        }
    }
    if asm.is_some() {
        p.broken = Some("truncated multi-frame response".into());
    }
    p
}

// ---- controller --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Op {
    Enter,
    Leave,
    TesterPresent,
    ReadDid(u16),
    ReadPid(u8),
}

impl Op {
    fn wire(self) -> String {
        match self {
            Op::Enter => "1003".into(),
            Op::Leave => "1001".into(),
            Op::TesterPresent => "3E80".into(),
            Op::ReadDid(d) => format!("22{d:04X}"),
            Op::ReadPid(p) => format!("01{p:02X}"),
        }
    }
    fn service(self) -> u8 {
        match self {
            Op::Enter | Op::Leave => 0x10,
            Op::TesterPresent => 0x3E,
            Op::ReadDid(_) => 0x22,
            Op::ReadPid(_) => 0x01,
        }
    }
    fn positive(self) -> Vec<u8> {
        match self {
            Op::Enter => vec![0x50, 0x03],
            Op::Leave => vec![0x50, 0x01],
            Op::TesterPresent => vec![0x7E, 0x00],
            Op::ReadDid(d) => vec![0x62, (d >> 8) as u8, d as u8],
            Op::ReadPid(p) => vec![0x41, p],
        }
    }
}

enum Reply {
    Positive(Vec<u8>),
    Nrc(u8),
    Silent,
}

struct Pending {
    resp: &'static str,
    echo: Vec<u8>,
    at: Ms,
}

pub struct Controller {
    pub sim: Sim,
    pub cfg: Config,
    pub sessions: Vec<Session>,
    pub wants: Vec<Want>,
    pub samples: Vec<Sample>,
    pub events: Vec<Ev>,
    pub stopped: Option<String>,
    pub late_discarded: u32,
    configured: Option<usize>,
    resync: bool,
    pending: Vec<Pending>,
    switch_costs: Vec<Ms>,
    tp_costs: Vec<Ms>,
    last_tx: Ms,
    capacity_logged: Vec<usize>,
}

fn avg(v: &[Ms]) -> Ms {
    let tail = &v[v.len().saturating_sub(8)..];
    if tail.is_empty() {
        0
    } else {
        tail.iter().sum::<Ms>() / tail.len() as Ms
    }
}

impl Controller {
    pub fn new(sim: Sim, targets: Vec<Target>, cfg: Config) -> Self {
        let sessions = targets
            .into_iter()
            .map(|target| Session {
                target,
                observed: Observed::Never,
                assumed: Assumed::Default,
                status: Status::Idle,
                attempts: 0,
                unanswered_reads: 0,
                canary: None,
            })
            .collect();
        Self {
            sim,
            cfg,
            sessions,
            wants: Vec::new(),
            samples: Vec::new(),
            events: Vec::new(),
            stopped: None,
            late_discarded: 0,
            configured: None,
            resync: false,
            pending: Vec::new(),
            switch_costs: Vec::new(),
            tp_costs: Vec::new(),
            last_tx: 0,
            capacity_logged: Vec::new(),
        }
    }

    fn now(&self) -> Ms {
        self.sim.clock
    }

    // ---- transport -------------------------------------------------------------------

    /// Shared-transport rule: any uncertainty stops everything. No traffic, no close attempts:
    /// without proven link, header and association no answer could be attributed.
    fn stop_all(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if self.stopped.is_some() {
            return;
        }
        for s in &mut self.sessions {
            if matches!(s.assumed, Assumed::Extended { .. }) {
                s.assumed = Assumed::Unconfirmed;
            }
            if !matches!(s.status, Status::Refused(_)) {
                s.status = Status::Isolated("stopped".into());
            }
        }
        self.events.push(Ev::StopAll(reason.clone()));
        self.stopped = Some(reason);
    }

    pub fn disconnect(&mut self) {
        self.sim.disconnected = true;
        self.stop_all("disconnect");
    }

    fn exchange(&mut self, cmd: &str) -> Result<String, ()> {
        match self.sim.exchange(cmd) {
            Ok(text) => Ok(text),
            Err(ExchangeError::NoPrompt) => {
                self.configured = None;
                self.stop_all("transport: no prompt");
                Err(())
            }
            Err(ExchangeError::Disconnected) => {
                self.stop_all("transport: disconnected");
                Err(())
            }
        }
    }

    fn expire_pending(&mut self) {
        let (now, w) = (self.now(), self.cfg.late_window_ms);
        self.pending.retain(|p| now.saturating_sub(p.at) <= w);
    }

    /// A stray frame is benign only when exactly one unexpired unanswered request explains it.
    fn take_pending_match(&mut self, hdr: &str, payload: &[u8]) -> bool {
        self.expire_pending();
        let hits: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.resp == hdr && payload.starts_with(&p.echo))
            .map(|(i, _)| i)
            .collect();
        if hits.len() == 1 {
            self.pending.remove(hits[0]);
            self.late_discarded += 1;
            self.events.push(Ev::LateDiscarded);
            true
        } else {
            false
        }
    }

    fn pending_blocks(&mut self, ti: usize, item: Item) -> bool {
        self.expire_pending();
        let resp = self.sessions[ti].target.resp;
        let echo = match item {
            Item::Did(d) => Op::ReadDid(d).positive(),
            Item::Pid(p) => Op::ReadPid(p).positive(),
        };
        self.pending
            .iter()
            .any(|p| p.resp == resp && p.echo == echo)
    }

    /// ATSH + ATCRA, strictly acknowledged. Also the resynchronisation after a silent request.
    fn configure(&mut self, ti: usize) -> Result<(), ()> {
        if self.configured == Some(ti) && !self.resync {
            return Ok(());
        }
        let t0 = self.now();
        let t = self.sessions[ti].target;
        self.configured = None;
        for cmd in [format!("ATSH {}", t.req), format!("ATCRA {}", t.resp)] {
            let text = self.exchange(&cmd)?;
            let p = parse(&text);
            if let Some(e) = p.adapter_error {
                self.stop_all(format!("adapter error during header setup: {e}"));
                return Err(());
            }
            if !p.ok || p.no_data || p.broken.is_some() {
                self.stop_all("header setup not cleanly acknowledged");
                return Err(());
            }
            for (h, pl) in &p.payloads {
                if !self.take_pending_match(h, pl) {
                    self.stop_all("unattributed frame during header setup");
                    return Err(());
                }
            }
        }
        self.configured = Some(ti);
        self.resync = false;
        let cost = self.now() - t0;
        self.switch_costs.push(cost);
        Ok(())
    }

    fn request(&mut self, ti: usize, op: Op) -> Result<Reply, ()> {
        if self.stopped.is_some() {
            return Err(());
        }
        // Never repeat a request whose late answer could still arrive and be mistaken for it.
        if matches!(op, Op::Enter | Op::Leave) {
            let (resp, echo) = (self.sessions[ti].target.resp, op.positive());
            self.expire_pending();
            let wait_until = self
                .pending
                .iter()
                .find(|p| p.resp == resp && p.echo == echo)
                .map(|p| p.at + self.cfg.late_window_ms + 1);
            if let Some(t) = wait_until {
                self.sim.clock = self.sim.clock.max(t);
            }
        }
        self.configure(ti)?;
        let t = self.sessions[ti].target;
        let t_tx = self.now();
        self.last_tx = t_tx;
        let text = self.exchange(&op.wire())?;
        if matches!(op, Op::TesterPresent) {
            self.tp_costs.push(self.now() - t_tx);
        }
        let p = parse(&text);
        if let Some(e) = p.adapter_error {
            self.stop_all(format!("adapter error: {e}"));
            return Err(());
        }
        if let Some(b) = p.broken {
            self.stop_all(format!("association: {b}"));
            return Err(());
        }
        let positive = op.positive();
        let mut finals: Vec<Vec<u8>> = Vec::new();
        for (hdr, payload) in p.payloads {
            let answers_us = hdr == t.resp
                && (payload.starts_with(&positive)
                    || (payload.len() == 3 && payload[0] == 0x7F && payload[1] == op.service()));
            if answers_us {
                if payload[0] == 0x7F && payload[2] == 0x78 {
                    continue;
                }
                if !finals.contains(&payload) {
                    finals.push(payload);
                }
            } else if !self.take_pending_match(&hdr, &payload) {
                self.stop_all(format!("association: unattributed frame from {hdr}"));
                return Err(());
            }
        }
        match finals.len() {
            0 if p.no_data => {
                if !matches!(op, Op::TesterPresent) {
                    self.pending.push(Pending {
                        resp: t.resp,
                        echo: positive,
                        at: t_tx,
                    });
                    // Drain right away: a late answer must meet the header setup while its
                    // unanswered request is still within the late window.
                    self.resync = true;
                    self.configure(ti)?;
                }
                Ok(Reply::Silent)
            }
            0 => {
                self.stop_all("association: empty response without NO DATA");
                Err(())
            }
            1 => {
                let f = finals.remove(0);
                if f[0] == 0x7F {
                    Ok(Reply::Nrc(f[2]))
                } else {
                    Ok(Reply::Positive(f))
                }
            }
            _ => {
                self.stop_all("association: conflicting responses");
                Err(())
            }
        }
    }

    // ---- per-target lifecycle ----------------------------------------------------------

    fn backoff(&mut self, ti: usize) {
        let now = self.now();
        let (base, max, cool) = (
            self.cfg.backoff_base_ms,
            self.cfg.episode_attempts,
            self.cfg.cooldown_ms,
        );
        let s = &mut self.sessions[ti];
        s.attempts += 1;
        if s.attempts >= max {
            s.attempts = 0;
            s.status = Status::CoolDown { until: now + cool };
            self.events.push(Ev::CoolDown(ti));
        } else {
            let next_at = now + base * (1 << (s.attempts - 1));
            s.status = Status::Retry { next_at };
            self.events.push(Ev::Retry {
                ti,
                attempt: s.attempts,
                next_at,
            });
        }
    }

    fn lost(&mut self, ti: usize, evidence: bool) {
        let now = self.now();
        let s = &mut self.sessions[ti];
        if evidence {
            s.observed = Observed::NotExtended(now);
        }
        s.assumed = Assumed::Unconfirmed;
        s.status = Status::Retry { next_at: now };
        s.canary = None;
        s.unanswered_reads = 0;
        self.events.push(Ev::SessionLost(ti));
    }

    fn isolate(&mut self, ti: usize, why: &str) {
        let s = &mut self.sessions[ti];
        s.assumed = Assumed::Unconfirmed;
        s.status = Status::Isolated(why.into());
        self.events.push(Ev::Isolated(ti, why.into()));
    }

    /// Bounded return to default: at most two `10 01`. Unconfirmed return isolates the target.
    fn close(&mut self, ti: usize) -> bool {
        for _ in 0..2 {
            match self.request(ti, Op::Leave) {
                Err(()) => return false,
                Ok(Reply::Positive(_)) => {
                    let now = self.now();
                    let s = &mut self.sessions[ti];
                    s.observed = Observed::Default(now);
                    s.assumed = Assumed::Default;
                    s.status = Status::Idle;
                    s.canary = None;
                    self.events.push(Ev::Closed(ti));
                    return true;
                }
                Ok(_) => {}
            }
        }
        self.isolate(ti, "unconfirmed return to default");
        false
    }

    /// The ECU may or may not have switched: never conclude default or extended. Bounded
    /// return to default; the backoff starts only after a positive `50 01`, otherwise `close`
    /// has isolated the target.
    fn unconfirmed_activation(&mut self, ti: usize) {
        self.sessions[ti].assumed = Assumed::Unconfirmed;
        if self.close(ti) {
            self.backoff(ti);
        }
    }

    fn try_activate(&mut self, ti: usize) {
        match self.request(ti, Op::Enter) {
            Err(()) => {}
            Ok(Reply::Positive(_)) => {
                let now = self.now();
                let s = &mut self.sessions[ti];
                s.observed = Observed::Extended(now);
                s.assumed = Assumed::Extended {
                    since: self.last_tx,
                    maintained: self.last_tx,
                };
                s.status = Status::Active;
                s.attempts = 0;
                s.unanswered_reads = 0;
                self.events.push(Ev::Activated(ti));
            }
            Ok(Reply::Nrc(n)) => match activation_effect(n) {
                // Not a final answer: nothing is observed, nothing is assumed.
                ActEffect::Unconfirmed => self.unconfirmed_activation(ti),
                effect => {
                    // A final negative answer: the ECU stayed in the default session.
                    let now = self.now();
                    self.sessions[ti].observed = Observed::Refused(n, now);
                    self.sessions[ti].assumed = Assumed::Default;
                    if effect == ActEffect::Definitive {
                        self.sessions[ti].status = Status::Refused(n);
                        self.events.push(Ev::Refused(ti, n));
                    } else {
                        self.backoff(ti);
                    }
                }
            },
            Ok(Reply::Silent) => self.unconfirmed_activation(ti),
        }
    }

    fn maintain(&mut self, ti: usize) {
        match self.request(ti, Op::TesterPresent) {
            Err(()) => {}
            Ok(Reply::Silent) | Ok(Reply::Positive(_)) => {
                if let Assumed::Extended { maintained, .. } = &mut self.sessions[ti].assumed {
                    *maintained = self.last_tx;
                }
            }
            Ok(Reply::Nrc(n)) => match maintenance_effect(n) {
                KeepEffect::Refused => {
                    self.events.push(Ev::Refused(ti, n));
                    if self.close(ti) {
                        self.sessions[ti].status = Status::Refused(n);
                    }
                }
                KeepEffect::SessionLost => self.lost(ti, true),
            },
        }
    }

    fn read(&mut self, wi: usize) {
        let w = self.wants[wi].clone();
        let op = match w.item {
            Item::Did(d) => Op::ReadDid(d),
            Item::Pid(p) => Op::ReadPid(p),
        };
        let reply = self.request(w.ti, op);
        let now = self.now();
        self.wants[wi].due = now + w.period_ms;
        self.wants[wi].served += 1;
        match reply {
            Err(()) => {}
            Ok(Reply::Positive(p)) => {
                if w.session_only {
                    let s = &mut self.sessions[w.ti];
                    s.observed = Observed::Extended(now);
                    s.unanswered_reads = 0;
                    if let Item::Did(d) = w.item {
                        s.canary = Some(d);
                    }
                }
                let s = &self.sessions[w.ti];
                self.samples.push(Sample {
                    ti: w.ti,
                    item: w.item,
                    data: p[op.positive().len()..].to_vec(),
                    at: now,
                    observed: s.observed.clone(),
                    assumed: s.assumed.clone(),
                });
                self.wants[wi].remaining = self.wants[wi].remaining.saturating_sub(1);
            }
            Ok(Reply::Nrc(n)) => {
                self.events.push(Ev::Rejected {
                    ti: w.ti,
                    item: w.item,
                    nrc: n,
                });
                match read_effect(n, w.session_only) {
                    ReadEffect::SessionLost => self.lost(w.ti, true),
                    ReadEffect::Unsupported if w.session_only => self.canary_check(w.ti, w.item),
                    _ => {}
                }
            }
            Ok(Reply::Silent) => {
                self.events.push(Ev::Unanswered {
                    ti: w.ti,
                    item: w.item,
                });
                if w.session_only {
                    self.sessions[w.ti].unanswered_reads += 1;
                    if self.sessions[w.ti].unanswered_reads >= 2 {
                        self.lost(w.ti, false);
                    }
                }
            }
        }
    }

    /// 0x31 on a session-only DID is ambiguous (unsupported, or the session is gone).
    /// A DID that answered earlier in this activation decides it.
    fn canary_check(&mut self, ti: usize, failed: Item) {
        let Some(d) = self.sessions[ti].canary else {
            return;
        };
        if failed == Item::Did(d) {
            self.lost(ti, true);
            return;
        }
        match self.request(ti, Op::ReadDid(d)) {
            Ok(Reply::Positive(_)) => {}
            Err(()) => {}
            Ok(_) => self.lost(ti, true),
        }
    }

    // ---- scheduler ---------------------------------------------------------------------

    fn needed(&self, ti: usize) -> bool {
        self.wants
            .iter()
            .any(|w| w.ti == ti && w.session_only && w.remaining > 0)
    }

    pub fn active_sessions(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| matches!(s.assumed, Assumed::Extended { .. }))
            .count()
    }

    fn admit(&mut self, ti: usize) -> bool {
        let active = self.active_sessions();
        let est = avg(&self.switch_costs) + avg(&self.tp_costs);
        let util_pm = (active as u64 + 1) * est * 1000 / self.cfg.period_ms;
        // Slow start: the next session waits until one maintenance cost has been measured.
        if active >= 1 && self.tp_costs.is_empty() {
            return false;
        }
        let ok = active < self.cfg.max_sessions && (active == 0 || util_pm <= self.cfg.max_util_pm);
        if !ok && !self.capacity_logged.contains(&ti) {
            self.capacity_logged.push(ti);
            self.events.push(Ev::CapacityRefused(ti));
        }
        ok
    }

    fn refresh_assumptions(&mut self) {
        let (now, horizon) = (self.now(), self.cfg.horizon_ms);
        for ti in 0..self.sessions.len() {
            if let Assumed::Extended { maintained, .. } = self.sessions[ti].assumed {
                if now.saturating_sub(maintained) > horizon {
                    self.sessions[ti].assumed = Assumed::Unconfirmed;
                    self.sessions[ti].status = Status::Retry { next_at: now };
                    self.sessions[ti].canary = None;
                    self.events.push(Ev::Overdue(ti));
                }
            }
        }
    }

    fn maintenance_due(&self, ti: usize) -> bool {
        matches!(self.sessions[ti].assumed, Assumed::Extended { maintained, .. } if self.now() >= maintained + self.cfg.period_ms)
    }

    fn due_maintenance(&self) -> Option<usize> {
        (0..self.sessions.len())
            .filter(|&ti| self.maintenance_due(ti))
            .min_by_key(|&ti| match self.sessions[ti].assumed {
                Assumed::Extended { maintained, .. } => maintained,
                _ => 0,
            })
    }

    fn eligible(&self, w: &Want) -> bool {
        let s = &self.sessions[w.ti];
        if matches!(s.status, Status::Isolated(_)) {
            return false;
        }
        !w.session_only || s.status == Status::Active
    }

    fn next_read(&mut self) -> Option<usize> {
        let now = self.now();
        let mut best: Option<usize> = None;
        for wi in 0..self.wants.len() {
            let w = self.wants[wi].clone();
            if w.remaining == 0
                || w.due > now
                || !self.eligible(&w)
                || self.pending_blocks(w.ti, w.item)
            {
                continue;
            }
            let better = best.map_or(true, |b| {
                let o = &self.wants[b];
                (w.prio, w.due, w.served) < (o.prio, o.due, o.served)
            });
            if better {
                best = Some(wi);
            }
        }
        best
    }

    fn urgent_elsewhere(&self, ti: usize) -> bool {
        let est_read = 100;
        (0..self.sessions.len()).filter(|&u| u != ti).any(|u| {
            matches!(self.sessions[u].assumed, Assumed::Extended { maintained, .. } if maintained + self.cfg.period_ms <= self.now() + est_read)
        })
    }

    fn burst(&mut self, ti: usize, first: Option<usize>) {
        if self.maintenance_due(ti) {
            self.maintain(ti);
        }
        let mut done = 0;
        if let Some(wi) = first {
            if self.stopped.is_none() {
                self.read(wi);
                done += 1;
            }
        }
        while done < self.cfg.burst_reads && self.stopped.is_none() {
            if self.urgent_elsewhere(ti) {
                break;
            }
            let Some(wi) = self.next_read() else { break };
            if self.wants[wi].ti != ti {
                break;
            }
            self.read(wi);
            done += 1;
        }
    }

    /// One scheduling decision. Order: due maintenance, closes, activations, reads, idle.
    pub fn step(&mut self) -> bool {
        if self.stopped.is_some() {
            return false;
        }
        self.refresh_assumptions();
        if let Some(ti) = self.due_maintenance() {
            self.burst(ti, None);
            return true;
        }
        for ti in 0..self.sessions.len() {
            if self.sessions[ti].status == Status::Active && !self.needed(ti) {
                self.close(ti);
                return true;
            }
        }
        for ti in 0..self.sessions.len() {
            let now = self.now();
            let ready = match self.sessions[ti].status {
                Status::Idle => true,
                Status::Retry { next_at } => next_at <= now,
                Status::CoolDown { until } => until <= now,
                _ => false,
            };
            if ready && self.needed(ti) && self.admit(ti) {
                self.try_activate(ti);
                return true;
            }
        }
        if let Some(wi) = self.next_read() {
            let ti = self.wants[wi].ti;
            self.burst(ti, Some(wi));
            return true;
        }
        self.sim.clock = self.next_wakeup();
        true
    }

    fn next_wakeup(&mut self) -> Ms {
        let now = self.now();
        let mut t = now + 100;
        let mut consider = |c: Ms| {
            if c > now && c < t {
                t = c;
            }
        };
        for s in &self.sessions {
            match (&s.assumed, &s.status) {
                (Assumed::Extended { maintained, .. }, _) => {
                    consider(maintained + self.cfg.period_ms)
                }
                (_, Status::Retry { next_at }) => consider(*next_at),
                (_, Status::CoolDown { until }) => consider(*until),
                _ => {}
            }
        }
        for w in &self.wants {
            if w.remaining > 0 {
                consider(w.due);
            }
        }
        t.max(now + 1)
    }

    pub fn run_until(&mut self, end: Ms) {
        while self.now() < end && self.step() {}
    }

    /// User stop: `10 01` to every target assumed in a session, when the transport is known safe.
    pub fn end_run(&mut self) {
        for ti in 0..self.sessions.len() {
            if matches!(self.sessions[ti].assumed, Assumed::Extended { .. })
                && self.stopped.is_none()
            {
                self.close(ti);
            }
        }
    }

    pub fn count_events(&self, f: impl Fn(&Ev) -> bool) -> usize {
        self.events.iter().filter(|e| f(e)).count()
    }
}
