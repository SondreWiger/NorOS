#!/bin/bash
# Turns the built image into a live ISO that boots from CD or USB on UEFI machines.
#
# Layout: a small EFI partition holds only GRUB; GRUB finds the disc by its
# label and loads the kernel and initrd from the ISO filesystem. The root
# filesystem is read-only on the disc; changes live in RAM (systemd.volatile=overlay).
# Runs inside the privileged build container after mkosi.
set -euxo pipefail

cd "$(dirname "$0")/.."
VERSION=0.4
LABEL=NOROS_LIVE
case "$(uname -m)" in
    x86_64)  ARCH=x86_64; EFI_NAME=BOOTX64.EFI;  GRUB_TARGET=x86_64-efi ;;
    aarch64) ARCH=arm64;  EFI_NAME=BOOTAA64.EFI; GRUB_TARGET=arm64-efi ;;
esac

WORK=out/iso
rm -rf "$WORK" && mkdir -p "$WORK"
if [ -d out/noros ]; then
    # Directory build: the tree is the root filesystem as-is.
    ROOTFS=out/noros
else
    # Disk image build: mount the root partition (partition 2).
    mkdir -p /mnt/noros
    LOOP=$(losetup --find --show --read-only --partscan out/noros.raw)
    trap 'umount /mnt/noros || true; losetup -d "$LOOP" || true' EXIT
    udevadm settle || sleep 2
    mount -o ro "${LOOP}p2" /mnt/noros
    ROOTFS=/mnt/noros
fi

cat > "$WORK/grub.cfg" <<CFG
search --no-floppy --label $LABEL --set=root
set timeout=0
menuentry "NorOS $VERSION (live)" {
    linux /live/vmlinuz root=LABEL=$LABEL rootfstype=iso9660 ro systemd.volatile=overlay quiet loglevel=3 systemd.show_status=auto rd.udev.log_level=3
    initrd /live/initrd
}
CFG

grub-mkstandalone -O "$GRUB_TARGET" -o "$WORK/$EFI_NAME" \
    --locales= --fonts= --themes= \
    --modules="part_gpt part_msdos iso9660 fat search search_label linux normal configfile echo" \
    "boot/grub/grub.cfg=$WORK/grub.cfg"

# EFI partition: just GRUB, small enough for CD (El Torito) firmware limits.
SIZE_KB=$(( $(du -k "$WORK/$EFI_NAME" | cut -f1) + 2048 ))
mkfs.vfat -C -n NOROS_EFI "$WORK/efiboot.img" "$SIZE_KB"
mmd -i "$WORK/efiboot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$WORK/efiboot.img" "$WORK/$EFI_NAME" ::/EFI/BOOT/

ISO="out/noros-$VERSION-$ARCH.iso"
xorriso -as mkisofs \
    -iso-level 3 -R -V "$LABEL" \
    -o "$ISO" \
    -append_partition 2 0xef "$WORK/efiboot.img" -appended_part_as_gpt \
    -e --interval:appended_partition_2:all:: -no-emul-boot \
    -graft-points \
    -m "$ROOTFS/boot/EFI" -m "$ROOTFS/efi" \
    /="$ROOTFS" \
    /live/vmlinuz=out/noros.vmlinuz \
    /live/initrd=out/noros.initrd

ls -lh "$ISO"
