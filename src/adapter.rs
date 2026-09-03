use crate::elm::{initialize_elm, require_response, verify_elm327, ElmExchange};
use btleplug::{
    api::{
        bleuuid::uuid_from_u16, Central, CharPropFlags, Characteristic, Manager as _,
        Peripheral as _, ScanFilter, ValueNotification, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures_util::{Stream, StreamExt};
use std::{pin::Pin, time::Duration};
use tokio::time::{sleep, timeout};

const CARLY_SERVICE: u16 = 0xFFE0;
const CARLY_CHANNEL: u16 = 0xFFE1;
const SCAN_TIMEOUT: Duration = Duration::from_secs(5);
const FIND_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct AdapterCandidate {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
}

/// The Carly CUA-V200 adapter backend owns all CoreBluetooth/btleplug details.
/// Its exchange implementation deliberately exposes only the shared ELM
/// dialect seam to the diagnostic session.
pub(crate) struct CarlyCuaV200 {
    _manager: Manager,
    peripheral: Peripheral,
    channel: Characteristic,
    notifications: Notifications,
    show_adapter_io: bool,
}

type Notifications = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

pub(crate) async fn scan() -> Result<Vec<AdapterCandidate>, String> {
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

impl CarlyCuaV200 {
    pub(crate) async fn connect(adapter_id: &str, show_adapter_io: bool) -> Result<Self, String> {
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
            let mut backend = Self {
                _manager: manager,
                peripheral,
                channel,
                notifications,
                show_adapter_io,
            };
            backend.initialize().await?;
            Ok(backend)
        }
        .await;
        if result.is_err() {
            best_effort_disconnect(&cleanup_peripheral).await;
        }
        result
    }

    pub(crate) async fn disconnect(&mut self) -> Result<(), String> {
        timeout(CONNECT_TIMEOUT, self.peripheral.disconnect())
            .await
            .map_err(|_| "Bluetooth disconnect timed out".to_string())?
            .map_err(|error| format!("Bluetooth disconnect failed: {error}"))
    }

    pub(crate) async fn disconnect_best_effort(&mut self) {
        let _ = timeout(SHUTDOWN_DISCONNECT_TIMEOUT, self.peripheral.disconnect()).await;
    }

    async fn initialize(&mut self) -> Result<(), String> {
        initialize_carly(self).await
    }
}

/// Carly-specific identity validation followed by the reusable ELM setup.
/// Keeping this ordering here preserves `ATI`, `AT@1`, `ATZ`, ... exactly.
pub(crate) async fn initialize_carly<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    verify_elm327(exchange).await?;
    let identity = exchange.exchange("AT@1\r", Duration::from_secs(3)).await?;
    require_response(
        &identity,
        "CARLY-UNIVERSAL",
        false,
        "AT@1 did not identify a Carly adapter",
    )?;
    initialize_elm(exchange).await
}

impl ElmExchange for CarlyCuaV200 {
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String> {
        elm_exchange(
            &self.peripheral,
            &self.channel,
            &mut self.notifications,
            command,
            command_timeout,
            self.show_adapter_io,
        )
        .await
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

async fn elm_exchange<S>(
    peripheral: &Peripheral,
    channel: &Characteristic,
    notifications: &mut S,
    command: &str,
    command_timeout: Duration,
    show_adapter_io: bool,
) -> Result<String, String>
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    discard_queued_notifications(notifications).await;
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
    if let Some(line) = adapter_io_line(show_adapter_io, "tx", command.trim()) {
        println!("{line}");
    }

    let response = timeout(command_timeout, wait_for_prompt(notifications, channel))
        .await
        .map_err(|_| format!("Carly command timed out: {}", command.trim()))??;
    if let Some(line) = adapter_io_line(show_adapter_io, "rx", response.escape_default()) {
        println!("{line}");
    }
    Ok(response)
}

async fn discard_queued_notifications<S>(notifications: &mut S)
where
    S: Stream<Item = ValueNotification> + Unpin,
{
    while matches!(
        timeout(Duration::from_millis(2), notifications.next()).await,
        Ok(Some(_))
    ) {}
}

fn adapter_io_line(
    show_adapter_io: bool,
    direction: &str,
    value: impl std::fmt::Display,
) -> Option<String> {
    show_adapter_io.then(|| format!("adapter {direction}  {value}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};

    struct ScriptedExchange {
        responses: VecDeque<String>,
        commands: Vec<String>,
    }

    impl ScriptedExchange {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: responses.into_iter().map(str::to_owned).collect(),
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
            self.commands.push(command.to_owned());
            self.responses
                .pop_front()
                .ok_or_else(|| "script ended before adapter response".to_string())
        }
    }

    #[tokio::test]
    async fn carly_initialization_checks_identity_before_generic_elm_setup() {
        let mut exchange = ScriptedExchange::new([
            "ELM327 v1.4 v100\r>",
            "carly-universal v200\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);

        initialize_carly(&mut exchange).await.unwrap();

        assert_eq!(
            exchange.commands,
            ["ATI\r", "AT@1\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"]
        );
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

    #[test]
    fn notification_framing_is_bounded_at_eight_kibibytes() {
        let mut response = Vec::new();
        assert!(!append_notification(&mut response, b"41 0C 00").unwrap());
        assert!(append_notification(&mut response, b" 00\r>").unwrap());
        assert_eq!(response, b"41 0C 00 00\r>");

        let mut oversized = vec![b'0'; MAX_RESPONSE];
        assert!(append_notification(&mut oversized, b"0").is_err());
    }

    #[test]
    fn adapter_io_logging_is_optional_and_keeps_direction() {
        assert_eq!(adapter_io_line(false, "tx", "010C"), None);
        assert_eq!(
            adapter_io_line(true, "tx", "010C"),
            Some("adapter tx  010C".into())
        );
        assert_eq!(
            adapter_io_line(true, "rx", "410C0000\\r>"),
            Some("adapter rx  410C0000\\r>".into())
        );
    }

    #[tokio::test]
    async fn prompt_framing_reassembles_fragments_and_rejects_missing_prompt() {
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
}
