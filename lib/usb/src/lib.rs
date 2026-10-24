//! USB HID keyboard device state machine: descriptors, control transfers,
//! report scheduling. Independent of MMIO; the register driver is in the
//! keyboard task. Report policy follows char/q65-max-hubris; 6KRO in both
//! boot and report protocols.

#![no_std]

#[cfg(test)]
extern crate std;

mod bytes;
mod descriptors;
mod hid;

use descriptors::{
    CONFIGURATION, CONFIGURATION_TYPE, CONFIGURATION_VALUE, CONTROL_MAX_PACKET, DEVICE,
    DEVICE_TYPE, HID_TYPE, KEYBOARD_HID, LANGUAGES, PRODUCT, PRODUCT_STRING, REPORT_TYPE,
    STRING_TYPE, US_ENGLISH,
};
use hid::{LED_MASK, TELEMETRY_BYTES};

/// A boot-protocol keyboard report: modifiers, a reserved byte and six key slots.
pub type Report = [u8; 8];

/// The HID interfaces, in configuration descriptor order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interface {
    Keyboard,
}

impl Interface {
    /// The interrupt IN endpoint number, without the direction bit.
    const fn endpoint(self) -> u8 {
        self as u8 + 1
    }

    const fn endpoint_address(self) -> u8 {
        DIRECTION_IN | self.endpoint()
    }
}

/// The direction bit of an endpoint address.
const DIRECTION_IN: u8 = 0x80;

// bmRequestType: direction, type and recipient.
const STANDARD_DEVICE_OUT: u8 = 0x00;
const STANDARD_INTERFACE_OUT: u8 = 0x01;
const STANDARD_ENDPOINT_OUT: u8 = 0x02;
const STANDARD_DEVICE_IN: u8 = 0x80;
const STANDARD_INTERFACE_IN: u8 = 0x81;
const STANDARD_ENDPOINT_IN: u8 = 0x82;
const CLASS_INTERFACE_OUT: u8 = 0x21;
const CLASS_INTERFACE_IN: u8 = 0xa1;

// Standard requests.
const GET_STATUS: u8 = 0;
const CLEAR_FEATURE: u8 = 1;
const SET_FEATURE: u8 = 3;
const SET_ADDRESS: u8 = 5;
const GET_DESCRIPTOR: u8 = 6;
const GET_CONFIGURATION: u8 = 8;
const SET_CONFIGURATION: u8 = 9;
const GET_INTERFACE: u8 = 10;
const SET_INTERFACE: u8 = 11;

// HID class requests.
const GET_REPORT: u8 = 1;
const GET_IDLE: u8 = 2;
const GET_PROTOCOL: u8 = 3;
const SET_REPORT: u8 = 9;
const SET_IDLE: u8 = 10;
const SET_PROTOCOL: u8 = 11;

/// GET_DESCRIPTOR's wValue: the descriptor type, then its index.
const fn descriptor(kind: u8, index: u8) -> u16 {
    u16::from_be_bytes([kind, index])
}

const DEVICE_DESCRIPTOR: u16 = descriptor(DEVICE_TYPE, 0);
const CONFIGURATION_DESCRIPTOR: u16 = descriptor(CONFIGURATION_TYPE, 0);
const LANGUAGES_DESCRIPTOR: u16 = descriptor(STRING_TYPE, 0);
const PRODUCT_DESCRIPTOR: u16 = descriptor(STRING_TYPE, PRODUCT_STRING);
const HID_DESCRIPTOR: u16 = descriptor(HID_TYPE, 0);
const REPORT_DESCRIPTOR: u16 = descriptor(REPORT_TYPE, 0);

// GET_REPORT and SET_REPORT's wValue: the report type, then report ID zero.
const INPUT_REPORT: u16 = 0x0100;
const OUTPUT_REPORT: u16 = 0x0200;
const FEATURE_REPORT: u16 = 0x0300;

// wIndex for interface, endpoint and string requests.
const KEYBOARD: u16 = Interface::Keyboard as u16;
const CONTROL_OUT: u16 = 0x00;
const CONTROL_IN: u16 = DIRECTION_IN as u16;
const KEYBOARD_IN: u16 = Interface::Keyboard.endpoint_address() as u16;
const LANGUAGE: u16 = US_ENGLISH;

