//! #359 transport harness: a virtual ELM adapter behind the production `mobile::Exchange` seam plus the
//! adverse-adapter scenarios of REQ-046.2 and REQ-046.5-.18.
//!
//! Everything here is synthetic and test-only. The adapter model answers by command *and* routing state
//! (header, response filter, echo/header/space flags, reset, protocol search, per-ECU session state), never by
//! FIFO position. All parsing, retry, routing and session policy under test belongs to the real `ElmSession`,
//! `MobileCore` and assistant policy; this module only supplies bytes, a virtual clock and a trace.
//!
//! Limits (stated, not hidden):
//! * The virtual clock drives the adapter model (reply latency, timeouts, ECU S3 timer). `MobileCore` itself
//!   still uses `std::time::Instant` for keepalive/expiry and `SystemTime` for result timestamps.
//! * Draining stale notification bytes before a write is the platform transport's job (Kotlin GATT layer), not
//!   the Rust seam's. The hostile default here keeps late bytes until the next request, so these scenarios prove
//!   what the Rust layer does when the platform hands it such bytes; they do not test the Java framing.
#![allow(dead_code)] // The pub(super) API is for the parent acceptance tests.

use super::*;
use crate::mobile::execute_nonblocking;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

const S3_MS: u64 = 5_000;
static VEHICLE_FIXTURE: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../tests/fixtures/virtual-vehicle.json")).unwrap()
});
fn vehicle_fixture() -> &'static Value {
    &VEHICLE_FIXTURE
}

// ------------------------------------------------------------------------------------------------------
// public-to-parent API
// ------------------------------------------------------------------------------------------------------

/// One adapter request as the transport boundary saw it.
#[derive(Clone, Debug)]
pub(super) struct Trace {
    pub(super) wire: String,
    pub(super) started_ms: u64,
    pub(super) ended_ms: u64,
    /// `Ok(raw adapter text)` or `Err(transport error text)`.
    pub(super) result: Result<String, String>,
}

/// A scripted adapter misbehavior that replaces the model's answer for one request.
#[derive(Clone)]
pub(super) enum Behavior {
    /// Raw adapter lines with their arrival time (ms after the request) and the time the prompt `>` arrives
    /// (`None`: never). Lines and prompt later than the request timeout are late: the exchange fails with a
    /// timeout and the late lines stay in the stale buffer.
    Lines {
        lines: Vec<(u64, String)>,
        prompt_at: Option<u64>,
    },
    /// The BLE link drops while the request is pending; the model's reply becomes an orphan.
    Disconnect,
}

impl Behavior {
    /// Lines arrive at 5 ms, the prompt at 10 ms.
    pub(super) fn reply(lines: &[&str]) -> Self {
        let timed: Vec<(u64, &str)> = lines.iter().map(|line| (5, *line)).collect();
        Self::timed(&timed, Some(10))
    }

    pub(super) fn timed(lines: &[(u64, &str)], prompt_at: Option<u64>) -> Self {
        Self::Lines {
            lines: lines
                .iter()
                .map(|(at, line)| (*at, (*line).to_owned()))
                .collect(),
            prompt_at,
        }
    }

    /// Nothing ever arrives.
    pub(super) fn silent() -> Self {
        Self::timed(&[], None)
    }
}

#[derive(Default)]
struct Gauge {
    active: AtomicUsize,
    peak: AtomicUsize,
}

/// The observer of a [`VirtualAdapter`]; clones share the adapter's state.
#[derive(Clone)]
pub(super) struct TraceProbe {
    sim: Arc<Mutex<Sim>>,
    gauge: Arc<Gauge>,
}

impl TraceProbe {
    pub(super) fn transactions(&self) -> Vec<Trace> {
        self.sim.lock().unwrap().traces.clone()
    }

    pub(super) fn len(&self) -> usize {
        self.sim.lock().unwrap().traces.len()
    }

    pub(super) fn transactions_since(&self, mark: usize) -> Vec<Trace> {
        self.sim.lock().unwrap().traces[mark..].to_vec()
    }

    pub(super) fn wires_since(&self, mark: usize) -> Vec<String> {
        self.transactions_since(mark)
            .into_iter()
            .map(|trace| trace.wire)
            .collect()
    }

    /// Requests inside `exchange` right now (counted outside the adapter lock, so overlap would show).
    pub(super) fn outstanding(&self) -> usize {
        self.gauge.active.load(AtomicOrdering::SeqCst)
    }

    /// The most requests that were ever inside `exchange` at once.
    pub(super) fn peak_outstanding(&self) -> usize {
        self.gauge.peak.load(AtomicOrdering::SeqCst)
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.sim.lock().unwrap().now
    }

    /// Idle virtual time (no sleeping).
    pub(super) fn advance_ms(&self, ms: u64) {
        self.sim.lock().unwrap().now += ms;
    }

    /// The next request with this exact wire (and, when given, this selected request header) is answered by
    /// `behavior`; scripts for the same match queue in order.
    pub(super) fn script(&self, wire: &str, header: Option<&str>, behavior: Behavior) {
        self.sim.lock().unwrap().scripts.push((
            wire.to_owned(),
            header.map(str::to_owned),
            behavior,
        ));
    }

    /// Scripts that were never consumed.
    pub(super) fn pending_scripts(&self) -> usize {
        self.sim.lock().unwrap().scripts.len()
    }

    /// Runs on the calling thread while the request is outstanding and the adapter lock is free.
    pub(super) fn on_command(&self, wire: &str, hook: impl Fn() + Send + Sync + 'static) {
        let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(hook);
        self.sim.lock().unwrap().hooks.push((wire.to_owned(), hook));
    }

    /// Stored (Mode 03) codes of the engine ECU; empty answers `43 00`.
    pub(super) fn set_stored_dtcs(&self, codes: &[[u8; 2]]) {
        self.ecu_mut("7E0", |ecu| ecu.stored = codes.to_vec());
    }

    /// Pending (Mode 07) codes of the engine ECU; empty answers `47 00`.
    pub(super) fn set_pending_dtcs(&self, codes: &[[u8; 2]]) {
        self.ecu_mut("7E0", |ecu| ecu.pending = codes.to_vec());
    }

    /// Mode 01 PID data of the engine ECU (`None` removes the PID so it is unsupported).
    pub(super) fn set_pid(&self, pid: u8, data: Option<&[u8]>) {
        self.ecu_mut("7E0", |ecu| match data {
            Some(data) => {
                ecu.pids.insert(pid, data.to_vec());
            }
            None => {
                ecu.pids.remove(&pid);
            }
        });
    }

    /// A DID data record of the ECU with this request id.
    pub(super) fn set_did(&self, request: &str, did: u16, data: &[u8], session_only: bool) {
        self.ecu_mut(request, |ecu| {
            ecu.dids.insert(did, (data.to_vec(), session_only));
        });
    }

    /// Another present ECU with the engine's DID table (identification DIDs, 0x1356, session DID 0x1628).
    pub(super) fn add_ecu(&self, request: &str, response: &str) {
        let mut sim = self.sim.lock().unwrap();
        let mut ecu = sim.ecus[0].clone();
        ecu.req = request.to_owned();
        ecu.resp = response.to_owned();
        ecu.extended = false;
        sim.ecus.push(ecu);
    }

