#!/usr/bin/env python3
"""Lumen starts a real distro's kernel itself, without GRUB (QEMU + OVMF).

  tools/test/linux-direct.py DISK.qcow2 [--secureboot] [--expect NAME] [--shot NAME]
  tools/test/linux-direct.py target/fixtures/unsigned-linux.img --secureboot --expect-refusal

DISK is an untouched x86_64 distro image (e.g. Debian's
debian-13-genericcloud-amd64.qcow2), attached read-only (snapshot). Lumen
runs from its own small ESP with "timeout 0", so it starts the only OS it
finds straight away. Passes when:
  - Lumen's log shows it started the kernel itself ("starting kernel ...
    directly") and no "direct start ... failed" (which would mean it fell
    back to the distro's loader),
  - and the kernel boots: its console output reaches userspace on the
    serial port, or, for distros that print nothing on serial (quiet,
    console on tty only), it's still running 2 minutes later (a kernel
    that fails to start returns to Lumen, which logs the failure); a
    screenshot is then saved as target/linux-direct/booted.png.

With --secureboot: Secure Boot on (Microsoft keys), Lumen behind Debian's
shim with its certificate enrolled as a MOK, as on a real install. A
Debian-signed kernel must then pass shim's check and boot.

With --expect-refusal: the kernel isn't signed (tools/test/make-unsigned-disk.sh)
and passing means Lumen refused to start it.

Build first:  cargo efi-x64 --features debugcon
"""
import os, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "target", "linux-direct")
SHARE = "/opt/homebrew/share/qemu"
LUMEN = os.path.join(ROOT, "target", "x86_64-lumen-uefi", "release", "lumen.efi")
DIST = os.path.join(ROOT, "dist", "x86_64")
VFV = os.path.expanduser("~/Library/Python/3.9/bin/virt-fw-vars")
sys.path.insert(0, os.path.join(ROOT, "tools"))
from vm import ppm_to_png  # noqa: E402


def main():
    args = [a for a in sys.argv[1:]]
    disk = next(a for a in args if not a.startswith("--") and a.endswith((".qcow2", ".img", ".raw")))
    sb = "--secureboot" in args
    expect = args[args.index("--expect") + 1] if "--expect" in args else None
    shot = args[args.index("--shot") + 1] if "--shot" in args else None
    refusal = "--expect-refusal" in args

    shutil.rmtree(OUT, ignore_errors=True)
    esp = os.path.join(OUT, "esp", "EFI", "BOOT")
    os.makedirs(esp)
    if sb:
        shutil.copy(f"{DIST}/shimx64.efi", f"{esp}/BOOTX64.EFI")
        shutil.copy(f"{DIST}/mmx64.efi", f"{esp}/mmx64.efi")
        shutil.copy(f"{DIST}/lumen.efi", f"{esp}/grubx64.efi")
        subprocess.check_call([VFV, "--input", f"{SHARE}/edk2-i386-vars.fd", "--output", f"{OUT}/vars.fd",
                               "--enroll-microsoft", "--microsoft-db", "all", "--sb",
                               "--add-mok", "605dab50-e046-4300-abb6-3dd810dd8b23", f"{DIST}/lumen.cer"],
                              stdout=subprocess.DEVNULL)
    else:
        shutil.copy(LUMEN, f"{esp}/BOOTX64.EFI")
        shutil.copy(f"{SHARE}/edk2-i386-vars.fd", f"{OUT}/vars.fd")
    with open(f"{esp}/lumen.conf", "w") as f:
        f.write("timeout 0\nstay-default off\n")

    code = "edk2-x86_64-secure-code.fd" if sb else "edk2-x86_64-code.fd"
    cmd = [
        "qemu-system-x86_64", "-machine", "q35,smm=on" if sb else "q35", "-m", "2048", "-smp", "2",
        # Modern distros (EL9+) need x86-64-v2; QEMU's default CPU lacks it.
        "-cpu", "max",
        "-global", "driver=cfi.pflash01,property=secure,value=on",
        "-drive", f"if=pflash,format=raw,readonly=on,file={SHARE}/{code}",
        "-drive", f"if=pflash,format=raw,file={OUT}/vars.fd",
        "-drive", f"file=fat:rw:{OUT}/esp,format=raw,if=none,id=d0", "-device", "ide-hd,drive=d0,bootindex=0",
        "-drive", f"file={disk},if=none,id=d1,snapshot=on,format={'qcow2' if disk.endswith('.qcow2') else 'raw'}",
        "-device", "virtio-blk-pci,drive=d1",
        "-net", "none", "-vga", "std", "-display", "none",
        "-serial", f"file:{OUT}/serial.log",
        "-debugcon", f"file:{OUT}/debug.log", "-global", "isa-debugcon.iobase=0xe9",
        "-monitor", f"unix:{OUT}/mon.sock,server,nowait",
    ]
    q = subprocess.Popen(cmd)
    ok, why = False, "timed out"
    try:
        deadline = time.time() + 900
        started = launched = None
        while time.time() < deadline and q.poll() is None:
            time.sleep(5)
            serial = read(f"{OUT}/serial.log")
            debug = read(f"{OUT}/debug.log")
            if "directly" in debug and started is None:
                started = time.time()
            if "direct start of" in debug:
                why = "direct start failed: " + debug.split("direct start of", 1)[1].splitlines()[0]
                ok = refusal and "isn't signed by a key this PC trusts" in why
                break
            if refusal and "Linux version" in serial:
                why = "an unsigned kernel was started"
                break
            if "starting" in debug and launched is None:
                launched = time.time()
            if launched and started is None and time.time() - launched > 20:
                why = "Lumen started the distro's loader, not the kernel"
                break
            if started is None:
                continue
            if "Kernel panic" in serial:
                why = "kernel panic: " + serial.split("Kernel panic", 1)[1].splitlines()[0]
                break
            # Userspace reached: systemd or the login prompt on the console.
            if any(m in serial for m in ("login:", "Reached target", "Welcome to", "systemd[1]")):
                ok, why = True, "kernel booted to userspace"
                break
            if time.time() - started > 120 and q.poll() is None:
                ok, why = True, "kernel running after 2 minutes (no serial console output; see booted.png)"
                shot = shot or "booted"
                break
        if shot:
            screendump(shot)
    finally:
        q.kill()
        q.wait()
    debug = read(f"{OUT}/debug.log")
    for line in debug.splitlines():
        if line.startswith(("[ INFO]", "[ WARN]", "[ERROR]")) and ("linux" in line or "entry" in line or "start" in line):
            print("  lumen:", line.split("]: ", 1)[-1][:200])
    serial = read(f"{OUT}/serial.log")
    for line in serial.splitlines():
        if "Linux version" in line or "Command line:" in line:
            print("  kernel:", line.strip()[:200])
    if ok and expect and f'linux "{expect}"' not in debug and f"linux {expect!r}" not in debug and f'"{expect}"' not in debug:
        ok, why = False, f"install not named {expect!r}"
    print(("PASS" if ok else "FAIL") + ": " + why)
    sys.exit(0 if ok else 1)


def read(p):
    try:
        return open(p, errors="replace").read()
    except FileNotFoundError:
        return ""


def screendump(name):
    s = socket.socket(socket.AF_UNIX)
    s.connect(f"{OUT}/mon.sock")
    s.sendall(f"screendump {OUT}/{name}.ppm\n".encode())
    time.sleep(2)
    s.close()
    ppm_to_png(f"{OUT}/{name}.ppm", f"{OUT}/{name}.png")


if __name__ == "__main__":
    main()
