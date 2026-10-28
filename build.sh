#!/bin/zsh
# Builds the Windows app and its installer on macOS, into dist/Snowdrift-Setup.exe or the folder given.
# The installer carries the app, so the app is built first and handed to it.
# Needs: rustup target add x86_64-pc-windows-msvc, cargo install cargo-xwin.
set -euo pipefail

# Relative to where the script was called from, so release.sh can pass its own dist.
out=${1:-$(dirname "$0")/../dist}
out=${out:A}
cd "$(dirname "$0")"
target=x86_64-pc-windows-msvc
release=$PWD/target/$target/release

cargo xwin build --release --target $target --bin Snowdrift
SNOWDRIFT_APP=$release/Snowdrift.exe cargo xwin build --release --target $target --bin Snowdrift-Setup --features setup
mkdir -p "$out"
cp "$release/Snowdrift-Setup.exe" "$out/Snowdrift-Setup.exe"
echo "$out/Snowdrift-Setup.exe"