    /// `"default"` or `"extended"`, from the model's S3 timer on the virtual clock.
    pub(super) fn session_of(&self, request: &str) -> &'static str {
        let sim = self.sim.lock().unwrap();
        let ecu = sim.ecus.iter().find(|ecu| ecu.req == request).unwrap();
        if ecu.extended && sim.now.saturating_sub(ecu.last_ms) <= S3_MS {
            "extended"
        } else {
            "default"
        }
    }

    pub(super) fn link_up(&self) -> bool {
        self.sim.lock().unwrap().link_up
    }

    /// A fresh physical connection: adapter flags reset, stale bytes dropped. With `leak_orphan` the reply of the
    /// request that was pending at the disconnect is offered, as stale bytes, to the first request afterwards.
    /// ECU session state is not touched: the vehicle does not know about the BLE link.
    pub(super) fn reconnect(&self, leak_orphan: bool) {
        let mut sim = self.sim.lock().unwrap();
        sim.link_up = true;
        sim.reset_adapter();
        sim.stale.clear();
        sim.stale_any = false;
        let orphan = std::mem::take(&mut sim.orphan);
        if leak_orphan {
            sim.stale = orphan;
            sim.stale_any = true;
        }
    }

    fn ecu_mut(&self, request: &str, change: impl FnOnce(&mut VEcu)) {
        let mut sim = self.sim.lock().unwrap();
        let ecu = sim
            .ecus
            .iter_mut()
            .find(|ecu| ecu.req == request)
            .unwrap_or_else(|| panic!("no virtual ECU {request}"));
        change(ecu);
    }
}

/// The virtual adapter: implements the production seam.
pub(super) struct VirtualAdapter {
    sim: Arc<Mutex<Sim>>,
    gauge: Arc<Gauge>,
}

impl Exchange for VirtualAdapter {
    fn exchange(&mut self, command: &str, timeout: Duration) -> Result<String, String> {
        struct Leave<'a>(&'a AtomicUsize);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, AtomicOrdering::SeqCst);
            }
        }
        let now = self.gauge.active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        self.gauge.peak.fetch_max(now, AtomicOrdering::SeqCst);
        let _leave = Leave(&self.gauge.active);
        let hook = self
            .sim
            .lock()
            .unwrap()
            .hooks
            .iter()
            .find(|(wire, _)| wire == command)
            .map(|(_, hook)| hook.clone());
        if let Some(hook) = hook {
            hook();
        }
        let timeout_ms = timeout.as_millis() as u64;
        self.sim.lock().unwrap().handle(command, timeout_ms)
    }
}

/// An uninitialized core on the virtual adapter (nothing sent yet).
pub(super) fn unbooted() -> (MobileCore<VirtualAdapter>, TraceProbe) {
    let sim = Arc::new(Mutex::new(Sim::new()));
    let gauge = Arc::new(Gauge::default());
    let adapter = VirtualAdapter {
        sim: sim.clone(),
        gauge: gauge.clone(),
    };
    (MobileCore::new(adapter).unwrap(), TraceProbe { sim, gauge })
}

/// The adapter alone, for direct model checks.
pub(super) fn adapter_only() -> (VirtualAdapter, TraceProbe) {
    let sim = Arc::new(Mutex::new(Sim::new()));
    let gauge = Arc::new(Gauge::default());
    (
        VirtualAdapter {
            sim: sim.clone(),
            gauge: gauge.clone(),
        },
        TraceProbe { sim, gauge },
    )
}

/// The parent's entry point: a core on which the `initialize` action has **already run** (so the trace starts with
/// `ATI, AT@1, ATZ, ATE0, ATL0, ATS1, ATH1, ATSP0, 0100`). The vehicle is NOT inventoried: the caller runs the
/// real `inventory` (and `ecu.discover`) actions; nothing is seeded into `MobileCore`.
///
/// Model vehicle: a synthetic VAG identity (VIN `WVWZZZ1KZAM123456`), engine ECU 7E0/7E8 (only ECU present),
/// Mode 01 support `0C` (RPM, `1A F8` = 1726 rpm) and `10` (MAF, `03 E8` = 10.00 g/s) only, stored DTC P0401
/// (`7E8 03 43 04 01`), no pending DTC (`7E8 02 47 00`), embedded identification DIDs (`62 DD DD 31 32 33`),
/// Knowledge DID 0x1356 = `62 13 56 00 64` (value 1.0), session-only DID 0x1628 = `62 16 28 0B 54`.
pub(super) fn fixture() -> (MobileCore<VirtualAdapter>, TraceProbe) {
    let (mut core, probe) = unbooted();
    let initialized = run(&mut core, "initialize", "");
    assert_eq!(initialized["ok"], true, "{initialized}");
    (core, probe)
}

/// Real `initialize`, `inventory` and `ecu.discover` on a core (used after a reconnect as well).
pub(super) fn establish(core: &mut MobileCore<VirtualAdapter>) {
    let initialized = run(core, "initialize", "");
    assert_eq!(initialized["ok"], true, "{initialized}");
    inventory_and_discover(core);
}

fn inventory_and_discover(core: &mut MobileCore<VirtualAdapter>) {
    let inventory = run(core, "inventory", "");
    assert_eq!(inventory["ok"], true, "{inventory}");
    assert_eq!(
        inventory["data"]["engine_target_validated"], true,
        "{inventory}"
    );
    let discovery = run(core, "ecu.discover", "");
    assert_eq!(discovery["ok"], true, "{discovery}");
}

// ------------------------------------------------------------------------------------------------------
// the adapter + vehicle model
// ------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct VEcu {
    req: String,
    resp: String,
    extended: bool,
    last_ms: u64,
    /// DID -> (record, only answered in the extended session).
    dids: BTreeMap<u16, (Vec<u8>, bool)>,
    pids: BTreeMap<u8, Vec<u8>>,
    stored: Vec<[u8; 2]>,
    pending: Vec<[u8; 2]>,
}

