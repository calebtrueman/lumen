#!/usr/bin/env python3
"""Test disks for Lumen on BIOS (SeaBIOS in QEMU).

  testdisk.py OUT.img [--size MB] [--no-lumen]

An MBR disk with one partition at 1 MiB whose "original" boot code just
prints ORIGINAL-MBR (so falling back to it is visible), with Lumen's
target/bios/lumen-bios.img installed into the gap the way the installers
do it: MBR code replaced (signature and partition table kept), Lumen's
area at the end of the gap, the original MBR saved in it.
"""
import struct, sys

def original_mbr():
    # mov si,msg; lodsb; or al,al; jz $; mov ah,0eh; int 10h; jmp loop
    code = bytes.fromhex("31c08ed8be1e7cac08c07406b40ecd10ebf5ebfe")
    code = code.ljust(0x1E, b"\x90") + b"ORIGINAL-MBR\0"
    sector = bytearray(512)
    sector[:len(code)] = code
    struct.pack_into("<I", sector, 0x1B8, 0x1234ABCD)
    # One Linux partition, active, from LBA 2048.
    sector[0x1BE:0x1CE] = bytes([0x80, 0, 0, 0, 0x83, 0, 0, 0]) + struct.pack("<II", 2048, 100000)
    sector[510:512] = b"\x55\xaa"
    return bytes(sector)

def install(disk, image):
    img = open(image, "rb").read()
    template, area = img[:512], bytearray(img[512:])
    total = len(area) // 512
    mbr = bytearray(disk[:512])
    first = min(struct.unpack_from("<I", mbr, 0x1BE + 16 * i + 8)[0] or 2**32 for i in range(4))
    base = first - total
    assert base >= 1, "gap too small"
    struct.pack_into("<Q", area, 16, base)
    area[512:1024] = mbr                       # original MBR
    new = bytearray(mbr)
    new[:440] = template[:440]
    struct.pack_into("<Q", new, 0x1B0, base)
    disk[:512] = new
    disk[base * 512:base * 512 + len(area)] = area
    return base

if __name__ == "__main__":
    out = sys.argv[1]
    size = int(sys.argv[sys.argv.index("--size") + 1]) if "--size" in sys.argv else 64
    disk = bytearray(size << 20)
    disk[:512] = original_mbr()
    if "--no-lumen" not in sys.argv:
        base = install(disk, "target/bios/lumen-bios.img")
        print(f"Lumen area at LBA {base}")
    open(out, "wb").write(disk)
