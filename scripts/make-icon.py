#!/usr/bin/env python3
"""Make the icon of the program.

The icon is the same mark the program shows in its own window: a
lightning bolt on a rounded square that goes from blue to purple.  It
is drawn here instead of being shipped as a picture so that it is the
same on every machine and can be changed by changing this file.

The picture is written as a PNG by hand: no image library is needed,
and the file is a couple of kilobytes.

Usage: make-icon.py <output.png> [size]
"""

import math
import struct
import sys
import zlib

# The shape of the lightning bolt, in the 24 by 24 grid the interface
# uses, and the same shape the interface draws.
BOLT = [
    (13.0, 2.0),
    (3.0, 14.0),
    (12.0, 14.0),
    (11.0, 22.0),
    (21.0, 10.0),
    (12.0, 10.0),
]

# The two colours of the square.
BLUE = (10, 108, 255)
PURPLE = (111, 75, 255)

# How much of the picture the bolt takes, and how much of that the
# square takes.
BOLT_SCALE = 1024.0 / 24.0 * 0.62
BOLT_CENTER = (512.0, 512.0)


def inside_polygon(x, y, polygon):
    """Whether the point is inside the polygon."""
    inside = False
    count = len(polygon)
    for index in range(count):
        x1, y1 = polygon[index]
        x2, y2 = polygon[(index + 1) % count]
        if (y1 > y) != (y2 > y):
            cut = x1 + (y - y1) * (x2 - x1) / (y2 - y1)
            if x < cut:
                inside = not inside
    return inside


def rounded_square(x, y, size, radius):
    """How much of the square the point is in, from 0 to 1."""
    half = size / 2.0
    dx = abs(x - half) - (half - radius)
    dy = abs(y - half) - (half - radius)
    if dx <= 0 and dy <= 0:
        return 1.0
    distance = math.hypot(max(dx, 0.0), max(dy, 0.0))
    return max(0.0, min(1.0, radius - distance + 0.5))


def render(size):
    """The picture, as rows of bytes."""
    radius = size * 0.225
    samples = 3
    bolt = [
        (
            BOLT_CENTER[0] + (point[0] - 12.0) * BOLT_SCALE,
            BOLT_CENTER[1] + (point[1] - 12.0) * BOLT_SCALE,
        )
        for point in BOLT
    ]
    rows = []
    for row in range(size):
        line = bytearray()
        for column in range(size):
            square = 0.0
            mark = 0.0
            for step_y in range(samples):
                for step_x in range(samples):
                    x = column + (step_x + 0.5) / samples
                    y = row + (step_y + 0.5) / samples
                    square += rounded_square(x, y, size, radius)
                    if inside_polygon(x, y, bolt):
                        mark += 1.0
            square /= samples * samples
            mark /= samples * samples

            # The square goes from blue at the top to purple at the
            # bottom.
            mix = row / max(1.0, size - 1.0)
            colour = [
                int(BLUE[channel] + (PURPLE[channel] - BLUE[channel]) * mix)
                for channel in range(3)
            ]
            alpha = square
            for channel in range(3):
                value = int(colour[channel] * (1.0 - mark) + 255 * mark)
                line.append(int(value * alpha + 255 * (1.0 - alpha)))
            line.append(int(255 * alpha))
        rows.append(bytes(line))
    return rows


def write_png(path, rows, size):
    """Write the rows out as a PNG file."""

    def chunk(tag, payload):
        data = tag + payload
        return (
            struct.pack(">I", len(payload))
            + data
            + struct.pack(">I", zlib.crc32(data) & 0xFFFFFFFF)
        )

    raw = bytearray()
    for row in rows:
        raw.append(0)
        raw.extend(row)

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    with open(path, "wb") as handle:
        handle.write(b"\x89PNG\r\n\x1a\n")
        handle.write(chunk(b"IHDR", header))
        handle.write(chunk(b"IDAT", zlib.compress(bytes(raw), 9)))
        handle.write(chunk(b"IEND", b""))


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    path = sys.argv[1]
    size = int(sys.argv[2]) if len(sys.argv) > 2 else 1024
    rows = render(size)
    write_png(path, rows, size)
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
