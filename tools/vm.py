#!/usr/bin/env python3
"""Boot Lumen in QEMU/OVMF with a simulated multi-disk, multi-OS machine.

  tools/vm.py                 # interactive window
  tools/vm.py --headless ...  # scripted: --shot NAME, --key KEY, --wait SECS
"""
import os, shutil, socket, struct, subprocess, sys, time, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "target", "vm")
REL = os.path.join(ROOT, "target", "x86_64-unknown-uefi", "release")
# Build with: cargo efi-x64 --features debugcon && cargo build --release --examples
SHARE = "/opt/homebrew/share/qemu"


SB = "--secureboot" in sys.argv
MOK = next((a.split("=", 1)[1] for a in sys.argv if a.startswith("--mok=")), "enrolled")
DIST = os.path.join(ROOT, "dist", "x86_64")
DEB = os.environ.get("LUMEN_TEST_DEBS")  # dir with Debian-signed grubx64.efi for SB chain tests
VFV = os.path.expanduser("~/Library/Python/3.9/bin/virt-fw-vars")


def stage_secureboot(put):
    """Secure Boot on, Microsoft keys only. Lumen boots through shim."""
    put("disk0", "EFI/BOOT/BOOTX64.EFI", f"{DIST}/shimx64.efi")
    put("disk0", "EFI/BOOT/mmx64.efi", f"{DIST}/mmx64.efi")
    put("disk0", "EFI/BOOT/grubx64.efi", f"{DIST}/lumen.efi")
    put("disk0", "EFI/BOOT/lumen.cer", f"{DIST}/lumen.cer")
    with open(os.path.join(OUT, "disk0", "EFI", "BOOT", "lumen.conf"), "w") as f:
        f.write("timeout -1\n")
    fake = os.path.join(REL, "examples", "fake_os.efi")
    put("disk0", "EFI/fedora/grubx64.efi", fake)  # unsigned: must be refused
    put("disk1", "EFI/Microsoft/Boot/bootmgfw.efi", fake)  # unsigned stand-in
    if DEB:
        put("disk0", "EFI/debian/shimx64.efi", f"{DIST}/shimx64.efi")
        put("disk0", "EFI/debian/grubx64.efi", f"{DEB}/grubx64.efi")
    # --sb-off: Microsoft keys enrolled but Secure Boot switched off, as on a
    # PC whose owner can turn it on later.
    cmd = [VFV, "--input", f"{SHARE}/edk2-i386-vars.fd", "--output", f"{OUT}/vars.fd",
           "--enroll-microsoft", "--microsoft-db", "all"] + ([] if "--sb-off" in sys.argv else ["--sb"])
    if MOK == "enrolled":
        cmd += ["--add-mok", "605dab50-e046-4300-abb6-3dd810dd8b23", f"{DIST}/lumen.cer"]
    elif MOK == "queued":
        cmd += ["--set-json", f"{OUT}/mok.json"]
    subprocess.check_call(cmd, stdout=subprocess.DEVNULL)


