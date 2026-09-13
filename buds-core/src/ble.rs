use std::time::Duration;

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;

use crate::protocol::{ADVERTISED_NAME_SUBSTRING, NOTIFY_CHAR_UUID, SERVICE_UUID, WRITE_CHAR_UUID};
use crate::{BudsError, PeripheralInfo};

const SCAN_TIMEOUT: Duration = Duration::from_secs(20);

/// Set `BUDS_DEBUG_SCAN=1` to log every BLE advertisement seen during a
/// scan (name, advertised services, manufacturer data), not just ones that
/// match the buds. Useful for confirming whether the buds are advertising
/// at all, and under what name/service, when the normal filtered scan
/// times out - this is a real hardware-timing question (the buds seem to
/// only broadcast a connectable advertisement briefly around power-on), not
/// something the code can detect for you.
fn debug_scan_enabled() -> bool {
    std::env::var("BUDS_DEBUG_SCAN").as_deref() == Ok("1")
}

pub(crate) async fn get_adapter() -> Result<Adapter, BudsError> {
    let manager = Manager::new()
        .await
        .map_err(|e| BudsError::Bluetooth(e.to_string()))?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|e| BudsError::Bluetooth(e.to_string()))?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| BudsError::Bluetooth("no Bluetooth adapter found".into()))
}

/// Equivalent of Swift's `retrieveConnectedPeripherals(withServices:)`.
/// Upstream btleplug has no such call - its peripheral cache is only ever
/// populated from live scan discoveries, which a peripheral the OS already
/// has a GATT connection to (e.g. reconnected automatically in the
/// background) may never emit. The vendored fork in vendor/btleplug adds
/// `Adapter::retrieve_connected`, which wraps the real CoreBluetooth call
/// and returns the UUIDs of whatever it finds directly.
///
/// This trusts those UUIDs the same way the Swift reference trusts
/// `retrieveConnectedPeripherals(withServices:)`'s result: it does NOT
/// additionally filter by advertised name (a peripheral found this way was
/// never necessarily seen advertising in this process, so it typically has
/// no local_name at all - filtering on it was a real bug that silently
/// discarded a correct match every time).
pub(crate) async fn find_already_connected(adapter: &Adapter) -> Result<Option<Peripheral>, BudsError> {
    let uuids = adapter
        .retrieve_connected(vec![SERVICE_UUID])
        .await
        .map_err(|e| BudsError::Bluetooth(e.to_string()))?;

    let Some(uuid) = uuids.into_iter().next() else {
        return Ok(None);
    };
    let id = uuid.into();

    // `retrieve_connected` replies as soon as CoreBluetooth hands back the
    // match, but registering it into btleplug's cache (so `peripheral(&id)`
    // can find it) happens one more async hop later, via a separate spawned
    // task inside the vendored fork. A few short retries absorb that race
    // without needing an arbitrary fixed delay.
    for attempt in 0..10 {
        match adapter.peripheral(&id).await {
            Ok(peripheral) => return Ok(Some(peripheral)),
            Err(_) if attempt < 9 => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(BudsError::Bluetooth(e.to_string())),
        }
    }
    unreachable!()
}

