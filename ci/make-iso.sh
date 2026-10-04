#!/bin/bash
# Turns the built image into a live ISO that boots from CD or USB on UEFI machines.
# The root filesystem is read-only on the disc; changes live in RAM (systemd.volatile=overlay).
# Runs inside the privileged build container after mkosi.
set -euxo pipefail

cd "$(dirname "$0")/.."
VERSION=0.1
case "$(uname -m)" in
    x86_64)  ARCH=x86_64; EFI_NAME=BOOTX64.EFI;  BOOT_STUB=systemd-bootx64.efi ;;
    aarch64) ARCH=arm64;  EFI_NAME=BOOTAA64.EFI; BOOT_STUB=systemd-bootaa64.efi ;;
esac

ROOT_PART=$(ls out/noros.root-*.raw | head -n1)
WORK=out/iso
rm -rf "$WORK" && mkdir -p "$WORK/esp" /mnt/noros
mount -o loop,ro "$ROOT_PART" /mnt/noros
trap 'umount /mnt/noros || true' EXIT

# EFI boot partition: systemd-boot, kernel, initrd and one boot entry.
ESP="$WORK/esp"
install -D /mnt/noros/usr/lib/systemd/boot/efi/$BOOT_STUB "$ESP/EFI/BOOT/$EFI_NAME"
install -D out/noros.vmlinuz "$ESP/noros/vmlinuz"
install -D out/noros.initrd  "$ESP/noros/initrd"
install -d "$ESP/loader/entries"
cat > "$ESP/loader/loader.conf" <<CONF
timeout 0
default noros.conf
CONF
cat > "$ESP/loader/entries/noros.conf" <<CONF
title   NorOS $VERSION (live)
linux   /noros/vmlinuz
initrd  /noros/initrd
options root=LABEL=NOROS_LIVE rootfstype=iso9660 ro systemd.volatile=overlay quiet loglevel=3 systemd.show_status=auto rd.udev.log_level=3
CONF

SIZE_KB=$(( $(du -sk "$ESP" | cut -f1) + 8192 ))
mkfs.vfat -C -n NOROS_EFI "$WORK/efiboot.img" "$SIZE_KB"
mcopy -s -i "$WORK/efiboot.img" "$ESP"/* ::/

ISO="out/noros-$VERSION-$ARCH.iso"
xorriso -as mkisofs \
    -iso-level 3 -R -V NOROS_LIVE \
    -o "$ISO" \
    -append_partition 2 0xef "$WORK/efiboot.img" -appended_part_as_gpt \
    -e --interval:appended_partition_2:all:: -no-emul-boot \
    /mnt/noros

ls -lh "$ISO"
