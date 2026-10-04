#!/bin/bash
# Builds Fjord, noros-shell and the bootable NorOS disk image.
# Runs inside a privileged debian:trixie container (see .github/workflows/build.yml).
set -euxo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config \
    libudev-dev libinput-dev libseat-dev libgbm-dev libdrm-dev libxkbcommon-dev \
    libgtk-4-dev libgtk4-layer-shell-dev \
    mkosi systemd-ukify systemd-repart systemd-boot-efi \
    apt debian-archive-keyring dosfstools mtools e2fsprogs cpio zstd kmod python3 xorriso grub-common "grub-efi-$(dpkg --print-architecture)-bin"

# Rust toolchain (cached between runs via RUSTUP_HOME / CARGO_HOME).
if ! command -v cargo >/dev/null && [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
fi
export PATH="$CARGO_HOME/bin:$PATH"
rustup update stable --no-self-update
cargo --version

cargo build --release
TARGET="${CARGO_TARGET_DIR:-target}"

# Stage our files into an extra tree for the image.
rm -rf out/stage
install -Dm755 "$TARGET"/release/fjord        out/stage/usr/bin/fjord
install -Dm755 "$TARGET"/release/noros-shell  out/stage/usr/bin/noros-shell
for app in noros-settings noros-files noros-text; do
    install -Dm755 "$TARGET/release/$app" "out/stage/usr/bin/$app"
done
install -Dm644 shell/theme/noros.css        out/stage/usr/share/noros/theme/noros.css

cd image
mkosi --extra-tree="$PWD/../out/stage" --force build
cd ..
bash ci/make-iso.sh
# The ISO is the deliverable; drop the large intermediate images.
rm -rf out/noros.raw out/noros out/iso
ls -lh out
