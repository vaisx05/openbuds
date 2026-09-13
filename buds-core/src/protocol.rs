use uuid::Uuid;

// Same UUIDs as nordbuds4.swift (OPO/HeyMelody protocol family)
pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x0000079A_D102_11E1_9B23_00025B00A5A5);
pub const WRITE_CHAR_UUID: Uuid = Uuid::from_u128(0x0100079A_D102_11E1_9B23_00025B00A5A5);
pub const NOTIFY_CHAR_UUID: Uuid = Uuid::from_u128(0x0200079A_D102_11E1_9B23_00025B00A5A5);

pub const ADVERTISED_NAME_SUBSTRING: &str = "OnePlus Buds";

// Confirmed mode bytes for Buds 4 (differs from public blog posts about
// similar OnePlus/Oppo earbuds). Adaptive mode byte is NOT yet solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncMode {
    Off = 0x01,
    Anc = 0x02,
    Transparency = 0x04,
}

impl AncMode {
    pub fn raw(self) -> u8 {
        self as u8
    }
}

pub const HELLO_PACKET: [u8; 10] = [0xAA, 0x07, 0x00, 0x00, 0x00, 0x01, 0x23, 0x00, 0x00, 0x12];

// Packet: AA 0A 00 00 04 04 [SEQ] 03 00 01 01 [MODE]
// SEQ is hardcoded to 0x44, matching the Swift reference (not critical for a
// one-shot command).
const ANC_SET_SEQ: u8 = 0x44;

pub fn anc_set_packet(mode: AncMode) -> [u8; 12] {
    [
        0xAA,
        0x0A,
        0x00,
        0x00,
        0x04,
        0x04,
        ANC_SET_SEQ,
        0x03,
        0x00,
        0x01,
        0x01,
        mode.raw(),
    ]
}
