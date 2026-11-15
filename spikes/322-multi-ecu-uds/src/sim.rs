//! Stateful ELM327-like adapter and ECU models on a virtual clock (milliseconds).

pub type Ms = u64;

#[derive(Debug, PartialEq, Eq)]
pub enum ExchangeError {
    /// No prompt within the transport timeout (5 s of virtual time pass).
    NoPrompt,
    Disconnected,
}

#[derive(Clone)]
pub struct Did {
    pub id: u16,
    pub data: Vec<u8>,
    pub session_only: bool,
}

pub struct Ecu {
    pub req: &'static str,
    pub resp: &'static str,
    /// Real S3 timeout of this simulated ECU. The controller never sees it.
    pub s3_ms: Ms,
    pub latency_ms: Ms,
    /// ISO 14229-2 says any request resets S3; some ECUs only honour 3E.
    pub any_request_resets_s3: bool,
    /// NRC answered for a session-only DID outside the extended session.
    pub gated_nrc: u8,
    pub dids: Vec<Did>,
    pub pids: Vec<(u8, Vec<u8>)>,
    pub extended: bool,
    pub last_s3_reset: Ms,
    pub expiries: Vec<Ms>,
}

impl Ecu {
    pub fn new(req: &'static str, resp: &'static str) -> Self {
        Self {
            req,
            resp,
            s3_ms: 5000,
            latency_ms: 60,
            any_request_resets_s3: true,
            gated_nrc: 0x31,
            dids: Vec::new(),
            pids: Vec::new(),
            extended: false,
            last_s3_reset: 0,
            expiries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    /// Answer with this negative response code for the request's service.
    Nrc(u8),
    /// The request never reaches the ECU: adapter says NO DATA.
    Lost,
    /// The ECU processes the request but its reply is lost.
    ReplyLost,
    /// The ECU processes the request; the reply arrives on the next exchange.
    Late,
    /// The ECU is busy for this long before answering normally.
    Block(Ms),
    /// Normal reply plus one extra line printed by the adapter.
    Extra(String),
    /// Multi-frame reply cut after the first frame.
    Truncated,
    /// The adapter never returns a prompt.
    NoPrompt,
}

pub struct Rule {
    /// Request id of the selected ECU (`ATSH` value) or `*`.
    pub ecu: &'static str,
    pub wire: String,
    skip: u32,
    times: Option<u32>,
    pub action: Action,
}

impl Rule {
    pub fn always(ecu: &'static str, wire: &str, action: Action) -> Self {
        Self {
            ecu,
            wire: wire.into(),
            skip: 0,
            times: None,
            action,
        }
    }
    /// Only the n-th (1-based) matching request.
    pub fn nth(ecu: &'static str, wire: &str, n: u32, action: Action) -> Self {
        Self {
            ecu,
            wire: wire.into(),
            skip: n - 1,
            times: Some(1),
            action,
        }
    }
    /// `times` consecutive matching requests starting with the n-th (1-based).
    pub fn nth_times(ecu: &'static str, wire: &str, n: u32, times: u32, action: Action) -> Self {
        Self {
            ecu,
            wire: wire.into(),
            skip: n - 1,
            times: Some(times),
            action,
        }
    }
}

pub struct Sim {
    pub clock: Ms,
    pub ecus: Vec<Ecu>,
    pub rules: Vec<Rule>,
    /// Every command that reached the adapter, with its virtual send time.
    pub log: Vec<(Ms, String)>,
    pub elm_timeout_ms: Ms,
    pub header_ms: Ms,
    pub sh: String,
    pub cra: Option<String>,
    pub disconnected: bool,
    late: Option<(String, Vec<String>)>,
}

fn decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn frames(resp: &str, payload: &[u8]) -> Vec<String> {
    let hex = |b: &[u8]| b.iter().map(|x| format!(" {x:02X}")).collect::<String>();
    if payload.len() <= 7 {
        return vec![format!("{resp} {:02X}{}", payload.len(), hex(payload))];
    }
    let mut out = vec![format!(
        "{resp} 1{:01X} {:02X}{}",
        payload.len() >> 8,
        payload.len() & 0xFF,
        hex(&payload[..6])
    )];
    for (i, chunk) in payload[6..].chunks(7).enumerate() {
        out.push(format!("{resp} 2{:01X}{}", (i + 1) % 16, hex(chunk)));
    }
    out
}

impl Sim {
    pub fn new(ecus: Vec<Ecu>) -> Self {
        Self {
            clock: 0,
            ecus,
            rules: Vec::new(),
            log: Vec::new(),
            elm_timeout_ms: 200,
            header_ms: 40,
            sh: "7DF".into(),
            cra: None,
            disconnected: false,
            late: None,
        }
    }

    fn expire(&mut self) {
        let now = self.clock;
        for e in &mut self.ecus {
            if e.extended && now.saturating_sub(e.last_s3_reset) > e.s3_ms {
                e.extended = false;
                e.expiries.push(now);
            }
        }
    }

    fn take_rule(&mut self, wire: &str) -> Option<Action> {
        for r in &mut self.rules {
            if (r.ecu == "*" || r.ecu == self.sh) && r.wire == wire {
                if r.skip > 0 {
                    r.skip -= 1;
                    continue;
                }
                match r.times {
                    Some(0) => continue,
                    Some(n) => r.times = Some(n - 1),
                    None => {}
                }
                return Some(r.action.clone());
            }
        }
        None
    }

    pub fn exchange(&mut self, cmd: &str) -> Result<String, ExchangeError> {
        if self.disconnected {
            return Err(ExchangeError::Disconnected);
        }
        self.log.push((self.clock, cmd.to_string()));
        self.expire();
        let mut out = String::new();
        if let Some((resp, lines)) = self.late.take() {
            if self.cra.as_deref().map_or(true, |c| c == resp) {
                for l in lines {
                    out.push_str(&l);
                    out.push('\r');
                }
            }
        }
        let rule = self.take_rule(cmd);
        if matches!(rule, Some(Action::NoPrompt)) {
            self.clock += 5000;
            return Err(ExchangeError::NoPrompt);
        }
        if let Some(h) = cmd.strip_prefix("ATSH ") {
            self.clock += self.header_ms;
            self.sh = h.to_string();
            out.push_str("OK\r");
        } else if let Some(r) = cmd.strip_prefix("ATCRA ") {
            self.clock += self.header_ms;
            self.cra = Some(r.to_string());
            out.push_str("OK\r");
        } else if let Some(bytes) = decode(cmd) {
            self.request(&bytes, rule, &mut out);
        } else {
            out.push_str("?\r");
        }
        out.push('>');
        Ok(out)
    }

    fn request(&mut self, bytes: &[u8], rule: Option<Action>, out: &mut String) {
        let Some(i) = self.ecus.iter().position(|e| e.req == self.sh) else {
            self.clock += self.elm_timeout_ms;
            out.push_str("NO DATA\r");
            return;
        };
        if matches!(rule, Some(Action::Lost)) {
            self.clock += self.elm_timeout_ms;
            out.push_str("NO DATA\r");
            return;
        }
        if let Some(Action::Block(ms)) = rule {
            self.clock += ms;
            self.expire();
        }
        let reply = match &rule {
            Some(Action::Nrc(n)) => {
                self.touch(i, bytes);
                Some(vec![0x7F, bytes[0], *n])
            }
            _ => self.process(i, bytes),
        };
        let resp = self.ecus[i].resp;
        let visible = self.cra.as_deref().map_or(true, |c| c == resp);
        let mut lines = reply
            .as_deref()
            .map(|p| frames(resp, p))
            .unwrap_or_default();
        if matches!(rule, Some(Action::Truncated)) {
            lines.truncate(1);
        }
        match rule {
            Some(Action::ReplyLost) => lines.clear(),
            Some(Action::Late) => {
                self.late = Some((resp.to_string(), std::mem::take(&mut lines)));
            }
            _ => {}
        }
        if !visible || lines.is_empty() {
            self.clock += self.elm_timeout_ms;
            out.push_str("NO DATA\r");
        } else {
            self.clock += self.ecus[i].latency_ms;
            for l in lines {
                out.push_str(&l);
                out.push('\r');
            }
        }
        if let Some(Action::Extra(line)) = rule {
            out.push_str(&line);
            out.push('\r');
        }
    }

    fn touch(&mut self, i: usize, bytes: &[u8]) {
        let now = self.clock;
        let e = &mut self.ecus[i];
        if e.extended && (e.any_request_resets_s3 || bytes == [0x3E, 0x80]) {
            e.last_s3_reset = now;
        }
    }

    fn process(&mut self, i: usize, bytes: &[u8]) -> Option<Vec<u8>> {
        let now = self.clock;
        self.touch(i, bytes);
        let e = &mut self.ecus[i];
        match bytes {
            [0x10, 0x03] => {
                e.extended = true;
                e.last_s3_reset = now;
                Some(vec![0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])
            }
            [0x10, 0x01] => {
                e.extended = false;
                Some(vec![0x50, 0x01, 0x00, 0x32, 0x01, 0xF4])
            }
            [0x3E, 0x80] => None,
            [0x22, h, l] => {
                let id = u16::from_be_bytes([*h, *l]);
                match e.dids.iter().find(|d| d.id == id) {
                    None => Some(vec![0x7F, 0x22, 0x31]),
                    Some(d) if d.session_only && !e.extended => Some(vec![0x7F, 0x22, e.gated_nrc]),
                    Some(d) => {
                        let mut p = vec![0x62, *h, *l];
                        p.extend(&d.data);
                        Some(p)
                    }
                }
            }
            [0x01, pid] => match e.pids.iter().find(|(p, _)| p == pid) {
                Some((_, data)) => {
                    let mut p = vec![0x41, *pid];
                    p.extend(data);
                    Some(p)
                }
                None => Some(vec![0x7F, 0x01, 0x12]),
            },
            [sid, ..] => Some(vec![0x7F, *sid, 0x11]),
            [] => None,
        }
    }
}
