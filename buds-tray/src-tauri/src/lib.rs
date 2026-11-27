use std::sync::Arc;

use buds_core::{
    battery_guess, connect, AncMode, BatteryStatus, BudsError, ConnectedBuds, ConnectionState,
    NotificationEvent,
};
use futures::StreamExt;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

struct AppState {
    connected: Mutex<Option<Arc<ConnectedBuds>>>,
    battery: Mutex<Option<BatteryStatus>>,
    anc_mode: Mutex<Option<&'static str>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            connected: Mutex::new(None),
            battery: Mutex::new(None),
            anc_mode: Mutex::new(None),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(tag = "state", rename_all = "camelCase")]
enum ConnectionStatePayload {
    Disconnected,
    LookingForConnectedDevice,
    Scanning,
    // `rename_all` on the enum only renames variant tags, not fields
    // inside a struct variant - needs repeating per-variant, or the field
    // silently keeps its snake_case name in the JSON the frontend reads.
    #[serde(rename_all = "camelCase")]
    Connected { device_name: String },
    Failed { reason: String },
}

#[derive(Serialize, Clone, Default)]
struct BatteryPayload {
    left: Option<u8>,
    right: Option<u8>,
    case: Option<u8>,
}

impl From<BatteryStatus> for BatteryPayload {
    fn from(b: BatteryStatus) -> Self {
        Self {
            left: b.left,
            right: b.right,
            case: b.case_,
        }
    }
}

const EVENT_CONNECTION_STATE: &str = "buds://connection-state";
const EVENT_BATTERY: &str = "buds://battery";
const EVENT_ANC_MODE: &str = "buds://anc-mode";

/// Frontend-facing key for a raw ANC mode byte (see `AncMode`). Unknown
/// bytes (e.g. Adaptive, not decoded yet) map to `None` so the UI shows no
/// selection instead of a wrong one.
fn anc_mode_key(raw: u8) -> Option<&'static str> {
    match raw {
        x if x == AncMode::Off.raw() => Some("off"),
        x if x == AncMode::Anc.raw() => Some("on"),
        x if x == AncMode::Transparency.raw() => Some("trans"),
        _ => None,
    }
}

/// Drives the connect flow once, storing the resulting `ConnectedBuds` in
/// app state and emitting a connection-state event at every transition -
/// mirrors buds-cli's main loop, but as a background task the frontend
/// observes via events instead of blocking on it.
fn spawn_connect_flow(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut states = Box::pin(connect());
        while let Some(state) = states.next().await {
            let payload = match &state {
                ConnectionState::Disconnected => ConnectionStatePayload::Disconnected,
                ConnectionState::LookingForConnectedDevice => {
                    ConnectionStatePayload::LookingForConnectedDevice
                }
                ConnectionState::Scanning => ConnectionStatePayload::Scanning,
                ConnectionState::Connected(buds) => ConnectionStatePayload::Connected {
                    device_name: buds.info.name.clone(),
                },
                ConnectionState::Failed(reason) => ConnectionStatePayload::Failed {
                    reason: reason.clone(),
                },
            };
            let _ = app.emit(EVENT_CONNECTION_STATE, payload);

            if let ConnectionState::Connected(buds) = state {
                if let Err(e) = buds.send_hello().await {
                    // A write timeout here is a known driver quirk (see
                    // buds-core::ble::write_packet) - the Hello likely still
                    // landed, so this isn't treated as fatal.
                    eprintln!("[buds-tray] Hello warning: {e}");
                }

                let app_state = app.state::<AppState>();
                *app_state.connected.lock().await = Some(buds.clone());

                let mut notifications = buds.subscribe_notifications();
                let app_for_notifications = app.clone();
                tauri::async_runtime::spawn(async move {
                    while let Ok(event) = notifications.recv().await {
                        // Heartbeats and diagnostics are noisy; log the rest.
                        match &event {
                            NotificationEvent::DeviceStatus { kind: 0xF1, .. } => {}
                            NotificationEvent::Unknown(raw) if raw.cat == 0x00 && raw.sub == 0x04 => {}
                            other => eprintln!("[buds-tray] event: {other:?}"),
                        }
                        // The buds echo the active mode whenever it changes -
                        // whether we set it, the phone did, or a touch gesture
                        // on the buds did - so this is the source of truth for
                        // what the UI highlights.
                        if let NotificationEvent::AncModeEcho { mode, .. } = &event {
                            let key = anc_mode_key(*mode);
                            let app_state = app_for_notifications.state::<AppState>();
                            *app_state.anc_mode.lock().await = key;
                            let _ = app_for_notifications.emit(EVENT_ANC_MODE, key);
                        }
                        if let NotificationEvent::DeviceStatus { .. } = &event {
                            if let Some(status) = battery_guess(&event) {
                                let app_state = app_for_notifications.state::<AppState>();
                                *app_state.battery.lock().await = Some(status);
                                let _ = app_for_notifications
                                    .emit(EVENT_BATTERY, BatteryPayload::from(status));
                            }
                        }
                    }
                });

                // The buds never report their current mode at connect, so the
                // app would otherwise have no idea what to highlight. Start
                // every connection from a known state: Off. The buds' mode
                // echo then drives the UI as usual.
                if let Err(e) = buds.set_anc_mode(AncMode::Off).await {
                    eprintln!("[buds-tray] initial ANC Off failed: {e}");
                }

                // Connect flow ends once Connected is reached and Hello is
                // sent - the stream would otherwise stay pending forever.
                break;
            }
        }
    });
}

