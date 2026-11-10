"""Renders the Skrivli feather and writes resources/skrivli-icon.svg and
resources/skrivli.ico (16, 20, 24, 32, 40, 48, 64 as BMP, 256 as PNG).
Pure standard library. Run: python resources/export_icon.py
"""
import math
import struct
import zlib
from pathlib import Path

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
CANVAS = 256
# Vane: quadratic curves base -> tip (side A) and tip -> base (side B).
BASE, TIP = (52, 204), (230, 26)
CTRL_A, CTRL_B = (50, 74), (182, 206)
STOPS = [(0.0, (20, 184, 166)), (0.5, (123, 92, 240)), (1.0, (255, 107, 91))]
INK = (30, 33, 64)
SHAFT_TOP, SHAFT_TIP, SHAFT_HALF = (214, 42), (18, 238), 7
HI = 1.0


def quad(p0, c, p1, n=24):
    return [
        ((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * c[0] + t * t * p1[0],
         (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * c[1] + t * t * p1[1])
        for t in (i / n for i in range(n + 1))
    ]


def vane_polygon():
    return quad(BASE, CTRL_A, TIP) + quad(TIP, CTRL_B, BASE)[1:]


def shaft_polygon():
    nx, ny = 1 / math.sqrt(2), 1 / math.sqrt(2)
    t, h = SHAFT_TOP, SHAFT_HALF
    mid = (60, 196)
    return [
        (t[0] + nx * h, t[1] + ny * h), (t[0] - nx * h, t[1] - ny * h),
        (mid[0] - nx * h, mid[1] - ny * h), SHAFT_TIP,
        (mid[0] + nx * h, mid[1] + ny * h),
    ]


def inside(poly, x, y):
    hit = False
    j = len(poly) - 1
    for i, (xi, yi) in enumerate(poly):
        xj, yj = poly[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            hit = not hit
        j = i
    return hit


def gradient(x, y):
    ax, ay = TIP[0] - BASE[0], TIP[1] - BASE[1]
    t = ((x - BASE[0]) * ax + (y - BASE[1]) * ay) / (ax * ax + ay * ay)
    t = min(1.0, max(0.0, t))
    for (t0, c0), (t1, c1) in zip(STOPS, STOPS[1:]):
        if t <= t1:
            f = (t - t0) / (t1 - t0)
            return tuple(c0[k] + (c1[k] - c0[k]) * f for k in range(3))
    return STOPS[-1][1]


def render(size):
    vane, shaft = vane_polygon(), shaft_polygon()
    ss = 8 if size < 128 else 3
    scale = CANVAS / size
    rows = []
    for py in range(size):
        row = []
        for px in range(size):
            r = g = b = a = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) / ss) * scale
                    y = (py + (sy + 0.5) / ss) * scale
                    if inside(shaft, x, y):
                        c = INK
                    elif inside(vane, x, y):
                        c = gradient(x, y)
                    else:
                        continue
                    r += c[0]; g += c[1]; b += c[2]; a += 1
            n = ss * ss
            if a == 0:
                row.append((0, 0, 0, 0))
            else:
                row.append((round(r / a), round(g / a), round(b / a), round(255 * a / n)))
        rows.append(row)
    return rows


def png(rows):
    size = len(rows)
    raw = b"".join(b"\x00" + bytes(v for p in row for v in p) for row in rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def bmp(rows):
    size = len(rows)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(bytes(v for r, g, b, a in row for v in (b, g, r, a))
                      for row in reversed(rows))
    mask_row = ((size + 31) // 32) * 4
    return header + pixels + bytes(mask_row * size)


def svg():
    def pts(poly):
        return " ".join(f"{x:.1f},{y:.1f}" for x, y in poly)

    stops = "".join(
        f'<stop offset="{o}" stop-color="#{c[0]:02X}{c[1]:02X}{c[2]:02X}"/>' for o, c in STOPS)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CANVAS} {CANVAS}" '
        f'width="{CANVAS}" height="{CANVAS}">\n'
        f'  <defs><linearGradient id="vane" gradientUnits="userSpaceOnUse" '
        f'x1="{BASE[0]}" y1="{BASE[1]}" x2="{TIP[0]}" y2="{TIP[1]}">{stops}</linearGradient></defs>\n'
        f'  <path fill="url(#vane)" d="M{BASE[0]} {BASE[1]} Q{CTRL_A[0]} {CTRL_A[1]} {TIP[0]} {TIP[1]} '
        f'Q{CTRL_B[0]} {CTRL_B[1]} {BASE[0]} {BASE[1]}Z"/>\n'
        f'  <polygon fill="#{INK[0]:02X}{INK[1]:02X}{INK[2]:02X}" points="{pts(shaft_polygon())}"/>\n'
        '</svg>\n')


def main():
    out = Path(__file__).resolve().parent
    (out / "skrivli-icon.svg").write_text(svg(), encoding="utf-8")
    images = [(s, png(render(s)) if s == 256 else bmp(render(s))) for s in SIZES]
    data = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    body = b""
    for s, img in images:
        d = 0 if s == 256 else s
        data += struct.pack("<BBBBHHII", d, d, 0, 0, 1, 32, len(img), offset + len(body))
        body += img
    (out / "skrivli.ico").write_bytes(data + body)


if __name__ == "__main__":
    main()
