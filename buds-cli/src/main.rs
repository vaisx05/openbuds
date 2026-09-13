use std::process::exit;
use std::time::Duration;

use buds_core::{battery_guess, connect, AncMode, BudsError, ConnectionState, NotificationEvent};
use futures::StreamExt;

const VERSION: &str = "0.1.0";

enum Command {
    On,
    Off,
    Transparency,
    Battery,
    Monitor,
    /// `Some((cat, sub))` for a single ad-hoc candidate (from extra CLI
    /// args); `None` runs the built-in candidate list.
    Probe(Option<(u8, u8)>),
    Help,
}

fn parse_hex_byte(s: &str) -> Option<u8> {
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u8::from_str_radix(s, 16).ok()
}

fn parse_command(args: &[String]) -> Option<Command> {
    match args[1].to_lowercase().as_str() {
        "on" | "anc" => Some(Command::On),
        "off" => Some(Command::Off),
        "trans" | "transparency" => Some(Command::Transparency),
        "battery" => Some(Command::Battery),
        "monitor" | "watch" => Some(Command::Monitor),
        "probe" => match (args.get(2), args.get(3)) {
            (Some(cat), Some(sub)) => {
                let cat = parse_hex_byte(cat)?;
                let sub = parse_hex_byte(sub)?;
                Some(Command::Probe(Some((cat, sub))))
            }
            (None, None) => Some(Command::Probe(None)),
            _ => None,
        },
        "help" | "--help" | "-h" => Some(Command::Help),
        _ => None,
    }
}

/// Exploratory: candidate "get status" request packets to try, on the
/// theory that the device-status push we only ever see passively (and
/// only with a phone connected) might actually be a response to a request
/// we've just never sent ourselves. Every confirmed request/response pair
/// so far echoes the SEQ byte back (Hello 0x23, ANC Set 0x44) - each
/// candidate here gets a distinct SEQ so any reply can be matched back to
/// whichever probe triggered it. Not based on any confirmed documentation -
/// pure trial and error, same way the ANC mode bytes themselves were found.
const PROBE_INTERVAL_SECS: u64 = 4;

const PROBE_CANDIDATES: &[(u8, u8, u8, &str)] = &[
    (0x04, 0x02, 0x50, "CAT=0x04 SUB=0x02 (mirrors the passive device-status push's own SUB)"),
    (0x00, 0x02, 0x51, "CAT=0x00 SUB=0x02"),
    (0x00, 0x08, 0x52, "CAT=0x00 SUB=0x08 (guess: battery lives under System)"),
    (0x04, 0x01, 0x53, "CAT=0x04 SUB=0x01 (mirrors Hello's SUB=0x01 under ANC category)"),
    (0x00, 0x03, 0x54, "CAT=0x00 SUB=0x03"),
];

