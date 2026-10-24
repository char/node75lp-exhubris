#!/usr/bin/env python3
"""Structural checks only; this does not establish vendor-loader acceptance."""

import argparse
import struct
from pathlib import Path

APP_BASE = 0x13000
MAX_IMAGE_SIZE = 0x45AF0  # Known stock extent, not a discovered partition limit.


def check_image(data: bytes) -> int:
    if len(data) < 148 or len(data) > MAX_IMAGE_SIZE or len(data) % 4:
        raise ValueError("image must be word aligned and fit the known stock extent")
    instruction = struct.unpack_from("<I", data)[0]
    if instruction & 0xFFF != 0x06F:
        raise ValueError("image must start with an uncompressed JAL x0")
    displacement = (
        ((instruction >> 31) << 20)
        | (((instruction >> 12) & 0xFF) << 12)
        | (((instruction >> 20) & 1) << 11)
        | (((instruction >> 21) & 0x3FF) << 1)
    )
    if displacement & (1 << 20):
        displacement -= 1 << 21
    if not 148 <= displacement < len(data):
        raise ValueError("reset jump must land after the vectors, inside the image")
    if struct.unpack_from("<I", data, 0x14)[0] != 0xF5F9BDA9:
        raise ValueError("missing stock image marker at file offset 0x14")
    return APP_BASE + displacement


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    try:
        data = args.image.read_bytes()
        reset = check_image(data)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print(f"{len(data)} bytes; base {APP_BASE:#x}; reset {reset:#x}")
