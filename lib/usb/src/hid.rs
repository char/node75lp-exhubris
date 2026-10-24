//! HID 1.11 report descriptors, written as short items and encoded at compile time.

use crate::bytes::Bytes;

#[derive(Clone, Copy)]
#[repr(u16)]
enum Page {
    GenericDesktop = 0x01,
    Keyboard = 0x07,
    Led = 0x08,
    Vendor = 0xff00,
}

#[derive(Clone, Copy)]
enum Item {
    UsagePage(Page),
    Usage(u16),
    UsageMinimum(u16),
    UsageMaximum(u16),
    LogicalMinimum(i32),
    LogicalMaximum(i32),
    ReportSize(u8),
    ReportCount(u8),
    Application,
    EndCollection,
    Input(u8),
    Output(u8),
    Feature(u8),
}

use Item::*;

// Main item data bits. Clear bits mean data, array and absolute.
const CONSTANT: u8 = 1 << 0;
const VARIABLE: u8 = 1 << 1;
const ARRAY: u8 = 0;

const KEYBOARD_USAGE: u16 = 0x06;
const TELEMETRY: u16 = 0x01;

const NO_EVENT: u16 = 0x00;
const F24: u16 = 0x73;
const LEFT_CONTROL: u16 = 0xe0;
const RIGHT_GUI: u16 = 0xe7;
const NUM_LOCK: u16 = 0x01;
const KANA: u16 = 0x05;
pub const TELEMETRY_BYTES: usize = 16;

/// One output bit per LED from `NUM_LOCK` to `KANA`.
pub const LED_MASK: u8 = (1 << (KANA - NUM_LOCK + 1)) - 1;

/// Boot-compatible: modifier bits, a reserved byte and six key slots, without
/// report IDs. The feature report carries diagnostic telemetry.
const KEYBOARD_ITEMS: &[Item] = &[
    UsagePage(Page::GenericDesktop),
    Usage(KEYBOARD_USAGE),
    Application,
    UsagePage(Page::Keyboard),
    UsageMinimum(LEFT_CONTROL),
    UsageMaximum(RIGHT_GUI),
    LogicalMinimum(0),
    LogicalMaximum(1),
    ReportSize(1),
    ReportCount(8),
    Input(VARIABLE),
    ReportCount(1),
    ReportSize(8),
    Input(CONSTANT),
    ReportCount(5),
    ReportSize(1),
    UsagePage(Page::Led),
    UsageMinimum(NUM_LOCK),
    UsageMaximum(KANA),
    Output(VARIABLE),
    ReportCount(1),
    ReportSize(3),
    Output(CONSTANT),
    ReportCount(6),
    ReportSize(8),
    LogicalMinimum(0),
    LogicalMaximum(F24 as i32),
    UsagePage(Page::Keyboard),
    UsageMinimum(NO_EVENT),
    UsageMaximum(F24),
    Input(ARRAY),
    UsagePage(Page::Vendor),
    Usage(TELEMETRY),
    LogicalMinimum(0),
    LogicalMaximum(0xff),
    ReportSize(8),
    ReportCount(TELEMETRY_BYTES as u8),
    Feature(VARIABLE),
    EndCollection,
];

pub const KEYBOARD: [u8; encoded_len(KEYBOARD_ITEMS)] = encode(KEYBOARD_ITEMS);

// Short item tags, before the data size is merged into the low two bits.
const USAGE_PAGE: u8 = 0x04;
const LOGICAL_MINIMUM: u8 = 0x14;
const LOGICAL_MAXIMUM: u8 = 0x24;
const REPORT_SIZE: u8 = 0x74;
const REPORT_COUNT: u8 = 0x94;
const USAGE: u8 = 0x08;
const USAGE_MINIMUM: u8 = 0x18;
const USAGE_MAXIMUM: u8 = 0x28;
const INPUT: u8 = 0x80;
const OUTPUT: u8 = 0x90;
const FEATURE: u8 = 0xb0;
const COLLECTION: u8 = 0xa0;
const END_COLLECTION: u8 = 0xc0;
const APPLICATION: u32 = 0x01;

impl Item {
    /// The tag and data bytes, in the shortest encoding that preserves the value.
    const fn encode(self) -> (u8, [u8; 4], usize) {
        let (tag, value, signed) = match self {
            UsagePage(page) => (USAGE_PAGE, page as u32, false),
            Usage(usage) => (USAGE, usage as u32, false),
            UsageMinimum(usage) => (USAGE_MINIMUM, usage as u32, false),
            UsageMaximum(usage) => (USAGE_MAXIMUM, usage as u32, false),
            LogicalMinimum(value) => (LOGICAL_MINIMUM, value as u32, true),
            LogicalMaximum(value) => (LOGICAL_MAXIMUM, value as u32, true),
            ReportSize(bits) => (REPORT_SIZE, bits as u32, false),
            ReportCount(count) => (REPORT_COUNT, count as u32, false),
            Application => (COLLECTION, APPLICATION, false),
            EndCollection => return (END_COLLECTION, [0; 4], 0),
            Input(flags) => (INPUT, flags as u32, false),
            Output(flags) => (OUTPUT, flags as u32, false),
            Feature(flags) => (FEATURE, flags as u32, false),
        };
        // Logical extents are signed, so 255 needs two bytes to stay positive.
        let len = if signed {
            let value = value as i32;
            if value == value as i8 as i32 {
                1
            } else if value == value as i16 as i32 {
                2
            } else {
                4
            }
        } else if value <= 0xff {
            1
        } else if value <= 0xffff {
            2
        } else {
            4
        };
        let size = if len == 4 { 3 } else { len as u8 };
        (tag | size, value.to_le_bytes(), len)
    }
}

const fn encoded_len(items: &[Item]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < items.len() {
        len += 1 + items[i].encode().2;
        i += 1;
    }
    len
}

const fn encode<const N: usize>(items: &[Item]) -> [u8; N] {
    let mut out = Bytes::new();
    let mut i = 0;
    while i < items.len() {
        let (prefix, data, len) = items[i].encode();
        out.push(prefix);
        let mut j = 0;
        while j < len {
            out.push(data[j]);
            j += 1;
        }
        i += 1;
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_use_the_shortest_encoding_that_preserves_their_value() {
        const ITEMS: &[Item] = &[
            LogicalMaximum(0x7f),
            LogicalMaximum(0xff),
            LogicalMinimum(-1),
            UsageMaximum(0xff),
            UsagePage(Page::Vendor),
            Input(ARRAY),
            EndCollection,
        ];
        let encoded: [u8; encoded_len(ITEMS)] = encode(ITEMS);
        assert_eq!(
            encoded,
            [
                0x25, 0x7f, 0x26, 0xff, 0x00, 0x15, 0xff, 0x29, 0xff, 0x06, 0x00, 0xff, 0x81, 0x00,
                0xc0
            ]
        );
    }
}
