#!/bin/bash
# Boots the built image in QEMU, waits for the desktop and takes screenshots.
# Usage: ci/boot-test.sh x86_64|arm64
set -uo pipefail

ARCH="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/out/boot-test"
IMAGE="$ROOT/out/noros.raw"
mkdir -p "$OUT"
cd "$OUT"

ACCEL=tcg
if [ -w /dev/kvm ]; then ACCEL=kvm; fi
echo "Booting $ARCH with $ACCEL"

case "$ARCH" in
    x86_64)
        cp /usr/share/OVMF/OVMF_VARS_4M.fd vars.fd
        CONSOLE="ttyS0,,115200"
        QEMU=(qemu-system-x86_64 -machine q35,accel=$ACCEL
              -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd
              -drive if=pflash,format=raw,file=vars.fd
              -device virtio-vga,xres=1440,yres=900)
        [ "$ACCEL" = kvm ] && QEMU+=(-cpu host)
        ;;
    arm64)
        cp /usr/share/AAVMF/AAVMF_VARS.fd vars.fd
        CONSOLE="ttyAMA0,,115200"
        QEMU=(qemu-system-aarch64 -machine virt,accel=$ACCEL
              -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.fd
              -drive if=pflash,format=raw,file=vars.fd
              -device virtio-gpu-pci,xres=1440,yres=900)
        if [ "$ACCEL" = kvm ]; then QEMU+=(-cpu host); else QEMU+=(-cpu max); fi
        ;;
    *) echo "unknown arch $ARCH"; exit 2 ;;
esac

QEMU+=(-m 4096 -smp 4
       -drive file="$IMAGE",format=raw,if=virtio
       -device qemu-xhci -device usb-kbd -device usb-tablet
       -smbios "type=11,value=io.systemd.stub.kernel-cmdline-extra=console=$CONSOLE systemd.journald.forward_to_console=1"
       -serial file:serial.log
       -display none
       -qmp unix:qmp.sock,server=on,wait=off)

"${QEMU[@]}" &
QEMU_PID=$!

# Software emulation (no KVM) is much slower to boot.
TIMEOUT=240
[ "$ACCEL" = tcg ] && TIMEOUT=900

python3 "$ROOT/ci/qmp_test.py" --timeout "$TIMEOUT" --serial serial.log --socket qmp.sock --out "$OUT"
STATUS=$?

kill "$QEMU_PID" 2>/dev/null
wait "$QEMU_PID" 2>/dev/null
echo "--- last 80 lines of serial log ---"
tail -n 80 serial.log || true
exit $STATUS