const ENDPOINT_HALT: u16 = 0;
const CONFIGURED: u16 = CONFIGURATION_VALUE as u16;
/// SET_IDLE and GET_IDLE count in 4 ms units.
const IDLE_UNIT_MS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    bytes: [u8; CONTROL_MAX_PACKET],
    len: usize,
}

impl Packet {
    fn new(bytes: &[u8]) -> Self {
        let mut packet = Self {
            bytes: [0; CONTROL_MAX_PACKET],
            len: bytes.len(),
        };
        packet.bytes[..bytes.len()].copy_from_slice(bytes);
        packet
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Send(Packet),
    Address(u8),
    Configure(bool),
    Halt(bool),
    Stall,
}

#[derive(Clone, Copy, Default)]
enum Protocol {
    Boot = 0,
    #[default]
    Report = 1,
}

#[derive(Clone, Copy)]
enum Commit {
    Address(u8),
    Configuration(bool),
    Halt(bool),
    Protocol(Protocol),
    Idle(u8),
    Leds(u8),
}

#[derive(Default)]
enum Control {
    #[default]
    Idle,
    DataIn,
    DescriptorIn {
        remaining: &'static [u8],
        zlp: bool,
    },
    StatusOut,
    StatusIn(Commit),
    LedsOut,
}

#[derive(Default)]
pub struct Device {
    control: Control,
    address: u8,
    configured: bool,
    halted: bool,
    suspended: bool,
    protocol: Protocol,
    idle: u8,
    leds: u8,
    report: Report,
    telemetry: [u8; TELEMETRY_BYTES],
    in_flight: Option<Report>,
    acknowledged: Option<Report>,
    last_report_at: u32,
}

impl Device {
    pub fn reset(&mut self) {
        *self = Self {
            report: self.report,
            telemetry: self.telemetry,
            ..Self::default()
        };
    }

    pub fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        if !suspended {
            self.acknowledged = None;
        }
    }

    pub fn active(&self) -> bool {
        self.configured && !self.suspended
    }

    pub fn leds(&self) -> u8 {
        self.leds
    }

    pub fn set_telemetry(&mut self, telemetry: [u8; TELEMETRY_BYTES]) {
        self.telemetry = telemetry;
    }

    pub fn set_report(&mut self, report: Report) {
        self.report = report;
    }

    pub fn report_due(&self, now: u32) -> Option<Report> {
        if !self.configured || self.suspended || self.halted || self.in_flight.is_some() {
            return None;
        }
        let changed = self.acknowledged != Some(self.report);
        let expired = self.idle != 0
            && now.wrapping_sub(self.last_report_at) >= u32::from(self.idle) * IDLE_UNIT_MS;
        (changed || expired).then_some(self.report)
    }

    /// Call only once the controller accepts this exact snapshot into its DMA buffer.
    pub fn report_queued(&mut self, report: Report) {
        self.in_flight = Some(report);
    }

    fn read(&mut self, bytes: &[u8], requested: u16) -> Action {
        if requested == 0 {
            return Action::Stall;
        }
        assert!(bytes.len() < CONTROL_MAX_PACKET);
        let length = bytes.len().min(usize::from(requested));
        self.control = Control::DataIn;
        Action::Send(Packet::new(&bytes[..length]))
    }

    fn read_descriptor(&mut self, bytes: &'static [u8], requested: u16) -> Action {
        if requested == 0 {
            return Action::Stall;
        }
        let length = bytes.len().min(usize::from(requested));
        let first = length.min(CONTROL_MAX_PACKET);
        let remainder = &bytes[first..length];
        let needs_zlp = length == bytes.len()
            && bytes.len() < usize::from(requested)
            && length.is_multiple_of(CONTROL_MAX_PACKET);
        self.control = Control::DescriptorIn {
            remaining: remainder,
            zlp: needs_zlp,
        };
        Action::Send(Packet::new(&bytes[..first]))
    }

    fn status(&mut self, commit: Commit) -> Action {
        self.control = Control::StatusIn(commit);
        Action::Send(Packet::new(&[]))
    }