def stage():
    shutil.rmtree(OUT, ignore_errors=True)
    os.makedirs(OUT)
    lumen = os.path.join(ROOT, "target", "x86_64-lumen-uefi", "release", "lumen.efi")
    fake = os.path.join(REL, "examples", "fake_os.efi")

    def put(disk, path, src):
        dst = os.path.join(OUT, disk, *path.split("/"))
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy(src, dst)

    # USB sticks for hot-plug tests (attached later with --cmd drive_add/device_add).
    # Debian live USB: blank label, only identifiable from its shim's CA.
    shim = os.path.join(ROOT, "vendor", "shim", "x86_64", "shimx64.efi")
    if os.path.exists(shim):
        put("usb_debian", "EFI/BOOT/BOOTX64.EFI", shim)
        put("usb_debian", "EFI/BOOT/grubx64.efi", os.path.join(REL, "examples", "fake_os.efi"))
    # Bazzite live USB: Fedora-based, so it also carries \EFI\fedora; it must
    # still show up as one "Bazzite (USB)" card, not as Fedora too.
    put("usb_bazzite", "EFI/BOOT/BOOTX64.EFI", os.path.join(REL, "examples", "fake_os.efi"))
    put("usb_bazzite", "EFI/fedora/shimx64.efi", os.path.join(REL, "examples", "fake_os.efi"))
    with open(os.path.join(OUT, "usb_bazzite", "EFI", "BOOT", "grub.cfg"), "w") as f:
        f.write("menuentry 'Install Bazzite' --class fedora {\n  linux /images/pxeboot/vmlinuz\n}\n")
    # Arch ISO: systemd-boot with an entry titled "Arch Linux install medium".
    put("usb_arch", "EFI/BOOT/BOOTX64.EFI", os.path.join(REL, "examples", "fake_os.efi"))
    os.makedirs(os.path.join(OUT, "usb_arch", "loader", "entries"))
    with open(os.path.join(OUT, "usb_arch", "loader", "entries", "01-archiso-linux.conf"), "w") as f:
        f.write("title    Arch Linux install medium (x86_64, UEFI)\nlinux    /arch/boot/x86_64/vmlinuz-linux\n")

    if "--demo" in sys.argv:
        # A tidy, realistic machine for README screenshots, at 1080p.
        put("disk0", "EFI/BOOT/BOOTX64.EFI", lumen)
        for d in ("ubuntu", "fedora"):
            put("disk0", f"EFI/{d}/shimx64.efi", fake)
        put("disk0", "EFI/Linux/arch-linux.efi", fake)
        with open(os.path.join(OUT, "disk0", "EFI", "BOOT", "lumen.conf"), "w") as f:
            f.write("timeout 30\nresolution 1920x1080\n")
        put("disk1", "EFI/Microsoft/Boot/bootmgfw.efi", fake)
        shutil.copy(os.path.join(SHARE, "edk2-i386-vars.fd"), os.path.join(OUT, "vars.fd"))
        return
    if SB:
        if MOK == "queued":
            subprocess.check_call([sys.executable, os.path.join(ROOT, "tools", "mok_request.py"),
                                   f"{DIST}/lumen.cer", "lumen", f"{OUT}/mok.json"])
        stage_secureboot(put)
        return
    # Disk 0: Linux drive whose ESP also holds Lumen. The fallback loader
    # registers NVRAM boot entries (like an OS installer) then starts Lumen.
    put("disk0", "EFI/BOOT/BOOTX64.EFI", os.path.join(REL, "examples", "register_boot.efi"))
    put("disk0", "EFI/lumen/lumen.efi", lumen)
    put("disk0", "EFI/ubuntu/shimx64.efi", fake)
    put("disk0", "EFI/ubuntu/grubx64.efi", fake)
    put("disk0", "EFI/fedora/grubx64.efi", fake)
    put("disk0", "EFI/Linux/arch-linux.efi", fake)
    with open(os.path.join(OUT, "disk0", "EFI", "lumen", "lumen.conf"), "w") as f:
        f.write("timeout -1\ndebug on\nentry Arch (fallback) | \\EFI\\Linux\\arch-linux.efi | initrd=\\initramfs-fallback.img\n")
    # Disk 1: a separate Windows drive with its own ESP.
    put("disk1", "EFI/Microsoft/Boot/bootmgfw.efi", fake)
    put("disk1", "EFI/Boot/bootx64.efi", fake)
    # A loader only the firmware's boot menu knows about.
    put("disk1", "EFI/Custom/Odd/boot.efi", fake)

    if "--mouse-test" in sys.argv:
        i = sys.argv.index("--mouse-test")
        with open(os.path.join(OUT, "disk0", "mouse-test"), "w") as f:
            f.write(f"{sys.argv[i + 1]} {sys.argv[i + 2]}")
    if "--heal-test" in sys.argv:
        open(os.path.join(OUT, "disk0", "heal-test"), "w").close()

    shutil.copy(os.path.join(SHARE, "edk2-i386-vars.fd"), os.path.join(OUT, "vars.fd"))


