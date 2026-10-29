//! USB HID device state machine: a boot/report keyboard on interface 0
//! (EP1) and a consumer-control interface on interface 1 (EP2) for media
//! keys. Descriptors, control transfers, report scheduling. Independent
//! of MMIO; the register driver is in the keyboard task.

#![no_std]

#[cfg(test)]
extern crate std;

mod bytes;
mod descriptors;
mod hid;
mod report;

use descriptors::{
    CONFIGURATION, CONFIGURATION_TYPE, CONFIGURATION_VALUE, CONSUMER_HID, CONTROL_MAX_PACKET,
    DEVICE, DEVICE_TYPE, HID_TYPE, KEYBOARD_HID, LANGUAGES, PRODUCT, PRODUCT_STRING, REPORT_TYPE,
    STRING_TYPE, US_ENGLISH,
};
use heapless::Deque;
use hid::{LED_MASK, TELEMETRY_BYTES};

pub use report::Report;

/// The HID interfaces, in configuration descriptor order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interface {
    Keyboard,
    Consumer,
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
const CONSUMER: u16 = Interface::Consumer as u16;
const CONTROL_OUT: u16 = 0x00;
const CONTROL_IN: u16 = DIRECTION_IN as u16;
const KEYBOARD_IN: u16 = Interface::Keyboard.endpoint_address() as u16;
const CONSUMER_IN: u16 = Interface::Consumer.endpoint_address() as u16;
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
    /// Endpoint address (0x81 or 0x82) and new halt state.
    Halt(u8, bool),
    /// Disarm EP1 without changing its data toggle when switching report formats.
    CancelKeyboardReport,
    Stall,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Protocol {
    Boot = 0,
    #[default]
    Report = 1,
}

#[derive(Clone, Copy)]
enum Commit {
    Address(u8),
    Configuration(bool),
    Halt(Interface, bool),
    Protocol(Protocol),
    Idle(u8),
    ConsumerIdle(u8),
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
    ep2_halted: bool,
    suspended: bool,
    protocol: Protocol,
    idle: u8,
    leds: u8,
    report: Report,
    pending: Deque<Report, 16>,
    telemetry: [u8; TELEMETRY_BYTES],
    in_flight: Option<Report>,
    acknowledged: Option<Report>,
    last_report_at: u32,
    consumer: u16,
    consumer_idle: u8,
    consumer_last_report_at: u32,
    consumer_in_flight: Option<u16>,
    consumer_acknowledged: Option<u16>,
    consumer_pending: Deque<u16, 16>,
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
        if suspended {
            // The register driver disarms both interrupt endpoints as well.
            self.in_flight = None;
            self.pending.clear();
            self.consumer_in_flight = None;
            self.cancel_consumer();
        } else {
            self.acknowledged = None;
            self.consumer_acknowledged = None;
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
        if report == self.report {
            return;
        }
        self.report = report;
        if !self.active() || self.halted || self.pending.push_back(report).is_err() {
            // On disconnect or overflow, converge to the current physical keys
            // rather than replaying stale input or leaving a released key down.
            self.pending.clear();
        }
    }

    pub fn report_packet(&self, report: Report) -> Packet {
        report.packet(self.protocol)
    }

    pub fn report_due(&self, now: u32) -> Option<Report> {
        if !self.configured || self.suspended || self.halted || self.in_flight.is_some() {
            return None;
        }
        if let Some(report) = self.pending.front() {
            return Some(*report);
        }
        let changed = self.acknowledged != Some(self.report);
        let expired = self.idle != 0
            && now.wrapping_sub(self.last_report_at) >= u32::from(self.idle) * IDLE_UNIT_MS;
        (changed || expired).then_some(self.report)
    }

    /// Call only once the controller accepts this exact snapshot into its DMA buffer.
    pub fn report_queued(&mut self, report: Report) {
        if let Some(pending) = self.pending.pop_front() {
            debug_assert_eq!(pending, report);
        }
        self.in_flight = Some(report);
    }

