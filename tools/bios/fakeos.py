#!/usr/bin/env python3
"""Fake boot code for BIOS tests: prints a word on screen and on the debug
console (port 0xE9), so tests can see which boot sector ran.

  fakeos.py windows-disk OUT.img    MBR with Windows-style boot code that
                                    prints WINDOWS-MBR, active NTFS partition
                                    whose boot sector prints WINDOWS-VBR
"""
import struct, sys

def code(msg, at):
    """Boot code placed at offset `at` of a sector loaded at 0x7C00."""
    m = 0x7C00 + at + 23
    c = bytes([0x31, 0xC0, 0x8E, 0xD8, 0xBE, m & 0xFF, m >> 8, 0xAC, 0x08, 0xC0, 0x74, 0x08,
               0xE6, 0xE9, 0xB4, 0x0E, 0xCD, 0x10, 0xEB, 0xF3, 0xF4, 0xEB, 0xFD])
    return c + msg.encode() + b"\r\n\0"

def windows_disk(path, size_mb=64):
    disk = bytearray(size_mb << 20)
    mbr = bytearray(512)
    c = code("WINDOWS-MBR", 0)
    mbr[:len(c)] = c
    # The strings Lumen uses to recognise Windows' MBR code.
    s = b"Invalid partition table\0Error loading operating system\0Missing operating system\0"
    mbr[0x100:0x100 + len(s)] = s
    struct.pack_into("<I", mbr, 0x1B8, 0x5EED5EED)
    mbr[0x1BE:0x1CE] = bytes([0x80, 0, 0, 0, 0x07, 0, 0, 0]) + struct.pack("<II", 2048, 100000)
    mbr[510:512] = b"\x55\xaa"
    disk[:512] = mbr
    vbr = bytearray(512)
    vbr[0:3] = b"\xEB\x3C\x90"
    vbr[3:11] = b"NTFS    "
    c = code("WINDOWS-VBR", 0x3E)
    vbr[0x3E:0x3E + len(c)] = c
    vbr[0x180:0x180 + 19] = b"BOOTMGR is missing\0"
    vbr[510:512] = b"\x55\xaa"
    disk[2048 * 512:2049 * 512] = vbr
    open(path, "wb").write(disk)

if __name__ == "__main__":
    {"windows-disk": windows_disk}[sys.argv[1]](sys.argv[2])
