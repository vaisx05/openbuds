<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/openbuds-logo-dark.svg">
    <img alt="OpenBuds - ANC control for OnePlus Buds" src="assets/openbuds-logo-light.svg" width="480">
  </picture>
</p>

# openbuds

Desktop controller for the OnePlus Buds 4 (noise control + battery), since
OnePlus doesn't officially support macOS/Windows/Linux. A Rust core
(`btleplug`) shared by a CLI and a Tauri/React tray app. macOS is the
supported platform today.

Current version: **0.2.1** - see the [changelog](CHANGELOG.md).

Protocol reverse-engineered from BLE traffic captures, starting from a
working macOS-only Swift/CoreBluetooth prototype ([`nordbuds4.swift`](nordbuds4.swift)).

## Status

- **ANC (on/off/transparency): confirmed working**, verified against the
  physical device and the official OnePlus app.
- **Battery (left/right/case %): confirmed working, but only while a phone
  running the OnePlus app is also connected to the buds.** This app doesn't
  request battery status - it just listens for a push the buds send on
  their own, and that push only happens with a phone in the picture (see
  [Protocol notes](#protocol-notes)).
- **Discovery: reliable.** Connects directly to an already-OS-connected
  peripheral (no BLE scan needed) via a vendored `btleplug` patch - see
  below.
- **Tray app / OpenBuds** (`buds-tray/`, Tauri + React): connection status,
  noise control (Off / ANC / Transparency, kept in sync with changes made
  from the phone or the buds), ring battery gauges with an "updated x ago"
  label, a case image that opens when connected, and automatic reconnect
  when the buds drop.
- Adaptive ANC mode: byte not yet found.
- Some passive status pushes (`kind=0x02` "device status", `cat=0x01
  sub=0x05`) are logged but not decoded - see [Protocol notes](#protocol-notes).

## Layout

```
buds-core/        BLE + protocol logic (library crate)
buds-cli/         CLI: on / off / trans / battery / monitor / help
buds-tray/        Tauri + React tray app, "OpenBuds" (own Cargo workspace)
scripts/          set-version.sh (sync versions), build-release.sh (shareable build)
vendor/btleplug/  vendored fork of btleplug 0.11.8 (see below)
nordbuds4.swift   original macOS-only Swift/CoreBluetooth prototype (reference)
```

> **Unofficial.** OpenBuds is an independent project and is not affiliated
> with or endorsed by OnePlus. "OnePlus" and "OnePlus Buds" are trademarks
> of their respective owner. It talks to the buds using a protocol worked out
> by experiment, so use it at your own risk.

## Install

Download `OpenBuds_<version>_aarch64.dmg` from the
[releases page](https://github.com/vaisx05/openbuds/releases) and drag
OpenBuds to Applications. The build is **not signed or notarized**, so the
first launch needs right-click → Open (or `xattr -cr /Applications/OpenBuds.app`).
Allow the Bluetooth permission prompt when it appears.

Pair the buds in macOS Bluetooth settings first. For battery readings the
OnePlus app on your phone must also be connected to the buds (see
[Status](#status)).

## Building

CLI:

```
cargo build
cargo run -p buds-cli -- help
```

Tray app (needs Node and the Rust toolchain; run `source "$HOME/.cargo/env"`
first in a fresh shell):

```
cd buds-tray
npm install
npm run tauri dev      # run with hot reload
npm run tauri build    # app bundle + DMG in src-tauri/target/release/bundle/
                       # (for a shareable build use scripts/build-release.sh - it
                       # strips your home path out of the binary)
```

Requires the buds to already be connected/paired via the OS Bluetooth
settings, or in range and freshly out of the case (BLE scan fallback).

## Versioning and releasing

The project follows [Semantic Versioning](https://semver.org/); changes are
recorded in [CHANGELOG.md](CHANGELOG.md). The version appears in five files
(`buds-core`, `buds-cli`, `buds-tray` Cargo manifests, `package.json`,
`tauri.conf.json`), so never edit it by hand:

```
scripts/set-version.sh 0.3.0     # updates all manifests and lockfiles
# add a CHANGELOG.md entry, commit
git tag v0.3.0
git push --follow-tags
scripts/build-release.sh         # like `npm run tauri build`, but scrubs local paths
gh release create v0.3.0 buds-tray/src-tauri/target/release/bundle/dmg/*.dmg \
  --title "v0.3.0" --notes "<changelog section>"
```

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
    against the OnePlus app. **Confirmed to require a phone (running the
    OnePlus app) connected to the buds at the same time** - with just a
    Mac connected, zero of these pushes appear no matter what physical
    action is performed; connecting a phone alongside it produces one
    within seconds.
  - `kind=0x02`: fluctuating small values (0-7), not yet decoded - looks
    more like a continuous signal (motion/RSSI?) than a clean wear-state
    enum. Same phone-required behavior as `kind=0x01` above. Likely a
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
