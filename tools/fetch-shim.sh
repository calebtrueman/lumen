#!/bin/sh
# Downloads Debian's Microsoft-signed shim and MokManager into vendor/shim/.
# shim is the small Microsoft-signed loader every major Linux distro uses to
# boot under Secure Boot; Lumen boots through it the same way GRUB does.
# Hashes are pinned so a tampered download is rejected.
set -eu
cd "$(dirname "$0")/.."
POOL=https://deb.debian.org/debian/pool/main/s
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

fetch() { # url sha256 file-in-package dest
    name=$(basename "$1")
    [ -f "$TMP/$name" ] || curl -fsSL -o "$TMP/$name" "$1"
    echo "$2  $TMP/$name" | shasum -a 256 -c - >/dev/null || { echo "hash mismatch for $name" >&2; exit 1; }
    mkdir -p "$TMP/x-$name"
    (cd "$TMP/x-$name" && ar x "../$name" && tar xf data.tar.*)
    mkdir -p "$(dirname "$4")"
    cp "$TMP/x-$name/$3" "$4"
    echo "  $4"
}

echo "Fetching shim 16.1 (Debian, Microsoft-signed):"
fetch $POOL/shim-signed/shim-signed_1.51+16.1-2_amd64.deb \
    993868c31cca3eab054a0c51c124a4a8b3904f49fb002feb4b09cb9415200e74 \
    usr/lib/shim/shimx64.efi.signed vendor/shim/x86_64/shimx64.efi
fetch "$POOL/shim-helpers-amd64-signed/shim-helpers-amd64-signed_1+16.1+2~deb13u1_amd64.deb" \
    7e950d75d5a40ecc0a99f727452ce35780b47a04d9a2cc3b1ff7c2bb316c033e \
    usr/lib/shim/mmx64.efi.signed vendor/shim/x86_64/mmx64.efi
fetch $POOL/shim-signed/shim-signed_1.51+16.1-2_arm64.deb \
    ad83dab5f01f550344e71dce28ef9d50d02251c96699828c1899c6d284f5cf54 \
    usr/lib/shim/shimaa64.efi.signed vendor/shim/aarch64/shimaa64.efi
fetch "$POOL/shim-helpers-arm64-signed/shim-helpers-arm64-signed_1+16.1+2~deb13u1_arm64.deb" \
    0563a90af1f9990a592a79f52845747daff51c1d345769adc07830d2547da325 \
    usr/lib/shim/mmaa64.efi.signed vendor/shim/aarch64/mmaa64.efi