impl VEcu {
    fn engine() -> Self {
        let fixture = vehicle_fixture();
        let catalog = KnowledgeCatalog::embedded_pinned().unwrap();
        let identification = parse_hex(fixture["identification"].as_str().unwrap()).unwrap();
        let extended = fixture["extended_dids"].as_array().unwrap();
        let mut dids = BTreeMap::new();
        for candidate in EcuIdentificationPlan::from_catalog(&catalog)
            .unwrap()
            .candidates()
        {
            dids.insert(candidate.did(), (identification.clone(), false));
        }
        for (key, value) in fixture["dids"].as_object().unwrap() {
            dids.insert(
                u16::from_str_radix(key, 16).unwrap(),
                (
                    parse_hex(value.as_str().unwrap()).unwrap(),
                    extended.iter().any(|v| v.as_str() == Some(key)),
                ),
            );
        }
        let pids = fixture["pids"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| {
                (
                    u8::from_str_radix(key, 16).unwrap(),
                    parse_hex(value.as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let codes = |field: &str| {
            fixture[field]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| {
                    let bytes = parse_hex(value.as_str().unwrap()).unwrap();
                    [bytes[0], bytes[1]]
                })
                .collect()
        };
        Self {
            req: fixture["request_id"].as_str().unwrap().into(),
            resp: fixture["response_id"].as_str().unwrap().into(),
            extended: false,
            last_ms: 0,
            dids,
            pids,
            stored: codes("stored_dtcs"),
            pending: codes("pending_dtcs"),
        }
    }

    /// ISO-TP frames (with PCI) of this ECU's answer; empty when it stays silent.
    fn respond(&mut self, request: &[u8], now: u64) -> Vec<Vec<u8>> {
        if self.extended && now.saturating_sub(self.last_ms) > S3_MS {
            self.extended = false; // S3 expired on the virtual clock
        }
        if self.extended {
            self.last_ms = now;
        }
        let payload: Option<Vec<u8>> = match request {
            [0x01, 0x00] => {
                let mut bitmap = 0u32;
                for pid in self.pids.keys().filter(|pid| (1..=32).contains(*pid)) {
                    bitmap |= 1 << (32 - u32::from(*pid));
                }
                let mut payload = vec![0x41, 0x00];
                payload.extend_from_slice(&bitmap.to_be_bytes());
                Some(payload)
            }
            [0x01, pid] => self.pids.get(pid).map(|data| {
                let mut payload = vec![0x41, *pid];
                payload.extend_from_slice(data);
                payload
            }),
            [0x03] => Some(dtc_payload(0x43, &self.stored)),
            [0x07] => Some(dtc_payload(0x47, &self.pending)),
            [0x09, 0x02] => {
                let mut payload = vec![0x49, 0x02, 0x01];
                payload.extend_from_slice(vehicle_fixture()["vin"].as_str().unwrap().as_bytes());
                Some(payload)
            }
            [0x22, high, low] => {
                let did = u16::from_be_bytes([*high, *low]);
                self.dids.get(&did).map(|(data, session_only)| {
                    if *session_only && !self.extended {
                        vec![0x7F, 0x22, 0x7F]
                    } else {
                        let mut payload = vec![0x62, *high, *low];
                        payload.extend_from_slice(data);
                        payload
                    }
                })
            }
            [0x10, 0x03] => {
                self.extended = true;
                self.last_ms = now;
                Some(vec![0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])
            }
            [0x10, 0x01] => {
                self.extended = false;
                Some(vec![0x50, 0x01])
            }
            [0x3E, 0x80] => None, // positive response suppressed; the S3 timer was refreshed above
            [0x3E, 0x00] => Some(vec![0x7E, 0x00]),
            [service, ..] => Some(vec![0x7F, *service, 0x11]),
            [] => None,
        };
        payload.map(|payload| isotp(&payload)).unwrap_or_default()
    }
}

fn dtc_payload(positive: u8, codes: &[[u8; 2]]) -> Vec<u8> {
    let mut payload = vec![positive];
    if codes.is_empty() {
        payload.push(0x00);
    }
    for code in codes {
        payload.extend_from_slice(code);
    }
    payload
}

fn isotp(payload: &[u8]) -> Vec<Vec<u8>> {
    if payload.len() <= 7 {
        let mut frame = vec![payload.len() as u8];
        frame.extend_from_slice(payload);
        return vec![frame];
    }
    let mut first = vec![
        0x10 | ((payload.len() >> 8) as u8 & 0x0F),
        payload.len() as u8,
    ];
    first.extend_from_slice(&payload[..6]);
    let mut frames = vec![first];
    for (index, chunk) in payload[6..].chunks(7).enumerate() {
        let mut frame = vec![0x20 | ((index + 1) & 0x0F) as u8];
        frame.extend_from_slice(chunk);
        frames.push(frame);
    }
    frames
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty()
        || !text.len().is_multiple_of(2)
        || !text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

struct Sim {
    now: u64,
    traces: Vec<Trace>,
    ecus: Vec<VEcu>,
    scripts: Vec<(String, Option<String>, Behavior)>,
    hooks: Vec<(String, Arc<dyn Fn() + Send + Sync>)>,
    /// Bytes that arrived after their exchange ended, delivered in front of the next request's reply.
    stale: Vec<String>,
    /// Delivered to the next request of any kind (a reconnect leak) instead of the next diagnostic request.
    stale_any: bool,
    orphan: Vec<String>,
    link_up: bool,
    // adapter state, reset by ATZ / a new connection
    echo: bool,
    headers: bool,
    spaces: bool,
    header: String,
    filter: Option<String>,
    searching: bool,
    /// Carly quirk (#215, #231): after any targeted `ATSH`, functional requests get `NO DATA` until `ATZ`.
    broken: bool,
}

impl Sim {
    fn new() -> Self {
        let mut sim = Self {
            now: 0,
            traces: Vec::new(),
            ecus: vec![VEcu::engine()],
            scripts: Vec::new(),
            hooks: Vec::new(),
            stale: Vec::new(),
            stale_any: false,
            orphan: Vec::new(),
            link_up: true,
            echo: true,
            headers: false,
            spaces: true,
            header: "7DF".into(),
            filter: None,
            searching: false,
            broken: false,
        };
        sim.reset_adapter();
        sim
    }

    fn reset_adapter(&mut self) {
        self.echo = true;
        self.headers = false;
        self.spaces = true;
        self.header = "7DF".into();
        self.filter = None;
        self.searching = false;
        self.broken = false;
    }

    fn handle(&mut self, command: &str, timeout_ms: u64) -> Result<String, String> {
        let started = self.now;
        let (result, elapsed) = self.resolve(command, timeout_ms);
        self.now = started + elapsed;
        self.traces.push(Trace {
            wire: command.to_owned(),
            started_ms: started,
            ended_ms: self.now,
            result: result.clone(),
        });
        result
    }

    fn take_script(&mut self, command: &str) -> Option<Behavior> {
        let header = self.header.clone();
        let index = self.scripts.iter().position(|(wire, wanted, _)| {
            wire == command && wanted.as_ref().is_none_or(|wanted| *wanted == header)
        })?;
        Some(self.scripts.remove(index).2)
    }

    fn resolve(&mut self, command: &str, timeout_ms: u64) -> (Result<String, String>, u64) {
        let shown = command.trim_end().to_owned();
        if !self.link_up {
            return (
                Err(format!("BLE link is down; request not sent: {shown}")),
                0,
            );
        }
        let diagnostic = !command.trim().to_ascii_uppercase().starts_with("AT");
        let mut prefix = Vec::new();
        if diagnostic || self.stale_any {
            prefix = std::mem::take(&mut self.stale);
            self.stale_any = false;
        }
        let (lines, prompt_at): (Vec<(u64, String)>, Option<u64>) = match self.take_script(command)
        {
            Some(Behavior::Lines { lines, prompt_at }) => (lines, prompt_at),
            Some(Behavior::Disconnect) => {
                let (model, _) = self.model(command);
                self.orphan = model;
                self.link_up = false;
                return (
                    Err(format!(
                        "BLE link lost while waiting for the reply to {shown}"
                    )),
                    50,
                );
            }
            None => {
                let (model, latency) = self.model(command);
                (
                    model.into_iter().map(|line| (latency, line)).collect(),
                    Some(latency),
                )
            }
        };
        let (arrived, late): (Vec<_>, Vec<_>) = lines
            .into_iter()
            .partition(|(arrival, _)| *arrival <= timeout_ms);
        self.stale.extend(late.into_iter().map(|(_, line)| line));
        match prompt_at {
            Some(prompt) if prompt <= timeout_ms => {
                let mut text = prefix;
                text.extend(arrived.into_iter().map(|(_, line)| line));
                (Ok(format!("{}\r>", text.join("\r"))), prompt)
            }
            _ => (
                Err(format!(
                    "adapter reply timed out after {timeout_ms} ms waiting for the prompt: {shown}"
                )),
                timeout_ms,
            ),
        }
    }

    fn render(&self, responder: &str, frame: &[u8]) -> String {
        let separator = if self.spaces { " " } else { "" };
        let body = frame
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(separator);
        if self.headers {
            format!("{responder}{separator}{body}")
        } else {
            body
        }
    }

    /// The adapter's lines and latency for one command; applies its state effects.
    fn model(&mut self, command: &str) -> (Vec<String>, u64) {
        let upper = command.trim().to_ascii_uppercase();
        let mut lines = Vec::new();
        if self.echo {
            lines.push(command.trim_end().to_owned());
        }
        if upper.starts_with("AT") {
            let mut latency = 20;
            match upper.as_str() {
                "ATZ" => {
                    self.reset_adapter();
                    lines.push("ELM327 v1.5".into());
                    latency = 800;
                }
                "ATI" => lines.push("ELM327 v1.5".into()),
                "AT@1" => lines.push("CARLY-UNIVERSAL".into()),
                "ATE0" | "ATE1" | "ATL0" | "ATL1" | "ATS0" | "ATS1" | "ATH0" | "ATH1" | "ATSP0" => {
                    match upper.as_str() {
                        "ATE0" => self.echo = false,
                        "ATE1" => self.echo = true,
                        "ATS0" => self.spaces = false,
                        "ATS1" => self.spaces = true,
                        "ATH0" => self.headers = false,
                        "ATH1" => self.headers = true,
                        "ATSP0" => self.searching = true,
                        _ => {}
                    }
                    lines.push("OK".into());
                }
                "ATCRA" => {
                    self.filter = None;
                    lines.push("OK".into());
                }
                other => {
                    let argument = other.split_once(' ').map(|(_, argument)| argument.trim());
                    let valid = argument.filter(|value| {
                        value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    });
                    match (other.split(' ').next(), valid) {
                        (Some("ATSH"), Some(value)) => {
                            self.header = value.to_owned();
                            if value != "7DF" {
                                self.broken = true;
                            }
                            lines.push("OK".into());
                        }
                        (Some("ATCRA"), Some(value)) => {
                            self.filter = Some(value.to_owned());
                            lines.push("OK".into());
                        }
                        _ => lines.push("?".into()),
                    }
                }
            }
            return (lines, latency);
        }
        let hex: String = upper.split_whitespace().collect();
        let Some(request) = parse_hex(&hex) else {
            lines.push("?".into());
            return (lines, 20);
        };
        let mut latency = 50;
        if self.searching {
            self.searching = false;
            lines.push("SEARCHING...".into());
            latency += 1_500;
        }
        let header = self.header.clone();
        let functional = header == "7DF";
        let now = self.now;
        let mut frames: Vec<(String, Vec<u8>)> = Vec::new();
        if !(functional && self.broken) {
            for ecu in self.ecus.iter_mut() {
                if functional || ecu.req == header {
                    for frame in ecu.respond(&request, now) {
                        frames.push((ecu.resp.clone(), frame));
                    }
                }
            }
        }
        if let Some(filter) = &self.filter {
            frames.retain(|(responder, _)| responder == filter);
        }
        if frames.is_empty() {
            lines.push("NO DATA".into());
            latency = 200;
        } else {
            latency += 30 * frames.len() as u64;
            for (responder, frame) in &frames {
                lines.push(self.render(responder, frame));
            }
        }
        (lines, latency)
    }
}

// ------------------------------------------------------------------------------------------------------
// scenarios
// ------------------------------------------------------------------------------------------------------

const KEY: &str = "air.charge_limit_overcharge";
const SESSION_KEY: &str = "air.intercooler_outlet_temperature";
const DID_READ: &str = "221356\r";
const ATSH_ENGINE: &str = "ATSH 7E0\r";
const ATCRA_ENGINE: &str = "ATCRA 7E8\r";
const GOOD: &str = "7E8 05 62 13 56 00 64";
const FULL_INIT: [&str; 9] = [
    "ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r", "0100\r",
];
const REINIT: [&str; 7] = [
    "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r", "0100\r",
];
const VIN_FF: &str = "7E8 10 14 49 02 01 57 56 57";
const VIN_CF1: &str = "7E8 21 5A 5A 5A 31 4B 5A 41";

/// Initialized, inventoried (real action) and ECU-discovered (real action) core.
fn engaged() -> (MobileCore<VirtualAdapter>, TraceProbe) {
    let (mut core, probe) = fixture();
    inventory_and_discover(&mut core);
    (core, probe)
}

fn value_of(envelope: &Value) -> Option<f64> {
    if envelope["ok"] == true {
        envelope["data"]["value"].as_f64()
    } else {
        None
    }
}

fn assert_tx(probe: &TraceProbe, mark: usize, expected: &[&str], what: &str) {
    let wanted: Vec<String> = expected.iter().map(|wire| (*wire).to_owned()).collect();
    assert_eq!(probe.wires_since(mark), wanted, "exact TX: {what}");
}

fn assert_unavailable(read: &Value, what: &str) {
    assert_eq!(read["ok"], true, "{what}: {read}");
    assert_eq!(read["data"]["status"], "unavailable", "{what}: {read}");
    assert!(read["data"]["value"].is_null(), "{what}: {read}");
}

fn assert_serial(probe: &TraceProbe) {
    assert_eq!(probe.outstanding(), 0);
    assert!(probe.peak_outstanding() <= 1, "serial adapter requests");
}

fn raw_of(trace: &Trace) -> &str {
    match &trace.result {
        Ok(text) => text,
        Err(error) => error,
    }
}

/// Runs every adverse transport case once (called from the parent's single measured suite test).
pub(super) fn run_fault_cases() {
    req_046_2_second_request_is_refused_while_an_exchange_is_outstanding();
    req_046_5_model_state_changes_the_observed_response();
    req_046_5_session_state_is_modeled_per_ecu_through_the_production_path();
    req_046_6_busy_is_intermediate_and_never_retried();
    req_046_7_pending_needs_a_decisive_reply_before_the_virtual_deadline();
    req_046_8_out_of_range_is_decisive_and_not_pending();
    req_046_9_a_reply_for_another_operation_gives_no_value();
    req_046_10_a_duplicated_valid_reply_is_one_measurement();
    req_046_11_a_non_target_ecu_never_supplies_the_value();
    req_046_12_a_late_reply_never_satisfies_the_expired_or_a_later_request();
    req_046_13_only_a_complete_reply_is_attributed();
    req_046_14_malformed_frames_give_no_value_and_no_healthy_zero();
    req_046_15_a_missing_prompt_is_a_bounded_timeout_and_no_assumed_readiness();
    req_046_16_unconfirmed_routing_stops_without_an_assumed_target();
    req_046_17_a_failed_return_to_default_session_stops_all_diagnosis();
    req_046_18_disconnect_fails_the_request_and_reconnect_cannot_fulfill_it();
}

// REQ-046.2: a second request while one exchange is outstanding is refused with a typed busy result, sends
// nothing, and the adapter probe observes at most one outstanding request. This proves the Rust owner lock
// (`execute_nonblocking`), not the Kotlin GATT framing.
fn req_046_2_second_request_is_refused_while_an_exchange_is_outstanding() {
    let (core, probe) = engaged();
    let core = Arc::new(Mutex::new(core));
    let refusal: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (hook_core, hook_slot) = (core.clone(), refusal.clone());
    probe.on_command(DID_READ, move || {
        let answer = execute_nonblocking(&hook_core, "assistant.tool", r#"{"tool":"signal_list"}"#);
        *hook_slot.lock().unwrap() = Some(answer);
    });
    let mark = probe.len();
    let read: Value = serde_json::from_str(&execute_nonblocking(&core, "read", KEY)).unwrap();
    assert_eq!(value_of(&read), Some(1.0), "{read}");
    let busy: Value =
        serde_json::from_str(refusal.lock().unwrap().as_ref().expect("hook ran")).unwrap();
    assert_eq!(busy["data"]["outcome"], "busy", "{busy}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.2",
    );
    assert_eq!(probe.peak_outstanding(), 1);
    assert_serial(&probe);
}

// REQ-046.5: the observed reply depends on adapter flags, reset, protocol search, routing header, response filter,
// multi-frame replies and per-ECU session state; the same wire gets different replies in different states.
fn req_046_5_model_state_changes_the_observed_response() {
    let (mut adapter, _probe) = adapter_only();
    let timeout = Duration::from_secs(3);
    let mut send = |wire: &str| -> String { adapter.exchange(wire, timeout).unwrap() };

    // power-on state: echo on
    assert_eq!(send("ATI\r"), "ATI\rELM327 v1.5\r>");
    assert_eq!(send("ATE0\r"), "ATE0\rOK\r>");
    assert_eq!(send("ATI\r"), "ELM327 v1.5\r>");
    for wire in ["ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"] {
        assert_eq!(send(wire), "OK\r>");
    }
    // protocol search happens once, on the first request after ATSP0
    assert_eq!(send("0100\r"), "SEARCHING...\r7E8 06 41 00 00 11 00 00\r>");
    assert_eq!(send("0100\r"), "7E8 06 41 00 00 11 00 00\r>");
    // header flag
    assert_eq!(send("010C\r"), "7E8 04 41 0C 1A F8\r>");
    assert_eq!(send("ATH0\r"), "OK\r>");
    assert_eq!(send("010C\r"), "04 41 0C 1A F8\r>");
    assert_eq!(send("ATH1\r"), "OK\r>");
    // multi-frame reply
    assert_eq!(
        send("0902\r"),
        "7E8 10 14 49 02 01 57 56 57\r7E8 21 5A 5A 5A 31 4B 5A 41\r7E8 22 4D 31 32 33 34 35 36\r>"
    );
    // routing header and response filter
    assert_eq!(send("ATSH 7E0\r"), "OK\r>");
    assert_eq!(send("ATCRA 7E8\r"), "OK\r>");
    assert_eq!(send("221356\r"), "7E8 05 62 13 56 00 64\r>");
    assert_eq!(send("ATCRA 7E9\r"), "OK\r>");
    assert_eq!(send("221356\r"), "NO DATA\r>"); // the filter hides 7E8
    assert_eq!(send("ATCRA\r"), "OK\r>");
    assert_eq!(send("221356\r"), "7E8 05 62 13 56 00 64\r>");
    assert_eq!(send("ATSH 7E2\r"), "OK\r>");
    assert_eq!(send("221356\r"), "NO DATA\r>"); // no ECU at 7E2
                                                // Carly quirk: functional requests stay dead after a targeted header until ATZ
    assert_eq!(send("ATSH 7DF\r"), "OK\r>");
    assert_eq!(send("010C\r"), "NO DATA\r>");
    // reset restores echo on, headers off and functional addressing
    assert_eq!(send("ATZ\r"), "ELM327 v1.5\r>"); // echo was off when ATZ arrived
    assert_eq!(send("010C\r"), "010C\r04 41 0C 1A F8\r>");
    // an unknown AT command or non-hex request is rejected by the adapter
    assert_eq!(send("ATXX\r"), "ATXX\r?\r>");
    assert_eq!(send("ZZ\r"), "ZZ\r?\r>");
}

// REQ-046.5 (continued): ECU session state (default/extended, S3 expiry on the virtual clock, tester present) is
// per ECU, and the real UDS session actions observe it.
fn req_046_5_session_state_is_modeled_per_ecu_through_the_production_path() {
    let (mut adapter, probe) = adapter_only();
    probe.add_ecu("7E1", "7E9");
    let timeout = Duration::from_secs(3);
    let mut send = |wire: &str| -> String { adapter.exchange(wire, timeout).unwrap() };
    for wire in ["ATZ\r", "ATE0\r", "ATH1\r", "ATSH 7E0\r"] {
        send(wire);
    }
    assert_eq!(send("221628\r"), "7E8 03 7F 22 7F\r>");
    assert_eq!(send("1003\r"), "7E8 06 50 03 00 32 01 F4\r>");
    assert_eq!(send("221628\r"), "7E8 05 62 16 28 0B 54\r>");
    assert_eq!(probe.session_of("7E0"), "extended");
    send("ATSH 7E1\r");
    assert_eq!(send("221628\r"), "7E9 03 7F 22 7F\r>"); // 7E1 never entered the session
    assert_eq!(probe.session_of("7E1"), "default");
    send("ATSH 7E0\r");
    probe.advance_ms(6_000);
    assert_eq!(probe.session_of("7E0"), "default"); // S3 expired without tester present
    assert_eq!(send("221628\r"), "7E8 03 7F 22 7F\r>");
    assert_eq!(send("1003\r"), "7E8 06 50 03 00 32 01 F4\r>");
    probe.advance_ms(3_000);
    assert_eq!(send("3E80\r"), "NO DATA\r>"); // suppressed positive response
    probe.advance_ms(3_000);
    assert_eq!(send("221628\r"), "7E8 05 62 16 28 0B 54\r>"); // tester present kept it alive
    assert_eq!(send("1001\r"), "7E8 02 50 01\r>");
    assert_eq!(send("221628\r"), "7E8 03 7F 22 7F\r>");
    assert_serial(&probe);

    // The production session actions: no 1003, no session value; with 1003, the value; 1001 ends it.
    let (mut core, probe) = engaged();
    let mark = probe.len();
    let enabled = run(&mut core, "uds.enable", "");
    assert_eq!(enabled["data"]["active"], true, "{enabled}");
    assert_eq!(probe.session_of("7E0"), "extended");
    let session = run(&mut core, "read", SESSION_KEY);
    assert_eq!(session["ok"], true, "{session}");
    assert!(value_of(&session).is_some(), "{session}");
    let disabled = run(&mut core, "uds.disable", "");
    assert_eq!(disabled["data"]["active"], false, "{disabled}");
    assert_eq!(probe.session_of("7E0"), "default");
    let wires = probe.wires_since(mark);
    assert_eq!(
        wires[..4].to_vec(),
        vec![ATSH_ENGINE, ATCRA_ENGINE, "1003\r", "221628\r"],
        "{wires:?}"
    );
    assert_eq!(wires[4], "1001\r", "{wires:?}");
    assert_serial(&probe);
}

// REQ-046.6: NRC 0x21 is intermediate. A decisive reply in the same exchange decides; busy alone yields no value
// and the busy response stays visible; the read is sent exactly once (no unbounded retry).
fn req_046_6_busy_is_intermediate_and_never_retried() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::timed(&[(5, "7E8 03 7F 22 21"), (300, GOOD)], Some(300)),
    );
    let mark = probe.len();
    let decisive = run(&mut core, "read", KEY);
    assert_eq!(decisive["data"]["status"], "available", "{decisive}");
    assert_eq!(value_of(&decisive), Some(1.0));
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.6 decisive",
    );
    let traces = probe.transactions_since(mark);
    assert_eq!(traces[2].ended_ms - traces[2].started_ms, 300);
    assert!(
        raw_of(&traces[2]).contains("7F 22 21") && raw_of(&traces[2]).contains("62 13 56 00 64")
    );

    probe.script(DID_READ, Some("7E0"), Behavior::reply(&["7E8 03 7F 22 21"]));
    let mark = probe.len();
    let busy = run(&mut core, "read", KEY);
    assert_unavailable(&busy, "046.6 busy only");
    assert_eq!(busy["data"]["nrc"], 0x21, "{busy}");
    assert_tx(&probe, mark, &[DID_READ], "046.6 busy only, one request");
    assert_eq!(
        busy["data"]["observations"][0]["responses"][0]["payload"],
        json!([0x7F, 0x22, 0x21])
    );
    assert_eq!(probe.pending_scripts(), 0);
    assert_serial(&probe);
}

// REQ-046.7: NRC 0x78 is intermediate. A decisive reply inside the exchange decides (here at 4000 ms); a pending-only
// reply gives no value (nrc 0x78 stays visible); a decisive reply after the 5000 ms deadline is a timeout.
fn req_046_7_pending_needs_a_decisive_reply_before_the_virtual_deadline() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::timed(&[(5, "7E8 03 7F 22 78"), (4_000, GOOD)], Some(4_000)),
    );
    let mark = probe.len();
    let decisive = run(&mut core, "read", KEY);
    assert_eq!(value_of(&decisive), Some(1.0), "{decisive}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.7 decisive",
    );
    let traces = probe.transactions_since(mark);
    assert_eq!(traces[2].ended_ms - traces[2].started_ms, 4_000);

