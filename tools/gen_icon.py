#!/usr/bin/env python3
"""生成 Gosslan 占位图标（512x512 PNG）。正式打包前可替换为设计好的 logo 后运行 `tauri icon`。"""
import math
import struct
import zlib

W = H = 512
TOP = (76, 131, 255)      # #4c83ff
BOTTOM = (43, 95, 217)    # #2b5fd9
BLUE = (51, 112, 255)     # #3370ff
WHITE = (255, 255, 255)


def inside_rrect(x, y, cx, cy, hw, hh, r):
    qx = abs(x - cx) - (hw - r)
    qy = abs(y - cy) - (hh - r)
    d = min(max(qx, qy), 0.0) + math.hypot(max(qx, 0.0), max(qy, 0.0)) - r
    return d <= 0


def inside_circle(x, y, cx, cy, r):
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


rows = []
for y in range(H):
    t = y / (H - 1)
    r0 = int(TOP[0] + (BOTTOM[0] - TOP[0]) * t)
    g0 = int(TOP[1] + (BOTTOM[1] - TOP[1]) * t)
    b0 = int(TOP[2] + (BOTTOM[2] - TOP[2]) * t)
    row = bytearray([0])  # filter type 0
    for x in range(W):
        r, g, b = r0, g0, b0
        if inside_rrect(x, y, 256, 252, 156, 112, 44):
            r, g, b = WHITE
        for cx in (196, 256, 316):
            if inside_circle(x, y, cx, 252, 18):
                r, g, b = BLUE
        row.extend((r, g, b))
    rows.append(bytes(row))

raw = b"".join(rows)


def chunk(typ, data):
    c = struct.pack(">I", len(data)) + typ + data
    return c + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF)


ihdr = struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0)
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", ihdr)
    + chunk(b"IDAT", zlib.compress(raw, 9))
    + chunk(b"IEND", b"")
)

out = "src-tauri/icons/icon.png"
import os

os.makedirs(os.path.dirname(out), exist_ok=True)
with open(out, "wb") as f:
    f.write(png)
print(f"written {out} ({len(png)} bytes)")
