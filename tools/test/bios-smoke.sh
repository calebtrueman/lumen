#!/bin/sh
# Lumen for BIOS under SeaBIOS (QEMU), with fake operating systems whose
# boot code prints a word to QEMU's debug console:
#  - the menu comes up; Windows on the same disk starts through its
#    original MBR, Windows on a second disk through its boot sector
#  - a damaged payload falls back to the original MBR
#  - Shut Down powers the PC off
set -eu
cd "$(dirname "$0")/../.."
LUMEN_FEATURES=debugcon tools/bios/build.sh
B=target/bios
INSTALL="cargo run -q -p lumen-biosinstall --"
[ -n "${HOST_TARGET:-}" ] && INSTALL="cargo run -q -p lumen-biosinstall --target $HOST_TARGET --"
fail=0
check() { if echo "$2" | grep -aq "$3"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }

python3 tools/bios/fakeos.py windows-disk $B/win-a.img
python3 tools/bios/fakeos.py windows-disk $B/win-b.img
$INSTALL install $B/win-a.img $B/lumen-bios.img
out=$(python3 tools/bios/vm.py $B/win-a.img $B/win-b.img --wait 20 --shot smoke-menu --key ret --wait 4 2>&1)
check "menu with 2 entries" "$out" "lumen: 2 entries"
check "Windows (disk 1) via its original MBR" "$out" "WINDOWS-MBR"
out=$(python3 tools/bios/vm.py $B/win-a.img $B/win-b.img --wait 20 --key right --key ret --wait 4 2>&1)
check "Windows (disk 2) via its boot sector" "$out" "WINDOWS-VBR"

cp $B/win-a.img $B/win-damaged.img
python3 - <<'PY'
import struct
f = open("target/bios/win-damaged.img", "r+b")
base, = struct.unpack_from("<Q", f.read(512), 0x1B0)
f.seek((base + 3 + 40) * 512); f.write(b"\xde\xad" * 256)
PY
out=$(python3 tools/bios/vm.py $B/win-damaged.img --wait 15 2>&1)
check "damaged payload falls back to the original MBR" "$out" "WINDOWS-MBR"

out=$(python3 tools/bios/vm.py $B/win-a.img --wait 20 --key tab --key right --key right --key ret --wait 8 2>&1)
check "Shut Down powers off (ACPI)" "$out" "powered off by itself"

$INSTALL uninstall $B/win-a.img
python3 tools/bios/fakeos.py windows-disk $B/win-ref.img
cmp -n 512 $B/win-a.img $B/win-ref.img && echo "PASS uninstall restored the MBR" || { echo "FAIL uninstall"; fail=1; }
exit $fail