    probe.script(DID_READ, Some("7E0"), Behavior::reply(&["7E8 03 7F 22 78"]));
    let mark = probe.len();
    let pending = run(&mut core, "read", KEY);
    assert_unavailable(&pending, "046.7 pending only");
    assert_eq!(pending["data"]["nrc"], 0x78, "{pending}");
    assert_tx(&probe, mark, &[DID_READ], "046.7 pending only, one request");

    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::timed(&[(5, "7E8 03 7F 22 78"), (6_000, GOOD)], Some(6_000)),
    );
    let (mark, started) = (probe.len(), probe.now_ms());
    let expired = run(&mut core, "read", KEY);
    assert_unavailable(&expired, "046.7 decisive after the deadline");
    assert_tx(&probe, mark, &[DID_READ], "046.7 expiry, one request");
    let traces = probe.transactions_since(mark);
    assert!(traces[0]
        .result
        .as_ref()
        .is_err_and(|error| error.contains("timed out")));
    assert_eq!(traces[0].ended_ms - traces[0].started_ms, 5_000);
    assert_eq!(
        probe.now_ms() - started,
        5_000,
        "the virtual deadline, not the late reply"
    );
    assert_serial(&probe);
}

// REQ-046.8: NRC 0x31 is decisive: no value, nrc visible, no further request as if it were pending.
fn req_046_8_out_of_range_is_decisive_and_not_pending() {
    let (mut core, probe) = engaged();
    probe.script(DID_READ, Some("7E0"), Behavior::reply(&["7E8 03 7F 22 31"]));
    let mark = probe.len();
    let rejected = run(&mut core, "read", KEY);
    assert_unavailable(&rejected, "046.8");
    assert_eq!(rejected["data"]["nrc"], 0x31, "{rejected}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.8",
    );
    assert_eq!(probe.pending_scripts(), 0);
    assert_serial(&probe);
}

