#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi==0.15.0"]
# ///
"""Experimental Node75 LP (ANSI) HID uploader."""

import argparse
import hashlib
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

from check_image import MAX_IMAGE_SIZE, check_image

VID = 0x19F5
APP_PID = 0x1030
LOADER_PID = 0x0730
REPORT_SIZE = 64
CHUNK_SIZE = 56
READ_TIMEOUT_MS = 500
ERASE_TIMEOUT_MS = 5000
ERASE = bytes.fromhex("81 07 00 00 00 00 00") + bytes(57)
END = bytes.fromhex("83 02 00 00") + bytes(60)
SUCCESS = bytes.fromhex("84 01 01 00") + bytes(60)
# EF ignores its arguments; the checksum of the zero-filled body is zero.
ENTER_LOADER = bytes.fromhex("55 ef") + bytes(62)


@dataclass(frozen=True)
class Firmware:
    data: bytes
    original_size: int
    sha256: str
    reset: int

    @classmethod
    def parse(cls, raw):
        if len(raw) > MAX_IMAGE_SIZE:
            raise ValueError("image exceeds the known stock extent")
        raw = bytes(raw)
        data = raw + bytes(-len(raw) % 4)
        reset = check_image(data)
        return cls(data, len(raw), hashlib.sha256(raw).hexdigest(), reset)


def data_reports(command, data):
    if command not in (0x80, 0x82) or not data or len(data) % 4:
        raise ValueError("expected program/verify command and word-aligned image")
    if len(data) > MAX_IMAGE_SIZE:
        raise ValueError("image exceeds the known stock extent")
    for offset in range(0, len(data), CHUNK_SIZE):
        payload = data[offset : offset + CHUNK_SIZE]
        yield (
            bytes([command, len(payload)]) + offset.to_bytes(4, "little") + payload
        ).ljust(REPORT_SIZE, b"\0")


class UploadError(RuntimeError):
    pass


def upload(transport, firmware, progress=None):
    check_image(firmware.data)
    phase = "erase"
    replies = {}
    try:
        # NuPhyIO only checks transport completion for these three commands.
        reply = transport.exchange(ERASE, ERASE_TIMEOUT_MS)
        if not 1 <= len(reply) <= REPORT_SIZE:
            raise UploadError("missing or oversized erase reply")
        replies[phase] = reply.hex()
        for command, name in [(0x80, "program"), (0x82, "verify")]:
            for packet in data_reports(command, firmware.data):
                offset = int.from_bytes(packet[2:6], "little")
                phase = f"{name} at image offset {offset:#x}"
                reply = transport.exchange(packet, READ_TIMEOUT_MS)
                if not 2 <= len(reply) <= REPORT_SIZE or reply[:2] != b"\0\0":
                    raise UploadError(
                        f"expected 00 00 acknowledgment, got {reply.hex()}"
                    )
                if progress:
                    progress(name, offset + packet[1], len(firmware.data))
        for phase, packet in [("end", END), ("success", SUCCESS)]:
            reply = transport.exchange(packet, READ_TIMEOUT_MS)
            if not 1 <= len(reply) <= REPORT_SIZE:
                raise UploadError("missing or oversized finalization reply")
            replies[phase] = reply.hex()
    except (OSError, UploadError) as error:
        detail = ""
        if phase in ("end", "success"):
            detail = " Verification completed, but finalization is unconfirmed."
        if replies:
            detail += f" Control replies received: {json.dumps(replies)}."
        raise UploadError(
            f"{phase}: {error}.{detail} Stopped without retry or cleanup commands."
        ) from error
    return replies


class HidTransport:
    def __init__(self, device):
        self.device = device
        self.write_attempted = False

    def drain(self):
        for _ in range(32):
            if not self.device.read(REPORT_SIZE + 1, 1):
                return
        raise UploadError(
            "device has continuous pending reports; close other HID clients"
        )

    def send(self, report):
        if len(report) != REPORT_SIZE:
            raise ValueError("expected a 64-byte unnumbered HID report")
        # hidapi requires a leading report ID even for unnumbered reports.
        framed = b"\0" + report
        self.write_attempted = True
        written = self.device.write(framed)
        if written != len(framed):
            raise UploadError(f"short HID write: {written} of {len(framed)} bytes")

    def exchange(self, report, timeout_ms):
        # Replies have no offset/sequence tag; do not credit a stale ACK.
        if self.device.read(REPORT_SIZE + 1, 1):
            raise UploadError("unexpected pending report before command")
        self.send(report)
        reply = bytes(self.device.read(REPORT_SIZE + 1, timeout_ms))
        if not reply:
            raise TimeoutError(f"no reply within {timeout_ms} ms")
        if len(reply) > REPORT_SIZE:
            raise UploadError("unexpected numbered or oversized HID input report")
        return reply


