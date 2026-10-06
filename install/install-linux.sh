#!/bin/sh
# Lumen installer for Linux.
#
#   sudo ./install-linux.sh                  install or update (asks before changing anything)
#   sudo ./install-linux.sh --uninstall      remove Lumen completely
#
# Options used by the one-click installer: --yes (don't ask), --code NNNN
# (Secure Boot approval code), --result FILE (write key=value results).
#
# On PCs that start in legacy BIOS mode, Lumen's BIOS edition is installed
# in the boot disk's MBR instead (see "legacy BIOS PCs" below).
#
# What it does on UEFI PCs, and why it can't leave the PC unbootable:
#  * Copies Lumen, Microsoft-signed shim and MokManager to \EFI\lumen\ on the
#    EFI system partition. Nothing that's already there is modified.
#  * Adds a firmware boot entry "Lumen" and makes it the *next* boot only.
#    The default stays as it was until Lumen has started successfully on this
#    PC once; Lumen then makes itself the default. If that first start fails
#    for any reason, the PC simply keeps booting the way it always has.
#  * Under Secure Boot, queues Lumen's key for a one-time approval on the
#    blue "Shim UEFI key management" screen.
#  * Installs lumen-heal so OS and firmware updates can't push Lumen out.
#  * Any failure undoes everything this run changed.
set -eu
umask 022

HERE=$(cd "$(dirname "$0")" && pwd)
VENDOR_GUID=4c756d65-6e00-4b6f-9f2a-6c756d656e21
SHIM_GUID=605dab50-e046-4300-abb6-3dd810dd8b23
EFIVARS=/sys/firmware/efi/efivars
YES=0
CODE=""
MODE=install
RESULT=""

while [ $# -gt 0 ]; do
    case $1 in
        --yes) YES=1 ;;
        --code) CODE=$2; shift ;;
        --uninstall) MODE=uninstall ;;
        --result) RESULT=$2; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

say() { echo "lumen: $*"; }
result() { if [ -n "$RESULT" ]; then echo "$1" >> "$RESULT"; fi; }
die() {
    echo "lumen: error: $*" >&2
    result "error=$*"
    exit 1
}
[ -z "$RESULT" ] || : > "$RESULT"

# ---- preflight -----------------------------------------------------------------

[ "$(id -u)" -eq 0 ] || die "please run as root (sudo)"