// REQ-046.9: the requested ECU answers another operation: a Knowledge DID echo for 0x1357, and a Mode 01 reply for
// PID 0D to a 010C request. Neither supplies a value; the actual replies stay in the trace.
fn req_046_9_a_reply_for_another_operation_gives_no_value() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E8 05 62 13 57 00 64"]),
    );
    let mark = probe.len();
    let other_did = run(&mut core, "read", KEY);
    assert_unavailable(&other_did, "046.9 wrong DID");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.9 wrong DID",
    );
    assert!(raw_of(&probe.transactions_since(mark)[2]).contains("62 13 57"));

    probe.script("010C\r", Some("7E0"), Behavior::reply(&["7E8 03 41 0D 32"]));
    let mark = probe.len();
    let other_pid = run(&mut core, "read", "engine.rpm");
    assert!(value_of(&other_pid).is_none(), "{other_pid}");
    assert_tx(
        &probe,
        mark,
        &["010C\r"],
        "046.9 wrong PID (target stays configured)",
    );
    assert!(raw_of(&probe.transactions_since(mark)[0]).contains("41 0D 32"));
    assert_serial(&probe);
}

// REQ-046.10: one valid reply delivered twice is one measurement: one request, one value, both lines kept as RX.
fn req_046_10_a_duplicated_valid_reply_is_one_measurement() {
    let (mut core, probe) = engaged();
    probe.script(
        "010C\r",
        Some("7E0"),
        Behavior::reply(&["7E8 04 41 0C 1A F8", "7E8 04 41 0C 1A F8"]),
    );
    let mark = probe.len();
    let rpm = run(&mut core, "read", "engine.rpm");
    assert_eq!(value_of(&rpm), Some(1726.0), "{rpm}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, "010C\r"],
        "046.10 mode 01",
    );
    assert_eq!(
        raw_of(&probe.transactions_since(mark)[2])
            .matches("41 0C 1A F8")
            .count(),
        2
    );
    assert_eq!(rpm["data"]["observations"].as_array().unwrap().len(), 1);

    probe.script(DID_READ, Some("7E0"), Behavior::reply(&[GOOD, GOOD]));
    let mark = probe.len();
    let knowledge = run(&mut core, "read", KEY);
    assert_eq!(value_of(&knowledge), Some(1.0), "{knowledge}");
    // the same target stays configured: exactly one request, no second "completed" read
    assert_tx(&probe, mark, &[DID_READ], "046.10 knowledge");
    assert_serial(&probe);
}