def select_device(devices, path, *, application=False):
    pid = APP_PID if application else LOADER_PID
    matches = [
        device
        for device in devices
        if isinstance(device.get("path"), (bytes, str))
        and os.fsdecode(device["path"]) == path
    ]
    identities = {
        (device.get("vendor_id"), device.get("product_id")) for device in matches
    }
    if identities != {(VID, pid)}:
        raise ValueError(
            f"path must identify a currently connected {VID:04x}:{pid:04x} device"
        )
    # hidapi can enumerate multiple collections with the same underlying path.
    if application:
        matches = [
            device
            for device in matches
            if (device.get("usage_page"), device.get("usage")) == (1, 0)
        ]
        if not matches:
            raise ValueError(
                "updater entry requires the application's usage-page 1, usage 0 interface"
            )
    return matches[0]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="list matching application/loader HID interfaces")
    enter = commands.add_parser(
        "enter-loader", help="ask stock firmware to reboot into its loader"
    )
    enter.add_argument(
        "--path", required=True, help="application interface path from list"
    )
    flash = commands.add_parser(
        "flash", help="erase/program/verify an already-connected loader"
    )
    flash.add_argument("image", type=Path)
    flash.add_argument("--path", required=True, help="exact loader path from list")
    args = parser.parse_args(argv)
    transport = None

    try:
        firmware = None
        if args.command == "flash":
            # Validate before touching any device.
            with args.image.open("rb") as source:
                firmware = Firmware.parse(source.read(MAX_IMAGE_SIZE + 1))

        try:
            import hid
        except ImportError as error:
            raise UploadError(
                "hidapi is required for device access; use uv run tools/upload.py"
            ) from error
        devices = [
            device
            for device in hid.enumerate(VID, 0)
            if device.get("vendor_id") == VID
            and device.get("product_id") in (APP_PID, LOADER_PID)
            and isinstance(device.get("path"), (bytes, str))
        ]
        if args.command == "list":
            if not devices:
                print(
                    "No matching Node75 LP application or loader HID interfaces found.",
                    file=sys.stderr,
                )
            print(
                json.dumps(
                    [
                        {
                            "path": os.fsdecode(device["path"]),
                            "mode": "loader"
                            if device["product_id"] == LOADER_PID
                            else "application",
                            "interface_number": device.get("interface_number"),
                            "usage_page": device.get("usage_page"),
                            "usage": device.get("usage"),
                            "serial": device.get("serial_number"),
                        }
                        for device in devices
                    ],
                    indent=2,
                )
            )
            return 0

        info = select_device(
            devices, args.path, application=args.command == "enter-loader"
        )
        device = hid.device()
        try:
            device.open_path(os.fsencode(info["path"]))
            transport = HidTransport(device)
            transport.drain()
            if args.command == "enter-loader":
                transport.send(ENTER_LOADER)
                print("Request sent. Run list again and select the new loader path.")
            else:
                print(
                    f"Flashing {firmware.original_size} bytes, SHA-256 {firmware.sha256}",
                    file=sys.stderr,
                )
                last = None

                def progress(phase, done, total):
                    nonlocal last
                    status = (phase, done * 100 // total)
                    if status != last:
                        print(
                            f"\r{phase}: {status[1]:3d}%",
                            end="",
                            file=sys.stderr,
                            flush=True,
                        )
                        last = status

                replies = upload(transport, firmware, progress)
                print(file=sys.stderr)
                print(
                    json.dumps(
                        {"verified": True, "finalization_replies": replies}, indent=2
                    )
                )
                print("Boot and application USB enumeration have not been checked.")
        finally:
            device.close()
        return 0
    except (OSError, ValueError, UploadError) as error:
        print(f"\nerror: {error}", file=sys.stderr)
        if transport is not None and transport.write_attempted:
            print(
                "Device state may have changed. Inspect it before retrying; no automatic recovery is attempted.",
                file=sys.stderr,
            )
        return 1
    except KeyboardInterrupt:
        message = "\nInterrupted."
        if transport is not None and transport.write_attempted:
            message += " Device state may have changed; no retry/cleanup was sent."
        print(message, file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
