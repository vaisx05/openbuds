# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/) (while below
1.0, minor versions may include breaking changes).

Version numbers live in several files; change them all at once with
`scripts/set-version.sh <version>` (see the README's "Releasing" section).

## [0.2.1] - 2026-11-27

### Fixed
- Noise control highlight stayed blank at launch when the buds were already
  Off: the buds only echo their mode when it changes, so the app now shows
  Off itself once the buds accept the initial Off command.
- Duplicate event handling after a reconnect: the previous connection's
  listener and watcher now stop once a newer connection replaces them.

## [0.2.0] - 2026-11-27

### Added
- OpenBuds logo and app icon (`assets/`), used for the app bundle, the app header and the
  README header.
- Tray app redesign: dark panel, ring battery gauges for left/right/case,
  and icon tiles for Off / ANC / Transparency.
- Case hero image: the opened case when connected, the closed (dimmed) case
  when disconnected or reconnecting.
- Auto-reconnect: when the buds drop (case closed, out of range, phone took
  over) the app shows "Reconnecting…" and retries with backoff (2s up to
  30s) without error toasts.
- Battery "Updated x min ago" label. The last reading is remembered across
  launches and shown dimmed until a fresh push arrives; partial pushes keep
  the previous values instead of blanking them.
- The app sends ANC Off after connecting so it always starts from a known
  mode.
- macOS app bundle and DMG, with the Bluetooth usage description macOS
  requires for a bundled app.
- `scripts/set-version.sh` to keep versions in sync across all manifests.

### Fixed
- Noise control highlight now follows the buds' own mode echo, so changes
  made from the phone app or a gesture on the buds show up, and a write
  timeout (a known btleplug/CoreBluetooth quirk) is no longer treated as a
  failure.
- Connection status stuck on "Disconnected": the UI now pulls the current
  state on startup and when the window regains focus instead of relying only
  on events that could fire before it was listening.

### Changed
- App renamed from `buds-tray` to **OpenBuds** (window title, bundle name).
- Window size is 480×800.
- Incoming packets and ANC writes are logged to stderr for debugging.

## [0.1.0] - 2026-09-14

### Added
- `buds-core`: BLE + protocol library (Hello handshake, ANC set, response
  classification, battery push parsing) on top of a vendored `btleplug`
  fork that can retrieve peripherals the OS already has connected.
- `buds-cli`: `on` / `off` / `trans` / `battery` / `monitor` / `probe` /
  `anc-raw` commands.
- First Tauri tray app shell (connection status, ANC buttons, battery).

[0.2.1]: https://github.com/vaisx05/openbuds/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/vaisx05/openbuds/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/vaisx05/openbuds/releases/tag/v0.1.0
