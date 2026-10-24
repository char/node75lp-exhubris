//! USB 2.0 standard descriptors and the HID class descriptor, built at compile time.

use crate::{Interface, Report, bytes::Bytes, hid};

pub const DEVICE_TYPE: u8 = 1;
pub const CONFIGURATION_TYPE: u8 = 2;
pub const STRING_TYPE: u8 = 3;
const INTERFACE_TYPE: u8 = 4;
const ENDPOINT_TYPE: u8 = 5;
pub const HID_TYPE: u8 = 0x21;
pub const REPORT_TYPE: u8 = 0x22;

const USB_2_0: u16 = 0x0200;
const HID_1_11: u16 = 0x0111;
pub const US_ENGLISH: u16 = 0x0409;

// Retain this physical board's vendor/product IDs, not an invented allocation.
const VENDOR_ID: u16 = 0x19f5;
const PRODUCT_ID: u16 = 0x1030;
const DEVICE_RELEASE: u16 = 0x8002;

pub const CONTROL_MAX_PACKET: usize = 64;
pub const PRODUCT_STRING: u8 = 1;
pub const CONFIGURATION_VALUE: u8 = 1;

const HID_CLASS: u8 = 3;
const BOOT_SUBCLASS: u8 = 1;
const KEYBOARD_PROTOCOL: u8 = 1;
const INTERRUPT: u8 = 0b11;
const BUS_POWERED: u8 = 0x80;
// Retain the tested USB budget; this is not a measured current draw.
const MAX_POWER_2MA: u8 = 250;

pub const DEVICE: [u8; 18] = {
    let mut out = Bytes::new();
    out.extend(&[18, DEVICE_TYPE]);
    out.extend(&USB_2_0.to_le_bytes());
    out.extend(&[0, 0, 0]); // class, subclass and protocol are declared per interface
    out.push(CONTROL_MAX_PACKET as u8);
    out.extend(&VENDOR_ID.to_le_bytes());
    out.extend(&PRODUCT_ID.to_le_bytes());
    out.extend(&DEVICE_RELEASE.to_le_bytes());
    out.extend(&[0, PRODUCT_STRING, 0]); // manufacturer, product and serial strings
    out.push(1); // configurations
    out.finish()
};

const fn hid(report_descriptor_len: usize) -> [u8; 9] {
    let mut out = Bytes::new();
    out.extend(&[9, HID_TYPE]);
    out.extend(&HID_1_11.to_le_bytes());
    out.extend(&[0, 1, REPORT_TYPE]); // not localized; one class descriptor follows
    out.extend(&(report_descriptor_len as u16).to_le_bytes());
    out.finish()
}

pub const KEYBOARD_HID: [u8; 9] = hid(hid::KEYBOARD.len());

const INTERFACE_LEN: usize = 9 + 9 + 7;

const fn interface(interface: Interface) -> [u8; INTERFACE_LEN] {
    // The boot keyboard subclass and protocol let BIOSes drive the keyboard.
    let (subclass, protocol, hid, max_packet) = match interface {
        Interface::Keyboard => (
            BOOT_SUBCLASS,
            KEYBOARD_PROTOCOL,
            KEYBOARD_HID,
            size_of::<Report>(),
        ),
    };
    let mut out = Bytes::new();
    out.extend(&[9, INTERFACE_TYPE, interface as u8, 0, 1]); // no alternate settings; one endpoint
    out.extend(&[HID_CLASS, subclass, protocol, 0]); // no string
    out.extend(&hid);
    out.extend(&[7, ENDPOINT_TYPE, interface.endpoint_address(), INTERRUPT]);
    out.extend(&(max_packet as u16).to_le_bytes());
    out.push(1); // poll every millisecond
    out.finish()
}

const CONFIGURATION_LEN: usize = 9 + INTERFACE_LEN;

pub const CONFIGURATION: [u8; CONFIGURATION_LEN] = {
    let mut out = Bytes::new();
    out.extend(&[9, CONFIGURATION_TYPE]);
    out.extend(&(CONFIGURATION_LEN as u16).to_le_bytes());
    out.extend(&[1, CONFIGURATION_VALUE, 0]); // interfaces; no string
    out.extend(&[BUS_POWERED, MAX_POWER_2MA]);
    out.extend(&interface(Interface::Keyboard));
    out.finish()
};

pub const LANGUAGES: [u8; 4] = {
    let [low, high] = US_ENGLISH.to_le_bytes();
    [4, STRING_TYPE, low, high]
};

pub const PRODUCT_NAME: &str = "Node75 LP";
pub const PRODUCT: [u8; 2 + 2 * PRODUCT_NAME.len()] = string(PRODUCT_NAME);

/// ASCII to UTF-16LE.
const fn string<const N: usize>(text: &str) -> [u8; N] {
    let text = text.as_bytes();
    let mut out = Bytes::new();
    out.extend(&[N as u8, STRING_TYPE]);
    let mut i = 0;
    while i < text.len() {
        assert!(text[i].is_ascii());
        out.extend(&[text[i], 0]);
        i += 1;
    }
    out.finish()
}
