#!/bin/sh
# Builds the one-click installers:
#   dist/Lumen-Installer-Windows.exe   (x64 + ARM64 Windows; needs mingw-w64)
#   dist/Lumen-Installer-Linux.run     (x86_64 + aarch64 Linux)
#   dist/SHA256SUMS
set -eu
cd "$(dirname "$0")/.."

# Ask for the signing key's passphrase once for both architectures.
if [ "${LUMEN_TEST_KEY:-}" != 1 ] && grep -q "ENCRYPTED PRIVATE KEY" keys/lumen.key 2>/dev/null && [ -z "${LUMEN_KEY_PASS:-}" ]; then
    printf 'Passphrase for the Lumen signing key: ' >&2
    stty -echo 2>/dev/null || true; read -r LUMEN_KEY_PASS; stty echo 2>/dev/null || true; echo >&2
    export LUMEN_KEY_PASS
fi
tools/dist.sh x86_64
tools/dist.sh aarch64
[ -f assets/lumen.ico ] || python3 tools/make_icon.py

echo "Building Windows installer…"
(cd installer/windows && cargo build -q --release --target x86_64-pc-windows-gnu)
cp installer/windows/target/x86_64-pc-windows-gnu/release/lumen-installer.exe dist/Lumen-Installer-Windows.exe

echo "Building Linux installer…"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp -R dist/x86_64 dist/aarch64 "$TMP/"
chmod +x "$TMP"/*/*.sh
tar czf "$TMP/payload.tgz" -C "$TMP" x86_64 aarch64
lines=$(wc -l < install/linux-gui.sh)
sed "s/__PAYLOAD_LINE__/$((lines + 1))/" install/linux-gui.sh > dist/Lumen-Installer-Linux.run
cat "$TMP/payload.tgz" >> dist/Lumen-Installer-Linux.run
chmod +x dist/Lumen-Installer-Linux.run

(cd dist && shasum -a 256 Lumen-Installer-Windows.exe Lumen-Installer-Linux.run > SHA256SUMS)
ls -lh dist/Lumen-Installer-* | awk '{ print "  " $5 "  " $9 }'
