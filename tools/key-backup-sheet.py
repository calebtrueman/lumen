#!/usr/bin/env python3
"""Printable paper backup of the Lumen release signing key.

  python3 tools/key-backup-sheet.py      -> keys/lumen-key-backup.html (open, print, file away)

The sheet holds the passphrase-protected private key (it refuses an
unprotected one) as QR codes plus the same text, a checksum, and restore
steps. The passphrase is NOT on the sheet: keep it in your password
manager. The certificate isn't needed on paper; it's public in
release/lumen.cer. Restore with tools/restore-key.sh.
"""
import base64, hashlib, html, io, os, subprocess, sys, textwrap
from datetime import date

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEY = os.path.join(ROOT, "keys", "lumen.key")
OUT = os.path.join(ROOT, "keys", "lumen-key-backup.html")
PARTS = 3

try:
    import qrcode
    import qrcode.image.svg
except ImportError:
    sys.exit("Needs the qrcode module: pip3 install --user qrcode")

pem = open(KEY).read().strip() + "\n"
if "ENCRYPTED PRIVATE KEY" not in pem:
    sys.exit("keys/lumen.key isn't passphrase-protected; create the release key with tools/genkey.sh --rotate first.")
fp = subprocess.check_output(["openssl", "x509", "-inform", "DER", "-in", os.path.join(ROOT, "release", "lumen.cer"),
                              "-noout", "-fingerprint", "-sha256"], text=True).strip().split("=", 1)[1]
digest = hashlib.sha256(pem.encode()).hexdigest()

# Split the PEM on whole lines into parts small enough to scan reliably
# from paper; tools/restore-key.sh reassembles them line by line.
pem_lines = pem.splitlines(keepends=True)
per = -(-len(pem_lines) // PARTS)
chunks = ["".join(pem_lines[i * per:(i + 1) * per]) for i in range(PARTS)]
qrs = []
for i, chunk in enumerate(chunks, 1):
    payload = f"LUMEN-KEY {i}/{PARTS}\n{chunk}"
    img = qrcode.make(payload, image_factory=qrcode.image.svg.SvgPathImage, error_correction=qrcode.constants.ERROR_CORRECT_M, box_size=4, border=2)
    buf = io.BytesIO()
    img.save(buf)
    qrs.append(buf.getvalue().decode())

# Human-readable copy, with a short checksum per line to catch typos.
lines = pem.strip().splitlines()
rows = "\n".join(
    f"<tr><td>{n:02d}</td><td>{html.escape(l)}</td><td>{hashlib.sha256(l.encode()).hexdigest()[:4]}</td></tr>"
    for n, l in enumerate(lines, 1))

page = f"""<!doctype html><meta charset="utf-8"><title>Lumen signing key backup</title>
<style>
  @page {{ size: auto; margin: 14mm; }}
  body {{ font: 11pt/1.45 -apple-system, "Segoe UI", sans-serif; color: #000; max-width: 190mm; margin: 0 auto; }}
  h1 {{ font-size: 18pt; margin: 0 0 4pt; }}
  .warn {{ border: 2px solid #000; padding: 8pt 10pt; margin: 10pt 0; }}
  .qrs {{ display: flex; gap: 8mm; justify-content: space-between; margin: 10pt 0; }}
  .qr {{ text-align: center; font-size: 9pt; }}
  .qr svg {{ width: 56mm; height: 56mm; }}
  table {{ border-collapse: collapse; font: 7.6pt/1.35 "SF Mono", Menlo, Consolas, monospace; width: 100%; }}
  td {{ padding: 0 4pt; white-space: pre; }} td:first-child, td:last-child {{ color: #555; }}
  code {{ font: 9pt "SF Mono", Menlo, Consolas, monospace; word-break: break-all; }}
  ol {{ padding-left: 16pt; }} .page2 {{ break-before: page; }}
</style>
<h1>Lumen release signing key — backup</h1>
<div>Printed {date.today().isoformat()} · certificate SHA-256 <code>{fp}</code></div>
<div class="warn"><b>Keep this sheet private.</b> It's the private key that signs Lumen for every PC that approved it.
It is protected by a passphrase that is <b>not</b> on this sheet (it's in the password manager). Without the passphrase this sheet is useless; without this sheet (or another backup) new Lumen releases can't be signed with this key.</div>
<div class="qrs">{''.join(f'<div class="qr">{svg}<div>Part {i} of {PARTS}</div></div>' for i, svg in enumerate(qrs, 1))}</div>
<h2 style="font-size:12pt">To restore</h2>
<ol>
  <li>Get the Lumen repository (it contains the public certificate, <code>release/lumen.cer</code>).</li>
  <li>Scan the three QR codes with a phone and paste the texts, in any order, into a file. Or type the key text from page 2 (each line has a 4-character check value).</li>
  <li>Run <code>tools/restore-key.sh that-file</code> and enter the passphrase. It checks the key matches the certificate and recreates <code>keys/</code>.</li>
</ol>
<div>Whole-key checksum (SHA-256 of the key text): <code>{digest}</code></div>
<div class="page2">
<h2 style="font-size:12pt">Key text (passphrase-protected, PKCS#8)</h2>
<table>{rows}</table>
</div>
"""
os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, "w") as f:
    f.write(page)
os.chmod(OUT, 0o600)
print(f"Wrote {OUT}\nOpen it in a browser and print it (2 pages), then delete the file if you like:\n  open '{OUT}'")
