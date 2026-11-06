#!/usr/bin/env bash
# Builds two packages for Yandex Station Mini 1 into dist/ (WSL Debian or any Debian/Ubuntu x86_64):
#   soundiya-mini1-bot-<date>.tar.gz  soundiya with STATE/JUMP (the fork's latest *-bot release,
#                                     or its branch bot-api before the first release)
#   soundiya-tg-armv7-<date>.tar.gz   this bot
# Usage: bash scripts/build.sh            (from the repository root)
# Override the soundiya source: SOUNDIYA_REPO=... SOUNDIYA_REF=... bash scripts/build.sh
set -Eeuo pipefail
trap 'echo "[error] line $LINENO: $BASH_COMMAND" >&2' ERR

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
readonly ROOT
readonly SOUNDIYA_REPO=${SOUNDIYA_REPO:-https://github.com/genaRijoff/soundiya.git}
latest_bot_release() {
    { git ls-remote --tags --refs "$SOUNDIYA_REPO" 'v*-bot*' 2>/dev/null || true; } \
        | sed 's#.*refs/tags/##' | sort -V | tail -n 1
}
SOUNDIYA_REF=${SOUNDIYA_REF:-$(latest_bot_release)}
readonly SOUNDIYA_REF=${SOUNDIYA_REF:-bot-api}
readonly WORK="${HOME}/.cache/soundiya-tg-build"
readonly OUT="$ROOT/dist"
[[ -f "$ROOT/Cargo.toml" ]] || { echo "run from the soundiya-tg repository"; exit 1; }

echo "== packages"
SUDO=""; [[ $(id -u) -eq 0 ]] || SUDO=sudo
$SUDO apt-get update -qq
$SUDO apt-get install -y -qq build-essential curl git pkg-config python3 xz-utils bzip2 ca-certificates file libasound2-dev >/dev/null

if ! command -v rustup >/dev/null && [[ ! -x "${HOME}/.cargo/bin/rustup" ]]; then
    echo "== Rust (rustup minimal)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"

rm -rf "$WORK"
mkdir -p "$WORK" "$OUT"
TS=$(date +%Y%m%d-%H%M)

echo "== soundiya ($SOUNDIYA_REF)"
git clone -q --branch "$SOUNDIYA_REF" "$SOUNDIYA_REPO" "$WORK/soundiya"
cd "$WORK/soundiya"
git log --oneline -1
chmod +x scripts/*.sh
cargo test --locked --bin soundiya
sh scripts/bootstrap-arm.sh
sh scripts/build-arm.sh
sh scripts/package.sh >/dev/null
cp dist/soundiya-armv7.tar.gz "$OUT/soundiya-mini1-bot-${TS}.tar.gz"

echo "== soundiya-tg"
TOOLCHAIN="${XDG_CACHE_HOME:-$HOME/.cache}/spotiya/gcc-linaro-6.5.0-2018.12-x86_64_arm-linux-gnueabihf"
[[ -x "$TOOLCHAIN/bin/arm-linux-gnueabihf-gcc" ]] || { echo "no ARM toolchain at $TOOLCHAIN"; exit 1; }
cp -r "$ROOT" "$WORK/soundiya-tg"
cd "$WORK/soundiya-tg"
rm -rf dist target
cargo test --locked
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="$TOOLCHAIN/bin/arm-linux-gnueabihf-gcc"
export CC_armv7_unknown_linux_gnueabihf="$TOOLCHAIN/bin/arm-linux-gnueabihf-gcc"
export AR_armv7_unknown_linux_gnueabihf="$TOOLCHAIN/bin/arm-linux-gnueabihf-ar"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS="-C target-cpu=cortex-a53 -C link-arg=-Wl,--gc-sections -C link-arg=-Wl,-z,max-page-size=4096"
cargo build --locked --release --target armv7-unknown-linux-gnueabihf
BIN=target/armv7-unknown-linux-gnueabihf/release/soundiya-tg

# The station has glibc 2.23: the binary must not need anything newer.
python3 - "$TOOLCHAIN/bin/arm-linux-gnueabihf-readelf" "$BIN" <<'PY'
import re, subprocess, sys
text = subprocess.check_output([sys.argv[1], "--version-info", sys.argv[2]], text=True)
versions = set(re.findall(r"\bGLIBC_([0-9][0-9.]*)", text))
too_new = sorted(v for v in versions if tuple(map(int, v.split("."))) > (2, 23))
if not versions or too_new:
    raise SystemExit("glibc > 2.23 required: " + ", ".join(too_new or ["(no versions?)"]))
print("ABI ok: glibc <= 2.23")
PY
file "$BIN"

PKG="$WORK/pkg/soundiya-tg"
mkdir -p "$PKG"
cp "$BIN" "$PKG/soundiya-tg"
cp packaging/install.sh packaging/run.sh packaging/start.sh packaging/stop.sh packaging/S99soundiya-tg "$PKG/"
chmod 755 "$PKG"/*
(cd "$PKG" && sha256sum soundiya-tg install.sh run.sh start.sh stop.sh S99soundiya-tg >SHA256SUMS)
tar -C "$WORK/pkg" -czf "$OUT/soundiya-tg-armv7-${TS}.tar.gz" soundiya-tg

echo "== done:"
ls -l "$OUT"/*"${TS}".tar.gz
