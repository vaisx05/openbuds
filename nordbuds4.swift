#!/usr/bin/env swift

// OnePlus Buds 4 — ANC Controller CLI
// Reverse-engineered protocol (OPO/HeyMelody family), adapted from
// "Cracking OPOv1" by Aasheesh Rathour (Nord Buds 3 Pro reference)
// Buds 4 uses a DIFFERENT mode-byte mapping than the original blog post —
// confirmed empirically via LightBlue + the official OnePlus app:
//   0x01 = Off
//   0x02 = Noise Cancellation (ANC)
//   0x04 = Transparency
//   Adaptive mode byte is NOT yet solved (see project notes)

import Foundation
import CoreBluetooth

let VERSION = "1.0.0"

enum Command {
    case ancOn
    case off
    case transparency
    case help
}

// Confirmed mode bytes for Buds 4 (differs from blog's Nord Buds 3 Pro mapping)
enum ANCMode: UInt8 {
    case off = 0x01
    case anc = 0x02
    case transparency = 0x04
}

class BudsCLI: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {

    var centralManager: CBCentralManager!
    var peripheral: CBPeripheral?
    var cmdChar: CBCharacteristic?      // 0100079A — write
    var notifyChar: CBCharacteristic?   // 0200079A — notify
    var command: Command = .help
    var done = false
    var helloSent = false

    static let opoServiceUUID = CBUUID(string: "0000079A-D102-11E1-9B23-00025B00A5A5")
    static let writeCharUUID  = CBUUID(string: "0100079A-D102-11E1-9B23-00025B00A5A5")
    static let notifyCharUUID = CBUUID(string: "0200079A-D102-11E1-9B23-00025B00A5A5")

    override init() {
        super.init()
        centralManager = CBCentralManager(delegate: self, queue: .main)
    }

    // MARK: - Bluetooth State

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        guard central.state == .poweredOn else {
            print("[ERROR] Bluetooth is not powered on")
            exit(1)
        }

        let retrieve = central.retrieveConnectedPeripherals(withServices: [BudsCLI.opoServiceUUID])
        if !retrieve.isEmpty {
            self.peripheral = retrieve[0]
            retrieve[0].delegate = self
            central.connect(retrieve[0], options: nil)
            return
        }

