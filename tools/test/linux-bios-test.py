#!/usr/bin/env python3
"""End-to-end test of Lumen on a legacy BIOS PC running Linux (SeaBIOS).

Boots an untouched Debian 13 amd64 cloud image (GPT, GRUB in BIOS mode)
whose cloud-init runs one stage per boot:

  1. (GRUB) run install-linux.sh from the bundle: Lumen goes in the MBR;
     then `grub-install` rewrites the MBR, as a GRUB update does, and
     lumen-heal must put Lumen back                              -> reboot
  2. (Lumen) the harness presses Enter on Lumen's menu; Debian must have
     been started by Lumen itself (boot_params type_of_loader 0xFF, not
     GRUB's); then uninstall                                      -> reboot
  3. (GRUB again) Lumen gone, GRUB starts Debian as before        -> power off

  LUMEN_TEST_KEY=1 LUMEN_FEATURES=debugcon tools/dist.sh x86_64
  tools/test/linux-bios-test.py ~/lumen-test-images/debian-13-genericcloud-amd64.qcow2
"""
import os, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "target", "linux-bios")
sys.path.insert(0, os.path.join(ROOT, "tools"))
from vm import ppm_to_png  # noqa: E402

STAGE_SCRIPT = r"""#!/bin/sh
exec >/dev/ttyS0 2>&1
# Results also go to a file: the serial console can drop lines when the
# login prompt takes it over. The last stage replays the whole record.
say() { echo "LUMENTEST $*"; echo "$*" >> /var/lib/lumen-test.log; }
show() { "$@" 2>&1 | sed 's/^/LUMENTEST   /'; }
STATE=/var/lib/lumen-test-stage
n=$(( $(cat $STATE 2>/dev/null || echo 0) + 1 )); echo $n > $STATE
say "stage $n"
loader() { od -An -tx1 -j 528 -N1 /sys/kernel/boot_params/data | tr -d ' '; }
say "type_of_loader $(loader)"
TOOL=/usr/local/lib/lumen/lumen-bios-install
DISK=/dev/vda
case $n in
1)
    mkdir -p /mnt/seed
    for d in /dev/disk/by-label/CIDATA /dev/disk/by-label/cidata /dev/sr0 /dev/vdb; do
        [ -e "$d" ] && mount -o ro "$d" /mnt/seed 2>/dev/null && break
    done
    rm -rf /root/lumen && cp -R /mnt/seed/x86_64 /root/lumen && chmod +x /root/lumen/bios/lumen-bios-install
    show ls -la /root/lumen /root/lumen/bios
    say "running installer"
    sh /root/lumen/install-linux.sh --yes 2>&1 | sed 's/^/LUMENTEST   | /'
    st=$($TOOL status $DISK); say "status: $st"
    case $st in installed*) say "PASS installed in the MBR" ;; *) say "FAIL not installed" ;; esac
    systemctl is-enabled lumen-heal.service >/dev/null && say "PASS heal service enabled" || say "FAIL heal service"
    say "running grub-install (as a GRUB update would)"
    show grub-install --target=i386-pc $DISK
    st=$($TOOL status $DISK); say "status: $st"
    case $st in displaced*) say "PASS grub-install displaced Lumen (as expected)" ;; *) say "FAIL expected displaced" ;; esac
    /usr/local/sbin/lumen-heal
    st=$($TOOL status $DISK); say "status: $st"
    case $st in installed*) say "PASS lumen-heal put Lumen back" ;; *) say "FAIL heal" ;; esac
    say "rebooting into Lumen"; sync; reboot ;;
2)
    [ "$(loader)" = ff ] && say "PASS Debian was started by Lumen directly (no GRUB)" || say "FAIL loader $(loader)"
    say "uninstalling"
    sh /root/lumen/install-linux.sh --uninstall --yes 2>&1 | sed 's/^/LUMENTEST   | /'
    [ ! -e /usr/local/lib/lumen ] && say "PASS files removed" || say "FAIL files remain"
    dd if=$DISK bs=512 count=1 2>/dev/null | grep -aq GRUB && say "PASS GRUB's boot code is back in the MBR" || say "FAIL MBR"
    say "rebooting"; sync; reboot ;;
3)
    [ "$(loader)" != ff ] && say "PASS GRUB starts Debian as before" || say "FAIL still Lumen"
    sed 's/^/LUMENRESULT /' /var/lib/lumen-test.log
    say "ALL DONE"; sync; poweroff ;;
esac
"""