/// The buds only advertise over BLE briefly (e.g. just after leaving the
/// case) - once bonded, macOS often reconnects them over classic Bluetooth
/// in the background without ever emitting a BLE advertisement our scan
/// would see. So alongside listening for `DeviceDiscovered` events, this
/// also polls `find_already_connected` periodically: if the OS finishes an
/// automatic reconnect while we're "scanning", we pick it up that way
/// instead of timing out.
pub(crate) async fn scan_for_peripheral(adapter: &Adapter) -> Result<Peripheral, BudsError> {
    let mut events = adapter
        .events()
        .await
        .map_err(|e| BudsError::Bluetooth(e.to_string()))?;

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| BudsError::Bluetooth(e.to_string()))?;

    let mut poll_interval = tokio::time::interval(Duration::from_secs(2));
    let debug_scan = debug_scan_enabled();

    let result = tokio::time::timeout(SCAN_TIMEOUT, async {
        loop {
            tokio::select! {
                event = events.next() => {
                    let Some(event) = event else { return None };
                    if let CentralEvent::DeviceDiscovered(id) = event {
                        let peripheral = match adapter.peripheral(&id).await {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        let props = match peripheral.properties().await {
                            Ok(Some(props)) => props,
                            _ => continue,
                        };
                        if debug_scan {
                            eprintln!(
                                "[DEBUG] discovered: name={:?} services={:?} manufacturer_data={:?}",
                                props.local_name, props.services, props.manufacturer_data
                            );
                        }
                        let name_matches = props
                            .local_name
                            .as_ref()
                            .is_some_and(|n| n.contains(ADVERTISED_NAME_SUBSTRING));
                        let service_matches = props.services.contains(&SERVICE_UUID);
                        if name_matches || service_matches {
                            return Some(peripheral);
                        }
                    }
                }
                _ = poll_interval.tick() => {
                    if let Ok(Some(peripheral)) = find_already_connected(adapter).await {
                        return Some(peripheral);
                    }
                }
            }
        }
    })
    .await;

    let _ = adapter.stop_scan().await;

    match result {
        Ok(Some(peripheral)) => Ok(peripheral),
        Ok(None) => Err(BudsError::Bluetooth("scan ended without finding buds".into())),
        Err(_) => Err(BudsError::Timeout(
            "scanning for OnePlus Buds - try taking them out of the case to wake BLE advertising"
                .into(),
        )),
    }
}

pub(crate) async fn connect_peripheral(peripheral: &Peripheral) -> Result<(), BudsError> {
    peripheral
        .connect()
        .await
        .map_err(|e| BudsError::Bluetooth(format!("connect failed: {e}")))?;
    peripheral
        .discover_services()
        .await
        .map_err(|e| BudsError::Bluetooth(format!("service discovery failed: {e}")))?;
    Ok(())
}

pub(crate) struct DiscoveredChars {
    pub write_char: btleplug::api::Characteristic,
}

pub(crate) async fn discover_characteristics(peripheral: &Peripheral) -> Result<DiscoveredChars, BudsError> {
    let characteristics = peripheral.characteristics();

    let write_char = characteristics
        .iter()
        .find(|c| c.uuid == WRITE_CHAR_UUID)
        .cloned()
        .ok_or_else(|| BudsError::Bluetooth("write characteristic not found".into()))?;
    let notify_char = characteristics
        .iter()
        .find(|c| c.uuid == NOTIFY_CHAR_UUID)
        .cloned()
        .ok_or_else(|| BudsError::Bluetooth("notify characteristic not found".into()))?;

    // Confirm the OPO service is actually present (sanity check, mirrors
    // Swift discovering [BudsCLI.opoServiceUUID] explicitly).
    if !peripheral.services().iter().any(|s| s.uuid == SERVICE_UUID) {
        return Err(BudsError::Bluetooth("OPO service not found".into()));
    }

    peripheral
        .subscribe(&notify_char)
        .await
        .map_err(|e| BudsError::Bluetooth(format!("subscribe failed: {e}")))?;

    Ok(DiscoveredChars { write_char })
}

pub(crate) async fn peripheral_info(peripheral: &Peripheral) -> PeripheralInfo {
    let name = peripheral
        .properties()
        .await
        .ok()
        .flatten()
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| "OnePlus Buds 4".to_string());
    PeripheralInfo { name }
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn write_packet(
    peripheral: &Peripheral,
    write_char: &btleplug::api::Characteristic,
    data: &[u8],
) -> Result<(), BudsError> {
    // MUST use write-without-response - a regular write causes a silent
    // disconnect on the Buds 4 (confirmed empirically, see nordbuds4.swift).
    //
    // Observed in testing: btleplug's macOS/CoreBluetooth backend can hang
    // indefinitely on a write-without-response call (its internal
    // "ready to send" flow-control signal appears to be missed in some
    // runs), even though the packet has already gone out over the air. So
    // this is wrapped in a timeout - a timeout here is treated as a
    // (probably harmless) driver quirk, not proof the write failed.
    match tokio::time::timeout(
        WRITE_TIMEOUT,
        peripheral.write(write_char, data, WriteType::WithoutResponse),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(BudsError::Bluetooth(format!("write failed: {e}"))),
        Err(_) => Err(BudsError::Timeout(
            "write did not confirm within 5s (packet was likely still sent - known btleplug/CoreBluetooth quirk)"
                .into(),
        )),
    }
}
