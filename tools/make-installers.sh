#!/bin/sh
# Builds the one-click installers:
#   dist/Lumen-Installer-Windows.exe   (x64 + ARM64 Windows; needs mingw-w64)
#   dist/Lumen-Installer-Linux.run     (x86_64 + aarch64 Linux)
#   dist/SHA256SUMS
set -eu
cd "$(dirname "$0")/.."

# Ask for the signing key's passphrase once for both architectures.
# In a terminal it's typed at a hidden prompt; without one (e.g. run from an
# editor or agent) macOS shows a password dialog instead.
if [ "${LUMEN_TEST_KEY:-}" != 1 ] && grep -q "ENCRYPTED PRIVATE KEY" keys/lumen.key 2>/dev/null && [ -z "${LUMEN_KEY_PASS:-}" ]; then
    if [ -t 0 ]; then
        printf 'Passphrase for the Lumen signing key: ' >&2
        stty -echo 2>/dev/null || true; read -r LUMEN_KEY_PASS; stty echo 2>/dev/null || true; echo >&2
    elif command -v osascript >/dev/null; then
        LUMEN_KEY_PASS=$(osascript -e 'text returned of (display dialog "Passphrase for the Lumen release signing key:" with title "Sign Lumen release" default answer "" with hidden answer buttons {"Cancel", "Sign"} default button "Sign" with icon caution)' 2>/dev/null) ||
            { echo "Signing cancelled." >&2; exit 1; }
    else
        echo "Set LUMEN_KEY_PASS or run this in a terminal to enter the signing passphrase." >&2
        exit 1
    fi
    # Check it now rather than after a long build.
    if ! LUMEN_KEY_PASS=$LUMEN_KEY_PASS openssl pkey -in keys/lumen.key -passin env:LUMEN_KEY_PASS -noout 2>/dev/null; then
        echo "That passphrase doesn't unlock keys/lumen.key." >&2
        exit 1
    fi
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
