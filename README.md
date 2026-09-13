# openbuds

Cross-platform BLE controller for the OnePlus Buds 4 (ANC + battery), since
OnePlus doesn't officially support macOS/Windows/Linux. Rust core
(`btleplug`) + a CLI today, with a Tauri/React/three.js tray app planned on
top of the same core.

Protocol reverse-engineered from BLE traffic captures, starting from a
working macOS-only Swift/CoreBluetooth prototype ([`nordbuds4.swift`](nordbuds4.swift)).

## Status

- **ANC (on/off/transparency): confirmed working**, verified against the
  physical device and the official OnePlus app.
- **Battery (left/right/case %): confirmed working**, verified against the
  OnePlus app's own displayed percentages.
- **Discovery: reliable.** Connects directly to an already-OS-connected
  peripheral (no BLE scan needed) via a vendored `btleplug` patch - see
  below.
- Adaptive ANC mode: byte not yet found.
- Some passive status pushes (`kind=0x02` "device status", `cat=0x01
  sub=0x05`) are logged but not decoded - see [Protocol notes](#protocol-notes).

## Layout

```
buds-core/   BLE + protocol logic (library crate)
buds-cli/    CLI: on / off / trans / battery / monitor / help
vendor/btleplug/  vendored fork of btleplug 0.11.8 (see below)
nordbuds4.swift   original macOS-only Swift/CoreBluetooth prototype (reference)
```

## Building

```
cargo build
cargo run -p buds-cli -- help
```

Requires the buds to already be connected/paired via the OS Bluetooth
settings, or in range and freshly out of the case (BLE scan fallback).

## The vendored btleplug patch

Upstream `btleplug` has no equivalent of CoreBluetooth's
`retrieveConnectedPeripherals(withServices:)` - its peripheral cache is only
ever populated from live scan discoveries. That's a problem here: the buds
are frequently already connected at the OS level (visible in the macOS
Bluetooth menu) without ever emitting a BLE advertisement our own scan could
catch, so a plain `btleplug` scan alone is unreliable.

`vendor/btleplug` adds one real CoreBluetooth call, wired through the same
message-passing internals upstream already uses for scanning:

- `Adapter::retrieve_connected(service_uuids) -> Vec<Uuid>` - wraps
  `CBCentralManager.retrieveConnectedPeripheralsWithServices(_:)` directly
  and returns the UUIDs found, registering them into btleplug's cache the
  same way a scan discovery would.

See `vendor/btleplug/src/corebluetooth/{internal,adapter}.rs` for the diff
against upstream 0.11.8. `buds-core/src/ble.rs::find_already_connected`
trusts the result directly (no additional name filtering) - a peripheral
found this way was never necessarily seen advertising, so it typically
carries no advertised name at all.

## Protocol notes

GATT UUIDs, packet framing, and confirmed ANC mode bytes are documented in
`buds-core/src/protocol.rs`. Response parsing and what's confirmed vs. not
is documented in `buds-core/src/response.rs`. Summary:

- **Hello** (`AA 07 00 00 00 01 23 00 00 12`) must be sent first; ack is
  `CAT=0x00 SUB=0x81`.
- **ANC Set** (`AA 0A 00 00 04 04 44 03 00 01 01 <mode>`, mode
  0x01=Off/0x02=ANC/0x04=Transparency) gets a mode-echo and a generic
  accepted-ack (`CAT=0x04 SUB=0x84`).
- **Device status** (`CAT=0x04 SUB=0x02`) pushed spontaneously by the buds,
  envelope `[LEN_u16_le, ...content...]`, `content[0]` is a "kind" byte:
  - `kind=0x01`: **battery**, tags 1/2/3 = left/right/case %. Confirmed
    against the OnePlus app.
  - `kind=0x02`: fluctuating small values (0-7), not yet decoded - looks
    more like a continuous signal (motion/RSSI?) than a clean wear-state
    enum. **Only observed while the OnePlus phone app was also connected**
    - with just this Mac connected, these pushes stopped appearing
      entirely even when performing the same physical actions. Likely a
      firmware behavior tied to the phone's audio session, not something
      fixable from this side.
  - `kind=0xF1`: periodic heartbeat, always `(01, 00)`.
- `CAT=0x00 SUB=0x04`: a JSON diagnostics blob (`boxchg_err_baseinfo`,
  charging/error counters), sent once after Hello.
- `CAT=0x01 SUB=0x05`: seen once per physical wear/gesture event
  (incrementing seq), payload `00 00` - not yet decoded.

`buds-cli`'s `monitor` command streams every raw event with an
elapsed-time prefix, for correlating a specific physical action against
the exact event(s) it produces.