    /// Preserve transitions until EP2 acknowledges them. On overflow or
    /// disconnect, discard gestures and converge to all-up instead of
    /// risking a stuck volume key. The producer must wait for a fresh touch.
    pub fn enqueue_consumer(&mut self, usage: u16) -> bool {
        if !self.active() || self.ep2_halted {
            self.cancel_consumer();
            return false;
        }
        if usage == self.consumer {
            return true;
        }
        if self.consumer_pending.push_back(usage).is_err() {
            self.cancel_consumer();
            return false;
        }
        self.consumer = usage;
        true
    }

    /// Discard pending gestures and release any usage already sent to the host.
    pub fn cancel_consumer(&mut self) {
        self.consumer = 0;
        self.consumer_pending.clear();
    }

    pub fn consumer_due(&self, now: u32) -> Option<[u8; 2]> {
        let expired = self.consumer_idle != 0
            && now.wrapping_sub(self.consumer_last_report_at)
                >= u32::from(self.consumer_idle) * IDLE_UNIT_MS;
        if !self.configured
            || self.suspended
            || self.ep2_halted
            || self.consumer_in_flight.is_some()
            || (self.consumer_pending.is_empty()
                && self.consumer_acknowledged == Some(self.consumer)
                && !expired)
        {
            return None;
        }
        let usage = self
            .consumer_pending
            .front()
            .copied()
            .unwrap_or(self.consumer);
        Some(usage.to_le_bytes())
    }

