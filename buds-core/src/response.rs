// Response parsing groundwork. The Swift CLI only ever prints raw hex for
// incoming notifications; this generalizes that one step so battery (and
// other not-yet-reverse-engineered features) can be inspected without
// touching the BLE plumbing later.

#[derive(Debug, Clone)]
pub struct RawNotification {
    pub cat: u8,
    pub sub: u8,
    pub seq: u8,
    pub payload: Vec<u8>,
}

impl RawNotification {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 7 {
            return None;
        }
        Some(Self {
            cat: bytes[4],
            sub: bytes[5],
            seq: bytes[6],
            payload: bytes[7..].to_vec(),
        })
    }

    pub fn hex(&self, full_frame: &[u8]) -> String {
        full_frame
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// All CAT=0x04/SUB=0x02 frames share one outer envelope, confirmed across
// every sample seen so far: `payload = [LEN_LO, LEN_HI, ...LEN bytes of
// content...]` (a little-endian u16 length prefix, then exactly that many
// content bytes). `content[0]` is a per-purpose "kind" byte that then
// determines how the rest of `content` is laid out:
//   - kind 0x03 ("ANC"): `[0x03, MARKER, ...]` - MARKER 0x02 means
//     `[0x03, 0x02, 0x01, <MODE>, 0x00]` (echoes the mode byte an ANC Set
//     command just set); MARKER 0x01 means `[0x03, 0x01, 0x01, <X>, <Y>]`,
//     some other ANC-adjacent status not yet decoded.
//   - kind 0x01 or 0x02 ("device status"): `[kind, COUNT, (tag, val) *
//     COUNT]` - a flat list of tagged byte values. CONFIRMED: kind 0x01
//     with tags 1/2/3 is the left/right/case battery percentage - verified
//     against the OnePlus app's own displayed values (80/80/90) while
//     testing. tag 3 = case is solidly confirmed (it was the one distinct
//     value, 90, and it lined up); tag 1 vs tag 2 for left vs right is the
//     natural reading but wasn't independently distinguishable in that test
//     since both were 80. kind 0x02 carries small values (0-7) that
//     fluctuate with wear/motion (observed changing live as buds were taken
//     in/out of ears and the case) - some per-component status, not yet
//     decoded.
// Anything else falls back to `Unknown` with the raw bytes intact.
fn content_of(payload: &[u8]) -> Option<&[u8]> {
    if payload.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    payload.get(2..2 + len)
}

fn parse_tagged_pairs(content: &[u8]) -> Option<(u8, Vec<(u8, u8)>)> {
    if content.len() < 2 {
        return None;
    }
    let kind = content[0];
    let count = content[1] as usize;
    let pairs_bytes = content.get(2..2 + count * 2)?;
    let pairs = pairs_bytes
        .chunks_exact(2)
        .map(|c| (c[0], c[1]))
        .collect::<Vec<_>>();
    Some((kind, pairs))
}

#[derive(Debug, Clone)]
pub enum NotificationEvent {
    HelloAck { seq: u8 },
    // Confirmed via live testing (all three modes, matching the Swift
    // reference byte-for-byte).
    AncModeEcho { seq: u8, mode: u8 },
    AncStateUpdate { seq: u8, payload: Vec<u8> },
    // See `content_of`/`parse_tagged_pairs` above - `kind` is 0x01 (battery
    // guess) or 0x02 (unknown) in all samples seen so far.
    DeviceStatus { seq: u8, kind: u8, fields: Vec<(u8, u8)> },
    // CAT=0x04, SUB=0x84 (0x04 | 0x80, same response-bit pattern as
    // Hello's 0x81), SEQ echoes the ANC Set command's SEQ (0x44). Payload
    // is a constant `01 00 00` regardless of mode - a generic "command
    // accepted" ack, not mode-specific.
    AncSetAck { seq: u8, payload: Vec<u8> },
    Unknown(RawNotification),
}

const CAT_SYSTEM: u8 = 0x00;
const SUB_SYSTEM_RESPONSE: u8 = 0x81;

const CAT_ANC: u8 = 0x04;
const SUB_ANC_STATE: u8 = 0x02;
const SUB_ANC_SET_ACK: u8 = 0x84;

const KIND_ANC: u8 = 0x03;
const ANC_MARKER_MODE_ECHO: u8 = 0x02;

pub fn classify(raw: RawNotification) -> NotificationEvent {
    if raw.cat == CAT_SYSTEM && raw.sub == SUB_SYSTEM_RESPONSE {
        return NotificationEvent::HelloAck { seq: raw.seq };
    }
    if raw.cat == CAT_ANC && raw.sub == SUB_ANC_SET_ACK {
        return NotificationEvent::AncSetAck {
            seq: raw.seq,
            payload: raw.payload,
        };
    }
    if raw.cat == CAT_ANC && raw.sub == SUB_ANC_STATE {
        if let Some(content) = content_of(&raw.payload) {
            if content.first() == Some(&KIND_ANC) {
                if content.get(1) == Some(&ANC_MARKER_MODE_ECHO) {
                    if let Some(&mode) = content.get(3) {
                        return NotificationEvent::AncModeEcho { seq: raw.seq, mode };
                    }
                }
                return NotificationEvent::AncStateUpdate {
                    seq: raw.seq,
                    payload: raw.payload,
                };
            }
            if let Some((kind, fields)) = parse_tagged_pairs(content) {
                return NotificationEvent::DeviceStatus {
                    seq: raw.seq,
                    kind,
                    fields,
                };
            }
        }
    }
    NotificationEvent::Unknown(raw)
}

/// Confirmed against the OnePlus app's own displayed battery percentages
/// (80/80/90) during live testing: a `DeviceStatus` with kind 0x01 carries
/// left/right/case as tags 1/2/3, values being direct percentages. The
/// case (tag 3) mapping is solidly verified (it was the one distinct
/// value and matched); left vs right (tags 1/2) is the natural reading but
/// wasn't independently distinguishable in that test since both read 80.
#[derive(Debug, Clone, Copy, Default)]
pub struct BatteryStatus {
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case_: Option<u8>,
}

/// Scans raw bytes for runs of printable ASCII (space through `~`) at
/// least `min_len` long. Useful for spotting a device name or similar text
/// buried in an otherwise-unclassified payload - e.g. the CAT=0x00 SUB=0x04
/// diagnostics blob turned out to be a literal JSON string this way. A
/// device-name/connected-devices feature (if the buds expose one at all)
/// would very plausibly show up the same way: as readable text inside a
/// frame we haven't classified yet.
pub fn ascii_strings(bytes: &[u8], min_len: usize) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = Vec::new();
    for &b in bytes {
        if (0x20..=0x7E).contains(&b) {
            current.push(b);
        } else if current.len() >= min_len {
            found.push(String::from_utf8_lossy(&current).into_owned());
            current.clear();
        } else {
            current.clear();
        }
    }
    if current.len() >= min_len {
        found.push(String::from_utf8_lossy(&current).into_owned());
    }
    found
}

const BATTERY_KIND: u8 = 0x01;

pub fn battery_guess(event: &NotificationEvent) -> Option<BatteryStatus> {
    let NotificationEvent::DeviceStatus { kind, fields, .. } = event else {
        return None;
    };
    if *kind != BATTERY_KIND {
        return None;
    }
    let get = |tag: u8| fields.iter().find(|(t, _)| *t == tag).map(|(_, v)| *v);
    Some(BatteryStatus {
        left: get(1),
        right: get(2),
        case_: get(3),
    })
}
