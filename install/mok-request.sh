#!/bin/sh
# Queue a MOK enrollment without mokutil (same format mokutil writes):
#   MokNew  = EFI_SIGNATURE_LIST with one X.509 certificate
#   MokAuth = SHA-256(MokNew || password as UTF-16LE)
set -eu
CER=$1
G=605dab50-e046-4300-abb6-3dd810dd8b23
V=${EFIVARS:-/sys/firmware/efi/efivars}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

if [ -n "${LUMEN_MOK_PASSWORD:-}" ]; then
    pw=$LUMEN_MOK_PASSWORD; pw2=$pw
else
    if [ -t 0 ]; then stty -echo; fi
    printf 'Password: '; read -r pw; printf '\nAgain: '; read -r pw2
    if [ -t 0 ]; then stty echo; fi; echo
fi
[ -n "$pw" ] && [ "$pw" = "$pw2" ] || { echo "passwords don't match" >&2; exit 1; }

le32() { printf "\\$(printf %03o $(($1 & 255)))\\$(printf %03o $(($1 >> 8 & 255)))\\$(printf %03o $(($1 >> 16 & 255)))\\$(printf %03o $(($1 >> 24 & 255)))"; }
n=$(wc -c < "$CER")
{
    printf '\241\131\300\245\344\224\247\112\207\265\253\025\134\053\360\162'   # EFI_CERT_X509_GUID
    le32 $((28 + 16 + n)); le32 0; le32 $((16 + n))
    printf '\120\253\135\140\106\340\000\103\253\266\075\330\020\335\213\043'   # shim GUID (owner)
    cat "$CER"
} > "$T/new"
{ cat "$T/new"; printf '%s' "$pw" | iconv -f UTF-8 -t UTF-16LE; } | openssl dgst -sha256 -binary > "$T/auth"

for v in MokNew MokAuth; do chattr -i "$V/$v-$G" 2>/dev/null || true; done
{ printf '\007\000\000\000'; cat "$T/new"; } > "$V/MokNew-$G"
{ printf '\007\000\000\000'; cat "$T/auth"; } > "$V/MokAuth-$G"
echo "Enrollment request queued."