    /// Call only once the controller accepts this exact snapshot into its DMA buffer.
    pub fn consumer_queued(&mut self, usage: u16) {
        self.consumer_in_flight = Some(usage);
        if let Some(pending) = self.consumer_pending.pop_front() {
            debug_assert_eq!(pending, usage);
        }
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
            (STANDARD_INTERFACE_IN, GET_DESCRIPTOR, HID_DESCRIPTOR, CONSUMER, _) => {
                self.read_descriptor(&CONSUMER_HID, length)
            }
            (STANDARD_INTERFACE_IN, GET_DESCRIPTOR, REPORT_DESCRIPTOR, CONSUMER, _) => {
                self.read_descriptor(&hid::CONSUMER, length)
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
            (STANDARD_INTERFACE_IN, GET_STATUS, 0, KEYBOARD | CONSUMER, 2) if self.configured => {
                self.read(&[0, 0], 2)
            }
            (STANDARD_ENDPOINT_IN, GET_STATUS, 0, CONTROL_OUT | CONTROL_IN, 2) => {
                self.read(&[0, 0], 2)
            }
            (STANDARD_ENDPOINT_IN, GET_STATUS, 0, KEYBOARD_IN, 2) if self.configured => {
                self.read(&[u8::from(self.halted), 0], 2)
            }
            (STANDARD_ENDPOINT_IN, GET_STATUS, 0, CONSUMER_IN, 2) if self.configured => {
                self.read(&[u8::from(self.ep2_halted), 0], 2)
            }
            (
                STANDARD_ENDPOINT_OUT,
                request @ (CLEAR_FEATURE | SET_FEATURE),
                ENDPOINT_HALT,
                KEYBOARD_IN,
                0,
            ) if self.configured => {
                self.status(Commit::Halt(Interface::Keyboard, request == SET_FEATURE))
            }
            (
                STANDARD_ENDPOINT_OUT,
                request @ (CLEAR_FEATURE | SET_FEATURE),
                ENDPOINT_HALT,
                CONSUMER_IN,
                0,
            ) if self.configured => {
                self.status(Commit::Halt(Interface::Consumer, request == SET_FEATURE))
            }
            (STANDARD_INTERFACE_IN, GET_INTERFACE, 0, KEYBOARD | CONSUMER, 1)
                if self.configured =>
            {
                self.read(&[0], 1)
            }
            // Re-selecting the only alternate setting resets the endpoint, as on a halt clear.
            (STANDARD_INTERFACE_OUT, SET_INTERFACE, 0, KEYBOARD, 0) if self.configured => {
                self.status(Commit::Halt(Interface::Keyboard, false))
            }
            (STANDARD_INTERFACE_OUT, SET_INTERFACE, 0, CONSUMER, 0) if self.configured => {
                self.status(Commit::Halt(Interface::Consumer, false))
            }
            (CLASS_INTERFACE_IN, GET_REPORT, INPUT_REPORT, KEYBOARD, _) if self.configured => {
                let packet = self.report_packet(self.report);
                self.read(packet.as_slice(), length)
            }
            (CLASS_INTERFACE_IN, GET_REPORT, FEATURE_REPORT, KEYBOARD, _) if self.configured => {
                let telemetry = self.telemetry;
                self.read(&telemetry, length)
            }
            (CLASS_INTERFACE_IN, GET_REPORT, OUTPUT_REPORT, KEYBOARD, _) if self.configured => {
                self.read(&[self.leds], length)
            }
            (CLASS_INTERFACE_IN, GET_REPORT, INPUT_REPORT, CONSUMER, _) if self.configured => {
                let consumer = self.consumer.to_le_bytes();
                self.read(&consumer, length)
            }
            (CLASS_INTERFACE_IN, GET_IDLE, 0, KEYBOARD, 1) if self.configured => {
                self.read(&[self.idle], 1)
            }
            (CLASS_INTERFACE_IN, GET_IDLE, 0, CONSUMER, 1) if self.configured => {
                self.read(&[self.consumer_idle], 1)
            }
            (CLASS_INTERFACE_IN, GET_PROTOCOL, 0, KEYBOARD, 1) if self.configured => {
                self.read(&[self.protocol as u8], 1)
            }
            (CLASS_INTERFACE_OUT, SET_IDLE, _, CONSUMER, 0)
                if self.configured && idle_report == 0 =>
            {
                self.status(Commit::ConsumerIdle(idle))
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
        if endpoint == Interface::Consumer.endpoint() {
            if let Some(usage) = self.consumer_in_flight.take() {
                self.consumer_acknowledged = Some(usage);
                self.consumer_last_report_at = now;
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
                    self.ep2_halted = false;
                    self.in_flight = None;
                    self.acknowledged = None;
                    self.pending.clear();
                    self.consumer_in_flight = None;
                    self.consumer_acknowledged = None;
                    self.cancel_consumer();
                    Action::Configure(configured)
                }
                Commit::Halt(interface, halted) => {
                    match interface {
                        Interface::Keyboard => {
                            self.halted = halted;
                            self.in_flight = None;
                            self.acknowledged = None;
                            self.pending.clear();
                        }
                        Interface::Consumer => {
                            self.ep2_halted = halted;
                            self.consumer_in_flight = None;
                            self.consumer_acknowledged = None;
                            self.cancel_consumer();
                        }
                    }
                    Action::Halt(interface.endpoint_address(), halted)
                }
                Commit::Protocol(protocol) if self.protocol == protocol => Action::None,
                Commit::Protocol(protocol) => {
                    self.protocol = protocol;
                    self.in_flight = None;
                    self.acknowledged = None;
                    self.pending.clear();
                    Action::CancelKeyboardReport
                }
                Commit::ConsumerIdle(idle) => {
                    self.consumer_idle = idle;
                    self.consumer_last_report_at = now;
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
    use std::{vec, vec::Vec};

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
        let mut interfaces = Vec::new();
        let mut endpoints = Vec::new();
        while !rest.is_empty() {
            let (descriptor, tail) = rest.split_at(rest[0] as usize);
            types.push(descriptor[1]);
            match descriptor[1] {
                4 => interfaces.push(descriptor[2]),
                5 => endpoints.push((
                    descriptor[2],
                    u16::from_le_bytes([descriptor[4], descriptor[5]]),
                )),
                _ => {}
            }
            rest = tail;
        }
        assert_eq!(types, [2, 4, 0x21, 5, 4, 0x21, 5]);
        assert_eq!(interfaces, [0, 1]);
        assert_eq!(endpoints, [(0x81, 15), (0x82, 2)]);
        assert_eq!(
            read_all(&mut device, setup(0x81, 6, 0x2200, 0, 255)),
            hid::KEYBOARD
        );
        assert_eq!(
            data(device.setup(setup(0x81, 6, 0x2100, 0, 255))),
            KEYBOARD_HID
        );
        assert_eq!(
            read_all(&mut device, setup(0x81, 6, 0x2200, 1, 255)),
            hid::CONSUMER
        );
        assert_eq!(
            data(device.setup(setup(0x81, 6, 0x2100, 1, 255))),
            CONSUMER_HID
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
    fn descriptor_declares_nkro_input_and_one_led_byte_without_ids() {
        let (mut page, mut first, mut last, mut size, mut count) = (0, 0, 0, 0, 0);
        let (mut input, mut output, mut feature) = (0, 0, 0);
        let mut fields = Vec::new();
        let mut bytes: &[u8] = &hid::KEYBOARD;
        while !bytes.is_empty() {
            let len = match bytes[0] & 3 {
                3 => 4,
                x => x as usize,
            };
            let tag = bytes[0] & 0xfc;
            let mut value = 0;
            for (i, byte) in bytes[1..1 + len].iter().enumerate() {
                value |= usize::from(*byte) << (8 * i);
            }
            match tag {
                0x04 => page = value,
                0x18 => first = value,
                0x28 => last = value,
                0x74 => size = value,
                0x94 => count = value,
                0x80 => {
                    fields.push((page, first, last, size, count, value));
                    input += size * count;
                }
                0x90 => output += size * count,
                0xb0 => feature += size * count,
                0x84 => panic!("keyboard report must not have an ID"),
                _ => {}
            }
            bytes = &bytes[1 + len..];
        }
        assert_eq!(fields, [(7, 0xe0, 0xe7, 1, 8, 2), (7, 4, 0x73, 1, 112, 2)]);
        assert_eq!((input, output, feature), (120, 8, 128));
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
        assert_eq!(device.report_due(0), Some(Report::default()));
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
            setup(0x81, 6, 0x2200, 2, 63),
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
        assert_eq!(device.report_due(0), Some(Report::default()));
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
    fn boot_and_report_protocols_encode_the_same_keys_differently() {
        let mut device = configured();
        let report = Report::new(2, [4, 5, 6]);
        device.set_report(report);
        for (protocol, expected) in [
            (0, vec![2, 0, 4, 5, 6, 0, 0, 0]),
            (1, vec![2, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        ] {
            device.setup(setup(0x21, 11, protocol, 0, 0));
            assert_eq!(device.in_complete(0, 0), Action::CancelKeyboardReport);
            assert_eq!(
                data(device.setup(setup(0xa1, 3, 0, 0, 1))),
                [protocol as u8]
            );
            assert_eq!(data(device.setup(setup(0xa1, 1, 0x0100, 0, 64))), expected);
            let due = device.report_due(0).unwrap();
            assert_eq!(device.report_packet(due).as_slice(), expected);
            assert_eq!(
                data(device.setup(setup(0xa1, 1, 0x0100, 0, 3))),
                expected[..3]
            );
        }
    }

    #[test]
    fn nkro_chords_release_individually_and_boot_recovers_from_rollover() {
        let mut device = configured();
        for protocol in [1, 0] {
            device.setup(setup(0x21, 11, protocol, 0, 0));
            device.in_complete(0, 0);
            for (usages, expected) in [
                (
                    4..=10,
                    if protocol == 1 {
                        vec![2, 0x7f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                    } else {
                        vec![2, 0, 1, 1, 1, 1, 1, 1]
                    },
                ),
                (
                    5..=10,
                    if protocol == 1 {
                        vec![2, 0x7e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                    } else {
                        vec![2, 0, 5, 6, 7, 8, 9, 10]
                    },
                ),
                (
                    0..=0,
                    if protocol == 1 {
                        vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                    } else {
                        vec![2, 0, 0, 0, 0, 0, 0, 0]
                    },
                ),
            ] {
                device.set_report(Report::new(2, usages));
                let report = device.report_due(0).unwrap();
                assert_eq!(device.report_packet(report).as_slice(), expected);
                assert_eq!(data(device.setup(setup(0xa1, 1, 0x0100, 0, 64))), expected);
                device.report_queued(report);
                device.in_complete(1, 0);
                assert_eq!(device.report_due(0), None);
            }
        }
    }

    #[test]
    fn protocol_switch_cancels_old_snapshots_and_resends_current_keys() {
        let mut device = configured();
        let held = Report::new(2, 4..=10);
        device.set_report(held);
        device.report_queued(held);
        device.set_report(Report::default());
        device.set_report(Report::new(0, [0x73]));

        device.setup(setup(0x21, 11, 0, 0, 0));
        assert_eq!(device.report_due(0), None);
        assert_eq!(device.in_complete(0, 0), Action::CancelKeyboardReport);
        device.in_complete(1, 0); // A canceled transfer cannot acknowledge the new state.
        let current = device.report_due(0).unwrap();
        assert_eq!(current, Report::new(0, [0x73]));
        assert_eq!(
            device.report_packet(current).as_slice(),
            [0, 0, 0x73, 0, 0, 0, 0, 0]
        );
        device.report_queued(current);
        device.in_complete(1, 0);
        assert_eq!(device.report_due(0), None);

        device.reset();
        device.setup(setup(0, 5, 7, 0, 0));
        device.in_complete(0, 0);
        device.setup(setup(0, 9, 1, 0, 0));
        device.in_complete(0, 0);
        assert_eq!(data(device.setup(setup(0xa1, 3, 0, 0, 1))), [1]);
        assert_eq!(
            device
                .report_packet(device.report_due(0).unwrap())
                .as_slice(),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );
    }

    #[test]
    fn aborted_and_unchanged_protocol_requests_preserve_pending_taps() {
        let mut device = configured();
        let chord = Report::new(2, 4..=10);
        device.set_report(chord);
        device.report_queued(chord);
        device.set_report(Report::default());

        device.setup(setup(0x21, 11, 0, 0, 0));
        device.setup(setup(0xa1, 3, 0, 0, 1));
        assert_eq!(device.in_complete(0, 0), Action::None);
        assert_eq!(data(device.setup(setup(0xa1, 3, 0, 0, 1))), [1]);
        device.setup(setup(0x21, 11, 1, 0, 0));
        assert_eq!(device.in_complete(0, 0), Action::None);
        assert_eq!(device.report_due(0), None);
        device.in_complete(1, 0);
        let released = device.report_due(0).unwrap();
        assert_eq!(device.report_packet(released).as_slice(), [0; 15]);
    }

    #[test]
    fn slow_host_keeps_dma_snapshot_then_gets_latest_state() {
        let mut device = configured();
        let down = Report::new(0, [4]);
        device.set_report(down);
        assert_eq!(device.report_due(0), Some(down));
        device.report_queued(down);
        device.set_report(Report::default());
        assert_eq!(device.report_due(100), None);
        device.in_complete(1, 100);
        assert_eq!(device.report_due(100), Some(Report::default()));
        device.report_queued(Report::default());
        device.in_complete(1, 101);
        assert_eq!(device.report_due(1000), None);
    }

    #[test]
    fn complete_key_taps_survive_a_busy_endpoint() {
        let mut device = configured();
        device.report_queued(Report::default());
        let caps = Report::new(0, [0x39]);
        let a = Report::new(0, [4]);
        for report in [caps, Report::default(), a, Report::default()] {
            device.set_report(report);
        }
        assert_eq!(device.report_due(0), None);
        device.in_complete(1, 1);
        for report in [caps, Report::default(), a, Report::default()] {
            assert_eq!(device.report_due(1), Some(report));
            device.report_queued(report);
            assert_eq!(device.report_due(1), None);
            device.in_complete(1, 2);
        }
        assert_eq!(device.report_due(2), None);
    }

    #[test]
    fn keyboard_overflow_eventually_delivers_the_current_physical_state() {
        let mut device = configured();
        let held = Report::new(2, [4]);
        device.set_report(held);
        device.report_queued(held);
        for usage in (4..30).cycle().take(200) {
            device.set_report(Report::new(0, [usage]));
        }
        device.set_report(Report::default());
        device.in_complete(1, 0);
        let mut last = held;
        let mut sent = 0;
        while let Some(report) = device.report_due(0) {
            assert!(sent < 200, "queue did not drain");
            device.report_queued(report);
            device.in_complete(1, 0);
            last = report;
            sent += 1;
        }
        assert_eq!(last, Report::default());
    }

    #[test]
    fn suspended_keyboard_discards_taps_but_keeps_physically_held_keys() {
        let mut device = configured();
        let a = Report::new(0, [4]);
        let shift = Report::new(2, []);
        device.set_report(a);
        device.report_queued(a);
        device.set_report(Report::default());
        device.set_suspended(true);
        device.set_report(shift);
        device.set_suspended(false);
        assert_eq!(device.report_due(0), Some(shift));
        device.report_queued(shift);
        device.in_complete(1, 0);
        assert_eq!(device.report_due(0), None);
    }

    #[test]
    fn idle_uses_four_ms_units_and_wraps() {
        let mut device = configured();
        device.report_queued(Report::default());
        device.in_complete(1, u32::MAX - 5);
        device.setup(setup(0x21, 10, 2 << 8, 0, 0));
        device.in_complete(0, u32::MAX - 5);
        assert_eq!(device.report_due(1), None);
        assert_eq!(device.report_due(2), Some(Report::default()));
        assert_eq!(data(device.setup(setup(0xa1, 2, 0, 0, 1))), [2]);
        device.setup(setup(0x21, 10, 0, 0, 0));
        device.in_complete(0, 3);
        assert_eq!(device.report_due(100), None);
    }

    #[test]
    fn halt_unconfigure_suspend_resume_and_reset() {
        let mut device = configured();
        device.report_queued(Report::default());
        device.setup(setup(2, 3, 0, 0x81, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(0x81, true));
        assert_eq!(device.report_due(0), None);
        assert_eq!(data(device.setup(setup(0x82, 0, 0, 0x81, 2))), [1, 0]);
        device.setup(setup(2, 1, 0, 0x81, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(0x81, false));
        assert_eq!(device.report_due(0), Some(Report::default()));
        device.set_suspended(true);
        assert_eq!(device.report_due(0), None);
        device.set_suspended(false);
        assert_eq!(device.report_due(0), Some(Report::default()));
        device.setup(setup(0, 9, 0, 0, 0));
        assert_eq!(device.in_complete(0, 0), Action::Configure(false));
        assert_eq!(device.report_due(0), None);
        device.reset();
        assert_eq!(device.setup(setup(0xa1, 3, 0, 0, 1)), Action::Stall);
        assert_eq!(device.report_due(0), None);
    }

    #[test]
    fn consumer_reports_flow_independently_of_the_keyboard() {
        let mut device = configured();
        assert_eq!(device.consumer_due(0), Some([0, 0]));
        device.consumer_queued(0);
        device.in_complete(2, 0);
        assert_eq!(device.consumer_due(0), None);
        assert!(device.enqueue_consumer(0x00e9)); // volume up
        assert_eq!(device.consumer_due(0), Some([0xe9, 0]));
        device.consumer_queued(0x00e9);
        // Release while the press is still in flight: it must wait its turn.
        assert!(device.enqueue_consumer(0));
        assert_eq!(device.consumer_due(0), None);
        device.in_complete(2, 10);
        assert_eq!(device.consumer_due(0), Some([0, 0]));
        device.consumer_queued(0);
        device.in_complete(2, 20);
        assert_eq!(device.consumer_due(0), None);
        assert_eq!(data(device.setup(setup(0xa1, 1, 0x0100, 1, 2))), [0, 0]);
    }

    #[test]
    fn consumer_endpoint_halts_independently() {
        let mut device = configured();
        assert!(device.enqueue_consumer(0x00cd));
        device.setup(setup(2, 3, 0, 0x82, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(0x82, true));
        assert_eq!(device.consumer_due(0), None);
        assert_eq!(device.report_due(0), Some(Report::default()));
        assert_eq!(data(device.setup(setup(0x82, 0, 0, 0x82, 2))), [1, 0]);
        device.setup(setup(2, 1, 0, 0x82, 0));
        assert_eq!(device.in_complete(0, 0), Action::Halt(0x82, false));
        assert_eq!(device.consumer_due(0), Some([0, 0]));
    }

    #[test]
    fn consumer_idle_is_independent_and_requires_report_zero() {
        let mut device = configured();
        device.consumer_queued(0);
        device.in_complete(2, 10);
        device.setup(setup(0x21, 10, 0x0200, 1, 0));
        device.in_complete(0, 10);
        assert_eq!(data(device.setup(setup(0xa1, 2, 0, 1, 1))), [2]);
        assert_eq!(data(device.setup(setup(0xa1, 2, 0, 0, 1))), [0]);
        assert_eq!(device.consumer_due(17), None);
        assert_eq!(device.consumer_due(18), Some([0, 0]));
        assert_eq!(device.setup(setup(0x21, 10, 0x0201, 1, 0)), Action::Stall);
        assert_eq!(device.setup(setup(0xa1, 3, 0, 1, 1)), Action::Stall);
    }

    #[test]
    fn canceled_volume_is_released_even_with_an_in_flight_press() {
        let mut device = configured();
        assert!(device.enqueue_consumer(0xe9));
        device.consumer_queued(0xe9);
        assert!(device.enqueue_consumer(0));
        assert!(device.enqueue_consumer(0xcd));
        device.cancel_consumer();
        device.in_complete(2, 1);
        assert_eq!(device.consumer_due(1), Some([0, 0]));
        device.consumer_queued(0);
        device.in_complete(2, 2);
        assert_eq!(device.consumer_due(2), None);
    }

    #[test]
    fn media_taps_survive_a_busy_endpoint() {
        let mut device = configured();
        device.consumer_queued(0);
        for usage in [0xb5, 0, 0xcd, 0, 0xea, 0] {
            assert!(device.enqueue_consumer(usage));
        }
        assert_eq!(device.consumer_due(0), None);
        device.in_complete(2, 1);
        for usage in [0xb5u16, 0, 0xcd, 0, 0xea, 0] {
            assert_eq!(device.consumer_due(0), Some(usage.to_le_bytes()));
            device.consumer_queued(usage);
            assert_eq!(device.consumer_due(0), None);
            device.in_complete(2, 2);
        }
        assert_eq!(device.consumer_due(0), None);
    }

    #[test]
    fn media_overflow_releases_the_in_flight_key() {
        let mut device = configured();
        assert!(device.enqueue_consumer(0xe9));
        device.consumer_queued(0xe9);
        let mut accepted = 0;
        for usage in [0, 0xcd].into_iter().cycle().take(100) {
            if !device.enqueue_consumer(usage) {
                break;
            }
            accepted += 1;
        }
        assert!((1..100).contains(&accepted));
        device.in_complete(2, 1);
        assert_eq!(device.consumer_due(0), Some([0, 0]));
        device.consumer_queued(0);
        device.in_complete(2, 2);
        assert_eq!(device.consumer_due(0), None);
    }

    #[test]
    fn suspend_and_reset_discard_stale_media_gestures() {
        let mut device = configured();
        assert!(device.enqueue_consumer(0xe9));
        device.consumer_queued(0xe9);
        assert!(device.enqueue_consumer(0));
        assert!(device.enqueue_consumer(0xcd));
        device.set_suspended(true);
        assert!(!device.enqueue_consumer(0xb5));
        device.set_suspended(false);
        // Resume must not wait for (or transmit) the canceled DMA press.
        assert_eq!(device.consumer_due(0), Some([0, 0]));
        device.in_complete(2, 1);
        assert_eq!(device.consumer_due(0), Some([0, 0]));
        assert!(device.enqueue_consumer(0xea));
        device.reset();
        assert!(!device.enqueue_consumer(0xcd));
        device.setup(setup(0, 5, 1, 0, 0));
        device.in_complete(0, 0);
        device.setup(setup(0, 9, 1, 0, 0));
        device.in_complete(0, 0);
        assert_eq!(device.consumer_due(0), Some([0, 0]));
    }
}
