#!/bin/sh
# Builds Lumen for BIOS PCs: target/bios/lumen-bios.img
#
# The image is what the installers write:
#   sector 0      MBR code template (the installer keeps the disk's own
#                 signature and partition table, and patches in where
#                 Lumen's area starts)
#   sector 1...   Lumen's area, written to the gap before the first
#                 partition: header, original-MBR slot, state sector,
#                 stage 2, then the zlib-compressed menu payload.
set -eu
cd "$(dirname "$0")/../.."
cargo bios ${LUMEN_FEATURES:+--features "$LUMEN_FEATURES"}
OUT=target/bios
mkdir -p $OUT
REL=target/i686-lumen-bios/release
python3 tools/bios/flatten.py $REL/stage2 $OUT/stage2.bin >/dev/null
python3 tools/bios/flatten.py $REL/lumen $OUT/payload.bin >/dev/null
python3 - "$OUT" <<'PY'
import struct, sys, zlib
out = sys.argv[1]
boot = open(f"{out}/stage2.bin", "rb").read()
mbr, stage2 = boot[:512], boot[512:]
assert len(mbr) <= 512 and mbr[0x1B0:0x1B8] == b"\0" * 8, "stage 1 too big"
mbr = mbr.ljust(512, b"\0")
stage2 = stage2.ljust(-(-len(stage2) // 512) * 512, b"\0")
s2_sectors = len(stage2) // 512
assert s2_sectors <= 127, f"stage 2 is {s2_sectors} sectors (max 127)"
payload = open(f"{out}/payload.bin", "rb").read()
packed = zlib.compress(payload, 9)
packed_padded = packed.ljust(-(-len(packed) // 512) * 512, b"\0")
p_sectors = len(packed_padded) // 512
total = 3 + s2_sectors + p_sectors
header = bytearray(512)
header[0:8] = b"LUMENBIO"
struct.pack_into("<I", header, 8, 1)
struct.pack_into("<IIIII", header, 24, s2_sectors, p_sectors, len(packed), len(payload), 0)
struct.pack_into("<I", header, 44, zlib.crc32(payload))
struct.pack_into("<I", header, 52, total)
area = bytes(header) + b"\0" * 512 * 2 + stage2 + packed_padded
open(f"{out}/lumen-bios.img", "wb").write(mbr + area)
print(f"{out}/lumen-bios.img: stage 2 {s2_sectors} sectors, payload {len(payload)} -> {len(packed)} bytes, area {total} sectors ({total * 512 // 1024} KiB)")
PY
