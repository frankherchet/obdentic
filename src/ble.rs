use crate::{ReadRequest, Transaction};
use btleplug::{
    api::{
        bleuuid::uuid_from_u16, Central, CharPropFlags, Characteristic, Manager as _,
        Peripheral as _, ScanFilter, ValueNotification, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, time::Duration};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

const CARLY_SERVICE: u16 = 0xFFE0;
const CARLY_CHANNEL: u16 = 0xFFE1;
const SCAN_TIMEOUT: Duration = Duration::from_secs(5);
const FIND_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct AdapterCandidate {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
}

pub async fn scan() -> Result<Vec<AdapterCandidate>, String> {
    let (_manager, adapter) = central().await?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|error| format!("Bluetooth scan failed: {error}"))?;
    sleep(SCAN_TIMEOUT).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|error| format!("Bluetooth scan result failed: {error}"));
    let _ = adapter.stop_scan().await;

    let mut candidates = Vec::new();
    for peripheral in peripherals? {
        let Some(properties) = peripheral.properties().await.ok().flatten() else {
            continue;
        };
        let Some(name) = properties.local_name else {
            continue;
        };
        if name.to_ascii_lowercase().contains("carly") {
            candidates.push(AdapterCandidate {
                id: peripheral.id().to_string(),
                name,
                rssi: properties.rssi,
            });
        }
    }
    Ok(candidates)
}

pub async fn read(adapter_id: &str, request: ReadRequest) -> Result<Transaction, String> {
    let mut session = DiagnosticSession::connect(adapter_id).await?;
    let result = tokio::select! {
        transaction = session.read(request) => transaction,
        _ = tokio::signal::ctrl_c() => Err("cancelled".into()),
    };
    match (result, session.disconnect().await) {
        (Ok(transaction), Ok(())) => Ok(transaction),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
    }
}

pub async fn start_session(adapter_id: &str) -> Result<SessionClient, String> {
    let session = DiagnosticSession::connect(adapter_id).await?;
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(session_actor(session, receiver));
    Ok(SessionClient { sender })
}

#[derive(Clone)]
pub struct SessionClient {
    sender: mpsc::Sender<SessionCommand>,
}

impl SessionClient {
    pub async fn read(&self, request: ReadRequest) -> Result<Transaction, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::Read { request, reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before responding".to_string())?
    }

    pub async fn shutdown(self) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(SessionCommand::Shutdown { reply })
            .await
            .map_err(|_| "diagnostic session is closed".to_string())?;
        result
            .await
            .map_err(|_| "diagnostic session stopped before disconnecting".to_string())?
    }
}

enum SessionCommand {
    Read {
        request: ReadRequest,
        reply: oneshot::Sender<Result<Transaction, String>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), String>>,
    },
}

async fn session_actor(
    mut session: DiagnosticSession,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            SessionCommand::Read { request, reply } => {
                let _ = reply.send(session.read(request).await);
            }
            SessionCommand::Shutdown { reply } => {
                let _ = reply.send(session.disconnect().await);
                return;
            }
        }
    }
    let _ = session.disconnect().await;
}

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

/// One connected, initialized, read-only Carly diagnostic path.
pub struct DiagnosticSession {
    _manager: Manager,
    peripheral: Peripheral,
    channel: Characteristic,
    notifications: Notifications,
    supported: Vec<u8>,
}

impl DiagnosticSession {
    pub async fn connect(adapter_id: &str) -> Result<Self, String> {
        let (manager, adapter) = central().await?;
        let peripheral = find_peripheral(&adapter, adapter_id).await?;
        let cleanup_peripheral = peripheral.clone();
        let result = async {
            timeout(CONNECT_TIMEOUT, peripheral.connect())
                .await
                .map_err(|_| "Bluetooth connection timed out".to_string())?
                .map_err(|error| format!("Bluetooth connection failed: {error}"))?;
            timeout(SETUP_TIMEOUT, peripheral.discover_services())
                .await
                .map_err(|_| "BLE service discovery timed out".to_string())?
                .map_err(|error| format!("BLE service discovery failed: {error}"))?;
            let channel = carly_channel(&peripheral)?;
            timeout(SETUP_TIMEOUT, peripheral.subscribe(&channel))
                .await
                .map_err(|_| "Carly notification activation timed out".to_string())?
                .map_err(|error| format!("Carly notifications unavailable: {error}"))?;
            let notifications = timeout(SETUP_TIMEOUT, peripheral.notifications())
                .await
                .map_err(|_| "Carly notification stream timed out".to_string())?
                .map_err(|error| format!("Carly notification stream unavailable: {error}"))?;
            let mut session = Self {
                _manager: manager,
                peripheral,
                channel,
                notifications,
                supported: Vec::new(),
            };
            session.initialize().await?;
            Ok(session)
        }
        .await;
        if result.is_err() {
            best_effort_disconnect(&cleanup_peripheral).await;
        }
        result
    }