fn print_help(bin: &str) {
    let lines = [
        format!("OnePlus Buds 4 ANC Controller v{VERSION}"),
        String::new(),
        format!("Usage: {bin} <command>"),
        String::new(),
        "Commands:".into(),
        "  on, anc        Enable Noise Cancellation (ANC)".into(),
        "  off            Disable ANC (Off mode)".into(),
        "  trans          Enable Transparency mode".into(),
        "  battery        Listen for a battery status push (left/right/case %) - not a".into(),
        "                 query, the buds push this on their own on wear/case changes".into(),
        "  monitor, watch Stream every raw notification with an elapsed-time prefix, for".into(),
        "                 as long as it runs (Ctrl-C to stop) - use this to correlate a".into(),
        "                 specific physical action (take one bud out, put it in the".into(),
        "                 case, etc.) with the exact event(s) it produces".into(),
        "  probe          (experimental) Try a handful of candidate \"get status\"".into(),
        "                 request packets, to see if battery can be read without a".into(),
        "                 phone connected - pure trial and error, not documented".into(),
        "  probe <cat> <sub>  Try one specific CAT/SUB pair instead (hex, e.g.".into(),
        "                 \"probe 00 05\" or \"probe 0x00 0x05\")".into(),
        "  help           Show this help message".into(),
        String::new(),
        "Notes:".into(),
        "  - Buds must be paired and connectable via Bluetooth on this machine".into(),
        "  - Adaptive mode is not yet supported (mode byte not confirmed)".into(),
        "  - Every run performs a fresh Hello handshake automatically".into(),
        "  - Already-connected buds (via system Bluetooth settings) are attached to".into(),
        "    directly; otherwise a scan is performed".into(),
        "  - The buds only broadcast a connectable BLE advertisement briefly, mainly".into(),
        "    right at power-on (e.g. fresh out of the case) - if a scan times out, try".into(),
        "    again right after taking them out of the case/ear".into(),
        "  - Set BUDS_DEBUG_SCAN=1 to log every nearby BLE device seen during a scan".into(),
        "    (name/services/manufacturer data), to check whether the buds are".into(),
        "    advertising at all and under what name".into(),
    ];
    println!("{}", lines.join("\n"));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let command = match args.get(1) {
        None => {
            print_help(&args[0]);
            exit(0);
        }
        Some(arg) => match parse_command(&args) {
            Some(Command::Help) => {
                print_help(&args[0]);
                exit(0);
            }
            Some(cmd) => cmd,
            None => {
                eprintln!("[ERROR] Unknown command or arguments: {arg} {:?}", &args[2..]);
                eprintln!("Run '{}' help for usage", args[0]);
                exit(1);
            }
        },
    };

    println!("[*] Looking for OnePlus Buds 4...");

    let mut states = Box::pin(connect());
    let connected = loop {
        match states.next().await {
            Some(ConnectionState::LookingForConnectedDevice) => {
                println!("[STATE] LookingForConnectedDevice");
            }
            Some(ConnectionState::Scanning) => {
                println!("[STATE] Scanning");
            }
            Some(ConnectionState::Connected(buds)) => {
                println!("[STATE] Connected ({})", buds.info.name);
                break buds;
            }
            Some(ConnectionState::Failed(reason)) => {
                eprintln!("[ERROR] {reason}");
                exit(1);
            }
            Some(ConnectionState::Disconnected) | None => {
                eprintln!("[ERROR] Disconnected before completing setup");
                exit(1);
            }
        }
    };

    let start = std::time::Instant::now();
    let mut notifications = connected.subscribe_notifications();
    tokio::spawn(async move {
        while let Ok(event) = notifications.recv().await {
            let t = start.elapsed().as_secs_f32();
            if let Some(status) = battery_guess(&event) {
                println!(
                    "[+{t:>6.2}s] [BATTERY] left={:?}% right={:?}% case={:?}%",
                    status.left, status.right, status.case_
                );
            }
            match event {
                NotificationEvent::HelloAck { seq } => {
                    println!("[+{t:>6.2}s] [RX] Hello ack (seq=0x{seq:02X})");
                }
                NotificationEvent::AncModeEcho { seq, mode } => {
                    println!("[+{t:>6.2}s] [RX] ANC mode echo (seq=0x{seq:02X}, mode=0x{mode:02X})");
                }
                NotificationEvent::AncStateUpdate { seq, payload } => {
                    println!("[+{t:>6.2}s] [RX] ANC state update (seq=0x{seq:02X}, payload={payload:02X?})");
                }
                NotificationEvent::DeviceStatus { seq, kind, fields } => {
                    println!("[+{t:>6.2}s] [RX] device status (seq=0x{seq:02X}, kind=0x{kind:02X}, fields={fields:02X?})");
                }
                NotificationEvent::AncSetAck { seq, payload } => {
                    println!("[+{t:>6.2}s] [RX] ANC set ack (seq=0x{seq:02X}, payload={payload:02X?})");
                }
                NotificationEvent::Unknown(raw) => {
                    println!(
                        "[+{t:>6.2}s] [RX] unknown cat=0x{:02X} sub=0x{:02X} seq=0x{:02X} payload={:02X?}",
                        raw.cat, raw.sub, raw.seq, raw.payload
                    );
                    for text in buds_core::response::ascii_strings(&raw.payload, 4) {
                        println!("[+{t:>6.2}s] [ASCII] \"{text}\"");
                    }
                }
            }
        }
    });

    match connected.send_hello().await {
        Ok(()) => println!("[TX] HELLO"),
        Err(BudsError::Timeout(msg)) => {
            println!("[TX] HELLO");
            eprintln!("[WARN] {msg}");
        }
        Err(e) => {
            eprintln!("[ERROR] Hello failed: {e}");
            exit(1);
        }
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;

    match command {
        Command::On => send_anc(&connected, AncMode::Anc, "ANC ON").await,
        Command::Off => send_anc(&connected, AncMode::Off, "OFF").await,
        Command::Transparency => send_anc(&connected, AncMode::Transparency, "TRANSPARENCY").await,
        Command::Battery => {
            const LISTEN_SECS: u64 = 20;
            println!(
                "[*] No battery query packet exists - listening for {LISTEN_SECS}s for the buds to \
                 push a status update on their own (they do this on wear/case changes)."
            );
            println!(
                "[*] To trigger one during this window: take a bud out of your ear (or put it back \
                 in), or put a bud back in the case and take it out again."
            );
            tokio::time::sleep(Duration::from_secs(LISTEN_SECS)).await;
            println!("[*] done listening - see any [BATTERY] lines above");
        }
        Command::Monitor => {
            const LISTEN_SECS: u64 = 120;
            println!(
                "[*] Streaming every raw notification for up to {LISTEN_SECS}s, with an elapsed-time \
                 prefix. Perform ONE physical action at a time (e.g. only take the left bud out, \
                 wait a couple seconds, then only put it back) so the timestamps let you correlate \
                 cause and effect precisely. Any readable text found in an unclassified payload is \
                 flagged as [ASCII] - useful for spotting a device name if the buds report connected \
                 devices this way. To test that specifically: leave this running, then toggle your \
                 phone's Bluetooth off and back on (or connect/disconnect it from the buds) and \
                 watch for a device-name string. Ctrl-C to stop early."
            );
            tokio::time::sleep(Duration::from_secs(LISTEN_SECS)).await;
            println!("[*] done monitoring");
        }
        Command::Probe(custom) => {
            const ADHOC_SEQ: u8 = 0x60;
            let candidates: Vec<(u8, u8, u8, String)> = match custom {
                Some((cat, sub)) => vec![(cat, sub, ADHOC_SEQ, "ad-hoc".to_string())],
                None => PROBE_CANDIDATES
                    .iter()
                    .map(|(c, s, sq, l)| (*c, *s, *sq, l.to_string()))
                    .collect(),
            };
            println!(
                "[*] Trying {} candidate \"get status\" packet(s), {}s apart. Watch the [RX] lines \
                 above/after each [PROBE] line for anything new - especially a device status or \
                 battery event with the SAME seq this probe just sent.",
                candidates.len(),
                PROBE_INTERVAL_SECS
            );
            for (cat, sub, seq, label) in candidates {
                let packet = buds_core::protocol::query_packet(cat, sub, seq);
                println!("[PROBE] sending CAT=0x{cat:02X} SUB=0x{sub:02X} SEQ=0x{seq:02X} - {label}");
                match connected.send_raw(&packet).await {
                    Ok(()) => {}
                    Err(BudsError::Timeout(msg)) => eprintln!("[WARN] {msg}"),
                    Err(e) => eprintln!("[ERROR] send failed: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(PROBE_INTERVAL_SECS)).await;
            }
            println!("[*] done probing - review [RX] lines above for any response");
        }
        Command::Help => unreachable!(),
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
}

async fn send_anc(connected: &buds_core::ConnectedBuds, mode: AncMode, label: &str) {
    println!("[TX] ANC SET: {label}");
    match connected.set_anc_mode(mode).await {
        Ok(()) => println!("[DONE] {label} command sent"),
        Err(BudsError::Timeout(msg)) => {
            eprintln!("[WARN] {msg}");
            println!("[DONE] {label} command sent (unconfirmed - see warning above)");
        }
        Err(e) => {
            eprintln!("[ERROR] ANC set failed: {e}");
            exit(1);
        }
    }
}
