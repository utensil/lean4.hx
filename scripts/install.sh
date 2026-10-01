#!/bin/sh
# Build and install the native library plus the Steel module into STEEL_HOME.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
steel_home=${STEEL_HOME:-}

if [ -z "$steel_home" ]; then
    echo "STEEL_HOME must point to the Steel installation directory" >&2
    exit 64
fi

cd "$repo_root"
cargo build --release --locked
mkdir -p "$steel_home/native" "$steel_home/cogs"
cp target/release/liblean4_hx.dylib "$steel_home/native/liblean4_hx.dylib"
cp host/extension.scm "$steel_home/cogs/lean4-hx.scm"
printf '%s\n' "installed lean4.hx into $steel_home"