def qemu_cmd(headless):
    code = "edk2-x86_64-secure-code.fd" if SB else "edk2-x86_64-code.fd"
    cmd = [
        "qemu-system-x86_64", "-machine", "q35,smm=on" if SB else "q35", "-m", "512",
        "-global", "driver=cfi.pflash01,property=secure,value=on",
        "-drive", f"if=pflash,format=raw,readonly=on,file={SHARE}/{code}",
        "-drive", f"if=pflash,format=raw,file={OUT}/vars.fd",
        "-drive", f"file=fat:rw:{OUT}/disk0,format=raw,if=none,id=d0", "-device", "ide-hd,drive=d0,bootindex=0",
        "-drive", f"file=fat:rw:{OUT}/disk1,format=raw,if=none,id=d1", "-device", "nvme,drive=d1,serial=WINDISK",
        "-net", "none", "-vga", "std",
        "-device", "qemu-xhci,id=xhci", "-device", "usb-mouse,bus=xhci.0",
        "-debugcon", f"file:{OUT}/debug.log", "-global", "isa-debugcon.iobase=0xe9",
    ]
    if headless:
        cmd += ["-display", "none", "-monitor", f"unix:{OUT}/mon.sock,server,nowait"]
    return cmd


def ppm_to_png(ppm, png):
    data = open(ppm, "rb").read()
    parts = data.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    pix = parts[3]
    raw = b"".join(b"\x00" + pix[y * w * 3:(y + 1) * w * 3] for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    with open(png, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def main():
    args = sys.argv[1:]
    # --reuse: boot the previous run's disks and firmware variables again.
    # --enable-sb: switch Secure Boot on in those variables first.
    if "--reuse" not in args:
        stage()
    if "--enable-sb" in args:
        subprocess.check_call([VFV, "--input", f"{OUT}/vars.fd", "--output", f"{OUT}/vars.fd", "--set-true", "SecureBootEnable"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if "--headless" not in args:
        subprocess.call(qemu_cmd(False))
        return
    if os.path.exists(f"{OUT}/mon.sock"):
        os.remove(f"{OUT}/mon.sock")  # stale socket from a previous (--reuse) run
    proc = subprocess.Popen(qemu_cmd(True))
    try:
        for _ in range(50):
            if os.path.exists(f"{OUT}/mon.sock"):
                break
            time.sleep(0.1)
        mon = socket.socket(socket.AF_UNIX)
        mon.connect(f"{OUT}/mon.sock")
        def send(c):
            mon.sendall((c + "\n").encode())
            time.sleep(0.3)
        it = iter(args)
        for a in it:
            if a.startswith("--mok=") or a in ("--secureboot", "--headless", "--heal-test", "--demo", "--sb-off", "--reuse", "--enable-sb"):
                continue
            if a == "--mouse-test":
                next(it), next(it)
                continue
            if a == "--wait":
                time.sleep(float(next(it)))
            elif a == "--cmd":
                send(next(it))
            elif a == "--plug":  # hot-plug a staged USB stick: --plug usb_debian
                n = next(it)
                send(f"drive_add 0 if=none,id={n},file=fat:rw:{OUT}/{n},format=raw")
                send(f"device_add usb-storage,bus=xhci.0,drive={n},id=dev_{n},removable=on")
            elif a == "--key":
                send(f"sendkey {next(it)}")
            elif a == "--shot":
                name = next(it)
                ppm = f"{OUT}/{name}.ppm"
                send(f"screendump {ppm}")
                time.sleep(1.0)
                ppm_to_png(ppm, f"{OUT}/{name}.png")
                print("shot", f"{OUT}/{name}.png")
    finally:
        # Quit cleanly so firmware variable writes reach vars.fd (a kill can
        # drop them).
        try:
            mon.sendall(b"quit\n")
            proc.wait(timeout=10)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    main()