    pub fn setup(&mut self, packet: [u8; 8]) -> Action {
        self.control = Control::Idle;
        let [request_type, request, value @ .., il, ih, ll, lh] = packet;
        let value = u16::from_le_bytes(value);
        let index = u16::from_le_bytes([il, ih]);
        let length = u16::from_le_bytes([ll, lh]);
        // SET_IDLE's wValue: the idle duration, then a report ID that must be zero.
        let [idle_report, idle] = value.to_le_bytes();
        match (request_type, request, value, index, length) {
            (STANDARD_DEVICE_IN, GET_DESCRIPTOR, DEVICE_DESCRIPTOR, 0, _) => {
                self.read_descriptor(&DEVICE, length)
            }
            (STANDARD_DEVICE_IN, GET_DESCRIPTOR, CONFIGURATION_DESCRIPTOR, 0, _) => {
                self.read_descriptor(&CONFIGURATION, length)
            }
            (STANDARD_DEVICE_IN, GET_DESCRIPTOR, LANGUAGES_DESCRIPTOR, 0, _) => {
                self.read_descriptor(&LANGUAGES, length)
            }
            (STANDARD_DEVICE_IN, GET_DESCRIPTOR, PRODUCT_DESCRIPTOR, 0 | LANGUAGE, _) => {
                self.read_descriptor(&PRODUCT, length)
            }
            (STANDARD_INTERFACE_IN, GET_DESCRIPTOR, HID_DESCRIPTOR, KEYBOARD, _) => {
                self.read_descriptor(&KEYBOARD_HID, length)
            }
            (STANDARD_INTERFACE_IN, GET_DESCRIPTOR, REPORT_DESCRIPTOR, KEYBOARD, _) => {
                self.read_descriptor(&hid::KEYBOARD, length)
            }
            (STANDARD_DEVICE_OUT, SET_ADDRESS, address @ 0..=127, 0, 0) if !self.configured => {
                self.status(Commit::Address(address as u8))
            }
            (STANDARD_DEVICE_OUT, SET_CONFIGURATION, configuration @ (0 | CONFIGURED), 0, 0)
                if self.address != 0 =>
            {
                self.status(Commit::Configuration(configuration == CONFIGURED))
            }
            (STANDARD_DEVICE_IN, GET_CONFIGURATION, 0, 0, 1) if self.address != 0 => {
                self.read(&[u8::from(self.configured)], 1)
            }
            (STANDARD_DEVICE_IN, GET_STATUS, 0, 0, 2) => self.read(&[0, 0], 2),
            (STANDARD_INTERFACE_IN, GET_STATUS, 0, KEYBOARD, 2) if self.configured => {
                self.read(&[0, 0], 2)
            }
            (STANDARD_ENDPOINT_IN, GET_STATUS, 0, CONTROL_OUT | CONTROL_IN, 2) => {
                self.read(&[0, 0], 2)
            }
            (STANDARD_ENDPOINT_IN, GET_STATUS, 0, KEYBOARD_IN, 2) if self.configured => {
                self.read(&[u8::from(self.halted), 0], 2)
            }
            (
                STANDARD_ENDPOINT_OUT,
                request @ (CLEAR_FEATURE | SET_FEATURE),
                ENDPOINT_HALT,
                KEYBOARD_IN,
                0,
            ) if self.configured => self.status(Commit::Halt(request == SET_FEATURE)),
            (STANDARD_INTERFACE_IN, GET_INTERFACE, 0, KEYBOARD, 1) if self.configured => {
                self.read(&[0], 1)
            }
            // Re-selecting the only alternate setting resets the endpoint, as on a halt clear.
            (STANDARD_INTERFACE_OUT, SET_INTERFACE, 0, KEYBOARD, 0) if self.configured => {
                self.status(Commit::Halt(false))
            }
            (CLASS_INTERFACE_IN, GET_REPORT, INPUT_REPORT, KEYBOARD, _) if self.configured => {
                let report = self.report;
                self.read(&report, length)
            }
            (CLASS_INTERFACE_IN, GET_REPORT, FEATURE_REPORT, KEYBOARD, _) if self.configured => {
                let telemetry = self.telemetry;
                self.read(&telemetry, length)
            }
            (CLASS_INTERFACE_IN, GET_REPORT, OUTPUT_REPORT, KEYBOARD, _) if self.configured => {
                self.read(&[self.leds], length)
            }
            (CLASS_INTERFACE_IN, GET_IDLE, 0, KEYBOARD, 1) if self.configured => {
                self.read(&[self.idle], 1)
            }
            (CLASS_INTERFACE_IN, GET_PROTOCOL, 0, KEYBOARD, 1) if self.configured => {
                self.read(&[self.protocol as u8], 1)
            }
            (CLASS_INTERFACE_OUT, SET_REPORT, OUTPUT_REPORT, KEYBOARD, 1) if self.configured => {
                self.control = Control::LedsOut;
                Action::None
            }
            (CLASS_INTERFACE_OUT, SET_IDLE, _, KEYBOARD, 0)
                if self.configured && idle_report == 0 =>
            {
                self.status(Commit::Idle(idle))
            }
            (CLASS_INTERFACE_OUT, SET_PROTOCOL, protocol @ 0..=1, KEYBOARD, 0)
                if self.configured =>
            {
                self.status(Commit::Protocol(if protocol == Protocol::Boot as u16 {
                    Protocol::Boot
                } else {
                    Protocol::Report
                }))
            }
            _ => Action::Stall,
        }
    }

