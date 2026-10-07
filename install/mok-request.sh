#!/bin/sh
# Queue a MOK enrollment without mokutil (same format mokutil writes):
#   MokNew  = one EFI_SIGNATURE_LIST per X.509 certificate given
#   MokAuth = SHA-256(MokNew || password as UTF-16LE)
#   mok-request.sh CERT.der [CERT.der...]
set -eu
[ $# -ge 1 ] || { echo "usage: mok-request.sh CERT.der..." >&2; exit 2; }
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
for CER in "$@"; do
    n=$(wc -c < "$CER")
    printf '\241\131\300\245\344\224\247\112\207\265\253\025\134\053\360\162'   # EFI_CERT_X509_GUID
    le32 $((28 + 16 + n)); le32 0; le32 $((16 + n))
    printf '\120\253\135\140\106\340\000\103\253\266\075\330\020\335\213\043'   # shim GUID (owner)
    cat "$CER"
done > "$T/new"
# Password as UTF-16LE: each ASCII character followed by a zero byte (the
# installers' codes are digits; iconv only for anything else). Only
# coreutils' sha256sum is needed: openssl isn't on every desktop install.
utf16le() {
    case $1 in
        *[!\ -~]*) printf '%s' "$1" | iconv -f UTF-8 -t UTF-16LE || { echo "can't encode the password (iconv missing)" >&2; exit 1; } ;;
        *) printf '%s' "$1" | od -An -v -tu1 | tr -s ' ' '\n' | grep . | while read -r c; do printf "\\$(printf %03o "$c")\\000"; done ;;
    esac
}
hex2bin() {
    printf "$(awk 'BEGIN { h = "0123456789abcdef" } { for (i = 1; i < length($0); i += 2) printf "\\%03o", (index(h, substr($0, i, 1)) - 1) * 16 + index(h, substr($0, i + 1, 1)) - 1 }')"
}
{ cat "$T/new"; utf16le "$pw"; } > "$T/hashed"
sha256sum "$T/hashed" | cut -c1-64 | hex2bin > "$T/auth"
[ "$(wc -c < "$T/auth")" -eq 32 ] || { echo "couldn't compute the request's hash" >&2; exit 1; }

# efivarfs needs each variable written in a single write() of attributes +
# data, so assemble it first and copy it in one go. An existing request is
# deleted first: efivarfs can't truncate a variable, so rewriting one in
# place fails (e.g. one queued earlier by the Windows installer).
put_var() { # name datafile
    f="$V/$1-$G"
    { printf '\007\000\000\000'; cat "$2"; } > "$T/$1.var"
    if [ -e "$f" ]; then
        chattr -i "$f" 2>/dev/null || true
        rm -f "$f" 2>/dev/null || true
    fi
    err=$(dd if="$T/$1.var" of="$f" bs="$(wc -c < "$T/$1.var")" count=1 conv=notrunc 2>&1) ||
        { echo "couldn't write $1 ($(echo "$err" | grep -v records | head -n1))" >&2; exit 1; }
}
put_var MokNew "$T/new"
put_var MokAuth "$T/auth"
echo "Enrollment request queued."
