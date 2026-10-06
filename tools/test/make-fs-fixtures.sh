#!/bin/sh
# Builds real file system images for Lumen's read-only readers, using the
# genuine mkfs tools in a Debian container, plus a manifest of every file's
# SHA-256 so tests can check each reader byte for byte.
#
#   tools/test/make-fs-fixtures.sh       -> target/fixtures/{ext4,xfs,btrfs,fat32,fat16}.img + manifest.txt
#
# Needs docker (any engine; on macOS e.g. `colima start lumen`). Set
# DOCKER_CONTEXT to choose the engine.
set -eu
cd "$(dirname "$0")/../.."
OUT=target/fixtures
mkdir -p "$OUT"
docker run --rm -i --privileged -v "$PWD/$OUT:/out" debian:trixie sh -eu <<'INNER'
apt-get update -qq >/dev/null
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq e2fsprogs xfsprogs btrfs-progs dosfstools mtools python3 >/dev/null
T=/tree; rm -rf $T; mkdir -p $T
# A boot-like tree: configs, symlinks, a big incompressible "kernel",
# a compressible "initrd", a big directory (hashed/btree directories),
# deep paths, and names with spaces/UTF-8.
mkdir -p $T/boot/grub $T/boot/loader/entries $T/etc $T/usr/lib $T/big $T/deep/a/b/c/d/e
printf 'NAME="Test Linux"\nID=testlinux\nPRETTY_NAME="Test Linux 1.0"\n' > $T/usr/lib/os-release
ln -s ../usr/lib/os-release $T/etc/os-release
ln -s /usr/lib/os-release $T/etc/abs-link
head -c 9000000 /dev/urandom > $T/boot/vmlinuz-6.1.0-test
python3 -c "
import random
r = random.Random(1)
words = [''.join(r.choice('abcdefghij') for _ in range(r.randint(2,9))) for _ in range(500)]
with open('$T/boot/initrd.img-6.1.0-test','w') as f:
    for i in range(400000): f.write(r.choice(words) + (' ' if i % 13 else '\n'))
"
ln -s vmlinuz-6.1.0-test $T/boot/vmlinuz
printf 'menuentry "Test" {\n  linux /boot/vmlinuz-6.1.0-test root=UUID=x ro quiet\n  initrd /boot/initrd.img-6.1.0-test\n}\n' > $T/boot/grub/grub.cfg
printf 'title Test Linux\nversion 6.1.0-test\nlinux /vmlinuz-6.1.0-test\ninitrd /initrd.img-6.1.0-test\noptions root=UUID=x ro\n' > $T/boot/loader/entries/test.conf
for i in $(seq 1 3000); do printf "file %d\n" $i > $T/big/file-$i.txt; done
echo deep > $T/deep/a/b/c/d/e/leaf.txt
printf 'x' > $T/tiny
: > $T/empty
echo "spaces and ü" > "$T/name with spaces ü.txt"
# A sparse file with a hole in the middle.
dd if=/dev/urandom of=$T/sparse bs=4096 count=4 2>/dev/null
dd if=/dev/urandom of=$T/sparse bs=4096 count=4 seek=300 conv=notrunc 2>/dev/null

cd $T
find . -type f | sort | while read f; do printf '%s %s\n' "$(sha256sum "$f" | cut -d' ' -f1)" "${f#.}"; done > /out/manifest.txt
find . -type l | sort | while read f; do printf 'link %s %s\n' "$(readlink "$f")" "${f#.}"; done >> /out/manifest.txt
cd /

mk() { rm -f /out/$1.img; truncate -s $2 /out/$1.img; }
mk ext4 160M; mkfs.ext4 -q -L LUMENEXT4 -d $T /out/ext4.img
# Older layouts: ext2 block maps (no extents), and ext4 with inline data.
mk ext2 160M; mkfs.ext2 -q -L LUMENEXT2 -d $T /out/ext2.img
mk ext4-inline 160M; mkfs.ext4 -q -O inline_data -d $T /out/ext4-inline.img
# xfs can't be populated from a directory, so mount it (needs --privileged).
mk xfs 400M; mkfs.xfs -q -L LUMENXFS /out/xfs.img
mkdir -p /mnt/x && mount -o loop /out/xfs.img /mnt/x && cp -a $T/. /mnt/x/ && umount /mnt/x
# btrfs: zstd for the main tree, plus subvolumes like Ubuntu (@) and
# openSUSE (default subvolume) layouts.
mk btrfs 400M
B=/btrfs-tree; rm -rf $B; mkdir -p $B
cp -a $T $B/@; cp -a $T $B/@zlib
mkfs.btrfs -q -L LUMENBTRFS --rootdir $B --subvol rw:@ --subvol rw:@zlib --compress zstd /out/btrfs.img
mk btrfs-zlib 400M; mkfs.btrfs -q --rootdir $T --compress zlib /out/btrfs-zlib.img
mk btrfs-lzo 400M; mkfs.btrfs -q --rootdir $T --compress lzo /out/btrfs-lzo.img
mk btrfs-plain 400M; mkfs.btrfs -q --rootdir $T /out/btrfs-plain.img
# FAT via mtools (no mounting needed).
for fat in 32 16; do
    mk fat$fat $([ $fat = 32 ] && echo 300M || echo 200M)
    mkfs.fat -F $fat -n LUMENFAT$fat /out/fat$fat.img >/dev/null
    LC_ALL=C.UTF-8 MTOOLS_SKIP_CHECK=1 mcopy -s -i /out/fat$fat.img $T/boot $T/etc $T/usr $T/big $T/deep $T/tiny $T/empty $T/sparse "$T/name with spaces ü.txt" ::/ 2>/dev/null || true
done
for f in /out/*.img; do blkid -o export "$f" | grep -E '^(TYPE|UUID|LABEL)=' | tr '\n' ' '; echo " $(basename $f)"; done > /out/ids.txt
chmod 666 /out/*
INNER
cat "$OUT/ids.txt"
