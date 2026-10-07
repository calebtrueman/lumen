#!/usr/bin/env python3
"""The Linux installer on a PC where Lumen already lives on another drive.

Ubuntu boots from its own disk. A second disk's EFI partition holds Lumen
with a "Lumen" boot entry (as if installed from Windows), and Ubuntu's own
EFI partition holds a stray second copy with a second "Lumen" entry (what
0.4.2's Linux installer did). Running install-linux.sh must update the copy
the first entry points at, and remove the stray copy and its entry.

  tools/test/linux-installer-dupes.py ~/lumen-test-images/ubuntu-24.04.qcow2
"""
import os, shutil, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "target", "installer-dupes")
SHARE = "/opt/homebrew/share/qemu"
DIST = os.path.join(ROOT, "dist", "x86_64")

SCRIPT = r"""#!/bin/sh
exec >/dev/ttyS0 2>&1
say() { echo "LUMENTEST $*"; }
mkdir -p /mnt/seed
for d in /dev/disk/by-label/CIDATA /dev/disk/by-label/cidata /dev/sr0; do
    [ -e "$d" ] && mount -o ro "$d" /mnt/seed 2>/dev/null && break
done
rm -rf /root/lumen && cp -R /mnt/seed/x86_64 /root/lumen
# The "Windows" drive: GPT with one EFI partition holding Lumen.
W=$(lsblk -dnpo NAME,SIZE | awk '$2 == "64M" { print $1 }' | head -n1)
say "windows disk: $W"
printf 'label: gpt\n,,C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n' | sfdisk -q "$W"
udevadm settle
mkfs.vfat -F 32 -n WINESP "${W}1" >/dev/null
mkdir -p /mnt/w && mount "${W}1" /mnt/w && mkdir -p /mnt/w/EFI/lumen
cp /root/lumen/shimx64.efi /root/lumen/mmx64.efi /root/lumen/lumen.cer /mnt/w/EFI/lumen/
echo old > /mnt/w/EFI/lumen/grubx64.efi
efibootmgr -q -c -d "$W" -p 1 -L Lumen -l '\EFI\lumen\shimx64.efi'
umount /mnt/w
# The stray copy on Ubuntu's own EFI partition, with a second entry.
mkdir -p /boot/efi/EFI/lumen && cp /root/lumen/shimx64.efi /root/lumen/lumen.cer /boot/efi/EFI/lumen/
ESPDEV=$(findmnt -no SOURCE /boot/efi)
efibootmgr -q -c -d "/dev/$(lsblk -no PKNAME "$ESPDEV")" -p "$(cat /sys/class/block/$(basename "$ESPDEV")/partition)" -L Lumen -l '\EFI\lumen\shimx64.efi'
say "before: $(efibootmgr | grep -c ' Lumen') Lumen entries"
sh /root/lumen/install-linux.sh --yes --no-distro-keys 2>&1 | sed 's/^/LUMENTEST   | /'
n=$(efibootmgr | grep -c ' Lumen')
[ "$n" = 1 ] && say "PASS one Lumen boot entry" || say "FAIL $n Lumen entries"
[ ! -e /boot/efi/EFI/lumen ] && say "PASS stray copy removed" || say "FAIL stray copy still there"
mount "${W}1" /mnt/w
cmp -s /mnt/w/EFI/lumen/grubx64.efi /root/lumen/lumen.efi && say "PASS the original copy was updated" || say "FAIL original copy not updated"
umount /mnt/w
efibootmgr -v | grep ' Lumen' | sed 's/^/LUMENTEST   /'
say "DONE"
poweroff
"""


def main():
    ubuntu = sys.argv[1]
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(OUT)
    with open(f"{OUT}/windows.img", "wb") as f:
        f.truncate(64 << 20)
    shutil.copy(f"{SHARE}/edk2-i386-vars.fd", f"{OUT}/vars.fd")
    seed = os.path.join(OUT, "seed")
    os.makedirs(seed)
    shutil.copytree(DIST, os.path.join(seed, "x86_64"))
    open(os.path.join(seed, "meta-data"), "w").write("instance-id: lumen-dupes\nlocal-hostname: lumen-dupes\n")
    body = "\n".join("      " + l for l in SCRIPT.splitlines())
    open(os.path.join(seed, "user-data"), "w").write(
        "#cloud-config\nwrite_files:\n  - path: /var/lib/cloud/scripts/per-boot/lumen-test.sh\n    permissions: '0755'\n    content: |\n"
        + body + "\n")
    subprocess.check_call(["hdiutil", "makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", "CIDATA",
                           "-o", os.path.join(OUT, "seed.iso"), seed])
    serial = os.path.join(OUT, "serial.log")
    q = subprocess.Popen([
        "qemu-system-x86_64", "-machine", "q35", "-cpu", "max", "-smp", "2", "-m", "2048",
        "-drive", f"if=pflash,format=raw,readonly=on,file={SHARE}/edk2-x86_64-code.fd",
        "-drive", f"if=pflash,format=raw,file={OUT}/vars.fd",
        "-drive", f"file={ubuntu},if=none,id=d1,snapshot=on,format=qcow2", "-device", "virtio-blk-pci,drive=d1,bootindex=0",
        "-drive", f"file={OUT}/windows.img,if=none,id=d3,format=raw", "-device", "virtio-blk-pci,drive=d3",
        "-drive", f"file={OUT}/seed.iso,if=none,id=d2,format=raw,readonly=on", "-device", "ide-cd,drive=d2,bus=ide.1",
        "-nic", "user", "-vga", "std", "-display", "none", "-serial", f"file:{serial}",
    ])
    deadline = time.time() + 1200
    while time.time() < deadline and q.poll() is None:
        time.sleep(3)
        if os.path.exists(serial) and "LUMENTEST DONE" in open(serial, errors="replace").read():
            time.sleep(3)
            break
    q.kill()
    lines = [l.split("LUMENTEST", 1)[1] for l in open(serial, errors="replace").read().splitlines() if "LUMENTEST" in l]
    for l in lines:
        print(l)
    ok = sum(" PASS" in l for l in lines) == 3 and not any(" FAIL" in l for l in lines)
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