// REQ-046.11: only a non-target ECU (7E9) answers a request to 7E0/7E8; the adapter leaks it past the 7E8 filter.
// No value; the actual source is visible.
fn req_046_11_a_non_target_ecu_never_supplies_the_value() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E9 05 62 13 56 00 64"]),
    );
    let mark = probe.len();
    let knowledge = run(&mut core, "read", KEY);
    assert_unavailable(&knowledge, "046.11 knowledge");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.11 knowledge",
    );
    assert_eq!(
        knowledge["data"]["observations"][0]["responses"][0]["responder"],
        "7E9"
    );
    assert!(knowledge["data"]["observations"][0]["selected_responder"].is_null());

    probe.script(
        "010C\r",
        Some("7E0"),
        Behavior::reply(&["7E9 04 41 0C 0C 80"]),
    );
    let mark = probe.len();
    let rpm = run(&mut core, "read", "engine.rpm");
    assert_eq!(rpm["ok"], false, "{rpm}");
    assert!(
        rpm["error"]
            .as_str()
            .unwrap()
            .contains("unexpected responder 7E9"),
        "{rpm}"
    );
    assert_eq!(rpm["observations"][0]["responses"][0]["responder"], "7E9");
    assert_tx(&probe, mark, &["010C\r"], "046.11 mode 01");
    assert_serial(&probe);
}

// REQ-046.12: the reply arrives at 6000 ms, after the 5000 ms exchange expired. The timeout stays; the late bytes
// (kept by the platform until the next request) are offered in front of the next request's reply and must not
// satisfy it; a later clean read gets only its own value.
fn req_046_12_a_late_reply_never_satisfies_the_expired_or_a_later_request() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::timed(&[(5, "7E8 03 7F 22 78"), (6_000, GOOD)], Some(6_000)),
    );
    let mark = probe.len();
    let expired = run(&mut core, "read", KEY);
    assert_unavailable(&expired, "046.12 expired");
    let traces = probe.transactions_since(mark);
    assert!(traces[2]
        .result
        .as_ref()
        .is_err_and(|error| error.contains("timed out")));

    // The ECU now genuinely rejects; the stale (valid-looking) value arrives in front of that reply.
    probe.script(DID_READ, Some("7E0"), Behavior::reply(&["7E8 03 7F 22 31"]));
    let mark = probe.len();
    let next = run(&mut core, "read", KEY);
    assert_unavailable(&next, "046.12 next request with stale bytes");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.12 next request",
    );
    let raw = raw_of(&probe.transactions_since(mark)[2]).to_owned();
    assert!(
        raw.contains("62 13 56 00 64") && raw.contains("7F 22 31"),
        "{raw}"
    );

    let mark = probe.len();
    let clean = run(&mut core, "read", KEY);
    assert_eq!(value_of(&clean), Some(1.0), "{clean}");
    assert_tx(&probe, mark, &[DID_READ], "046.12 clean read");
    assert!(!raw_of(&probe.transactions_since(mark)[0]).contains("7F"));
    assert_serial(&probe);
}