    pub async fn read(&mut self, request: ReadRequest) -> Result<Transaction, String> {
        if !supports_pid(&self.supported, request.pid()) {
            return Err(format!(
                "vehicle does not advertise support for {}",
                crate::hex(&request.bytes())
            ));
        }
        let response = {
            let mut exchange = LiveExchange {
                peripheral: &self.peripheral,
                channel: &self.channel,
                notifications: &mut self.notifications,
            };
            read_elm(&mut exchange, request).await?
        };
        request.complete("user", response)
    }

    pub async fn disconnect(&mut self) -> Result<(), String> {
        timeout(CONNECT_TIMEOUT, self.peripheral.disconnect())
            .await
            .map_err(|_| "Bluetooth disconnect timed out".to_string())?
            .map_err(|error| format!("Bluetooth disconnect failed: {error}"))
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let supported = {
            let mut exchange = LiveExchange {
                peripheral: &self.peripheral,
                channel: &self.channel,
                notifications: &mut self.notifications,
            };
            initialize_elm(&mut exchange).await?
        };
        self.supported = supported;
        Ok(())
    }
}

async fn best_effort_disconnect(peripheral: &Peripheral) {
    let _ = timeout(CONNECT_TIMEOUT, peripheral.disconnect()).await;
}

async fn central() -> Result<(Manager, Adapter), String> {
    let manager = Manager::new()
        .await
        .map_err(|error| format!("Bluetooth manager unavailable: {error}"))?;
    let adapter = manager
        .adapters()
        .await
        .map_err(|error| format!("Bluetooth adapter lookup failed: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no Bluetooth adapter available".to_string())?;
    Ok((manager, adapter))
}

async fn find_peripheral(adapter: &Adapter, requested_id: &str) -> Result<Peripheral, String> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|error| format!("Bluetooth scan failed: {error}"))?;
    sleep(FIND_TIMEOUT).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|error| format!("Bluetooth scan result failed: {error}"));
    let _ = adapter.stop_scan().await;
    peripherals?
        .into_iter()
        .find(|peripheral| {
            peripheral
                .id()
                .to_string()
                .eq_ignore_ascii_case(requested_id)
        })
        .ok_or_else(|| format!("adapter {requested_id} was not found after 10 seconds"))
}

fn carly_channel(peripheral: &Peripheral) -> Result<Characteristic, String> {
    let service = uuid_from_u16(CARLY_SERVICE);
    let channel = uuid_from_u16(CARLY_CHANNEL);
    if !peripheral
        .services()
        .iter()
        .any(|item| item.uuid == service)
    {
        return Err("Carly FFE0 service unavailable".into());
    }
    let channel = peripheral
        .characteristics()
        .iter()
        .find(|item| item.service_uuid == service && item.uuid == channel)
        .cloned()
        .ok_or_else(|| "Carly FFE1 characteristic unavailable".to_string())?;
    if !channel.properties.contains(CharPropFlags::NOTIFY) {
        return Err("Carly FFE1 notifications unavailable".into());
    }
    if !channel
        .properties
        .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
    {
        return Err("Carly FFE1 write unavailable".into());
    }
    Ok(channel)
}

trait ElmExchange {
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String>;
}

struct LiveExchange<'a, S> {
    peripheral: &'a Peripheral,
    channel: &'a Characteristic,
    notifications: &'a mut S,
}

impl<S> ElmExchange for LiveExchange<'_, S>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String> {
        elm_exchange(
            self.peripheral,
            self.channel,
            self.notifications,
            command,
            command_timeout,
        )
        .await
    }
}

