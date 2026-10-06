#!/usr/bin/env python3
"""End-to-end test of the one-click Linux installer on a real Debian VM.

Boots a Debian 13 arm64 cloud image (hardware-accelerated on Apple
Silicon) whose cloud-init drives this lifecycle, one stage per boot:

  1. run Lumen-Installer-Linux.run (terminal mode)   -> reboot
     (Lumen should appear via BootNext, mark itself healthy, promote
      itself to first, then auto-start Debian)
  2. check Lumen is first; simulate a distro update pushing it down -> reboot
     (lumen-heal's shutdown hook should put it back)
  3. check Lumen is first again; uninstall              -> reboot
  4. check Lumen is gone                                -> power off

  tools/linux-vm-test.py /path/to/debian-13-genericcloud-arm64.qcow2
"""
import os, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "target", "linuxvm")
SHARE = "/opt/homebrew/share/qemu"
sys.path.insert(0, os.path.join(ROOT, "tools"))
from vm import ppm_to_png  # noqa: E402

STAGE_SCRIPT = r"""#!/bin/sh
exec >/dev/console 2>&1
say() { echo "LUMENTEST $*"; }
show() { "$@" 2>&1 | sed 's/^/LUMENTEST   /'; }
STATE=/var/lib/lumen-test-stage
n=$(( $(cat $STATE 2>/dev/null || echo 0) + 1 )); echo $n > $STATE
say "stage $n"
show efibootmgr
show sh -c 'ls /sys/firmware/efi/efivars | grep -i lumen || echo "(no Lumen variables)"'
lumen() { command -v efibootmgr >/dev/null || { echo none; return; }; efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\} Lumen.*/\1/p' | head -n1; }
first() { efibootmgr | sed -n 's/^BootOrder: //p' | cut -d, -f1; }
case $n in
1)
    mkdir -p /mnt/seed
    for d in /dev/disk/by-label/CIDATA /dev/disk/by-label/cidata /dev/vdb /dev/sr0; do
        [ -e "$d" ] && mount -o ro "$d" /mnt/seed 2>/dev/null && break
    done
    show lsblk -f
    show ls -la /mnt/seed
    cp /mnt/seed/Lumen-Installer-Linux.run /root/ 2>/dev/null || cp /mnt/seed/*.run /root/Lumen-Installer-Linux.run || cp /mnt/seed/LUMEN_IN.RUN /root/Lumen-Installer-Linux.run || { say "FAIL installer not found on seed disk"; say "ALL DONE"; poweroff; exit 0; }
    say "running installer"
    echo y | sh /root/Lumen-Installer-Linux.run 2>&1 | sed 's/^/LUMENTEST   | /'
    show efibootmgr
    show systemctl is-enabled lumen-heal.service
    [ -n "$(lumen)" ] && [ "$(lumen)" != none ] && say "PASS Lumen boot entry created" || { say "FAIL no Lumen entry"; say "ALL DONE"; poweroff; exit 0; }
    [ "$(efibootmgr | sed -n 's/^BootNext: //p')" = "$(lumen)" ] && say "PASS BootNext points at Lumen" || say "FAIL BootNext"
    [ "$(first)" != "$(lumen)" ] && say "PASS default boot unchanged until Lumen proves itself" || say "FAIL default changed early"
    # Lumen waits for a choice by default; let it auto-start Debian here.
    echo "timeout 3" >> /boot/efi/EFI/lumen/lumen.conf
    say "rebooting into Lumen"; sync; reboot ;;
2)
    [ -e /sys/firmware/efi/efivars/LumenHealthy-4c756d65-6e00-4b6f-9f2a-6c756d656e21 ] && say "PASS Lumen marked itself healthy" || say "FAIL not healthy"
    [ "$(first)" = "$(lumen)" ] && say "PASS Lumen promoted itself to first" || say "FAIL Lumen not first"
    num=$(lumen)
    rest=$(efibootmgr | sed -n 's/^BootOrder: //p' | tr ',' '\n' | grep -vix "$num" | paste -sd, -)
    efibootmgr -q -o "$rest,$num"
    say "simulated an update pushing Lumen down: BootOrder $(efibootmgr | sed -n 's/^BootOrder: //p')"
    say "rebooting (lumen-heal runs at shutdown)"; sync; reboot ;;
3)
    [ "$(first)" = "$(lumen)" ] && say "PASS lumen-heal restored Lumen to first" || say "FAIL heal"
    say "uninstalling"
    echo y | sh /root/Lumen-Installer-Linux.run --uninstall 2>&1 | sed 's/^/LUMENTEST   | /'
    [ -z "$(lumen)" ] && say "PASS Lumen entry removed" || say "FAIL entry still present"
    [ ! -e /etc/systemd/system/lumen-heal.service ] && say "PASS heal service removed" || say "FAIL heal service remains"
    [ ! -e /boot/efi/EFI/lumen ] && say "PASS files removed" || say "FAIL files remain"
    say "rebooting"; sync; reboot ;;
4)
    [ -z "$(lumen)" ] && say "PASS PC boots as before" || say "FAIL"
    say "ALL DONE"; sync; poweroff ;;
esac
"""


