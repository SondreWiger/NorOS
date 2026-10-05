#!/bin/bash
# Boots the built image in QEMU, waits for the desktop and takes screenshots.
# Usage: ci/boot-test.sh x86_64|arm64 [live|install]
#   live     boot the ISO and try the desktop and apps (default)
#   install  boot the ISO, install onto a blank virtual disk, then boot the installed system
set -uo pipefail

ARCH="$1"
MODE="${2:-live}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/out/boot-test"
ISO=$(ls "$ROOT"/out/noros-*-"$ARCH".iso | head -n1)
mkdir -p "$OUT"
cd "$OUT"

ACCEL=tcg
if [ -w /dev/kvm ]; then ACCEL=kvm; fi
echo "Booting $ARCH with $ACCEL"

case "$ARCH" in
    x86_64)
        VARS_TEMPLATE=/usr/share/OVMF/OVMF_VARS_4M.fd
        cp "$VARS_TEMPLATE" vars.fd
        QEMU=(qemu-system-x86_64 -machine q35,accel=$ACCEL
              -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd
              -drive if=pflash,format=raw,file=vars.fd
              -device virtio-vga,xres=1440,yres=900)
        [ "$ACCEL" = kvm ] && QEMU+=(-cpu host)
        ;;
    arm64)
        VARS_TEMPLATE=/usr/share/AAVMF/AAVMF_VARS.fd
        cp "$VARS_TEMPLATE" vars.fd
        QEMU=(qemu-system-aarch64 -machine virt,accel=$ACCEL
              -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.fd
              -drive if=pflash,format=raw,file=vars.fd
              -device virtio-gpu-pci,xres=1440,yres=900)
        if [ "$ACCEL" = kvm ]; then QEMU+=(-cpu host); else QEMU+=(-cpu max); fi
        ;;
    *) echo "unknown arch $ARCH"; exit 2 ;;
esac

COMMON=(-m "${NOROS_TEST_MEM:-4096}" -smp 4
        -device qemu-xhci -device usb-kbd -device usb-tablet
        -display none
        -qmp unix:qmp.sock,server=on,wait=off)
CDROM=(-device virtio-scsi-pci,id=scsi
       -drive if=none,id=cd,media=cdrom,readonly=on,format=raw,file="$ISO"
       -device scsi-cd,drive=cd,bus=scsi.0)

# Software emulation (no KVM) is much slower to boot.
TIMEOUT=240
[ "$ACCEL" = tcg ] && TIMEOUT=900

boot() {  # boot <phase> <serial log> <extra qemu args...>
    local phase="$1" log="$2"; shift 2
    rm -f qmp.sock
    "${QEMU[@]}" "${COMMON[@]}" -serial "file:$log" "$@" &
    QEMU_PID=$!
    python3 "$ROOT/ci/qmp_test.py" --phase "$phase" --timeout "$TIMEOUT" --serial "$log" --socket qmp.sock --out "$OUT"
    local status=$?
    python3 "$ROOT/ci/qmp_test.py" --phase quit --socket qmp.sock --serial "$log" --out "$OUT" >/dev/null 2>&1
    kill "$QEMU_PID" 2>/dev/null
    wait "$QEMU_PID" 2>/dev/null
    echo "--- last 40 lines of $log ---"
    tail -n 40 "$log" || true
    return $status
}

case "$MODE" in
    live)
        # A network, so the Vakt firewall has something to guard.
        boot live serial.log "${CDROM[@]}" -netdev user,id=net0 -device virtio-net-pci,netdev=net0,romfile= \
            -audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0
        exit $?
        ;;
    debug)
        boot debug serial.log "${CDROM[@]}" -netdev user,id=net0 -device virtio-net-pci,netdev=net0,romfile=
        exit $?
        ;;
    install)
        rm -f disk.qcow2
        qemu-img create -q -f qcow2 disk.qcow2 16G
        DISK=(-drive file=disk.qcow2,format=qcow2,if=virtio)
        boot install serial-install.log "${CDROM[@]}" "${DISK[@]}" -nic none || exit 1
        # Start the installed system with fresh firmware settings and no ISO attached.
        cp "$VARS_TEMPLATE" vars.fd
        boot installed serial-installed.log "${DISK[@]}" -nic none
        status=$?
        rm -f disk.qcow2
        exit $status
        ;;
esac
