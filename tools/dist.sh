#!/bin/sh
# Builds a signed, installable bundle: dist/<arch>/
#   tools/dist.sh [x86_64|aarch64]
#
# Signs with the release key in keys/lumen.key and checks it matches the
# committed release/lumen.cer: PCs that approved Lumen trust only that key.
# For throwaway test builds (CI), set LUMEN_TEST_KEY=1: a separate test key
# in keys-test/ is used (created if needed) and the check is skipped.
set -eu
cd "$(dirname "$0")/.."
ARCH=${1:-x86_64}
case $ARCH in x86_64) S=x64 ;; aarch64) S=aa64 ;; *) echo "arch: x86_64 or aarch64" >&2; exit 1 ;; esac
OUT=dist/$ARCH

if [ "${LUMEN_TEST_KEY:-}" = 1 ]; then
    KEYS=keys-test
    [ -f "$KEYS/lumen.key" ] || tools/genkey.sh "$KEYS"
    echo "WARNING: test build signed with a throwaway key; don't install it on a real PC." >&2
else
    KEYS=keys
    if [ ! -f "$KEYS/lumen.key" ]; then
        echo "The release key keys/lumen.key is missing. Restore it from your backup." >&2
        echo "(Generating a new one would make PCs that approved Lumen stop trusting new builds.)" >&2
        exit 1
    fi
    # Everything is checked against the committed release certificate.
    release_der=$(openssl x509 -inform DER -in release/lumen.cer -outform DER | od -An -tx1 | tr -d ' \n')
    for f in "$KEYS/lumen.cer:DER" "$KEYS/lumen.crt:PEM"; do
        der=$(openssl x509 -inform "${f##*:}" -in "${f%:*}" -outform DER 2>/dev/null | od -An -tx1 | tr -d ' \n')
        if [ "$der" != "$release_der" ]; then
            echo "${f%:*} isn't the release certificate (release/lumen.cer); refusing to sign." >&2
            exit 1
        fi
    done
    key_pub=$(openssl pkey -in "$KEYS/lumen.key" -pubout 2>/dev/null)
    cert_pub=$(openssl x509 -inform DER -in release/lumen.cer -noout -pubkey)
    if [ "$key_pub" != "$cert_pub" ]; then
        echo "keys/lumen.key doesn't belong to the release certificate; refusing to sign." >&2
        exit 1
    fi
fi
[ -f "vendor/shim/$ARCH/shim$S.efi" ] || tools/fetch-shim.sh
cargo build --release --target "$ARCH-unknown-uefi"

rm -rf "$OUT" && mkdir -p "$OUT"
osslsigncode sign -h sha256 -certs "$KEYS/lumen.crt" -key "$KEYS/lumen.key" \
    -in "target/$ARCH-unknown-uefi/release/lumen.efi" -out "$OUT/lumen.efi" >/dev/null
cp "vendor/shim/$ARCH/shim$S.efi" "vendor/shim/$ARCH/mm$S.efi" "$KEYS/lumen.cer" "$OUT/"
cp install/install-linux.sh install/lumen-heal.sh install/mok-request.sh install/lumen-windows.ps1 install/lumen.conf "$OUT/"
echo "Bundle ready in $OUT:"
ls -1 "$OUT" | sed 's/^/  /'
