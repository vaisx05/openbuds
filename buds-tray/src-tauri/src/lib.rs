use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    // Latest connection state, so a webview that loads (or wakes up) after
    // an event was emitted can still ask for the current state - events
    // alone are lost if nothing was listening yet.
    connection: Mutex<ConnectionStatePayload>,
    // Bumped by every connect flow so an older one (e.g. an auto-reconnect
    // loop) stops as soon as a newer one starts.
    generation: AtomicU64,
}

impl AppState {
    fn new() -> Self {
        Self {
            connected: Mutex::new(None),
            battery: Mutex::new(None),
            anc_mode: Mutex::new(None),
            connection: Mutex::new(ConnectionStatePayload::Disconnected),
            generation: AtomicU64::new(0),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(tag = "state", rename_all = "camelCase")]
enum ConnectionStatePayload {
    Disconnected,
    LookingForConnectedDevice,
    Scanning,
    Reconnecting,
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

async fn publish_connection(app: &AppHandle, payload: ConnectionStatePayload) {
    *app.state::<AppState>().connection.lock().await = payload.clone();
    let _ = app.emit(EVENT_CONNECTION_STATE, payload);
}

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

/// Drives the connect flow in the background, storing the resulting
/// `ConnectedBuds` in app state and emitting a connection-state event at
/// every transition.
///
/// `auto` is for reconnecting after a drop: instead of giving up on the
/// first failure (and toasting an error) it keeps retrying with backoff,
/// showing "Reconnecting…" in between, until it connects or a newer flow
/// (e.g. the user pressing reconnect) supersedes it.
fn spawn_connect_flow(app: AppHandle, auto: bool) {
    let generation = app
        .state::<AppState>()
        .generation
        .fetch_add(1, Ordering::SeqCst)
        + 1;

    tauri::async_runtime::spawn(async move {
        let is_current = |app: &AppHandle| {
            app.state::<AppState>().generation.load(Ordering::SeqCst) == generation
        };
        let mut backoff = Duration::from_secs(2);

        loop {
            let mut states = Box::pin(connect());
            let mut connected: Option<Arc<ConnectedBuds>> = None;

            while let Some(state) = states.next().await {
                if !is_current(&app) {
                    return;
                }
                let payload = match &state {
                    ConnectionState::Disconnected => ConnectionStatePayload::Disconnected,
                    ConnectionState::LookingForConnectedDevice => {
                        ConnectionStatePayload::LookingForConnectedDevice
                    }
                    ConnectionState::Scanning => ConnectionStatePayload::Scanning,
                    ConnectionState::Connected(buds) => ConnectionStatePayload::Connected {
                        device_name: buds.info.name.clone(),
                    },
                    // While auto-reconnecting, a failed attempt is expected
                    // (buds still in the case) - not worth an error toast.
                    ConnectionState::Failed(_) if auto => ConnectionStatePayload::Reconnecting,
                    ConnectionState::Failed(reason) => ConnectionStatePayload::Failed {
                        reason: reason.clone(),
                    },
                };
                publish_connection(&app, payload).await;

                if let ConnectionState::Connected(buds) = state {
                    connected = Some(buds);
                    break;
                }
            }

            if let Some(buds) = connected {
                if is_current(&app) {
                    on_connected(app.clone(), buds).await;
                }
                return;
            }
            if !auto || !is_current(&app) {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
}

async fn is_current_connection(app: &AppHandle, id: usize) -> bool {
    app.state::<AppState>()
        .connected
        .lock()
        .await
        .as_ref()
        .is_some_and(|c| Arc::as_ptr(c) as usize == id)
}

/// Everything that happens once a connection is up: handshake, publish to
/// app state, forward notifications to the UI, start from a known ANC mode,
/// and watch for the buds dropping so the app can reconnect by itself.
async fn on_connected(app: AppHandle, buds: Arc<ConnectedBuds>) {
    if let Err(e) = buds.send_hello().await {
        // A write timeout here is a known driver quirk (see
        // buds-core::ble::write_packet) - the Hello likely still
        // landed, so this isn't treated as fatal.
        eprintln!("[buds-tray] Hello warning: {e}");
    }

    let app_state = app.state::<AppState>();
    *app_state.connected.lock().await = Some(buds.clone());

    // Identity of this connection, so tasks tied to it can tell when a newer
    // connection has replaced it (see `is_current_connection`).
    let connection_id = Arc::as_ptr(&buds) as usize;

    let mut notifications = buds.subscribe_notifications();
    let app_for_notifications = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok(event) = notifications.recv().await {
            // After a reconnect the old connection's stream can still be
            // delivering (same underlying peripheral) - stop forwarding so
            // every event isn't handled twice.
            if !is_current_connection(&app_for_notifications, connection_id).await {
                break;
            }
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
    match buds.set_anc_mode(AncMode::Off).await {
        // The buds only echo their mode when it *changes*, so if they were
        // already Off there is no echo and the UI would stay blank. The set
        // was accepted, so publish Off ourselves (a real echo, if any,
        // arrives later and corrects it).
        Ok(()) | Err(BudsError::Timeout(_)) => {
            *app.state::<AppState>().anc_mode.lock().await = Some("off");
            let _ = app.emit(EVENT_ANC_MODE, Some("off"));
        }
        Err(e) => eprintln!("[buds-tray] initial ANC Off failed: {e}"),
    }

    // Auto-reconnect: when the buds drop, clear state, tell the UI, and
    // start a retrying connect flow - unless this connection was already
    // replaced (manual reconnect), in which case that flow owns the state.
    let watched = buds.clone();
    tauri::async_runtime::spawn(async move {
        // Also bail out if this connection is replaced (manual reconnect),
        // so the old one isn't kept alive forever.
        loop {
            tokio::select! {
                _ = watched.wait_disconnected() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    if !is_current_connection(&app, connection_id).await {
                        return;
                    }
                }
            }
        }
        let state = app.state::<AppState>();
        let still_current = state
            .connected
            .lock()
            .await
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &watched));
        if !still_current {
            return;
        }
        eprintln!("[buds-tray] buds disconnected - reconnecting");
        *state.connected.lock().await = None;
        *state.anc_mode.lock().await = None;
        let _ = app.emit(EVENT_ANC_MODE, None::<&str>);
        publish_connection(&app, ConnectionStatePayload::Reconnecting).await;
        spawn_connect_flow(app.clone(), true);
    });
}

#[tauri::command]
async fn reconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    *state.connected.lock().await = None;
    *state.battery.lock().await = None;
    *state.anc_mode.lock().await = None;
    spawn_connect_flow(app, false);
    Ok(())
}

#[tauri::command]
async fn get_connection_state(
    state: State<'_, AppState>,
) -> Result<ConnectionStatePayload, String> {
    Ok(state.connection.lock().await.clone())
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
            get_connection_state,
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

            spawn_connect_flow(app_handle, false);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