#[tauri::command]
async fn reconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    *state.connected.lock().await = None;
    *state.battery.lock().await = None;
    *state.anc_mode.lock().await = None;
    spawn_connect_flow(app);
    Ok(())
}

#[tauri::command]
async fn get_battery(state: State<'_, AppState>) -> Result<Option<BatteryPayload>, String> {
    Ok(state.battery.lock().await.map(BatteryPayload::from))
}

#[tauri::command]
async fn get_anc_mode(state: State<'_, AppState>) -> Result<Option<&'static str>, String> {
    Ok(*state.anc_mode.lock().await)
}

async fn set_anc_mode(state: &AppState, mode: AncMode) -> Result<(), String> {
    let buds = state
        .connected
        .lock()
        .await
        .clone()
        .ok_or_else(|| "not connected".to_string())?;
    eprintln!("[buds-tray] sending ANC mode {mode:?}");
    let result = buds.set_anc_mode(mode).await;
    eprintln!("[buds-tray] ANC write result: {result:?}");
    match result {
        Ok(()) => Ok(()),
        // Write-timeout is a known btleplug/CoreBluetooth quirk: the packet
        // has already gone out. Treating it as an error made the UI skip the
        // mode update even though the buds switched, so it's a success here;
        // the buds' mode echo confirms the real state either way.
        Err(BudsError::Timeout(msg)) => {
            eprintln!("[buds-tray] ANC write timeout (ignored): {msg}");
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn anc_on(state: State<'_, AppState>) -> Result<(), String> {
    set_anc_mode(&state, AncMode::Anc).await
}

#[tauri::command]
async fn anc_off(state: State<'_, AppState>) -> Result<(), String> {
    set_anc_mode(&state, AncMode::Off).await
}

#[tauri::command]
async fn anc_transparency(state: State<'_, AppState>) -> Result<(), String> {
    set_anc_mode(&state, AncMode::Transparency).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            reconnect,
            get_battery,
            get_anc_mode,
            anc_on,
            anc_off,
            anc_transparency
        ])
        .setup(|app| {
            let anc_on = MenuItem::with_id(app, "anc_on", "ANC On", true, None::<&str>)?;
            let anc_off = MenuItem::with_id(app, "anc_off", "ANC Off", true, None::<&str>)?;
            let anc_trans =
                MenuItem::with_id(app, "anc_trans", "Transparency", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&anc_on, &anc_off, &anc_trans, &separator, &quit],
            )?;

            let app_handle = app.handle().clone();
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    let mode = match event.id.as_ref() {
                        "anc_on" => Some(AncMode::Anc),
                        "anc_off" => Some(AncMode::Off),
                        "anc_trans" => Some(AncMode::Transparency),
                        "quit" => {
                            app.exit(0);
                            None
                        }
                        _ => None,
                    };
                    if let Some(mode) = mode {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state: State<AppState> = app.state();
                            if let Err(e) = set_anc_mode(&state, mode).await {
                                eprintln!("[buds-tray] ANC set failed: {e}");
                            }
                        });
                    }
                })
                .build(app)?;

            spawn_connect_flow(app_handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
