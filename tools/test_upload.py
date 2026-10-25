"""Uploader wire-format, sequencing, device-selection, and transport tests.

The synthetic peer is not an implementation of the missing vendor loader.
"""

import contextlib
import hashlib
import io
import itertools
import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from upload import (
    ENTER_LOADER,
    Firmware,
    HidTransport,
    UploadError,
    data_reports,
    find_loader,
    main,
    select_device,
    upload,
    wait_for_loader,
)


class Peer:
    def __init__(self, failure=None, corrupt=False):
        self.calls = []
        self.memory = bytearray(b"\xff" * 1024)
        self.failure = failure
        self.corrupt = corrupt

    def exchange(self, packet, timeout):
        index = len(self.calls)
        self.calls.append((packet, timeout))
        result = None
        if self.failure and index == self.failure[0]:
            result = self.failure[1]
            if isinstance(result, Exception):
                raise result
        command, size = packet[:2]
        offset = int.from_bytes(packet[2:6], "little")
        if command == 0x80:
            self.memory[offset : offset + size] = packet[6 : 6 + size]
            if self.corrupt and offset == 0:
                self.memory[0] ^= 1
        elif command == 0x82:
            if self.memory[offset : offset + size] != packet[6 : 6 + size]:
                return b"\x01\0"
        return b"\0\0" if result is None else result


class FakeHid:
    def __init__(self, reads=(), write_result=None):
        self.reads = iter(reads)
        self.read_calls = []
        self.writes = []
        self.write_result = write_result
        self.open_path = Mock()
        self.close = Mock()

    def read(self, size, timeout):
        self.read_calls.append((size, timeout))
        result = next(self.reads, [])
        if isinstance(result, Exception):
            raise result
        return result

    def write(self, data):
        self.writes.append(data)
        return len(data) if self.write_result is None else self.write_result


