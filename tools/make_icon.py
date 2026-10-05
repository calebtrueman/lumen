#!/usr/bin/env python3
"""Builds assets/lumen.ico (and lumen.png) from the gallery tool's renders.

Each size is rendered on black and on white; alpha = 1 - (white - black),
colour = black / alpha. Sizes are stored as PNGs inside the .ico."""
import os, struct, subprocess, sys, tempfile, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def read_ppm(p):
    d = open(p, "rb").read().split(b"\n", 3)
    w, h = map(int, d[1].split())
    return w, h, d[3]


def png_rgba(w, h, rgba):
    raw = b"".join(b"\x00" + rgba[y * w * 4:(y + 1) * w * 4] for y in range(h))
    chunk = lambda t, b: struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


tmp = tempfile.mkdtemp()
subprocess.check_call(["cargo", "run", "--release", "-q", "--manifest-path", os.path.join(ROOT, "tools/gallery/Cargo.toml"),
                       "--target", os.environ.get("HOST_TARGET", "aarch64-apple-darwin"), "--", "--icon", tmp])
images = []
for size in [16, 24, 32, 48, 64, 128, 256]:
    w, h, black = read_ppm(f"{tmp}/icon-{size}-black.ppm")
    _, _, white = read_ppm(f"{tmp}/icon-{size}-white.ppm")
    out = bytearray()
    for i in range(0, len(black), 3):
        a = 255 - max(0, min(255, (white[i] - black[i] + white[i + 1] - black[i + 1] + white[i + 2] - black[i + 2]) // 3))
        rgb = [min(255, black[i + c] * 255 // a) if a else 0 for c in range(3)]
        out += bytes(rgb + [a])
    images.append((size, png_rgba(w, h, bytes(out))))

ico = struct.pack("<HHH", 0, 1, len(images))
offset = 6 + 16 * len(images)
for size, data in images:
    ico += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
ico += b"".join(d for _, d in images)
open(os.path.join(ROOT, "assets", "lumen.ico"), "wb").write(ico)
open(os.path.join(ROOT, "assets", "lumen.png"), "wb").write(images[-1][1])
print("wrote assets/lumen.ico and assets/lumen.png")
