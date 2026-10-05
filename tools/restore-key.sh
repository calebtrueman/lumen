#!/bin/sh
# Restores keys/ from a backup of the release key.
#
#   tools/restore-key.sh FILE
#
# FILE may be the key file itself, or the texts scanned from the paper
# backup's QR codes pasted together in any order (each starts with
# "LUMEN-KEY n/3"). The key must match release/lumen.cer.
set -eu
cd "$(dirname "$0")/.."
SRC=${1:?usage: tools/restore-key.sh FILE}
[ -e keys/lumen.key ] && { echo "keys/lumen.key already exists; move it away first." >&2; exit 1; }
umask 077
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if grep -q '^LUMEN-KEY [0-9]/[0-9]' "$SRC"; then
    # Reassemble the QR parts in order.
    awk '/^LUMEN-KEY [0-9]+\/[0-9]+/ { split($2, p, "/"); part = p[1]; next } { print > ("'"$TMP"'/part" part) }' "$SRC"
    total=$(sed -n 's/^LUMEN-KEY [0-9]*\/\([0-9]*\).*/\1/p' "$SRC" | head -n1)
    i=1
    while [ "$i" -le "$total" ]; do
        [ -s "$TMP/part$i" ] || { echo "QR part $i of $total is missing; scan all $total codes." >&2; exit 1; }
        i=$((i + 1))
    done
    cat "$TMP"/part* | sed '/^$/d' > "$TMP/lumen.key"
else
    sed '/^$/d' "$SRC" > "$TMP/lumen.key"
fi
grep -q "BEGIN ENCRYPTED PRIVATE KEY" "$TMP/lumen.key" && grep -q "END ENCRYPTED PRIVATE KEY" "$TMP/lumen.key" ||
    { echo "That doesn't look like a complete Lumen key backup (are all parts there?)." >&2; exit 1; }

printf 'Passphrase: '; stty -echo 2>/dev/null || true; read -r LUMEN_KEY_PASS; stty echo 2>/dev/null || true; echo
export LUMEN_KEY_PASS
key_pub=$(openssl pkey -in "$TMP/lumen.key" -passin env:LUMEN_KEY_PASS -pubout 2>/dev/null) ||
    { echo "Wrong passphrase, or the key text has a typo." >&2; exit 1; }
cert_pub=$(openssl x509 -inform DER -in release/lumen.cer -noout -pubkey)
[ "$key_pub" = "$cert_pub" ] || { echo "This key doesn't match release/lumen.cer." >&2; exit 1; }

mkdir -p keys
cp "$TMP/lumen.key" keys/lumen.key
cp release/lumen.cer keys/lumen.cer
openssl x509 -inform DER -in release/lumen.cer -out keys/lumen.crt
chmod 600 keys/lumen.key
echo "Restored keys/ — release builds can be signed again."