// REQ-046.13: fragments. A complete first+consecutive frame is attributed; a first frame alone, and a VIN missing its
// last consecutive frame, never yield a value or identity (the VIN read is repeated once by the real inventory).
fn req_046_13_only_a_complete_reply_is_attributed() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E8 10 05 62 13 56 00", "7E8 21 64"]),
    );
    let mark = probe.len();
    let complete = run(&mut core, "read", KEY);
    assert_eq!(value_of(&complete), Some(1.0), "{complete}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.13 complete",
    );

    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E8 10 05 62 13 56 00"]),
    );
    let mark = probe.len();
    let first_only = run(&mut core, "read", KEY);
    assert_unavailable(&first_only, "046.13 first frame only");
    assert_tx(&probe, mark, &[DID_READ], "046.13 first frame only");

    // VIN: first attempt truncated, retry complete -> identity from the complete reply only
    let (mut core, probe) = fixture();
    probe.script("0902\r", Some("7DF"), Behavior::reply(&[VIN_FF, VIN_CF1]));
    let mark = probe.len();
    let inventory = run(&mut core, "inventory", "");
    assert_eq!(inventory["data"]["vin"], "WVWZZZ1KZAM123456", "{inventory}");
    assert!(inventory["data"]["identity_error"].is_null(), "{inventory}");
    assert_tx(
        &probe,
        mark,
        &[
            "0100\r",
            "0902\r",
            "0100\r",
            "0902\r",
            ATSH_ENGINE,
            ATCRA_ENGINE,
            "010C\r",
        ],
        "046.13 VIN retry once",
    );
    assert_eq!(core.vehicle_manufacturer.as_deref(), Some("vag"));

    // VIN: both attempts truncated -> no identity, no manufacturer, discovery refuses without TX
    let (mut core, probe) = fixture();
    probe.script("0902\r", Some("7DF"), Behavior::reply(&[VIN_FF, VIN_CF1]));
    probe.script("0902\r", Some("7DF"), Behavior::reply(&[VIN_FF, VIN_CF1]));
    let mark = probe.len();
    let inventory = run(&mut core, "inventory", "");
    assert_eq!(
        inventory["data"]["identity_error"], "truncated ISO-TP Mode 09 response",
        "{inventory}"
    );
    assert!(inventory["data"]["vin"].is_null(), "{inventory}");
    assert!(core.vehicle_manufacturer.is_none());
    assert_tx(
        &probe,
        mark,
        &[
            "0100\r",
            "0902\r",
            "0100\r",
            "0902\r",
            ATSH_ENGINE,
            ATCRA_ENGINE,
            "010C\r",
        ],
        "046.13 VIN truncated twice",
    );
    let mark = probe.len();
    let discovery = run(&mut core, "ecu.discover", "");
    assert_eq!(discovery["ok"], false, "{discovery}");
    assert_eq!(probe.len(), mark, "no TX on an unidentified vehicle");
    assert_serial(&probe);
}

// REQ-046.14: malformed frames (non-hex byte, truncated single frames, truncated DTC answer) give no value and no
// "no DTCs" claim.
fn req_046_14_malformed_frames_give_no_value_and_no_healthy_zero() {
    let (mut core, probe) = engaged();
    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E8 05 62 13 5G 00 64"]),
    );
    let mark = probe.len();
    let bad_hex = run(&mut core, "read", KEY);
    assert_unavailable(&bad_hex, "046.14 non-hex");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.14 non-hex",
    );
    assert!(raw_of(&probe.transactions_since(mark)[2]).contains("5G"));

    probe.script(
        DID_READ,
        Some("7E0"),
        Behavior::reply(&["7E8 05 62 13 56 00"]),
    );
    let mark = probe.len();
    let truncated = run(&mut core, "read", KEY);
    assert_unavailable(&truncated, "046.14 truncated single frame");
    assert_tx(&probe, mark, &[DID_READ], "046.14 truncated single frame");

    probe.script("010C\r", Some("7E0"), Behavior::reply(&["7E8 04 41 0C 1A"]));
    let mark = probe.len();
    let rpm = run(&mut core, "read", "engine.rpm");
    assert!(value_of(&rpm).is_none(), "{rpm}");
    assert_tx(&probe, mark, &["010C\r"], "046.14 truncated Mode 01");

    probe.script("03\r", Some("7DF"), Behavior::reply(&["7E8 03 43 04"]));
    let mark = probe.len();
    let scan = run(&mut core, "dtc.scan", "");
    let wires = probe.wires_since(mark);
    assert_eq!(wires[..REINIT.len()].to_vec(), REINIT.to_vec(), "{wires:?}");
    assert!(
        wires[REINIT.len()..] == ["03\r"] || wires[REINIT.len()..] == ["03\r", "07\r"],
        "{wires:?}"
    );
    let reports_no_dtcs = scan["data"]["observations"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["no_dtcs"] == true));
    assert!(
        !reports_no_dtcs,
        "a truncated DTC answer is not a clean scan: {scan}"
    );
    assert_serial(&probe);
}

// REQ-046.15: the prompt never comes: a bounded timeout at the request's own limit (5000 ms targeted read, 3000 ms
// adapter setup), no retry, no autonomous follow-up; setup stops at the failed command and later actions are refused
// without TX.
fn req_046_15_a_missing_prompt_is_a_bounded_timeout_and_no_assumed_readiness() {
    let (mut core, probe) = engaged();
    probe.script(
        "010C\r",
        Some("7E0"),
        Behavior::timed(&[(5, "7E8 04 41 0C 1A F8")], None),
    );
    let mark = probe.len();
    let rpm = run(&mut core, "read", "engine.rpm");
    assert!(value_of(&rpm).is_none(), "{rpm}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, "010C\r"],
        "046.15 targeted read",
    );
    let traces = probe.transactions_since(mark);
    assert!(traces[2]
        .result
        .as_ref()
        .is_err_and(|error| error.contains("timed out")));
    assert_eq!(traces[2].ended_ms - traces[2].started_ms, 5_000);
    assert_eq!(
        probe.len(),
        mark + 3,
        "nothing is sent on its own afterwards"
    );
    // only a new request starts a new exchange, and it re-establishes the target first
    let retry_mark = probe.len();
    let retry = run(&mut core, "read", "engine.rpm");
    assert_eq!(value_of(&retry), Some(1726.0), "{retry}");
    assert_tx(
        &probe,
        retry_mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, "010C\r"],
        "046.15 new request reconfigures",
    );

    let (mut core, probe) = unbooted();
    probe.script("ATSP0\r", Some("7DF"), Behavior::timed(&[(5, "OK")], None));
    let init = run(&mut core, "initialize", "");
    assert_eq!(init["ok"], false, "{init}");
    assert_tx(
        &probe,
        0,
        &[
            "ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r",
        ],
        "046.15 initialize stops at the failed command",
    );
    let last = probe.transactions().pop().unwrap();
    assert_eq!(last.ended_ms - last.started_ms, 3_000);
    let mark = probe.len();
    for action in ["inventory", "dtc.scan", "ecu.discover"] {
        let refused = run(&mut core, action, "");
        assert_eq!(refused["ok"], false, "{action}: {refused}");
        assert!(refused["error"]
            .as_str()
            .unwrap()
            .contains("initialize the adapter first"));
    }
    let refused = run(&mut core, "read", "engine.rpm");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        probe.len(),
        mark,
        "no new adapter exchange on assumed readiness"
    );
    assert_serial(&probe);
}

