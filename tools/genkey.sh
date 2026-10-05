#!/bin/sh
# Creates the key Lumen releases are signed with. Users enroll the public
# half (lumen.cer) once as a Machine Owner Key; shim then trusts Lumen.
# KEEP lumen.key PRIVATE: anything signed with it boots on enrolled machines.
set -eu
DIR=${1:-keys}
if [ -e "$DIR/lumen.key" ]; then
    echo "$DIR/lumen.key already exists; refusing to replace it (PCs that approved it would stop trusting new builds)." >&2
    exit 1
fi
mkdir -p "$DIR"
umask 077
openssl req -new -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -subj "/CN=Lumen Boot Manager Signing Key/" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=codeSigning" \
    -keyout "$DIR/lumen.key" -out "$DIR/lumen.crt" 2>/dev/null
openssl x509 -in "$DIR/lumen.crt" -outform DER -out "$DIR/lumen.cer"
chmod 644 "$DIR/lumen.crt" "$DIR/lumen.cer"
echo "Created $DIR/lumen.key (private) and $DIR/lumen.cer (enroll this)."
