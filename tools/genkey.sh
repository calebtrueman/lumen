#!/bin/sh
# Lumen release signing key.
#
#   tools/genkey.sh --rotate         replace this project's release key (old one is
#                                    kept in keys-retired-DATE/); every PC must
#                                    approve the new key once
#   tools/genkey.sh --new-project    first key for a new project or fork
#   tools/genkey.sh --test DIR       throwaway unencrypted key for CI and VM tests
#
# The release key is RSA-4096 and passphrase-protected from the start. Its
# public certificate goes to release/lumen.cer (committed); the private key
# stays in keys/lumen.key (git-ignored). PCs that approved Lumen under Secure
# Boot trust only that key, so it must outlive any one computer: back it up
# with tools/key-backup-sheet.py right after creating it.
set -eu
cd "$(dirname "$0")/.."

make_cert() { # keydir passin-arg cn
    openssl req -new -x509 -key "$1/lumen.key" $2 -sha256 -days 10950 \
        -subj "/CN=$3/O=Lumen/" \
        -addext "basicConstraints=critical,CA:FALSE" \
        -addext "keyUsage=critical,digitalSignature" \
        -addext "extendedKeyUsage=codeSigning" \
        -out "$1/lumen.crt"
    openssl x509 -in "$1/lumen.crt" -outform DER -out "$1/lumen.cer"
    chmod 644 "$1/lumen.crt" "$1/lumen.cer"
}

case ${1:-} in
    --test)
        DIR=${2:-keys-test}
        [ -e "$DIR/lumen.key" ] && exit 0
        mkdir -p "$DIR"
        (umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$DIR/lumen.key" 2>/dev/null)
        make_cert "$DIR" "" "Lumen TEST key - not for release"
        echo "Created throwaway test key in $DIR/."
        exit 0
        ;;
    --rotate)
        if [ -e keys/lumen.key ]; then
            echo "This replaces Lumen's release key. Every PC that approved the current key"
            echo "will need to approve the new one once, after installing a build signed with it."
            printf 'Type "rotate" to continue: '
            read -r answer
            [ "$answer" = rotate ] || { echo "Cancelled."; exit 1; }
            retired=keys-retired-$(date +%Y%m%d-%H%M%S)
            mv keys "$retired"
            echo "Old key moved to $retired/ (keep it until no PC still relies on it)."
        fi
        ;;
    --new-project)
        if [ -e keys/lumen.key ]; then echo "keys/lumen.key already exists." >&2; exit 1; fi
        ;;
    *)
        if [ -e keys/lumen.key ]; then
            echo "keys/lumen.key already exists. To replace it deliberately: tools/genkey.sh --rotate" >&2
        elif [ -e release/lumen.cer ]; then
            echo "This project already has a release key (release/lumen.cer), but keys/lumen.key is missing." >&2
            echo "Restore it from your backup with tools/restore-key.sh." >&2
            echo "If this is a fork that should sign with its own key: tools/genkey.sh --new-project" >&2
        else
            echo "usage: tools/genkey.sh --new-project | --rotate | --test DIR" >&2
        fi
        exit 1
        ;;
esac

if [ -z "${LUMEN_KEY_PASS:-}" ]; then
    echo "Choose a passphrase for the release key. Use a long one and save it in your"
    echo "password manager: without it, the key and its backups can't be used."
    while :; do
        printf 'Passphrase: '; stty -echo 2>/dev/null || true; read -r p1; stty echo 2>/dev/null || true; echo
        printf 'Again: '; stty -echo 2>/dev/null || true; read -r p2; stty echo 2>/dev/null || true; echo
        if [ "$p1" != "$p2" ]; then echo "They don't match; try again."
        elif [ ${#p1} -lt 12 ]; then echo "Use at least 12 characters."
        else break; fi
    done
    LUMEN_KEY_PASS=$p1
fi
export LUMEN_KEY_PASS

mkdir -p keys
(umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:4096 -aes-256-cbc -pass env:LUMEN_KEY_PASS -out keys/lumen.key)
make_cert keys "-passin env:LUMEN_KEY_PASS" "Lumen Release Signing Key"
cp keys/lumen.cer release/lumen.cer
fp=$(openssl x509 -in keys/lumen.crt -noout -fingerprint -sha256 | cut -d= -f2)
if [ -f release/README.md ]; then
    sed -i.bak "s/^\`[0-9A-F:]\{95\}\`$/\`$fp\`/" release/README.md && rm -f release/README.md.bak
fi
echo
echo "Created keys/lumen.key (RSA-4096, passphrase-protected) and release/lumen.cer."
echo "SHA-256 fingerprint: $fp"
echo
echo "Next:"
echo "  1. Print the backup sheet:   python3 tools/key-backup-sheet.py"
echo "  2. Commit release/lumen.cer (public) so builds can check against it."
