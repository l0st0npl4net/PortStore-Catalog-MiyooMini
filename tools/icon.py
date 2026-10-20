#!/usr/bin/env python3
"""Draw Port Store's icon (74x74 RGBA, as Onion's app icons): a box with an arrow
going into it. Standard library only.  python3 tools/icon.py assets/icon.png"""
import math
import struct
import sys
import zlib

N, S = 74, 4  # size, supersampling


def inside(x, y):
    """Colour of the point (in icon pixels), or None for transparent."""
    # Box: a rounded rectangle in the lower part, with a slot in its lid.
    bx0, by0, bx1, by1, r = 12, 34, 62, 66, 7
    cx, cy = min(max(x, bx0 + r), bx1 - r), min(max(y, by0 + r), by1 - r)
    in_box = bx0 <= x <= bx1 and by0 <= y <= by1 and math.hypot(x - cx, y - cy) <= r
    # Arrow: shaft and head, pointing down into the box.
    shaft = 31 <= x <= 43 and 6 <= y <= 30
    head = y >= 26 and y <= 46 and abs(x - 37) <= (46 - y) * 0.95
    if shaft or head:
        return (0xff, 0xff, 0xff)
    if in_box:
        # The arrow's outline keeps it apart from the box.
        near_head = y >= 22 and y <= 50 and abs(x - 37) <= (50 - y) * 0.95 + 2
        if near_head:
            return None
        return (0x4f, 0xa3, 0xff)
    return None


def main(out):
    rows = []
    for py in range(N):
        row = bytearray([0])
        for px in range(N):
            acc = [0, 0, 0, 0]
            for sy in range(S):
                for sx in range(S):
                    c = inside(px + (sx + 0.5) / S, py + (sy + 0.5) / S)
                    if c:
                        acc[0] += c[0]
                        acc[1] += c[1]
                        acc[2] += c[2]
                        acc[3] += 1
            n = acc[3]
            if n:
                row += bytes([acc[0] // n, acc[1] // n, acc[2] // n, 255 * n // (S * S)])
            else:
                row += bytes(4)
        rows.append(bytes(row))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
    open(out, "wb").write(png)


if __name__ == "__main__":
    main(sys.argv[1])
