#!/usr/bin/env python3
"""Compares install/lumen-windows.ps1's byte output with reference encoders."""
import json, os, struct, subprocess, sys, uuid
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "tools"))
import mok_request

pwsh = sys.argv[1] if len(sys.argv) > 1 else "pwsh"
out = os.path.join(ROOT, "target", "ps-test.json")
os.makedirs(os.path.dirname(out), exist_ok=True)
subprocess.check_call([pwsh, "-NoProfile", "-File", os.path.join(ROOT, "tools", "test", "windows-installer.ps1"), "-Out", out])
got = {k: bytes.fromhex(v) for k, v in json.load(open(out)).items()}

hd = struct.pack("<BBHIQQ", 4, 1, 42, 1, 1048576 // 512, 104857600 // 512) + uuid.UUID("0f2c4e5a-1b3d-4c6e-8f90-a1b2c3d4e5f6").bytes_le + bytes([2, 2])
name = "\\EFI\\lumen\\shimx64.efi\0".encode("utf-16-le")
path = hd + struct.pack("<BBH", 4, 4, 4 + len(name)) + name + bytes([0x7F, 0xFF, 4, 0])
want_opt = struct.pack("<IH", 1, len(path)) + "Lumen\0".encode("utf-16-le") + path
cert = open(os.path.join(ROOT, "keys", "lumen.cer"), "rb").read()
new = mok_request.mok_new(cert)

ok = True
for k, want in [("option", want_opt), ("mok_new", new), ("mok_auth", mok_request.mok_auth(new, "hunter2"))]:
    match = got[k] == want
    ok &= match
    print(f"{k:9} {'OK' if match else 'MISMATCH'}")
sys.exit(0 if ok else 1)