def main():
    base = sys.argv[1]
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(os.path.join(OUT, "seed"))
    subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", "-b", os.path.abspath(base), "-F", "qcow2",
                           os.path.join(OUT, "disk.qcow2"), "6G"])
    seed = os.path.join(OUT, "seed")
    shutil.copy(os.path.join(ROOT, "dist", "Lumen-Installer-Linux.run"), seed)
    open(os.path.join(seed, "meta-data"), "w").write("instance-id: lumen-test\nlocal-hostname: lumen-test\n")
    indented = "\n".join("      " + l for l in STAGE_SCRIPT.splitlines())
    open(os.path.join(seed, "user-data"), "w").write(
        "#cloud-config\npassword: lumen\nchpasswd: { expire: false }\nwrite_files:\n"
        "  - path: /var/lib/cloud/scripts/per-boot/lumen-test.sh\n    permissions: '0755'\n    content: |\n" + indented + "\n")
    subprocess.check_call(["hdiutil", "makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", "CIDATA",
                           "-o", os.path.join(OUT, "seed.iso"), seed])
    code = os.path.join(OUT, "code.fd")
    shutil.copy(os.path.join(SHARE, "edk2-aarch64-code.fd"), code)
    with open(os.path.join(OUT, "vars.fd"), "wb") as f:
        f.truncate(os.path.getsize(code))

    serial = os.path.join(OUT, "serial.log")
    mon = os.path.join(OUT, "mon.sock")
    proc = subprocess.Popen([
        "qemu-system-aarch64", "-machine", "virt", "-accel", "hvf", "-cpu", "host", "-smp", "2", "-m", "2048",
        "-drive", f"if=pflash,format=raw,readonly=on,file={code}",
        "-drive", f"if=pflash,format=raw,file={OUT}/vars.fd",
        "-drive", f"if=virtio,format=qcow2,file={OUT}/disk.qcow2",
        "-drive", f"if=virtio,format=raw,readonly=on,file={OUT}/seed.iso",
        "-device", "virtio-gpu-pci", "-device", "qemu-xhci", "-device", "usb-kbd",
        "-nic", "user,model=virtio-net-pci",
        "-display", "none", "-serial", f"file:{serial}", "-monitor", f"unix:{mon},server,nowait",
    ])
    for _ in range(50):
        if os.path.exists(mon):
            break
        time.sleep(0.1)
    m = socket.socket(socket.AF_UNIX)
    m.connect(mon)

    def shot(name):
        m.sendall(f"screendump {OUT}/{name}.ppm\n".encode())
        time.sleep(1.0)
        ppm_to_png(f"{OUT}/{name}.ppm", f"{OUT}/{name}.png")
        print(f"  [screenshot {OUT}/{name}.png]")

    seen = 0
    shots = {"rebooting into Lumen": "lumen_first_boot", "rebooting (lumen-heal": "lumen_after_heal"}
    deadline = time.time() + 1500
    try:
        while time.time() < deadline and proc.poll() is None:
            time.sleep(1)
            text = open(serial, errors="replace").read() if os.path.exists(serial) else ""
            lines = [l for l in text.splitlines() if "LUMENTEST" in l]
            for l in lines[seen:]:
                print(l.split("LUMENTEST", 1)[1])
                for marker, name in shots.items():
                    if marker in l:
                        # Lumen shows its menu for 5 s before starting Debian.
                        for delay in (5, 3, 3, 3, 3):
                            time.sleep(delay)
                            shot(f"{name}_{delay}")
            seen = len(lines)
            if any("ALL DONE" in l for l in lines):
                time.sleep(5)
                break
    finally:
        proc.kill()


if __name__ == "__main__":
    main()
