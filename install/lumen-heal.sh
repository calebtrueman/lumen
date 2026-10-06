#!/bin/sh
# lumen-heal: keeps Lumen first in the firmware boot order.
#
# Windows updates, `grub-install` (run by distro GRUB updates) and firmware
# updates can reorder or delete boot entries. This runs at every boot and
# shutdown; it recreates Lumen's entry if it vanished and moves it back to
# the front if something jumped ahead. Other entries are never removed.
#
# Lumen is only ever promoted after it has started successfully on this PC
# (it then sets the LumenHealthy firmware variable), so a Lumen that can't
# run here never becomes the default.
set -eu
# Legacy BIOS PCs: put Lumen back in the MBR if something (grub-install on
# a GRUB update, say) wrote its own boot code there.
if [ ! -d /sys/firmware/efi ]; then
    LIB=/usr/local/lib/lumen
    [ -x "$LIB/lumen-bios-install" ] && [ -f "$LIB/disk" ] || exit 0
    "$LIB/lumen-bios-install" heal "$(cat "$LIB/disk")" "$LIB/lumen-bios.img" >/dev/null 2>&1 || true
    exit 0
fi
[ -d /sys/firmware/efi/efivars ] || exit 0
command -v efibootmgr >/dev/null || exit 0

case $(uname -m) in
    x86_64) S=x64 ;;
    aarch64) S=aa64 ;;
    *) exit 0 ;;
esac

find_esp() {
    if command -v bootctl >/dev/null && bootctl --print-esp-path 2>/dev/null; then return; fi
    for d in /boot/efi /efi /boot; do
        if [ "$(findmnt -n -o FSTYPE "$d" 2>/dev/null)" = vfat ] && [ -d "$d/EFI" ]; then echo "$d"; return; fi
    done
    return 1
}
ESP=$(find_esp) || exit 0
[ -f "$ESP/EFI/lumen/shim$S.efi" ] || exit 0

healthy() { [ -e /sys/firmware/efi/efivars/LumenHealthy-4c756d65-6e00-4b6f-9f2a-6c756d656e21 ]; }
sb_on() { od -An -t u1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | awk 'NF { v = $NF } END { exit !(v == 1) }'; }
hexof() { od -An -tx1 -v "$1" | tr -d ' \n'; }
# Under Secure Boot, shim only starts Lumen if its key was approved. A queued
# approval counts: its screen appears on the next boot, which must go ahead.
can_start() {
    sb_on || return 0
    cer="$ESP/EFI/lumen/lumen.cer"
    [ -f "$cer" ] || return 0
    want=$(hexof "$cer")
    for v in MokListRT MokNew; do
        f=/sys/firmware/efi/efivars/$v-605dab50-e046-4300-abb6-3dd810dd8b23
        [ -e "$f" ] && hexof "$f" | grep -q "$want" && return 0
    done
    return 1
}

num=$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\} Lumen\([[:space:]].*\)\{0,1\}$/\1/p' | head -n1)
if [ -z "$num" ]; then
    src=$(findmnt -n -o SOURCE "$ESP")
    disk=/dev/$(lsblk -no PKNAME "$src" | head -n1)
    part=$(cat "/sys/class/block/$(basename "$src")/partition")
    old=$(efibootmgr | sed -n 's/^BootOrder: //p')
    # --create puts the new entry first in BootOrder.
    efibootmgr --quiet --create --disk "$disk" --part "$part" --label Lumen --loader "\\EFI\\lumen\\shim$S.efi"
    echo "lumen-heal: recreated the Lumen boot entry"
    if { ! healthy || ! can_start; } && [ -n "$old" ]; then
        num=$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\} Lumen\([[:space:]].*\)\{0,1\}$/\1/p' | head -n1)
        efibootmgr --quiet --bootorder "$old,$num"
    fi
    exit 0
fi
if ! can_start; then
    # Secure Boot was turned on without approving Lumen's key: don't let every
    # boot stop at shim's "Verification failed" screen.
    order=$(efibootmgr | sed -n 's/^BootOrder: //p')
    if [ "${order%%,*}" = "$num" ]; then
        rest=$(echo "$order" | tr ',' '\n' | grep -vix "$num" | paste -sd, -)
        efibootmgr --quiet --bootorder "${rest:+$rest,}$num"
        echo "lumen-heal: Secure Boot is on but Lumen isn't approved; the PC starts its next entry until the installer is run again"
    fi
    [ "$(efibootmgr | sed -n 's/^BootNext: //p')" = "$num" ] && efibootmgr --quiet --delete-bootnext
    exit 0
fi
healthy || exit 0

order=$(efibootmgr | sed -n 's/^BootOrder: //p')
first=${order%%,*}
if [ "$first" != "$num" ]; then
    rest=$(echo "$order" | tr ',' '\n' | grep -vix "$num" | paste -sd, -)
    efibootmgr --quiet --bootorder "$num${rest:+,$rest}"
    echo "lumen-heal: moved Lumen (Boot$num) back to first"
fi
