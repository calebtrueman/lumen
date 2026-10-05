#!/usr/bin/env python3
"""Print PE section table of an EFI binary (to check .sbat, alignment, W^X)."""
import struct, sys
d = open(sys.argv[1], "rb").read()
pe = struct.unpack_from("<I", d, 0x3C)[0]
nsec = struct.unpack_from("<H", d, pe + 6)[0]
optsz = struct.unpack_from("<H", d, pe + 20)[0]
dllchar = struct.unpack_from("<H", d, pe + 24 + 70)[0]
print(f"DllCharacteristics=0x{dllchar:04x} (NX_COMPAT={'yes' if dllchar & 0x100 else 'no'})")
off = pe + 24 + optsz
for i in range(nsec):
    name, vsize, vaddr, rawsz, rawptr = struct.unpack_from("<8sIIII", d, off)
    ch = struct.unpack_from("<I", d, off + 36)[0]
    flags = ("R" if ch & 0x40000000 else "-") + ("W" if ch & 0x80000000 else "-") + ("X" if ch & 0x20000000 else "-")
    print(f"{name.rstrip(bytes(1)).decode():8} vaddr=0x{vaddr:06x} vsize=0x{vsize:06x} {flags}")
    if name.startswith(b".sbat"):
        print("   ", d[rawptr:rawptr + vsize].decode(errors="replace").strip().replace("\n", "\n    "))
    off += 40