class UploaderTests(unittest.TestCase):
    def setUp(self):
        raw = bytearray(range(256))
        struct.pack_into("<I", raw, 0, 0x0940006F)
        struct.pack_into("<I", raw, 0x14, 0xF5F9BDA9)
        self.raw = bytes(raw)
        self.firmware = Firmware.parse(self.raw)

    def test_parse_preserves_raw_digest_and_zero_pads_only_the_tail(self):
        raw = self.raw[:-1]
        firmware = Firmware.parse(raw)
        self.assertEqual(firmware.data, raw + b"\0")
        self.assertEqual(firmware.original_size, 255)
        self.assertEqual(firmware.sha256, hashlib.sha256(raw).hexdigest())

    def test_rejects_elf_address_padding_wrong_marker_and_oversize(self):
        for raw in (
            b"\x7fELF" + bytes(252),
            bytes(0x13000) + self.raw,
            self.raw[:20] + bytes(4) + self.raw[24:],
            self.raw + bytes(0x45AF0),
            b"",
        ):
            with self.subTest(size=len(raw)), self.assertRaises(ValueError):
                Firmware.parse(raw)

    def test_packet_vectors_and_offset_byte_order(self):
        packets = list(data_reports(0x80, bytes(range(60))))
        self.assertEqual(
            packets[0], bytes.fromhex("80 38 00 00 00 00") + bytes(range(56)) + bytes(2)
        )
        self.assertEqual(
            packets[1], bytes.fromhex("80 04 38 00 00 00 38 39 3a 3b") + bytes(54)
        )
        packets = list(data_reports(0x82, bytes(0x10030)))
        self.assertEqual(packets[-1][:6], bytes.fromhex("82 08 28 00 01 00"))
        self.assertTrue(all(len(packet) == 64 for packet in packets))

    def test_full_sequence_programs_and_verifies_identical_image(self):
        peer = Peer()
        progress = []
        replies = upload(peer, self.firmware, lambda *args: progress.append(args))
        packets = [packet for packet, _ in peer.calls]
        self.assertEqual(packets[0], bytes.fromhex("81 07") + bytes(62))
        self.assertEqual(packets[-2], bytes.fromhex("83 02") + bytes(62))
        self.assertEqual(packets[-1], bytes.fromhex("84 01 01") + bytes(61))
        self.assertEqual(
            [packet[0] for packet in packets],
            [0x81] + [0x80] * 5 + [0x82] * 5 + [0x83, 0x84],
        )
        for programmed, verified in zip(packets[1:6], packets[6:11]):
            self.assertEqual(programmed[1:], verified[1:])
        self.assertEqual(peer.memory[:256], self.raw)
        self.assertEqual(peer.calls[0][1], 5000)
        self.assertTrue(all(timeout == 500 for _, timeout in peer.calls[1:]))
        self.assertEqual(progress[-1], ("verify", 256, 256))
        self.assertEqual(set(replies), {"erase", "end", "success"})

    def test_corrupted_program_never_finalizes(self):
        peer = Peer(corrupt=True)
        with self.assertRaisesRegex(UploadError, "verify at image offset 0x0"):
            upload(peer, self.firmware)
        self.assertEqual(peer.calls[-1][0][0], 0x82)
        self.assertFalse(any(packet[0] in (0x83, 0x84) for packet, _ in peer.calls))

    def test_timeout_at_every_phase_stops_without_retry_or_cleanup_command(self):
        for index in (0, 1, 5, 6, 10, 11, 12):
            with self.subTest(index=index):
                peer = Peer(failure=(index, TimeoutError("injected timeout")))
                with self.assertRaises(UploadError):
                    upload(peer, self.firmware)
                self.assertEqual(len(peer.calls), index + 1)
                if index < 11:
                    self.assertFalse(
                        any(packet[0] in (0x83, 0x84) for packet, _ in peer.calls)
                    )

    def test_invalid_program_or_verify_ack_stops(self):
        for index in (1, 6):
            for reply in (b"", b"\0", b"\x01\0", b"\0\x01", bytes(65)):
                with self.subTest(index=index, reply=reply):
                    peer = Peer(failure=(index, reply))
                    with self.assertRaises(UploadError):
                        upload(peer, self.firmware)
                    self.assertEqual(len(peer.calls), index + 1)

    def test_control_replies_have_no_invented_status_interpretation(self):
        for index in (0, 11, 12):
            with self.subTest(index=index):
                peer = Peer(failure=(index, b"\x99"))
                upload(peer, self.firmware)
                self.assertEqual(len(peer.calls), 13)

    def test_invalid_firmware_never_touches_transport(self):
        invalid = Firmware(b"\0" * 256, 256, "", 0)
        peer = Peer()
        with self.assertRaises(ValueError):
            upload(peer, invalid)
        self.assertEqual(peer.calls, [])

    def test_enter_loader_frame(self):
        self.assertEqual(ENTER_LOADER, b"\x55\xef" + bytes(62))
        self.assertEqual(sum(ENTER_LOADER[4:]) & 255, ENTER_LOADER[3])

    def test_hidapi_report_id_and_input_framing(self):
        hid = FakeHid(reads=[b"old", [], [], [0, 0]])
        transport = HidTransport(hid)
        transport.drain()
        self.assertEqual(
            transport.exchange(bytes.fromhex("81 07") + bytes(62), 5000), b"\0\0"
        )
        self.assertEqual(hid.writes, [bytes.fromhex("00 81 07") + bytes(62)])
        self.assertEqual(hid.read_calls[-1], (65, 5000))

    def test_pending_report_or_short_write_cannot_be_accepted_as_ack(self):
        hid = FakeHid(reads=[b"\0\0"])
        with self.assertRaisesRegex(UploadError, "pending"):
            HidTransport(hid).exchange(bytes(64), 500)
        self.assertEqual(hid.writes, [])
        hid = FakeHid(write_result=64)
        with self.assertRaisesRegex(UploadError, "short HID write"):
            HidTransport(hid).exchange(bytes(64), 500)
        self.assertEqual(len(hid.read_calls), 1)

    def test_transport_timeout_oversize_and_busy_input(self):
        for reply, error in [
            (b"", TimeoutError),
            (bytes(65), UploadError),
            (OSError("disconnect"), OSError),
        ]:
            with self.subTest(reply=reply):
                hid = FakeHid(reads=[[], reply])
                with self.assertRaises(error):
                    HidTransport(hid).exchange(bytes(64), 500)
                self.assertEqual(len(hid.writes), 1)
        hid = FakeHid(reads=[b"busy"] * 32)
        with self.assertRaises(UploadError):
            HidTransport(hid).drain()
        self.assertEqual(hid.writes, [])

    def test_device_selection_rejects_wrong_model_mode_and_ambiguous_path(self):
        loader = {"path": b"loader", "vendor_id": 0x19F5, "product_id": 0x0730}
        app = {
            "path": b"app",
            "vendor_id": 0x19F5,
            "product_id": 0x1030,
            "usage_page": 1,
            "usage": 0,
        }
        self.assertEqual(select_device([loader, app], "loader"), loader)
        self.assertEqual(select_device([loader, app], "app", application=True), app)
        self.assertEqual(
            select_device([loader, loader | {"usage": 2}], "loader"), loader
        )
        self.assertEqual(
            select_device([app | {"usage": 6}, app, app], "app", application=True), app
        )
        for devices, path, application in [
            ([loader, app], "app", False),
            ([loader, app], "loader", True),
            ([loader | {"product_id": 0x0731}], "loader", False),
            ([loader | {"vendor_id": 1}], "loader", False),
            ([loader, loader | {"product_id": 0x1030}], "loader", False),
            ([loader | {"path": None}], "loader", False),
            ([{"vendor_id": 0x19F5, "product_id": 0x0730}], "loader", False),
            ([app | {"usage": 6}], "app", True),
            ([], "loader", False),
        ]:
            with self.subTest(devices=devices), self.assertRaises(ValueError):
                select_device(devices, path, application=application)

    def test_loader_lookup_matches_serial_and_rejects_ambiguity(self):
        loader = {
            "path": b"loader",
            "vendor_id": 0x19F5,
            "product_id": 0x0730,
            "serial_number": "NuPhy Kick75",
        }
        app = {
            "path": b"app",
            "vendor_id": 0x19F5,
            "product_id": 0x1030,
            "serial_number": "NuPhy Keybord 0720",
        }
        self.assertIsNone(find_loader([app], "kick75"))
        self.assertIsNone(find_loader([app, loader], "keybord"))
        self.assertEqual(find_loader([app, loader], "kick75"), loader)
        self.assertEqual(find_loader([app, loader], None), loader)
        with self.assertRaises(ValueError):
            find_loader([loader, loader | {"path": b"other"}], "kick75")

    def test_wait_for_loader_polls_until_one_appears(self):
        loader = {
            "path": b"loader",
            "vendor_id": 0x19F5,
            "product_id": 0x0730,
            "serial_number": "NuPhy Kick75",
        }
        seen = [[], [], [loader]]
        with (
            patch("upload.matching_devices", side_effect=lambda: seen.pop(0)),
            patch("upload.time.sleep") as sleep,
            contextlib.redirect_stderr(io.StringIO()),
        ):
            self.assertEqual(wait_for_loader("kick75"), loader)
        self.assertEqual(sleep.call_count, 2)

    def test_wait_for_loader_gives_up_at_the_timeout(self):
        ticks = itertools.count(0, 0.5)
        with (
            patch("upload.matching_devices", return_value=[]),
            patch("upload.time.sleep"),
            patch("upload.time.monotonic", side_effect=lambda: next(ticks)),
            contextlib.redirect_stderr(io.StringIO()),
            self.assertRaises(UploadError),
        ):
            wait_for_loader("kick75", timeout=2.0)

    def test_cli_flash_can_wait_for_the_loader(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "test.bin"
            image.write_bytes(self.raw)
            device = FakeHid()
            hid = Mock()
            hid.enumerate.return_value = []
            hid.device.return_value = device
            with (
                patch.dict("sys.modules", {"hid": hid}),
                patch(
                    "upload.wait_for_loader",
                    return_value={
                        "path": b"loader",
                        "vendor_id": 0x19F5,
                        "product_id": 0x0730,
                    },
                ) as waited,
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(main(["flash", str(image), "--serial", "kick75"]), 1)
            waited.assert_called_once_with("kick75", None)
            device.open_path.assert_called_once_with(b"loader")

    def test_cli_invalid_image_does_not_access_hid(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "test.bin"
            invalid = bytearray(self.raw)
            invalid[0x14] ^= 1
            image.write_bytes(invalid)
            hid = Mock()
            errors = io.StringIO()
            with (
                patch.dict("sys.modules", {"hid": hid}),
                contextlib.redirect_stderr(errors),
            ):
                self.assertEqual(main(["flash", str(image), "--path", "loader"]), 1)
            hid.enumerate.assert_not_called()
            hid.device.assert_not_called()
            self.assertNotIn("Device state may have changed", errors.getvalue())

    def test_cli_closes_device_after_failed_flash(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "test.bin"
            image.write_bytes(self.raw)
            device = FakeHid()
            hid = Mock()
            hid.enumerate.return_value = [
                {"path": b"loader", "vendor_id": 0x19F5, "product_id": 0x0730}
            ]
            hid.device.return_value = device
            with (
                patch.dict("sys.modules", {"hid": hid}),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(
                    main(["flash", str(image), "--path", "loader"]),
                    1,
                )
            device.open_path.assert_called_once_with(b"loader")
            device.close.assert_called_once()
            self.assertEqual(len(device.writes), 1)

    def test_parsed_image_is_not_mutated_by_its_callers_buffer(self):
        raw = bytearray(self.raw)
        firmware = Firmware.parse(raw)
        raw[148] ^= 1
        self.assertEqual(firmware.data, self.raw)
        self.assertEqual(firmware.sha256, hashlib.sha256(self.raw).hexdigest())

    def test_full_size_reports_are_accepted_at_every_stage(self):
        for index in range(13):
            with self.subTest(index=index):
                peer = Peer(failure=(index, bytes(64)))
                upload(peer, self.firmware)
                self.assertEqual(len(peer.calls), 13)

    def test_mid_image_failure_retains_offset_and_control_reply(self):
        peer = Peer(failure=(8, b"\x01\0"))
        with self.assertRaises(UploadError) as failure:
            upload(peer, self.firmware)
        self.assertIn("verify at image offset 0x70", str(failure.exception))
        self.assertIn('"erase": "0000"', str(failure.exception))
        self.assertEqual(len(peer.calls), 9)

    def test_cli_enter_loader_sends_without_waiting_for_reply(self):
        device = FakeHid(reads=[[], OSError("application disconnected")])
        hid = Mock()
        app = {
            "path": b"app",
            "vendor_id": 0x19F5,
            "product_id": 0x1030,
            "usage_page": 1,
            "usage": 0,
        }
        hid.enumerate.return_value = [app | {"usage": 6}, app]
        hid.device.return_value = device
        with (
            patch.dict("sys.modules", {"hid": hid}),
            contextlib.redirect_stderr(io.StringIO()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(main(["enter-loader", "--path", "app"]), 0)
        device.open_path.assert_called_once_with(b"app")
        device.close.assert_called_once()
        self.assertEqual(device.writes, [b"\0\x55\xef" + bytes(62)])
        self.assertEqual(len(device.read_calls), 1)
