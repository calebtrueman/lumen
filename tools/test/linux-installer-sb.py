#!/usr/bin/env python3
"""The Linux installer under Secure Boot, in the situation that broke on a
real PC: Lumen's key approved but not the distro's, Lumen on one disk's EFI
partition and Ubuntu on another disk with its own, and Ubuntu running
through its own shim (Lumen's direct start was refused), where MokListRT
makes Canonical's key look approved.

  LUMEN_TEST_KEY=1 LUMEN_FEATURES=debugcon tools/dist.sh x86_64
  tools/test/linux-installer-sb.py ~/lumen-test-images/ubuntu-24.04.qcow2

Passes when the installer, run inside Ubuntu by cloud-init, queues an
approval that includes Canonical's key ("distros=Ubuntu").
"""
import os, shutil, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "target", "installer-sb")
SHARE = "/opt/homebrew/share/qemu"
DIST = os.path.join(ROOT, "dist", "x86_64")
VFV = os.path.expanduser("~/Library/Python/3.9/bin/virt-fw-vars")
MOK = "605dab50-e046-4300-abb6-3dd810dd8b23"

SCRIPT = r"""#!/bin/sh
exec >/dev/ttyS0 2>&1
mkdir -p /mnt/seed
for d in /dev/disk/by-label/CIDATA /dev/disk/by-label/cidata /dev/sr0; do
    [ -e "$d" ] && mount -o ro "$d" /mnt/seed 2>/dev/null && break
done
rm -rf /root/lumen && cp -R /mnt/seed/x86_64 /root/lumen
echo "LUMENTEST MokListRT lists Canonical's key: $(grep -ac 'Canonical Ltd. Master' /sys/firmware/efi/efivars/MokListRT-605dab50-e046-4300-abb6-3dd810dd8b23)"
echo "LUMENTEST LumenApproved: $(ls /sys/firmware/efi/efivars/ | grep -c LumenApproved)"
sh /root/lumen/install-linux.sh --yes --result /root/result 2>&1 | sed 's/^/LUMENTEST   | /'
sed 's/^/LUMENTEST result: /' /root/result
echo "LUMENTEST DONE"
poweroff
"""


def main():
    ubuntu = sys.argv[1]
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(OUT)
    # Lumen's EFI partition: a real FAT image (QEMU's virtual FAT mangles writes).
    esp = os.path.join(OUT, "esp.img")
    subprocess.check_call(["mkfile", "-n", "64m", esp])
    subprocess.check_call(["sh", "-c", f"""
dev=$(hdiutil attach -nomount -imagekey diskimage-class=CRawDiskImage {esp} | awk '{{print $1}}' | head -1)
newfs_msdos -F 32 -v ESP $dev >/dev/null
mkdir -p {OUT}/mnt && mount -t msdos $dev {OUT}/mnt
mkdir -p {OUT}/mnt/EFI/BOOT
cp {DIST}/shimx64.efi {OUT}/mnt/EFI/BOOT/BOOTX64.EFI
cp {DIST}/mmx64.efi {OUT}/mnt/EFI/BOOT/mmx64.efi
cp {DIST}/lumen.efi {OUT}/mnt/EFI/BOOT/grubx64.efi
printf 'timeout 0\\nstay-default off\\n' > {OUT}/mnt/EFI/BOOT/lumen.conf
umount {OUT}/mnt && hdiutil detach -quiet $dev"""])
    # Secure Boot on (Microsoft keys); only Lumen's key approved.
    subprocess.check_call([VFV, "--input", f"{SHARE}/edk2-i386-vars.fd", "--output", f"{OUT}/vars.fd",
                           "--enroll-microsoft", "--microsoft-db", "all", "--sb", "--add-mok", MOK, f"{DIST}/lumen.cer"],
                          stdout=subprocess.DEVNULL)
    # cloud-init seed with the bundle and the test script.
    seed = os.path.join(OUT, "seed")
    os.makedirs(seed)
    shutil.copytree(DIST, os.path.join(seed, "x86_64"))
    open(os.path.join(seed, "meta-data"), "w").write("instance-id: lumen-sb\nlocal-hostname: lumen-sb\n")
    body = "\n".join("      " + l for l in SCRIPT.splitlines())
    open(os.path.join(seed, "user-data"), "w").write(
        "#cloud-config\nwrite_files:\n  - path: /var/lib/cloud/scripts/per-boot/lumen-test.sh\n    permissions: '0755'\n    content: |\n"
        + body + "\n")
    subprocess.check_call(["hdiutil", "makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", "CIDATA",
                           "-o", os.path.join(OUT, "seed.iso"), seed])
    serial = os.path.join(OUT, "serial.log")
    q = subprocess.Popen([
        "qemu-system-x86_64", "-machine", "q35,smm=on", "-cpu", "max", "-smp", "2", "-m", "2048",
        "-global", "driver=cfi.pflash01,property=secure,value=on",
        "-drive", f"if=pflash,format=raw,readonly=on,file={SHARE}/edk2-x86_64-secure-code.fd",
        "-drive", f"if=pflash,format=raw,file={OUT}/vars.fd",
        "-drive", f"file={esp},format=raw,if=none,id=d0", "-device", "ide-hd,drive=d0,bootindex=0",
        "-drive", f"file={ubuntu},if=none,id=d1,snapshot=on,format=qcow2", "-device", "virtio-blk-pci,drive=d1",
        "-drive", f"file={OUT}/seed.iso,if=none,id=d2,format=raw,readonly=on", "-device", "ide-cd,drive=d2,bus=ide.1",
        "-nic", "user", "-vga", "std", "-display", "none", "-serial", f"file:{serial}",
        "-debugcon", f"file:{OUT}/debug.log", "-global", "isa-debugcon.iobase=0xe9",
    ])
    deadline = time.time() + 1200
    while time.time() < deadline and q.poll() is None:
        time.sleep(3)
        if "LUMENTEST DONE" in open(serial, errors="replace").read() if os.path.exists(serial) else False:
            time.sleep(3)
            break
    q.kill()
    text = open(serial, errors="replace").read()
    lines = [l.split("LUMENTEST", 1)[1] for l in text.splitlines() if "LUMENTEST" in l]
    for l in lines:
        print(l)
    dbg = open(f"{OUT}/debug.log", errors="replace").read()
    print("lumen:", [l for l in dbg.splitlines() if "approved keys" in l or "direct start" in l])
    ok = any("result: mok=queued" in l for l in lines) and any("result: distros=" in l and "Ubuntu" in l for l in lines)
    print("PASS: the approval includes Canonical's key" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