        print("[*] Scanning for OnePlus Buds 4...")
        centralManager.scanForPeripherals(withServices: nil, options: nil)
    }

    func centralManager(_ central: CBCentralManager,
                         didDiscover peripheral: CBPeripheral,
                         advertisementData: [String: Any],
                         rssi RSSI: NSNumber) {
        if let name = peripheral.name, name.contains("OnePlus Buds") {
            print("[FOUND] \(name)")
            self.peripheral = peripheral
            peripheral.delegate = self
            centralManager.stopScan()
            centralManager.connect(peripheral, options: nil)
        }
    }

    // MARK: - Connection

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("[OK] Connected to \(peripheral.name ?? "OnePlus Buds 4")")
        peripheral.discoverServices([BudsCLI.opoServiceUUID])
    }

    func centralManager(_ central: CBCentralManager,
                         didFailToConnect peripheral: CBPeripheral,
                         error: Error?) {
        print("[ERROR] Failed to connect: \(error?.localizedDescription ?? "unknown")")
        exit(1)
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        for service in peripheral.services ?? [] {
            peripheral.discoverCharacteristics([BudsCLI.writeCharUUID, BudsCLI.notifyCharUUID], for: service)
        }
    }

    func peripheral(_ peripheral: CBPeripheral,
                     didDiscoverCharacteristicsFor service: CBService,
                     error: Error?) {
        for char in service.characteristics ?? [] {
            if char.uuid == BudsCLI.writeCharUUID {
                cmdChar = char
                print("[+] Found command channel (ENCO RX Data)")
            }
            if char.uuid == BudsCLI.notifyCharUUID {
                notifyChar = char
                peripheral.setNotifyValue(true, for: char)
                print("[+] Found response channel (ENCO TX Data), subscribed")
            }
        }

        if cmdChar != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                self.sendHelloThenCommand()
            }
        }
    }

    // MARK: - Response Handling

    func peripheral(_ peripheral: CBPeripheral,
                     didUpdateValueFor characteristic: CBCharacteristic,
                     error: Error?) {
        guard let data = characteristic.value else { return }
        let bytes = [UInt8](data)
        let hex = bytes.map { String(format: "%02X", $0) }.joined(separator: " ")
        print("[RX] \(hex)")
    }

    // MARK: - Packet Sending

    func sendPacket(_ data: [UInt8], name: String) {
        guard let cmdChar = cmdChar else {
            print("[ERROR] Command channel not ready")
            return
        }
        let hex = data.map { String(format: "%02X", $0) }.joined(separator: " ")
        print("[TX] \(name): \(hex)")
        peripheral?.writeValue(Data(data), for: cmdChar, type: .withoutResponse)
    }

    // MARK: - Handshake + Command Flow
    // Every invocation redoes Hello first (auto-handshake on every run,
    // per project decision — simpler and more robust than trying to
    // keep a long-lived session alive between CLI invocations).

    func sendHelloThenCommand() {
        sendPacket([0xAA, 0x07, 0x00, 0x00, 0x00, 0x01, 0x23, 0x00, 0x00, 0x12], name: "HELLO")

        // Give the buds a moment to process Hello before sending the real command.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            self.runCommand()
        }
    }

    func runCommand() {
        if done { return }
        done = true

        switch command {
        case .off:
            sendANCMode(.off, label: "OFF")
        case .ancOn:
            sendANCMode(.anc, label: "ANC ON")
        case .transparency:
            sendANCMode(.transparency, label: "TRANSPARENCY")
        case .help:
            printHelp()
            exit(0)
        }
    }

    func sendANCMode(_ mode: ANCMode, label: String) {
        // Packet: AA 0A 00 00 04 04 [SEQ] 03 00 01 01 [MODE]
        // SEQ is not critical for a single one-shot command, but we vary
        // it slightly from the Hello SEQ to avoid any dedup weirdness.
        let seq: UInt8 = 0x44
        let packet: [UInt8] = [0xAA, 0x0A, 0x00, 0x00, 0x04, 0x04, seq, 0x03, 0x00, 0x01, 0x01, mode.rawValue]
        sendPacket(packet, name: "ANC SET: \(label)")

        // Give the write a moment to land, then exit.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
            print("\n[DONE] \(label) command sent")
            exit(0)
        }
    }

    // MARK: - Help

    func printHelp() {
        print("""
        OnePlus Buds 4 ANC Controller v\(VERSION)

        Usage: \(CommandLine.arguments[0]) <command>

        Commands:
          on, anc        Enable Noise Cancellation (ANC)
          off            Disable ANC (Off mode)
          trans          Enable Transparency mode
          help           Show this help message

        Notes:
          - Buds must be paired and connectable via Bluetooth on this Mac
          - Adaptive mode is not yet supported (mode byte not confirmed)
          - Every run performs a fresh Hello handshake automatically
        """)
    }
}

// MARK: - Main

func main() {
    let args = CommandLine.arguments

    if args.count < 2 {
        let cli = BudsCLI()
        cli.printHelp()
        exit(0)
    }

    let cmd = args[1].lowercased()
    var command: Command = .help

    switch cmd {
    case "on", "anc":
        command = .ancOn
        print("[*] Target: ANC ON")
    case "off":
        command = .off
        print("[*] Target: OFF")
    case "trans", "transparency":
        command = .transparency
        print("[*] Target: TRANSPARENCY")
    case "help", "--help", "-h":
        let cli = BudsCLI()
        cli.printHelp()
        exit(0)
    default:
        print("[ERROR] Unknown command: \(cmd)")
        print("Run '\(args[0]) help' for usage")
        exit(1)
    }

    print("[*] Looking for OnePlus Buds 4...")

    let cli = BudsCLI()
    cli.command = command

    RunLoop.main.run(until: Date(timeIntervalSinceNow: 15.0))

    print("[ERROR] Timeout - buds not responding")
    exit(1)
}

main()