// REQ-046.16: routing (response filter) is not confirmed (`?` instead of OK). The read stops before any request; the
// next read re-establishes routing. During inventory the engine target is not validated, never assumed, and no
// targeted request follows.
fn req_046_16_unconfirmed_routing_stops_without_an_assumed_target() {
    let (mut core, probe) = engaged();
    probe.script(ATCRA_ENGINE, Some("7E0"), Behavior::reply(&["?"]));
    let mark = probe.len();
    let stopped = run(&mut core, "read", KEY);
    assert_unavailable(&stopped, "046.16 read");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE],
        "046.16: no DID request after unconfirmed routing",
    );
    let mark = probe.len();
    let again = run(&mut core, "read", KEY);
    assert_eq!(value_of(&again), Some(1.0), "{again}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.16: routing is re-confirmed, not assumed",
    );

    let (mut core, probe) = fixture();
    probe.script(ATCRA_ENGINE, Some("7E0"), Behavior::reply(&["?"]));
    let mark = probe.len();
    let inventory = run(&mut core, "inventory", "");
    assert_eq!(inventory["ok"], true, "{inventory}");
    assert!(
        inventory["data"]["engine_target_error"].as_str().is_some(),
        "{inventory}"
    );
    assert!(
        inventory["data"]["engine_target_validated"].is_null(),
        "{inventory}"
    );
    assert!(core.engine.is_none());
    assert_tx(
        &probe,
        mark,
        &["0100\r", "0902\r", ATSH_ENGINE, ATCRA_ENGINE],
        "046.16 inventory stops at the unconfirmed routing",
    );
    // Bounded existing cleanup: with no validated target a read is a functional read after a full reinitialization;
    // it never uses the unconfirmed 7E0/7E8 routing.
    let mark = probe.len();
    let read = run(&mut core, "read", "engine.rpm");
    assert_eq!(value_of(&read), Some(1726.0), "{read}");
    let mut expected = REINIT.to_vec();
    expected.push("010C\r");
    assert_tx(
        &probe,
        mark,
        &expected,
        "046.16 functional read after reinitialization",
    );
    assert_serial(&probe);
}

// REQ-046.17: the return to the default session fails (negative reply, then timeout). The session is indeterminate,
// the ECU model still is in the extended session, and every later diagnostic action is refused without TX until a
// new connection is established.
fn req_046_17_a_failed_return_to_default_session_stops_all_diagnosis() {
    for (name, behavior) in [
        ("negative reply", Behavior::reply(&["7E8 03 7F 10 22"])),
        ("timeout", Behavior::silent()),
    ] {
        let (mut core, probe) = engaged();
        let mark = probe.len();
        let enabled = run(&mut core, "uds.enable", "");
        assert_eq!(enabled["data"]["active"], true, "{name}: {enabled}");
        assert_tx(
            &probe,
            mark,
            &[ATSH_ENGINE, ATCRA_ENGINE, "1003\r"],
            "046.17 enable",
        );
        probe.script("1001\r", Some("7E0"), behavior);
        let mark = probe.len();
        let disabled = run(&mut core, "uds.disable", "");
        assert_eq!(
            disabled["data"]["state"], "indeterminate",
            "{name}: {disabled}"
        );
        assert!(
            disabled["data"]["error"].as_str().is_some(),
            "{name}: {disabled}"
        );
        assert_tx(
            &probe,
            mark,
            &["1001\r"],
            "046.17 exactly one return attempt",
        );
        if name == "negative reply" {
            assert_eq!(
                probe.session_of("7E0"),
                "extended",
                "a refusal leaves the vehicle in the session"
            );
        }

        let mark = probe.len();
        for action in ["dtc.scan", "inventory"] {
            let refused = run(&mut core, action, "");
            assert_eq!(refused["ok"], false, "{name} {action}: {refused}");
            assert!(
                refused["error"].as_str().unwrap().contains("indeterminate"),
                "{refused}"
            );
        }
        let read = run(&mut core, "read", KEY);
        assert_eq!(read["ok"], false, "{name}: {read}");
        let rpm = run(&mut core, "read", "engine.rpm");
        assert_eq!(rpm["ok"], false, "{name}: {rpm}");
        let again = run(&mut core, "uds.enable", "");
        assert_eq!(again["data"]["active"], false, "{name}: {again}");
        assert_eq!(again["data"]["state"], "indeterminate", "{name}: {again}");
        assert_eq!(
            probe.len(),
            mark,
            "{name}: no TX while the session state is unknown"
        );

        // only a newly established connection clears it
        probe.advance_ms(6_000);
        probe.reconnect(false);
        core.on_reconnect();
        let mark = probe.len();
        let initialized = run(&mut core, "initialize", "");
        assert_eq!(initialized["ok"], true, "{name}: {initialized}");
        assert_tx(&probe, mark, &FULL_INIT, "046.17 new connection state");
        assert_eq!(probe.session_of("7E0"), "default");
        assert_serial(&probe);
    }
}

// REQ-046.18: the link drops while a request is pending. The request fails as a disconnect; with the link down only
// failed attempts happen; after `on_disconnect` nothing is sent; after a reconnect (even with the orphaned reply
// leaking into the first exchange) nothing is fulfilled from it, a new request needs newly established state, and
// it gets only its own value.
fn req_046_18_disconnect_fails_the_request_and_reconnect_cannot_fulfill_it() {
    let (mut core, probe) = engaged();
    probe.script(DID_READ, Some("7E0"), Behavior::Disconnect);
    let mark = probe.len();
    let dropped = run(&mut core, "read", KEY);
    assert_unavailable(&dropped, "046.18 disconnect");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.18",
    );
    let traces = probe.transactions_since(mark);
    assert!(traces[2]
        .result
        .as_ref()
        .is_err_and(|error| error.contains("link lost")));
    assert!(!probe.link_up());

    // link still down, platform has not told the core yet: the attempt fails at the boundary
    let mark = probe.len();
    let blind = run(&mut core, "read", KEY);
    assert_unavailable(&blind, "046.18 link down");
    assert!(probe
        .transactions_since(mark)
        .iter()
        .all(|trace| trace.result.is_err()));

    core.on_disconnect();
    let mark = probe.len();
    let refused = run(&mut core, "read", KEY);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(probe.len(), mark, "no TX after the disconnect");

    probe.set_did("7E0", 0x1356, &[0x00, 0xC8], false); // the vehicle's value afterwards: 2.0, not 1.0
    probe.reconnect(true);
    core.on_reconnect();
    let refused = run(&mut core, "read", KEY);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        probe.len(),
        mark,
        "reconnect alone re-sends or fulfills nothing"
    );

    let mark = probe.len();
    establish(&mut core);
    let after = probe.transactions_since(mark);
    assert_eq!(after[0].wire, "ATI\r");
    assert!(
        raw_of(&after[0]).contains("62 13 56 00 64"),
        "the orphaned reply was offered to the first exchange"
    );
    let mark = probe.len();
    let fresh = run(&mut core, "read", KEY);
    assert_eq!(value_of(&fresh), Some(2.0), "{fresh}");
    assert_tx(
        &probe,
        mark,
        &[ATSH_ENGINE, ATCRA_ENGINE, DID_READ],
        "046.18 newly established state",
    );
    assert_serial(&probe);
}