# ---- legacy BIOS PCs ---------------------------------------------------------------
# Lumen's BIOS edition goes into the boot disk's MBR and the free space
# before its first partition (lumen-bios-install does the disk work and
# refuses if that space isn't free). The previous boot code (usually GRUB)
# stays reachable from Lumen, and holding Shift at power-on skips Lumen.
if [ ! -d /sys/firmware/efi ]; then
    LIB=/usr/local/lib/lumen
    BIOS="$HERE/bios"
    [ -x "$BIOS/lumen-bios-install" ] || BIOS=$LIB
    [ -x "$BIOS/lumen-bios-install" ] || die "the installer is incomplete (the BIOS edition of Lumen is missing)"
    # The disk /boot (or /) lives on, through LVM/LUKS if need be, named
    # stably (sdX names can change between boots).
    boot_disk() {
        src=$(findmnt -n -o SOURCE /boot 2>/dev/null || findmnt -n -o SOURCE /)
        src=${src%%\[*}
        name=$(lsblk -nso NAME,TYPE "$src" 2>/dev/null | awk '$2 == "disk" { gsub(/[^a-zA-Z0-9_-]/, "", $1); print $1; exit }')
        [ -n "$name" ] || return 1
        for l in /dev/disk/by-id/wwn-* /dev/disk/by-id/ata-* /dev/disk/by-id/nvme-* /dev/disk/by-id/*; do
            [ -e "$l" ] && [ "$(basename "$(readlink -f "$l")")" = "$name" ] && case $l in *-part*) ;; *) echo "$l"; return ;; esac
        done
        echo "/dev/$name"
    }
    if [ "$MODE" = uninstall ]; then
        if command -v systemctl >/dev/null; then
            systemctl disable --now lumen-heal.service >/dev/null 2>&1 || true
            rm -f /etc/systemd/system/lumen-heal.service
            systemctl daemon-reload || true
        fi
        DISK=$(cat "$LIB/disk" 2>/dev/null || boot_disk) || die "couldn't tell which disk this PC starts from"
        "$BIOS/lumen-bios-install" uninstall "$DISK" || die "couldn't remove Lumen from $DISK"
        rm -f /usr/local/sbin/lumen-heal
        rm -rf "$LIB"
        result "ok=1"
        say "Lumen has been removed. Your PC will start the way it did before."
        exit 0
    fi
    DISK=$(boot_disk) || die "couldn't tell which disk this PC starts from"
    UPDATE=0
    "$BIOS/lumen-bios-install" status "$DISK" | grep -q '^not installed' || UPDATE=1
    if [ "$YES" -ne 1 ]; then
        printf 'Install Lumen on %s (BIOS boot sector)? Your current boot loader stays available. [Y/n] ' "$DISK"
        read -r answer </dev/tty || answer=y
        case $answer in [nN]*) exit 0 ;; esac
    fi
    mkdir -p "$LIB"
    if [ "$BIOS" != "$LIB" ]; then
        install -m 0755 "$BIOS/lumen-bios-install" "$LIB/lumen-bios-install"
        install -m 0644 "$BIOS/lumen-bios.img" "$LIB/lumen-bios.img"
    fi
    out=$("$LIB/lumen-bios-install" install "$DISK" "$LIB/lumen-bios.img" 2>&1) || die "${out#lumen-bios-install: }"
    say "$out"
    echo "$DISK" > "$LIB/disk"
    install -m 0755 "$HERE/lumen-heal.sh" /usr/local/sbin/lumen-heal
    if command -v systemctl >/dev/null && [ -d /etc/systemd/system ]; then
        cat > /etc/systemd/system/lumen-heal.service <<'UNIT'
[Unit]
Description=Keep Lumen as the boot menu
After=local-fs.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/lumen-heal
ExecStop=/usr/local/sbin/lumen-heal

[Install]
WantedBy=multi-user.target
UNIT
        systemctl daemon-reload
        systemctl enable lumen-heal.service >/dev/null 2>&1 || true
    fi
    result "ok=1"
    result "update=$UPDATE"
    result "bios=1"
    say "done."
    [ "$YES" -eq 1 ] || echo "Restart your PC to see Lumen."
    exit 0
fi
case $(uname -m) in
    x86_64) S=x64 ;;
    aarch64) S=aa64 ;;
    *) die "unsupported processor $(uname -m)" ;;
esac

# Firmware variables must be readable and writable.
if ! grep -q " $EFIVARS efivarfs" /proc/mounts; then
    mount -t efivarfs efivarfs "$EFIVARS" 2>/dev/null || die "couldn't mount efivarfs"
fi
if grep " $EFIVARS efivarfs" /proc/mounts | grep -q '[ ,]ro[ ,]'; then
    mount -o remount,rw "$EFIVARS" || die "firmware variables are read-only and couldn't be remounted"
fi

if ! command -v efibootmgr >/dev/null; then
    say "installing efibootmgr…"
    # Retry after refreshing package lists: fresh installs often have none.
    if command -v apt-get >/dev/null; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y efibootmgr >/dev/null 2>&1 ||
            { apt-get update >/dev/null 2>&1; DEBIAN_FRONTEND=noninteractive apt-get install -y efibootmgr >/dev/null 2>&1; } || true
    elif command -v dnf >/dev/null; then dnf install -y efibootmgr >/dev/null 2>&1 || true
    elif command -v pacman >/dev/null; then
        pacman -S --noconfirm --needed efibootmgr >/dev/null 2>&1 || pacman -Sy --noconfirm --needed efibootmgr >/dev/null 2>&1 || true
    elif command -v zypper >/dev/null; then zypper --non-interactive install efibootmgr >/dev/null 2>&1 || true
    elif command -v xbps-install >/dev/null; then xbps-install -y efibootmgr >/dev/null 2>&1 || true
    elif command -v apk >/dev/null; then apk add efibootmgr >/dev/null 2>&1 || { apk update >/dev/null 2>&1; apk add efibootmgr >/dev/null 2>&1; } || true
    fi
    command -v efibootmgr >/dev/null || die "efibootmgr is needed; please install it with your package manager and run this again"
fi

for f in lumen.efi "shim$S.efi" "mm$S.efi" lumen.cer lumen-heal.sh mok-request.sh; do
    [ -f "$HERE/$f" ] || die "the installer is incomplete ($f is missing); please download it again"
done

ESP_MOUNTED_BY_US=""
find_esp() {
    if command -v bootctl >/dev/null; then
        p=$(bootctl --print-esp-path 2>/dev/null || true)
        if [ -n "$p" ]; then echo "$p"; return; fi
    fi
    for d in /boot/efi /efi /boot /boot/EFI; do
        if [ "$(findmnt -n -o FSTYPE "$d" 2>/dev/null)" = vfat ] && [ -d "$d/EFI" ]; then echo "$d"; return; fi
    done
    # Not mounted: find the partition by its GPT type and mount it ourselves.
    dev=$(lsblk -rno PATH,PARTTYPE 2>/dev/null | awk 'tolower($2) == "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" { print $1; exit }')
    if [ -n "$dev" ]; then
        mkdir -p /run/lumen-esp
        mount -t vfat "$dev" /run/lumen-esp && ESP_MOUNTED_BY_US=/run/lumen-esp && echo /run/lumen-esp && return
    fi
    return 1
}
ESP=$(find_esp) || die "couldn't find the EFI system partition"
[ -w "$ESP" ] || die "the EFI system partition ($ESP) is read-only"
D="$ESP/EFI/lumen"

lumen_entry() {
    efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\} Lumen\([[:space:]].*\)\{0,1\}$/\1/p' | head -n1
}
healthy() { [ -e "$EFIVARS/LumenHealthy-$VENDOR_GUID" ]; }
sb_on() { od -An -t u1 "$EFIVARS/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c" 2>/dev/null | awk 'NF { v = $NF } END { exit !(v == 1) }'; }
hexof() { od -An -tx1 -v "$1" | tr -d ' \n'; }
key_enrolled() {
    f="$EFIVARS/MokListRT-$SHIM_GUID"
    [ -e "$f" ] && hexof "$f" | grep -q "$(hexof "$HERE/lumen.cer")"
}

# ---- uninstall -------------------------------------------------------------------

if [ "$MODE" = uninstall ]; then
    # Stop the heal service first: its shutdown hook would otherwise
    # recreate the entry we're about to delete.
    if command -v systemctl >/dev/null; then
        systemctl disable --now lumen-heal.service >/dev/null 2>&1 || true
        rm -f /etc/systemd/system/lumen-heal.service
        systemctl daemon-reload || true
    fi
    rm -f /usr/local/sbin/lumen-heal
    num=$(lumen_entry)
    if [ -n "$num" ]; then
        if [ "$(efibootmgr | sed -n 's/^BootNext: //p')" = "$num" ]; then efibootmgr --quiet --delete-bootnext || true; fi
        efibootmgr --quiet --delete-bootnum --bootnum "$num" || true
        say "removed firmware boot entry Boot$num"
    fi
    rm -rf "$D"
    for v in LumenHealthy LumenLastBoot; do
        chattr -i "$EFIVARS/$v-$VENDOR_GUID" 2>/dev/null || true
        rm -f "$EFIVARS/$v-$VENDOR_GUID"
    done
    # Withdraw a Secure Boot approval request for our key that was never confirmed.
    if [ -e "$EFIVARS/MokNew-$SHIM_GUID" ] && [ -f "$HERE/lumen.cer" ] && hexof "$EFIVARS/MokNew-$SHIM_GUID" | grep -q "$(hexof "$HERE/lumen.cer")"; then
        for v in MokNew MokAuth; do
            chattr -i "$EFIVARS/$v-$SHIM_GUID" 2>/dev/null || true
            rm -f "$EFIVARS/$v-$SHIM_GUID"
        done
    fi
    [ -z "$(lumen_entry)" ] || die "the firmware didn't remove the Lumen boot entry"
    result "ok=1"
    say "Lumen has been removed. Your PC will start the way it did before."
    exit 0
fi

# ---- install -------------------------------------------------------------------

ESP_FREE_KB=$(df -Pk "$ESP" | awk 'NR == 2 { print $4 }')
NEED_KB=$(( ($(du -sk "$HERE" | cut -f1)) + 512 ))
[ "${ESP_FREE_KB:-0}" -ge "$NEED_KB" ] || [ -d "$D" ] || die "the EFI system partition is full (${ESP_FREE_KB} KB free, ${NEED_KB} KB needed)"

UPDATE=0
[ -d "$D" ] && UPDATE=1
if [ "$YES" -ne 1 ]; then
    if [ $UPDATE -eq 1 ]; then printf 'Update Lumen on %s? [Y/n] ' "$ESP"; else printf 'Install Lumen on %s? Your current boot setup is kept. [Y/n] ' "$ESP"; fi
    read -r answer </dev/tty || answer=y
    case $answer in [nN]*) exit 0 ;; esac
fi

# Undo everything on failure.
CREATED_DIR=""
CREATED_ENTRY=""
DONE=0
rollback() {
    [ $DONE -eq 1 ] && return
    say "something went wrong; undoing changes…"
    [ -n "$CREATED_ENTRY" ] && efibootmgr --quiet --delete-bootnum --bootnum "$CREATED_ENTRY" 2>/dev/null || true
    [ -n "$CREATED_DIR" ] && rm -rf "$CREATED_DIR"
}
cleanup() {
    rollback
    if [ -n "$ESP_MOUNTED_BY_US" ]; then umount "$ESP_MOUNTED_BY_US" 2>/dev/null || true; fi
}
trap cleanup EXIT

[ $UPDATE -eq 1 ] || CREATED_DIR=$D
mkdir -p "$D"
put() { # src dst: copy via a temp name so a crash never leaves a half-written loader
    cp "$1" "$2.new" && sync && mv -f "$2.new" "$2" && cmp -s "$1" "$2" || die "couldn't write $2"
}
put "$HERE/shim$S.efi" "$D/shim$S.efi"
put "$HERE/mm$S.efi" "$D/mm$S.efi"
put "$HERE/lumen.cer" "$D/lumen.cer"
put "$HERE/lumen.efi" "$D/grub$S.efi" # shim starts grub<arch>.efi from its own folder
# Installs before 0.3 shipped "timeout 5" from the template; Lumen now waits
# by default, so replace that untouched template line.
if [ -f "$D/lumen.conf" ] && grep -qx '# Seconds before the highlighted entry starts. 0 = immediately, -1 = wait forever.' "$D/lumen.conf"; then
    awk '
        /^# Seconds before the highlighted entry starts\. 0 = immediately, -1 = wait forever\.$/ { held = $0; next }
        held != "" && /^timeout 5[ \t]*$/ {
            print "# Lumen waits until you choose. To start the highlighted entry automatically,"
            print "# use the Auto-start button in Lumen'"'"'s menu (Off, 5, 10 or 30 seconds), or set"
            print "# a number of seconds here (0 = immediately). The button overrides this line."
            print "# timeout 10"
            held = ""; next
        }
        held != "" { print held; held = "" }
        { print }
    ' "$D/lumen.conf" > "$D/lumen.conf.new" && mv -f "$D/lumen.conf.new" "$D/lumen.conf"
fi
if [ ! -f "$D/lumen.conf" ]; then
    cp "$HERE/lumen.conf" "$D/lumen.conf" 2>/dev/null || printf 'default last\n' > "$D/lumen.conf"
    # BitLocker measures the boot chain; hand Windows to its own firmware
    # entry so it never asks for the recovery key.
    if command -v blkid >/dev/null && blkid -t TYPE=BitLocker >/dev/null 2>&1; then
        printf '\n# Added by installer: BitLocker detected\nbootnext Windows\n' >> "$D/lumen.conf"
        result "bitlocker=1"
    fi
fi
sync
say "copied Lumen to $D"

num=$(lumen_entry)
order=$(efibootmgr | sed -n 's/^BootOrder: //p')
if [ -z "$num" ]; then
    src=$(findmnt -n -o SOURCE "$ESP")
    disk=/dev/$(lsblk -no PKNAME "$src" | head -n1)
    part=$(cat "/sys/class/block/$(basename "$src")/partition")
    efibootmgr --quiet --create --disk "$disk" --part "$part" --label Lumen --loader "\\EFI\\lumen\\shim$S.efi" ||
        die "the firmware refused to add a boot entry"
    num=$(lumen_entry)
    [ -n "$num" ] || die "the firmware didn't keep the new boot entry"
    CREATED_ENTRY=$num
    say "added firmware boot entry Boot$num"
fi
if healthy; then
    # Lumen already ran fine on this PC: keep it the default.
    rest=$(echo "$order" | tr ',' '\n' | grep -vix "$num" | grep . | paste -sd, - || true)
    efibootmgr --quiet --bootorder "$num${rest:+,$rest}"
else
    # Not proven yet: leave the default alone, just try Lumen next boot.
    if [ -n "$order" ]; then
        rest=$(echo "$order" | tr ',' '\n' | grep -vix "$num" | grep . | paste -sd, - || true)
        efibootmgr --quiet --bootorder "${rest:+$rest,}$num"
    fi
    efibootmgr --quiet --bootnext "$num" || die "couldn't schedule Lumen for the next boot"
fi

install -m 0755 "$HERE/lumen-heal.sh" /usr/local/sbin/lumen-heal
if command -v systemctl >/dev/null && [ -d /etc/systemd/system ]; then
    cat > /etc/systemd/system/lumen-heal.service <<'UNIT'
[Unit]
Description=Keep Lumen first in the firmware boot order
After=local-fs.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/lumen-heal
ExecStop=/usr/local/sbin/lumen-heal

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable lumen-heal.service >/dev/null 2>&1 || true
fi

# Ask for the one-time Secure Boot approval even when Secure Boot is off, so
# turning it on later doesn't stop Lumen from starting.
if key_enrolled; then
    result "mok=enrolled"
else
    if [ -z "$CODE" ]; then CODE=$(od -An -N2 -tu2 /dev/urandom | awk '{ printf "%04d", $1 % 10000 }'); fi
    LUMEN_MOK_PASSWORD=$CODE "$HERE/mok-request.sh" "$HERE/lumen.cer" >/dev/null || die "couldn't queue the Secure Boot key"
    result "mok=queued"
    result "code=$CODE"
fi
sb_on && result "secureboot=1" || result "secureboot=0"

DONE=1
result "ok=1"
result "update=$UPDATE"
say "done."
if [ "$YES" -ne 1 ] && [ -n "$CODE" ]; then
    cat <<MSG

Restart your PC. A blue "Shim UEFI key management" screen will appear once:
  press any key -> Enroll MOK -> Continue -> Yes -> type $CODE -> Reboot
MSG
else
    [ "$YES" -eq 1 ] || echo "Restart your PC to see Lumen."
fi
