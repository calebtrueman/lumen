#!/usr/bin/env python3
"""Which function holds an address in an ELF32 (for crash reports).

  addr2sym.py ELF ADDR...
"""
import struct, sys

def symbols(path):
    d = open(path, "rb").read()
    shoff, = struct.unpack_from("<I", d, 0x20)
    shentsize, shnum, _ = struct.unpack_from("<HHH", d, 0x2E)
    secs = [struct.unpack_from("<10I", d, shoff + i * shentsize) for i in range(shnum)]
    out = []
    for s in secs:
        if s[1] == 2:  # SYMTAB
            strtab = secs[s[6]]
            for o in range(s[4], s[4] + s[5], 16):
                name, value, size, info, _, _ = struct.unpack_from("<IIIBBH", d, o)
                if info & 0xF == 2:  # FUNC
                    nm = d[strtab[4] + name:d.index(b"\0", strtab[4] + name)].decode(errors="replace")
                    out.append((value, size, nm))
    return out

syms = symbols(sys.argv[1])
for a in sys.argv[2:]:
    addr = int(a, 16)
    hit = [s for s in syms if s[0] <= addr < s[0] + max(s[1], 1)]
    print(a, "->", hit[0][2] + f"+{addr - hit[0][0]:#x}" if hit else "?")
