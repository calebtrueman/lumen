#!/usr/bin/env python3
"""ELF32 -> flat image of its loadable segments (by load address).

  flatten.py IN.elf OUT.bin [--base ADDR]

Gaps between segments are zero-filled; NOLOAD (.bss) isn't emitted.
"""
import struct, sys

def main():
    src, dst = sys.argv[1], sys.argv[2]
    data = open(src, "rb").read()
    assert data[:4] == b"\x7fELF" and data[4] == 1, "not ELF32"
    phoff, = struct.unpack_from("<I", data, 0x1C)
    phentsize, phnum = struct.unpack_from("<HH", data, 0x2A)
    segs = []
    for i in range(phnum):
        p_type, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, _, _ = struct.unpack_from("<8I", data, phoff + i * phentsize)
        if p_type == 1 and p_filesz:
            segs.append((p_paddr, data[p_offset:p_offset + p_filesz]))
    base = min(a for a, _ in segs)
    if "--base" in sys.argv:
        base = int(sys.argv[sys.argv.index("--base") + 1], 0)
    end = max(a + len(d) for a, d in segs)
    out = bytearray(end - base)
    for a, d in segs:
        out[a - base:a - base + len(d)] = d
    open(dst, "wb").write(out)
    print(f"{dst}: {len(out)} bytes from 0x{base:x}")

main()
