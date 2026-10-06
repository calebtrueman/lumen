#!/usr/bin/env python3
"""Boot Lumen for BIOS in QEMU (SeaBIOS).

  tools/bios/vm.py DISK [DISK...] [--wait S] [--shot NAME] [--key K]... [--mem MB] [--window]

Disks are raw images (first is the boot disk; .qcow2 attached with
snapshot=on). Steps run in order: --wait seconds, --key a QEMU key name
(e.g. ret, right, shift), --shot writes target/bios/NAME.png.
The debug console log (test builds) goes to target/bios/debug.log.
"""
import os, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "target", "bios")
sys.path.insert(0, os.path.join(ROOT, "tools"))
from vm import ppm_to_png  # noqa: E402


def main():
    args = sys.argv[1:]
    takes_value = {"--wait", "--shot", "--key", "--mem"}
    disks = [a for i, a in enumerate(args)
             if not a.startswith("--") and (i == 0 or args[i - 1] not in takes_value)
             and a.endswith((".img", ".qcow2", ".raw")) and os.path.isfile(a)]
    mem = args[args.index("--mem") + 1] if "--mem" in args else "2048"
    os.makedirs(OUT, exist_ok=True)
    sock = f"{OUT}/mon.sock"
    if os.path.exists(sock):
        os.remove(sock)
    log = f"{OUT}/debug.log"
    open(log, "w").close()
    cmd = ["qemu-system-x86_64", "-m", mem, "-vga", "std", "-net", "none",
           "-debugcon", f"file:{log}", "-serial", f"file:{OUT}/serial.log",
           "-monitor", f"unix:{sock},server,nowait"]
    if "--window" not in args:
        cmd += ["-display", "none"]
    for i, d in enumerate(disks):
        fmt = "qcow2" if d.endswith(".qcow2") else "raw"
        snap = ",snapshot=on" if fmt == "qcow2" or "--snapshot" in args else ""
        cmd += ["-drive", f"file={d},format={fmt},if=ide,index={i}{snap}"]
    q = subprocess.Popen(cmd, stderr=subprocess.PIPE)
    for _ in range(50):
        if os.path.exists(sock):
            break
        time.sleep(0.1)
    if q.poll() is not None:
        sys.exit(q.stderr.read().decode())
    mon = socket.socket(socket.AF_UNIX)
    mon.connect(sock)

    def send(c):
        try:
            mon.sendall((c + "\n").encode())
        except OSError:
            pass  # the VM has gone (e.g. powered off)
        time.sleep(0.3)

    i = 0
    while i < len(args):
        a = args[i]
        if a == "--wait":
            time.sleep(float(args[i + 1])); i += 1
        elif a == "--key":
            send(f"sendkey {args[i + 1]}"); i += 1
        elif a == "--shot":
            name = args[i + 1]; i += 1
            send(f"screendump {OUT}/{name}.ppm")
            time.sleep(1)
            ppm_to_png(f"{OUT}/{name}.ppm", f"{OUT}/{name}.png")
            print("shot", f"{OUT}/{name}.png")
        i += 1
    if "--window" in args:
        q.wait()
    if q.poll() is not None:
        print("vm: powered off by itself")
    q.kill()
    q.wait()
    print(open(log, errors="replace").read()[-3000:])


if __name__ == "__main__":
    main()
