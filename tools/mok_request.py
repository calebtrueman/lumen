#!/usr/bin/env python3
"""Reference implementation of a MOK enrollment request (what `mokutil
--import` writes), used to test install-windows.ps1's equivalent logic.

  mok_request.py CERT.cer PASSWORD OUT.json   # virt-fw-vars --set-json format

MokNew  = EFI_SIGNATURE_LIST holding one X.509 certificate
MokAuth = SHA-256(MokNew || password as UTF-16LE)
Both live under shim's vendor GUID with NV|BS|RT attributes. On the next
boot shim sees MokNew and runs MokManager, which asks the user to confirm
and type the password.
"""
import hashlib, json, struct, sys, uuid

SHIM_GUID = "605dab50-e046-4300-abb6-3dd810dd8b23"
X509_GUID = "a5c059a1-94e4-4aa7-87b5-ab155c2bf072"


def mok_new(cert: bytes) -> bytes:
    sig_size = 16 + len(cert)
    header = uuid.UUID(X509_GUID).bytes_le + struct.pack("<III", 28 + sig_size, 0, sig_size)
    return header + uuid.UUID(SHIM_GUID).bytes_le + cert


def mok_auth(new: bytes, password: str) -> bytes:
    return hashlib.sha256(new + password.encode("utf-16-le")).digest()


if __name__ == "__main__":
    cert, password, out = open(sys.argv[1], "rb").read(), sys.argv[2], sys.argv[3]
    new = mok_new(cert)
    var = lambda name, data: {"name": name, "guid": SHIM_GUID, "attr": 7, "data": data.hex()}
    json.dump({"version": 2, "variables": [var("MokNew", new), var("MokAuth", mok_auth(new, password))]}, open(out, "w"), indent=2)
