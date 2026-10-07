# Sourced by install-linux.sh. The signing certificates of the Linux
# distributions installed on this PC, read from their own shims on the EFI
# system partition: shim keeps its distro's CA in a ".vendor_cert" section
# (one DER certificate, or an EFI signature list of them). Approving these
# alongside Lumen's key lets Lumen start those distros' kernels directly
# under Secure Boot, verified by shim exactly as their own shim would.
# Plain sh + od + dd, so it works on every distro.

_u16() { od -An -tu2 -j "$2" -N2 "$1" | tr -d ' '; }
_u32() { od -An -tu4 -j "$2" -N4 "$1" | tr -d ' '; }
_hex() { dd if="$1" bs=1 skip="$2" count="$3" 2>/dev/null | od -An -tx1 -v | tr -d ' \n'; }

# vendor_cert SHIM OUT: writes the section's "authorized" blob to OUT.
vendor_cert() {
    [ "$(_hex "$1" 0 2)" = 4d5a ] || return 1
    pe=$(_u32 "$1" 60)
    [ "$(_hex "$1" "$pe" 4)" = 50450000 ] || return 1
    n=$(_u16 "$1" $((pe + 6))); opt=$(_u16 "$1" $((pe + 20)))
    strtab=$(( $(_u32 "$1" $((pe + 12))) + $(_u32 "$1" $((pe + 16))) * 18 ))
    st=$((pe + 24 + opt)); i=0
    while [ "$i" -lt "$n" ] && [ "$i" -lt 64 ]; do
        o=$((st + i * 40))
        name=$(dd if="$1" bs=1 skip="$o" count=8 2>/dev/null | tr -d '\000')
        case $name in /[0-9]*) name=$(dd if="$1" bs=1 skip=$((strtab + ${name#/})) count=16 2>/dev/null | tr '\000' '\n' | head -n1) ;; esac
        if [ "$name" = .vendor_cert ]; then
            rp=$(_u32 "$1" $((o + 20)))
            size=$(_u32 "$1" "$rp"); off=$(_u32 "$1" $((rp + 8)))
            [ "$size" -gt 0 ] && [ "$size" -lt 65536 ] || return 1
            dd if="$1" of="$2" bs=1 skip=$((rp + off)) count="$size" 2>/dev/null
            return 0
        fi
        i=$((i + 1))
    done
    return 1
}

# split_certs BLOB PREFIX: DER certificate(s) from a blob -> PREFIX.N.der
split_certs() {
    if [ "$(_hex "$1" 0 1)" = 30 ]; then cp "$1" "$2.0.der"; return; fi
    total=$(wc -c < "$1"); at=0; k=0
    while [ $((at + 28)) -le "$total" ] && [ "$k" -lt 32 ]; do
        type=$(_hex "$1" "$at" 16); lsize=$(_u32 "$1" $((at + 16)))
        hsize=$(_u32 "$1" $((at + 20))); ssize=$(_u32 "$1" $((at + 24)))
        [ "$lsize" -ge 28 ] && [ "$ssize" -gt 16 ] || return
        if [ "$type" = a159c0a5e494a74a87b5ab155c2bf072 ]; then   # EFI_CERT_X509_GUID
            s=$((at + 28 + hsize))
            while [ $((s + ssize)) -le $((at + lsize)) ]; do
                dd if="$1" of="$2.$k.der" bs=1 skip=$((s + 16)) count=$((ssize - 16)) 2>/dev/null
                k=$((k + 1)); s=$((s + ssize))
            done
        fi
        at=$((at + lsize))
    done
}

# Who a certificate belongs to, from the organisation names inside it.
key_name() {
    for pair in "Canonical:Ubuntu" "Fedora:Fedora" "openSUSE:openSUSE" "SUSE:SUSE" "AlmaLinux:AlmaLinux" \
        "Rocky:Rocky Linux" "CentOS:CentOS" "Red Hat:Red Hat" "Oracle:Oracle Linux" "Debian:Debian"; do
        if grep -aq "${pair%%:*}" "$1"; then echo "${pair#*:}"; return; fi
    done
    case $2 in BOOT|boot) echo Linux ;; *) echo "$2" ;; esac
}

# distro_keys ESP OUTDIR OWNBLOB: one .der per distro CA on that EFI
# partition, except OWNBLOB (the key built into Lumen's shim, Debian's).
# Prints the distros they belong to ("Ubuntu, Fedora").
distro_keys() {
    own_blob=$3
    seen=""
    for shim in "$1"/EFI/*/shim*.efi "$1"/EFI/*/SHIM*.EFI "$1"/EFI/BOOT/BOOT*.EFI "$1"/EFI/boot/boot*.efi; do
        [ -f "$shim" ] || continue
        dir=$(basename "$(dirname "$shim")")
        case $(echo "$dir" | tr 'A-Z' 'a-z') in lumen|microsoft) continue ;; esac
        vendor_cert "$shim" "$2/blob" 2>/dev/null || continue
        cmp -s "$2/blob" "$own_blob" && continue
        split_certs "$2/blob" "$2/$dir"
        name=$(key_name "$2/blob" "$dir")
        case " $seen " in *" $name "*) ;; *) seen="${seen:+$seen, }$name" ;; esac
    done
    rm -f "$2/blob"
    # Drop duplicates (the same distro's shim in two folders).
    for a in "$2"/*.der; do
        [ -f "$a" ] || continue
        for b in "$2"/*.der; do
            [ "$a" != "$b" ] && [ -f "$b" ] && cmp -s "$a" "$b" && rm -f "$b"
        done
    done
    echo $seen
}
