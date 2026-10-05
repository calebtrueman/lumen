#!/bin/sh
# lumen-heal: keeps Lumen first in the firmware boot order.
#
# Windows updates, `grub-install` (run by distro GRUB updates) and firmware
# updates can reorder or delete boot entries. This runs at every boot and
# shutdown; it recreates Lumen's entry if it vanished and moves it back to
# the front if something jumped ahead. Other entries are never removed.
set -eu
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

num=$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\} Lumen\([[:space:]].*\)\{0,1\}$/\1/p' | head -n1)
if [ -z "$num" ]; then
    src=$(findmnt -n -o SOURCE "$ESP")
    disk=/dev/$(lsblk -no PKNAME "$src" | head -n1)
    part=$(cat "/sys/class/block/$(basename "$src")/partition")
    # --create puts the new entry first in BootOrder.
    efibootmgr --quiet --create --disk "$disk" --part "$part" --label Lumen --loader "\\EFI\\lumen\\shim$S.efi"
    echo "lumen-heal: recreated the Lumen boot entry"
    exit 0
fi

order=$(efibootmgr | sed -n 's/^BootOrder: //p')
first=${order%%,*}
if [ "$first" != "$num" ]; then
    rest=$(echo "$order" | tr ',' '\n' | grep -vix "$num" | paste -sd, -)
    efibootmgr --quiet --bootorder "$num${rest:+,$rest}"
    echo "lumen-heal: moved Lumen (Boot$num) back to first"
fi