    pub fn out(&mut self, packet: &[u8]) -> Action {
        match core::mem::take(&mut self.control) {
            Control::LedsOut if packet.len() == 1 => {
                self.status(Commit::Leds(packet[0] & LED_MASK))
            }
            Control::StatusOut if packet.is_empty() => Action::None,
            _ => Action::Stall,
        }
    }

    pub fn in_complete(&mut self, endpoint: u8, now: u32) -> Action {
        if endpoint == Interface::Keyboard.endpoint() {
            if let Some(report) = self.in_flight.take() {
                self.acknowledged = Some(report);
                self.last_report_at = now;
            }
            return Action::None;
        }
        if endpoint != 0 {
            return Action::None;
        }
        match core::mem::take(&mut self.control) {
            Control::DataIn => {
                self.control = Control::StatusOut;
                Action::None
            }
            Control::DescriptorIn { remaining, zlp } if !remaining.is_empty() => {
                let length = remaining.len().min(CONTROL_MAX_PACKET);
                let (packet, tail) = remaining.split_at(length);
                self.control = Control::DescriptorIn {
                    remaining: tail,
                    zlp,
                };
                Action::Send(Packet::new(packet))
            }
            Control::DescriptorIn { zlp: true, .. } => {
                self.control = Control::DataIn;
                Action::Send(Packet::new(&[]))
            }
            Control::DescriptorIn { .. } => {
                self.control = Control::StatusOut;
                Action::None
            }
            Control::StatusIn(commit) => match commit {
                Commit::Address(address) => {
                    self.address = address;
                    Action::Address(address)
                }
                Commit::Configuration(configured) => {
                    self.configured = configured;
                    self.halted = false;
                    self.in_flight = None;
                    self.acknowledged = None;
                    Action::Configure(configured)
                }
                Commit::Halt(halted) => {
                    self.halted = halted;
                    self.in_flight = None;
                    self.acknowledged = None;
                    Action::Halt(halted)
                }
                Commit::Protocol(protocol) => {
                    self.protocol = protocol;
                    self.acknowledged = None;
                    Action::None
                }
                Commit::Idle(idle) => {
                    self.idle = idle;
                    self.last_report_at = now;
                    Action::None
                }
                Commit::Leds(leds) => {
                    self.leds = leds;
                    Action::None
                }
            },
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn setup(kind: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
        let [vl, vh] = value.to_le_bytes();
        let [il, ih] = index.to_le_bytes();
        let [ll, lh] = length.to_le_bytes();
        [kind, request, vl, vh, il, ih, ll, lh]
    }

    fn data(action: Action) -> Vec<u8> {
        match action {
            Action::Send(packet) => packet.as_slice().to_vec(),
            other => panic!("expected data, got {other:?}"),
        }
    }

    fn read_all(device: &mut Device, packet: [u8; 8]) -> Vec<u8> {
        let mut bytes = data(device.setup(packet));
        loop {
            match device.in_complete(0, 0) {
                Action::Send(packet) => bytes.extend_from_slice(packet.as_slice()),
                Action::None => break,
                other => panic!("unexpected completion: {other:?}"),
            }
        }
        bytes
    }

    fn configured() -> Device {
        let mut device = Device::default();
        assert!(data(device.setup(setup(0, 5, 7, 0, 0))).is_empty());
        assert_eq!(device.in_complete(0, 0), Action::Address(7));
        assert!(data(device.setup(setup(0, 9, 1, 0, 0))).is_empty());
        assert_eq!(device.in_complete(0, 0), Action::Configure(true));
        device
    }

    #[test]
    fn enumeration_descriptors_and_short_reads() {
        let mut device = Device::default();
        let prefix = data(device.setup(setup(0x80, 6, 0x0100, 0, 8)));
        assert_eq!(prefix, DEVICE[..8]);
        assert_eq!(device.in_complete(0, 0), Action::None);
        assert_eq!(device.out(&[]), Action::None);
        let config = data(device.setup(setup(0x80, 6, 0x0200, 0, 255)));
        assert_eq!(
            u16::from_le_bytes([config[2], config[3]]) as usize,
            config.len()
        );
        let mut rest = config.as_slice();
        let mut types = Vec::new();
        while !rest.is_empty() {
            let (descriptor, tail) = rest.split_at(rest[0] as usize);
            types.push(descriptor[1]);
            match descriptor[1] {
                4 => assert_eq!(&descriptor[5..8], &[3, 1, 1]),
                0x21 => assert_eq!(
                    u16::from_le_bytes([descriptor[7], descriptor[8]]) as usize,
                    hid::KEYBOARD.len()
                ),
                5 => assert_eq!(&descriptor[2..], &[0x81, 3, 8, 0, 1]),
                _ => {}
            }
            rest = tail;
        }
        assert_eq!(types, [2, 4, 0x21, 5]);
        assert_eq!(
            read_all(&mut device, setup(0x81, 6, 0x2200, 0, 255)),
            hid::KEYBOARD
        );
        assert_eq!(
            data(device.setup(setup(0x81, 6, 0x2100, 0, 255))),
            KEYBOARD_HID
        );
        let string = data(device.setup(setup(0x80, 6, 0x0301, 0x0409, 255)));
        let utf16: Vec<u16> = string[2..]
            .chunks_exact(2)
            .map(|x| u16::from_le_bytes([x[0], x[1]]))
            .collect();
        assert_eq!(
            std::string::String::from_utf16(&utf16).unwrap(),
            descriptors::PRODUCT_NAME
        );
    }

    #[test]
    fn descriptor_declares_exactly_eight_input_bytes_and_one_led_byte_without_ids() {
        let (mut size, mut count, mut input, mut output, mut feature) = (0, 0, 0, 0, 0);
        let mut bytes: &[u8] = &hid::KEYBOARD;
        while !bytes.is_empty() {
            let len = match bytes[0] & 3 {
                3 => 4,
                x => x as usize,
            };
            let tag = bytes[0] & 0xfc;
            let value = if len > 0 { bytes[1] as usize } else { 0 };
            match tag {
                0x74 => size = value,
                0x94 => count = value,
                0x80 => input += size * count,
                0x90 => output += size * count,
                0xb0 => feature += size * count,
                0x84 => panic!("boot-compatible report must not have an ID"),
                _ => {}
            }
            bytes = &bytes[1 + len..];
        }
        assert_eq!((input, output, feature), (64, 8, 128));
    }

    #[test]
    fn descriptor_reads_stream_packets_zlps_and_honor_requested_lengths() {
        for length in [1, 63, 64, 65, hid::KEYBOARD.len() as u16, 255] {
            let mut device = Device::default();
            let result = read_all(&mut device, setup(0x81, 6, 0x2200, 0, length));
            assert_eq!(
                result,
                hid::KEYBOARD[..hid::KEYBOARD.len().min(length as usize)]
            );
        }
        static SIXTY_FOUR: [u8; 64] = [0x5a; 64];
        static ONE_TWENTY_EIGHT: [u8; 128] = [0xa5; 128];
        for (descriptor, requested, expected_packets) in [
            (&SIXTY_FOUR[..], 255, 2),
            (&ONE_TWENTY_EIGHT[..], 255, 3),
            (&ONE_TWENTY_EIGHT[..], 128, 2),
        ] {
            let mut device = Device::default();
            let mut action = device.read_descriptor(descriptor, requested);
            let mut packets = Vec::new();
            loop {
                packets.push(data(action));
                action = device.in_complete(0, 0);
                if action == Action::None {
                    break;
                }
            }
            assert_eq!(packets.len(), expected_packets);
            assert!(
                packets
                    .iter()
                    .take(expected_packets - 1)
                    .all(|p| p.len() == 64)
            );
            if descriptor.len() % 64 == 0 && requested > descriptor.len() as u16 {
                assert!(packets.last().unwrap().is_empty());
            } else {
                assert!(packets.iter().all(|packet| !packet.is_empty()));
            }
        }
    }

    #[test]
    fn feature_report_is_read_only_and_survives_reset_and_aborted_writes() {
        let mut device = configured();
        let telemetry = *b"N75D\x01\x00\x10\x00\x00\x00\x00\x00\x83\x00\x00\x00";
        device.set_telemetry(telemetry);
        for protocol in [0, 1] {
            device.setup(setup(0x21, 11, protocol, 0, 0));
            device.in_complete(0, 0);
            for (length, expected) in [(1, 1), (16, 16), (17, 16)] {
                assert_eq!(
                    data(device.setup(setup(0xa1, 1, 0x0300, 0, length))),
                    telemetry[..expected]
                );
            }
            assert_eq!(device.setup(setup(0xa1, 1, 0x0301, 0, 16)), Action::Stall);
            assert_eq!(device.setup(setup(0xa1, 1, 0x0300, 1, 16)), Action::Stall);
            assert_eq!(device.setup(setup(0x21, 9, 0x0300, 0, 16)), Action::Stall);
            device.setup(setup(0x21, 9, 0x0200, 0, 1));
            device.setup(setup(0xa1, 1, 0x0300, 0, 16));
            device.reset();
            device.setup(setup(0, 5, 7, 0, 0));
            device.in_complete(0, 0);
            device.setup(setup(0, 9, 1, 0, 0));
            device.in_complete(0, 0);
            assert_eq!(data(device.setup(setup(0xa1, 1, 0x0300, 0, 16))), telemetry);
        }
    }

    #[test]
    fn address_and_configuration_wait_for_status_ack_and_can_be_aborted() {
        let mut device = Device::default();
        device.setup(setup(0, 5, 42, 0, 0));
        device.setup(setup(0x80, 6, 0x0100, 0, 18));
        assert_eq!(device.in_complete(0, 0), Action::None);
        assert_eq!(device.setup(setup(0, 9, 1, 0, 0)), Action::Stall);
        device.setup(setup(0, 5, 42, 0, 0));
        assert_eq!(device.in_complete(0, 0), Action::Address(42));
        device.setup(setup(0, 9, 1, 0, 0));
        assert_eq!(device.report_due(0), None);
        assert_eq!(device.in_complete(0, 0), Action::Configure(true));
        assert_eq!(device.report_due(0), Some([0; 8]));
    }

    #[test]
    fn malformed_and_unsupported_requests_stall_without_state_changes() {
        let mut device = configured();
        for packet in [
            setup(0x80, 6, 0x0100, 0, 0),
            setup(0x81, 6, 0x2200, 0, 0),
            setup(0xa1, 1, 0x0100, 0, 0),
            setup(0xa1, 1, 0x0200, 0, 0),
            setup(0xa1, 1, 0x0300, 0, 0),
            setup(0x40, 6, 0x0100, 0, 18),
            setup(0, 6, 0x0100, 0, 18),
            setup(0x81, 6, 0x2200, 1, 63),
            setup(0x80, 6, 0x0600, 0, 10),
            setup(0, 5, 128, 0, 0),
            setup(0, 9, 2, 0, 0),
            setup(0, 9, 0, 1, 0),
            setup(0x21, 11, 2, 0, 0),
            setup(0x21, 10, 1, 0, 0),
            setup(0x21, 9, 0x0200, 0, 2),
            setup(0xa1, 1, 0x0101, 0, 8),
            setup(0x01, 11, 1, 0, 0),
            setup(0x02, 1, 0, 0x01, 0),
            setup(0, 3, 1, 0, 0),
        ] {
            assert_eq!(device.setup(packet), Action::Stall, "{packet:?}");
        }
        assert_eq!(device.report_due(0), Some([0; 8]));
    }

    #[test]
    fn led_output_is_committed_only_after_a_valid_transfer() {
        let mut device = configured();
        let request = setup(0x21, 9, 0x0200, 0, 1);
        assert_eq!(device.setup(request), Action::None);
        assert!(data(device.out(&[2])).is_empty());
        assert_eq!(device.leds(), 0);
        device.in_complete(0, 0);
        assert_eq!(device.leds(), 2);
        assert_eq!(data(device.setup(setup(0xa1, 1, 0x0200, 0, 1))), [2]);
        device.setup(request);
        assert_eq!(device.out(&[]), Action::Stall);
        device.setup(request);
        device.out(&[1]);
        device.setup(setup(0x80, 6, 0x0100, 0, 18));
        device.in_complete(0, 0);
        assert_eq!(device.leds(), 2);
    }

    #[test]
    fn boot_and_report_protocols_both_return_six_kro() {
        let mut device = configured();
        let report = [2, 0, 4, 5, 6, 0, 0, 0];
        device.set_report(report);
        for protocol in [0, 1] {
            device.setup(setup(0x21, 11, protocol, 0, 0));
            device.in_complete(0, 0);
            assert_eq!(
                data(device.setup(setup(0xa1, 3, 0, 0, 1))),
                [protocol as u8]
            );
            assert_eq!(data(device.setup(setup(0xa1, 1, 0x0100, 0, 8))), report);
        }
    }

    #[test]
    fn slow_host_keeps_dma_snapshot_then_gets_latest_state() {
        let mut device = configured();
        let down = [0, 0, 4, 0, 0, 0, 0, 0];
        device.set_report(down);
        assert_eq!(device.report_due(0), Some(down));
        device.report_queued(down);
        device.set_report([0; 8]);
        assert_eq!(device.report_due(100), None);
        device.in_complete(1, 100);
        assert_eq!(device.report_due(100), Some([0; 8]));
        device.report_queued([0; 8]);
        device.in_complete(1, 101);
        assert_eq!(device.report_due(1000), None);
    }

    #[test]
    fn idle_uses_four_ms_units_and_wraps() {
        let mut device = configured();
        device.report_queued([0; 8]);
        device.in_complete(1, u32::MAX - 5);
        device.setup(setup(0x21, 10, 2 << 8, 0, 0));
        device.in_complete(0, u32::MAX - 5);
        assert_eq!(device.report_due(1), None);
        assert_eq!(device.report_due(2), Some([0; 8]));
        assert_eq!(data(device.setup(setup(0xa1, 2, 0, 0, 1))), [2]);
        device.setup(setup(0x21, 10, 0, 0, 0));
        device.in_complete(0, 3);
        assert_eq!(device.report_due(100), None);
    }

    #[test]
    fn halt_unconfigure_suspend_resume_and_reset() {
        let mut device = configured();
        device.report_queued([0; 8]);
        device.setup(setup(2, 3, 0, 0x81, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(true));
        assert_eq!(device.report_due(0), None);
        assert_eq!(data(device.setup(setup(0x82, 0, 0, 0x81, 2))), [1, 0]);
        device.setup(setup(2, 1, 0, 0x81, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(false));
        assert_eq!(device.report_due(0), Some([0; 8]));
        device.set_suspended(true);
        assert_eq!(device.report_due(0), None);
        device.set_suspended(false);
        assert_eq!(device.report_due(0), Some([0; 8]));
        device.setup(setup(0, 9, 0, 0, 0));
        assert_eq!(device.in_complete(0, 0), Action::Configure(false));
        assert_eq!(device.report_due(0), None);
        device.reset();
        assert_eq!(device.setup(setup(0xa1, 3, 0, 0, 1)), Action::Stall);
        assert_eq!(device.report_due(0), None);
    }
}
