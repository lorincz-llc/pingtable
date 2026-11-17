#!/usr/bin/env python3
"""Draw the pingtable icon and write crates/app/assets/pingtable.ico.

The mark is a rectangle, so it is drawn directly at each size instead of
scaling a picture. Small sizes are stored as bitmaps and 256 px as PNG, which
is what Windows and the resource compiler both accept.

Run from the repository root:  python3 scripts/make-icon.py
"""
import struct
import zlib
from pathlib import Path

CLEAR = (0, 0, 0, 0)
PETROL = (0x15, 0x96, 0xA6, 255)  # a mid Petrol that reads on light and dark taskbars

# (x, y, width, height, colour) on a 64 x 64 grid. Keep in step with ui/icon.svg.
# The mark is candidate B, "Square", from brand/icons/preview.html: one status square.
SHAPES = [
    (12, 12, 40, 40, PETROL),
]


def draw(size):
    """Rows of RGBA pixels, top row first, on a transparent ground."""
    scale = size / 64
    rows = [[CLEAR] * size for _ in range(size)]
    for x, y, w, h, colour in SHAPES:
        x0, y0 = round(x * scale), round(y * scale)
        x1, y1 = max(x0 + 1, round((x + w) * scale)), max(y0 + 1, round((y + h) * scale))
        for row in rows[y0:y1]:
            row[x0:x1] = [colour] * (x1 - x0)
    return rows


def png(rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    size = len(rows)
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def bitmap(rows):
    """32-bit icon bitmap: header, BGRA rows bottom-up, then the transparency mask."""
    size = len(rows)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(bytes((b, g, r, a)) for row in reversed(rows) for (r, g, b, a) in row)
    stride = ((size + 31) // 32) * 4
    mask = b""
    for row in reversed(rows):
        bits = bytearray(stride)
        for x, (_, _, _, alpha) in enumerate(row):
            if alpha == 0:
                bits[x // 8] |= 0x80 >> (x % 8)
        mask += bytes(bits)
    return header + pixels + mask


def main():
    sizes = [16, 20, 24, 32, 40, 48, 64, 256]
    images = [png(draw(s)) if s == 256 else bitmap(draw(s)) for s in sizes]
    out = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    for size, data in zip(sizes, images):
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    out += b"".join(images)
    target = Path(__file__).resolve().parent.parent / "crates" / "app" / "assets" / "pingtable.ico"
    target.write_bytes(out)
    print(f"wrote {target} ({len(out)} bytes, sizes {sizes})")


if __name__ == "__main__":
    main()