async fn initialize_elm<E>(exchange: &mut E) -> Result<Vec<u8>, String>
where
    E: ElmExchange,
{
    let ati = exchange.exchange("ATI\r", Duration::from_secs(3)).await?;
    require_response(
        &ati,
        "ELM327",
        false,
        "ATI did not identify an ELM327 adapter",
    )?;
    let identity = exchange.exchange("AT@1\r", Duration::from_secs(3)).await?;
    require_response(
        &identity,
        "CARLY-UNIVERSAL",
        false,
        "AT@1 did not identify a Carly adapter",
    )?;
    let reset = exchange.exchange("ATZ\r", Duration::from_secs(3)).await?;
    require_response(
        &reset,
        "ELM327",
        false,
        "ATZ did not reset an ELM327 adapter",
    )?;
    for command in ["ATE0\r", "ATL0\r", "ATS0\r", "ATH0\r", "ATSP0\r"] {
        let response = exchange.exchange(command, Duration::from_secs(3)).await?;
        require_response(&response, "OK", true, &format!("{} failed", command.trim()))?;
    }
    let protocols = exchange.exchange("0100\r", Duration::from_secs(10)).await?;
    normalize_pid_support(&protocols)
}

async fn read_elm<E>(exchange: &mut E, request: ReadRequest) -> Result<Vec<u8>, String>
where
    E: ElmExchange,
{
    let command = obd_command(request);
    let response = exchange.exchange(&command, COMMAND_TIMEOUT).await?;
    normalize_mode01(&response, request.pid(), request.data_len())
}

fn supports_pid(response: &[u8], pid: u8) -> bool {
    if response.len() != 6 || response[..2] != [0x41, 0x00] || !(1..=0x20).contains(&pid) {
        return false;
    }
    let bitmap = u32::from_be_bytes([response[2], response[3], response[4], response[5]]);
    bitmap & (1 << (32 - pid)) != 0
}

async fn elm_exchange<S>(
    peripheral: &Peripheral,
    channel: &Characteristic,
    notifications: &mut S,
    command: &str,
    command_timeout: Duration,
) -> Result<String, String>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    let write_type = if channel
        .properties
        .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
    {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    };
    timeout(
        command_timeout,
        peripheral.write(channel, command.as_bytes(), write_type),
    )
    .await
    .map_err(|_| format!("Carly write timed out: {}", command.trim()))?
    .map_err(|error| format!("Carly write failed: {error}"))?;
    println!("adapter tx  {}", command.trim());

    let response = timeout(command_timeout, wait_for_prompt(notifications, channel))
        .await
        .map_err(|_| format!("Carly command timed out: {}", command.trim()))??;
    println!("adapter rx  {}", response.escape_default());
    Ok(response)
}

async fn wait_for_prompt<S>(
    notifications: &mut S,
    channel: &Characteristic,
) -> Result<String, String>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    let mut response = Vec::new();
    loop {
        let notification = notifications
            .next()
            .await
            .ok_or_else(|| "Carly notification stream ended".to_string())?;
        if notification.uuid != channel.uuid {
            continue;
        }
        if append_notification(&mut response, &notification.value)? {
            return Ok(String::from_utf8_lossy(&response).into_owned());
        }
    }
}

fn append_notification(response: &mut Vec<u8>, fragment: &[u8]) -> Result<bool, String> {
    response.extend_from_slice(fragment);
    if response.len() > MAX_RESPONSE {
        return Err("Carly response exceeded 8 KiB".into());
    }
    Ok(response.contains(&b'>'))
}

fn obd_command(request: ReadRequest) -> String {
    let mut command = String::with_capacity(request.bytes().len() * 2 + 1);
    for byte in request.bytes() {
        command.push_str(&format!("{byte:02X}"));
    }
    command.push('\r');
    command
}

fn require_response(
    response: &str,
    expected: &str,
    exact: bool,
    error: &str,
) -> Result<(), String> {
    let upper = response.to_ascii_uppercase();
    if upper.split(['\r', '\n']).any(|line| {
        let line = line.trim().trim_end_matches('>').trim();
        line == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| line.contains(status))
    }) {
        return Err(format!("{error}: {response:?}"));
    }
    upper
        .split(['\r', '\n'])
        .map(|line| line.trim().trim_end_matches('>').trim())
        .any(|line| {
            if exact {
                line == expected
            } else {
                line.starts_with(expected)
            }
        })
        .then_some(())
        .ok_or_else(|| format!("{error}: {response:?}"))
}

fn normalize_mode01(response: &str, pid: u8, data_len: usize) -> Result<Vec<u8>, String> {
    let matches = mode01_responses(response, pid, data_len)?;
    let first = matches[0].clone();
    if matches.iter().any(|value| value != &first) {
        return Err(format!("conflicting 01{pid:02X} responses"));
    }
    Ok(first)
}

