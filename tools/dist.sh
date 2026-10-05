#!/bin/sh
# Builds a signed, installable bundle: dist/<arch>/
#   tools/dist.sh [x86_64|aarch64]
# Uses keys/lumen.key (override with LUMEN_KEYS=dir); creates one if missing.
set -eu
cd "$(dirname "$0")/.."
ARCH=${1:-x86_64}
case $ARCH in x86_64) S=x64 ;; aarch64) S=aa64 ;; *) echo "arch: x86_64 or aarch64" >&2; exit 1 ;; esac
KEYS=${LUMEN_KEYS:-keys}
OUT=dist/$ARCH

[ -f "$KEYS/lumen.key" ] || tools/genkey.sh "$KEYS"
[ -f "vendor/shim/$ARCH/shim$S.efi" ] || tools/fetch-shim.sh
cargo build --release --target "$ARCH-unknown-uefi"

rm -rf "$OUT" && mkdir -p "$OUT"
osslsigncode sign -h sha256 -certs "$KEYS/lumen.crt" -key "$KEYS/lumen.key" \
    -in "target/$ARCH-unknown-uefi/release/lumen.efi" -out "$OUT/lumen.efi" >/dev/null
cp "vendor/shim/$ARCH/shim$S.efi" "vendor/shim/$ARCH/mm$S.efi" "$KEYS/lumen.cer" "$OUT/"
cp install/install-linux.sh install/lumen-heal.sh install/mok-request.sh install/lumen-windows.ps1 install/lumen.conf "$OUT/"
echo "Bundle ready in $OUT:"
ls -1 "$OUT" | sed 's/^/  /'
