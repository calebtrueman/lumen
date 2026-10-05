#!/bin/sh
# Install Lumen from Linux:   sudo ./install-linux.sh
#
# - Copies Lumen, Microsoft-signed shim and MokManager to \EFI\lumen\ on the
#   EFI system partition, and adds a firmware boot entry "Lumen" first in
#   the boot order. Existing entries (GRUB, Windows Boot Manager) are left
#   alone, so if Lumen can't start the firmware simply boots the next one.
# - Secure Boot: queues Lumen's certificate for enrollment (one-time
#   confirmation on the next boot).
# - Installs lumen-heal so updates can't push Lumen out of first place.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
die() { echo "error: $*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "please run with sudo"
[ -d /sys/firmware/efi ] || die "this system wasn't booted in UEFI mode; Lumen needs UEFI"
command -v efibootmgr >/dev/null || die "please install efibootmgr first"
case $(uname -m) in
    x86_64) S=x64 ;;
    aarch64) S=aa64 ;;
    *) die "unsupported CPU $(uname -m)" ;;
esac
for f in lumen.efi "shim$S.efi" "mm$S.efi" lumen.cer lumen-heal.sh; do
    [ -f "$HERE/$f" ] || die "missing $f next to this script (use the bundle from tools/dist.sh)"
done

ESP=""
if command -v bootctl >/dev/null; then ESP=$(bootctl --print-esp-path 2>/dev/null || true); fi
if [ -z "$ESP" ]; then
    for d in /boot/efi /efi /boot; do
        if [ "$(findmnt -n -o FSTYPE "$d" 2>/dev/null)" = vfat ]; then ESP=$d; break; fi
    done
fi
[ -n "$ESP" ] || die "couldn't find a mounted EFI system partition"

D="$ESP/EFI/lumen"
mkdir -p "$D"
cp "$HERE/shim$S.efi" "$HERE/mm$S.efi" "$HERE/lumen.cer" "$D/"
cp "$HERE/lumen.efi" "$D/grub$S.efi"   # shim starts grub<arch>.efi from its own folder
if [ ! -f "$D/lumen.conf" ]; then
    cp "$HERE/lumen.conf" "$D/lumen.conf" 2>/dev/null || printf 'timeout 5\ndefault last\n' > "$D/lumen.conf"
    # BitLocker measures the boot chain; hand Windows to the firmware's own
    # entry so it never asks for the recovery key.
    if command -v blkid >/dev/null && blkid -t TYPE=BitLocker >/dev/null 2>&1; then
        printf '\n# Added by installer: BitLocker detected\nbootnext Windows\n' >> "$D/lumen.conf"
        echo "BitLocker detected: Windows will be started via its own firmware entry."
    fi
fi
echo "Installed to $D"

install -m 0755 "$HERE/lumen-heal.sh" /usr/local/sbin/lumen-heal
if command -v systemctl >/dev/null; then
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
    systemctl enable --now lumen-heal.service >/dev/null 2>&1 || true
fi
/usr/local/sbin/lumen-heal || true

sb_on() { od -An -t u1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c 2>/dev/null | awk '{exit !($NF==1)}'; }
if sb_on; then
    if command -v mokutil >/dev/null && mokutil --test-key "$HERE/lumen.cer" 2>/dev/null | grep -q "already enrolled"; then
        echo "Secure Boot: Lumen's key is already enrolled."
    else
        echo
        echo "Secure Boot is on. Choose a one-time password; you'll type it once on the"
        echo "next boot to approve Lumen's key."
        if command -v mokutil >/dev/null; then
            mokutil --import "$HERE/lumen.cer"
        else
            "$HERE/mok-request.sh" "$HERE/lumen.cer"
        fi
        cat <<'MSG'

On the next boot a blue "Shim UEFI key management" screen appears:
  press a key -> Enroll MOK -> Continue -> Yes -> type the password -> Reboot
MSG
    fi
fi
echo "Done. Lumen will appear on the next boot."
