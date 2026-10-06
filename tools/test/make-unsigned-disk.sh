#!/bin/sh
# A GPT disk with one ext4 "Linux" install whose kernel is an unsigned EFI
# program (Lumen's fake_os example): under Secure Boot Lumen must refuse to
# start it. -> target/fixtures/unsigned-linux.img
set -eu
cd "$(dirname "$0")/../.."
cargo build -q -p lumen --release --examples
mkdir -p target/fixtures/unsigned
cp target/x86_64-unknown-uefi/release/examples/fake_os.efi target/fixtures/unsigned/kernel.efi
docker run --rm -i -v "$PWD/target/fixtures:/out" debian:trixie sh -eu <<'INNER'
apt-get update -qq >/dev/null
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq e2fsprogs fdisk >/dev/null
T=/tree; mkdir -p $T/boot/grub $T/etc
cp /out/unsigned/kernel.efi $T/boot/vmlinuz-9.9.9-unsigned
echo initrd > $T/boot/initrd.img-9.9.9-unsigned
printf 'NAME="Unsigned Linux"\nID=unsignedlinux\n' > $T/etc/os-release
printf 'menuentry "Unsigned Linux" {\n search --fs-uuid --set=root 11111111-2222-3333-4444-555555555555\n linux /boot/vmlinuz-9.9.9-unsigned root=UUID=11111111-2222-3333-4444-555555555555 ro\n initrd /boot/initrd.img-9.9.9-unsigned\n}\n' > $T/boot/grub/grub.cfg
truncate -s 64M /root.img
mkfs.ext4 -q -U 11111111-2222-3333-4444-555555555555 -d $T /root.img
rm -f /out/unsigned-linux.img; truncate -s 70M /out/unsigned-linux.img
printf 'label: gpt\nstart=2048, size=131072, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4\n' | sfdisk -q /out/unsigned-linux.img
dd if=/root.img of=/out/unsigned-linux.img bs=512 seek=2048 conv=notrunc status=none
chmod 666 /out/unsigned-linux.img
INNER
echo "target/fixtures/unsigned-linux.img"
