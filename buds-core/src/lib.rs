mod ble;
pub mod protocol;
pub mod response;

use std::sync::Arc;

use btleplug::api::{Characteristic, Peripheral as _};
use btleplug::platform::Peripheral;
use futures::Stream;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

pub use protocol::AncMode;
pub use response::{battery_guess, BatteryStatus, NotificationEvent, RawNotification};

#[derive(Debug, thiserror::Error)]
pub enum BudsError {
    #[error("bluetooth error: {0}")]
    Bluetooth(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone)]
pub struct PeripheralInfo {
    pub name: String,
}

#[derive(Clone)]
pub enum ConnectionState {
    Disconnected,
    LookingForConnectedDevice,
    Scanning,
    Connected(Arc<ConnectedBuds>),
    Failed(String),
}

/// A live, subscribed connection to the buds. Only obtainable via a
/// `ConnectionState::Connected` yielded from `connect()` - there is no way
/// to construct one without having gone through discovery + subscribe, so
/// "only show feature controls once actually connected" is enforced by the
/// type, not just a UI convention.
pub struct ConnectedBuds {
    peripheral: Peripheral,
    write_char: Characteristic,
    pub info: PeripheralInfo,
    notifications_tx: broadcast::Sender<NotificationEvent>,
    _reader_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ConnectedBuds {
    pub async fn send_hello(&self) -> Result<(), BudsError> {
        ble::write_packet(&self.peripheral, &self.write_char, &protocol::HELLO_PACKET).await
    }

    pub async fn set_anc_mode(&self, mode: AncMode) -> Result<(), BudsError> {
        let packet = protocol::anc_set_packet(mode);
        ble::write_packet(&self.peripheral, &self.write_char, &packet).await
    }

    /// Send an arbitrary raw packet - for probing undocumented request
    /// opcodes (see `buds-cli`'s `probe` command). Not used by any
    /// confirmed feature; exists purely for protocol exploration.
    pub async fn send_raw(&self, data: &[u8]) -> Result<(), BudsError> {
        ble::write_packet(&self.peripheral, &self.write_char, data).await
    }

    /// Battery packet format is not yet reverse-engineered. This just gives
    /// callers a way to listen to whatever notifications arrive so the
    /// format can be captured/inspected (see plan Verification).
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<NotificationEvent> {
        self.notifications_tx.subscribe()
    }

    /// No battery *query* packet is confirmed yet, and none seems to be
    /// needed - the buds push a `DeviceStatus{kind: 0x01, ..}` notification
    /// on their own (observed after a wear/case state change). Use
    /// `subscribe_notifications()` + `response::battery_guess()` to read
    /// one when it arrives; this stays as a placeholder for a real
    /// request-response query if/when one is found.
    pub async fn read_battery_best_effort(&self) -> Option<BatteryStatus> {
        None
    }
}

/// Drives the full connect flow and yields a state at each transition:
/// look for an already-connected device first (mirrors what already works
/// when the user manually connects via macOS Bluetooth settings before
/// running the CLI), and only fall back to a fresh scan if that comes up
/// empty.
pub fn connect() -> impl Stream<Item = ConnectionState> {
    async_stream::stream! {
        yield ConnectionState::LookingForConnectedDevice;

        let adapter = match ble::get_adapter().await {
            Ok(a) => a,
            Err(e) => {
                yield ConnectionState::Failed(e.to_string());
                return;
            }
        };

        let peripheral = match ble::find_already_connected(&adapter).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                yield ConnectionState::Scanning;
                match ble::scan_for_peripheral(&adapter).await {
                    Ok(p) => p,
                    Err(e) => {
                        yield ConnectionState::Failed(e.to_string());
                        return;
                    }
                }
            }
            Err(e) => {
                yield ConnectionState::Failed(e.to_string());
                return;
            }
        };

        if let Err(e) = ble::connect_peripheral(&peripheral).await {
            yield ConnectionState::Failed(e.to_string());
            return;
        }

        let chars = match ble::discover_characteristics(&peripheral).await {
            Ok(c) => c,
            Err(e) => {
                yield ConnectionState::Failed(e.to_string());
                return;
            }
        };

        let info = ble::peripheral_info(&peripheral).await;

        let (tx, _rx) = broadcast::channel(32);
        let notification_stream = match peripheral.notifications().await {
            Ok(s) => s,
            Err(e) => {
                yield ConnectionState::Failed(format!("notifications failed: {e}"));
                return;
            }
        };

        let tx_clone = tx.clone();
        let reader_task = tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = notification_stream;
            while let Some(notification) = stream.next().await {
                if let Some(raw) = RawNotification::parse(&notification.value) {
                    let event = response::classify(raw);
                    let _ = tx_clone.send(event);
                }
            }
        });

        let connected = ConnectedBuds {
            peripheral,
            write_char: chars.write_char,
            info,
            notifications_tx: tx,
            _reader_task: Mutex::new(Some(reader_task)),
        };

        yield ConnectionState::Connected(Arc::new(connected));
    }
}