fn normalize_pid_support(response: &str) -> Result<Vec<u8>, String> {
    let matches = mode01_responses(response, 0x00, 4)?;
    let bitmap = matches.iter().fold(0_u32, |bitmap, value| {
        bitmap | u32::from_be_bytes([value[2], value[3], value[4], value[5]])
    });
    let mut normalized = vec![0x41, 0x00];
    normalized.extend_from_slice(&bitmap.to_be_bytes());
    Ok(normalized)
}

fn mode01_responses(response: &str, pid: u8, data_len: usize) -> Result<Vec<Vec<u8>>, String> {
    let mut matches = Vec::new();
    for raw_line in response.split(['\r', '\n']) {
        let line = raw_line.trim().trim_end_matches('>').trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let compact = upper.replace(' ', "");
        if compact == format!("01{pid:02X}")
            || upper.starts_with("SEARCHING")
            || (upper.starts_with("BUS INIT") && !upper.contains("ERROR"))
        {
            continue;
        }
        if upper == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| upper.contains(status))
        {
            return Err(format!("ELM327 rejected 01{pid:02X}: {line}"));
        }
        if compact.len() % 2 != 0 || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("malformed ELM327 response line: {line:?}"));
        }
        let bytes = compact
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                std::str::from_utf8(pair)
                    .map_err(|error| error.to_string())
                    .and_then(|pair| {
                        u8::from_str_radix(pair, 16).map_err(|error| error.to_string())
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let negative = if bytes.len() > 1 && bytes[1] == 0x7f && (bytes[0] as usize) < bytes.len() {
            &bytes[1..]
        } else {
            &bytes[..]
        };
        if negative.first() == Some(&0x7f) {
            return Err(format!("negative OBD-II response: {line}"));
        }
        let expected_len = data_len + 2;
        let payload = if bytes.first() == Some(&(expected_len as u8)) {
            &bytes[1..]
        } else {
            &bytes[..]
        };
        if payload.len() < expected_len || payload[..2] != [0x41, pid] {
            continue;
        }
        if payload[expected_len..]
            .iter()
            .any(|byte| !matches!(byte, 0x00 | 0xaa))
        {
            return Err(format!("unexpected bytes after OBD-II response: {line}"));
        }
        matches.push(payload[..expected_len].to_vec());
    }
    (!matches.is_empty())
        .then_some(matches)
        .ok_or_else(|| format!("01{pid:02X} response not found in {response:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    const INIT_COMMANDS: [&str; 9] = [
        "ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS0\r", "ATH0\r", "ATSP0\r", "0100\r",
    ];

    struct ScriptedExchange {
        responses: VecDeque<Result<String, String>>,
        commands: Vec<String>,
    }

    impl ScriptedExchange {
        fn captured(responses: Vec<String>) -> Self {
            Self {
                responses: responses.into_iter().map(Ok).collect(),
                commands: Vec::new(),
            }
        }
    }

    impl ElmExchange for ScriptedExchange {
        async fn exchange(
            &mut self,
            command: &str,
            _command_timeout: Duration,
        ) -> Result<String, String> {
            self.commands.push(command.into());
            self.responses
                .pop_front()
                .unwrap_or_else(|| Err("script ended before adapter response".into()))
        }
    }

    fn captured_responses() -> Vec<String> {
        [
            "ELM327 v1.4 v100\r>",
            "carly-universal v200\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "4100BE3EB813\r>",
            "410C0000\r>",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn channel() -> Characteristic {
        Characteristic {
            uuid: uuid_from_u16(CARLY_CHANNEL),
            service_uuid: uuid_from_u16(CARLY_SERVICE),
            properties: CharPropFlags::NOTIFY,
            descriptors: BTreeSet::new(),
        }
    }

    fn notification(value: &[u8]) -> ValueNotification {
        ValueNotification {
            uuid: uuid_from_u16(CARLY_CHANNEL),
            service_uuid: uuid_from_u16(CARLY_SERVICE),
            value: value.into(),
        }
    }

    #[tokio::test]
    async fn session_client_serializes_read_commands_and_shutdown() {
        let (sender, mut commands) = mpsc::channel(4);
        let actor = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(command) = commands.recv().await {
                match command {
                    SessionCommand::Read { request, reply } => {
                        seen.push(request.bytes());
                        let response = match request.bytes() {
                            [0x01, 0x0c] => vec![0x41, 0x0c, 0x00, 0x00],
                            [0x01, 0x05] => vec![0x41, 0x05, 0x5a],
                            _ => unreachable!("closed test request vocabulary"),
                        };
                        let _ = reply.send(request.complete("user", response));
                    }
                    SessionCommand::Shutdown { reply } => {
                        let _ = reply.send(Ok(()));
                        return seen;
                    }
                }
            }
            seen
        });
        let client = SessionClient { sender };

        assert_eq!(
            client
                .read(crate::prepare_read("engine.rpm").unwrap())
                .await
                .unwrap()
                .value(),
            0.0
        );
        assert_eq!(
            client
                .read(crate::prepare_read("engine.coolant_temperature").unwrap())
                .await
                .unwrap()
                .value(),
            50.0
        );
        client.shutdown().await.unwrap();
        assert_eq!(actor.await.unwrap(), vec![[0x01, 0x0c], [0x01, 0x05]]);
    }

    #[tokio::test]
    async fn session_client_reports_a_closed_actor() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let client = SessionClient { sender };
        assert!(client
            .read(crate::prepare_read("engine.rpm").unwrap())
            .await
            .is_err());
    }

    #[test]
    fn normalizes_prompt_terminated_rpm_response() {
        assert_eq!(
            normalize_mode01("010C\r410C1AF8\r>", 0x0c, 2),
            Ok(vec![0x41, 0x0c, 0x1a, 0xf8])
        );
        assert_eq!(
            normalize_mode01(
                "04 41 0C 00 00 00 00 00\r04 41 0C 00 00 AA AA AA\r>",
                0x0c,
                2
            ),
            Ok(vec![0x41, 0x0c, 0x00, 0x00])
        );
        assert_eq!(
            normalize_mode01("SEARCHING...\r064100BE3EB813\r>", 0x00, 4),
            Ok(vec![0x41, 0x00, 0xbe, 0x3e, 0xb8, 0x13])
        );
    }

    #[test]
    fn rejects_missing_or_truncated_rpm_response() {
        assert!(normalize_mode01("NO DATA\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("STOPPED\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("?\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("410C00\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("410C0000\r410C0004\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("7F0111\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("037F0111\r410C0000\r>", 0x0c, 2).is_err());
        assert!(normalize_mode01("41 0C ZZ 00\r>", 0x0c, 2).is_err());
        assert!(require_response("OK\rERROR\r>", "OK", true, "command failed").is_err());
        assert!(require_response("NOTCARLY\r>", "CARLY-UNIVERSAL", false, "identity").is_err());
    }

    #[test]
    fn mode01_support_bitmap_gates_target_pids() {
        let response = [0x41, 0x00, 0x08, 0x18, 0x00, 0x00];
        assert!(supports_pid(&response, 0x05));
        assert!(supports_pid(&response, 0x0c));
        assert!(supports_pid(&response, 0x0d));
        assert!(!supports_pid(&response, 0x10));
        assert!(!supports_pid(&response, 0x00));

        let combined = normalize_pid_support("410008180000\r410000010000\r>").unwrap();
        assert!(supports_pid(&combined, 0x05));
        assert!(supports_pid(&combined, 0x10));
    }

    #[test]
    fn assembles_fragmented_notifications_through_prompt_with_a_size_bound() {
        let mut response = Vec::new();
        assert!(!append_notification(&mut response, b"41 0C 00").unwrap());
        assert!(append_notification(&mut response, b" 00\r>").unwrap());
        assert_eq!(response, b"41 0C 00 00\r>");

        let mut oversized = vec![b'0'; MAX_RESPONSE];
        assert!(append_notification(&mut oversized, b"0").is_err());
    }

    #[tokio::test]
    async fn frames_fragments_and_rejects_missing_prompt_or_oversize_response() {
        let mut fragments =
            futures_util::stream::iter(vec![notification(b"41 0C 00"), notification(b" 00\r>")]);
        assert_eq!(
            wait_for_prompt(&mut fragments, &channel()).await,
            Ok("41 0C 00 00\r>".into())
        );

        let mut missing_prompt = futures_util::stream::iter(vec![notification(b"41 0C 00 00\r")]);
        assert!(wait_for_prompt(&mut missing_prompt, &channel())
            .await
            .is_err());

        let mut oversized =
            futures_util::stream::iter(vec![notification(&vec![b'0'; MAX_RESPONSE + 1])]);
        assert!(wait_for_prompt(&mut oversized, &channel()).await.is_err());
    }

    #[tokio::test]
    async fn prompt_wait_honors_timeout_without_hardware() {
        let mut notifications = futures_util::stream::pending::<ValueNotification>();
        assert!(timeout(
            Duration::from_millis(1),
            wait_for_prompt(&mut notifications, &channel())
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn replays_captured_zero_rpm_session_in_exact_command_order() {
        let mut exchange = ScriptedExchange::captured(captured_responses());
        let request = crate::prepare_read("engine.rpm").unwrap();
        let supported = initialize_elm(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, request.pid()));
        let transaction = request
            .complete("user", read_elm(&mut exchange, request).await.unwrap())
            .unwrap();

        assert_eq!(exchange.commands[..9], INIT_COMMANDS);
        assert_eq!(exchange.commands[9], "010C\r");
        assert_eq!(transaction.response(), [0x41, 0x0c, 0x00, 0x00]);
        assert_eq!(transaction.value(), 0.0);

        let path = std::env::temp_dir().join(format!(
            "obdentic-session-replay-{}-{}.tsv",
            std::process::id(),
            transaction.timestamp_ms()
        ));
        crate::record(&path, &transaction).unwrap();
        let replayed = crate::replay(&path).await.unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(replayed.response(), transaction.response());
        assert_eq!(replayed.value(), transaction.value());
    }

    #[tokio::test]
    async fn scripted_session_reads_each_closed_standard_signal() {
        for (semantic, command, response, value, unit) in [
            (
                "engine.coolant_temperature",
                "0105\r",
                "41055A\r>",
                50.0,
                "°C",
            ),
            ("vehicle.speed", "010D\r", "410D64\r>", 100.0, "km/h"),
            ("engine.maf", "0110\r", "411001F4\r>", 5.0, "g/s"),
        ] {
            let mut responses = captured_responses();
            responses[8] = "410008190000\r>".into();
            responses[9] = response.into();
            let mut exchange = ScriptedExchange::captured(responses);
            let request = crate::prepare_read(semantic).unwrap();
            let supported = initialize_elm(&mut exchange).await.unwrap();
            assert!(supports_pid(&supported, request.pid()));
            let transaction = request
                .complete("user", read_elm(&mut exchange, request).await.unwrap())
                .unwrap();

            assert_eq!(exchange.commands[..9], INIT_COMMANDS);
            assert_eq!(exchange.commands[9], command);
            assert_eq!(transaction.value(), value);
            assert_eq!(transaction.unit(), unit);
        }
    }

    #[tokio::test]
    async fn initialization_and_pid_support_are_cached_across_sequential_reads() {
        let mut responses = captured_responses();
        responses.push("41055A\r>".into());
        let mut exchange = ScriptedExchange::captured(responses);
        let rpm = crate::prepare_read("engine.rpm").unwrap();
        let coolant = crate::prepare_read("engine.coolant_temperature").unwrap();

        let supported = initialize_elm(&mut exchange).await.unwrap();
        assert!(supports_pid(&supported, rpm.pid()));
        assert!(supports_pid(&supported, coolant.pid()));
        let rpm = rpm
            .complete("user", read_elm(&mut exchange, rpm).await.unwrap())
            .unwrap();
        let coolant = coolant
            .complete("user", read_elm(&mut exchange, coolant).await.unwrap())
            .unwrap();

        assert_eq!(rpm.value(), 0.0);
        assert_eq!(coolant.value(), 50.0);
        assert_eq!(
            exchange.commands,
            [INIT_COMMANDS.as_slice(), &["010C\r", "0105\r"],].concat()
        );
        assert_eq!(
            exchange
                .commands
                .iter()
                .filter(|command| *command == "0100\r")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn session_failures_stop_before_later_commands() {
        for (index, response) in [
            (0, "unknown adapter\r>"),
            (1, "unknown identity\r>"),
            (3, "?\r>"),
            (8, "NO DATA\r>"),
            (9, "NO DATA\r>"),
        ] {
            let mut responses = captured_responses();
            responses[index] = response.into();
            let mut exchange = ScriptedExchange::captured(responses);

            let request = crate::prepare_read("engine.rpm").unwrap();
            let failed = if index < INIT_COMMANDS.len() {
                initialize_elm(&mut exchange).await.is_err()
            } else {
                initialize_elm(&mut exchange).await.unwrap();
                read_elm(&mut exchange, request).await.is_err()
            };
            assert!(failed);
            assert_eq!(
                exchange.commands,
                [INIT_COMMANDS.as_slice(), &["010C\r"]][..].concat()[..=index]
            );
        }
    }
}