def main():
    base = sys.argv[1]
    shutil.rmtree(OUT, ignore_errors=True)
    seed = os.path.join(OUT, "seed")
    os.makedirs(seed)
    subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", "-b", os.path.abspath(base), "-F", "qcow2",
                           os.path.join(OUT, "disk.qcow2"), "6G"])
    shutil.copytree(os.path.join(ROOT, "dist", "x86_64"), os.path.join(seed, "x86_64"))
    open(os.path.join(seed, "meta-data"), "w").write("instance-id: lumen-bios-test\nlocal-hostname: lumen-bios-test\n")
    indented = "\n".join("      " + l for l in STAGE_SCRIPT.splitlines())
    open(os.path.join(seed, "user-data"), "w").write(
        "#cloud-config\npassword: lumen\nchpasswd: { expire: false }\nwrite_files:\n"
        "  - path: /var/lib/cloud/scripts/per-boot/lumen-test.sh\n    permissions: '0755'\n    content: |\n" + indented + "\n")
    subprocess.check_call(["hdiutil", "makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", "CIDATA",
                           "-o", os.path.join(OUT, "seed.iso"), seed])

    serial, debug, mon = (os.path.join(OUT, n) for n in ("serial.log", "debug.log", "mon.sock"))
    proc = subprocess.Popen([
        "qemu-system-x86_64", "-cpu", "max", "-smp", "2", "-m", "2048", "-vga", "std",
        "-drive", f"if=virtio,format=qcow2,file={OUT}/disk.qcow2",
        "-drive", f"if=virtio,format=raw,readonly=on,file={OUT}/seed.iso",
        "-nic", "user,model=virtio-net-pci",
        "-display", "none", "-serial", f"file:{serial}",
        "-debugcon", f"file:{debug}", "-monitor", f"unix:{mon},server,nowait",
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

    seen, menus, results = 0, 0, []
    deadline = time.time() + 2400
    try:
        while time.time() < deadline and proc.poll() is None:
            time.sleep(1)
            text = open(serial, errors="replace").read() if os.path.exists(serial) else ""
            lines = [l for l in text.splitlines() if "LUMENTEST" in l]
            for l in lines[seen:]:
                l = l.split("LUMENTEST", 1)[1]
                print(l)
                results.append(l)
            seen = len(lines)
            # Lumen's menu is up: choose the highlighted entry (Debian).
            dbg = open(debug, errors="replace").read() if os.path.exists(debug) else ""
            if dbg.count(" entries, mouse") > menus:
                menus = dbg.count(" entries, mouse")
                time.sleep(3)
                shot(f"lumen_menu_{menus}")
                m.sendall(b"sendkey ret\n")
                print("  [pressed Enter in Lumen]")
            if any("ALL DONE" in l for l in lines):
                time.sleep(5)
                break
    finally:
        proc.kill()
    finished = any("ALL DONE" in r for r in results)
    text = open(serial, errors="replace").read() if os.path.exists(serial) else ""
    record = [l.split("LUMENRESULT ", 1)[1] for l in text.splitlines() if "LUMENRESULT " in l]
    if record:
        print("\nFull record:")
        for r in record:
            print("  " + r)
        results = record
    fails = [r for r in results if r.lstrip().startswith("FAIL")]
    passes = [r for r in results if r.lstrip().startswith("PASS")]
    print(f"\n{len(passes)} passed, {len(fails)} failed" + ("" if finished else " (didn't finish)"))
    sys.exit(1 if fails or not finished else 0)


if __name__ == "__main__":
    main()
