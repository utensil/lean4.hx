#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
host_tmp=${TMPDIR:-/tmp}
helix_dir=${HOST_HELIX_DIR:-$host_tmp/lean4hx-helix}
steel_dir=${HOST_STEEL_DIR:-$host_tmp/lean4hx-steel}
expected_helix=df595c7dc5729e2712c79dd2e35977e3474b3ec6
expected_steel=24cd21598c091fb88bc10a6375a1ded25e677c37
[ -d "$helix_dir/.git" ] || { echo "set HOST_HELIX_DIR to a detached pinned Helix checkout" >&2; exit 1; }
[ -d "$steel_dir/.git" ] || { echo "set HOST_STEEL_DIR to a detached pinned Steel checkout" >&2; exit 1; }
actual_helix=$(git -C "$helix_dir" rev-parse HEAD)
actual_steel=$(git -C "$steel_dir" rev-parse HEAD)
[ "$actual_helix" = "$expected_helix" ] || { echo "Helix checkout is $actual_helix, expected $expected_helix" >&2; exit 1; }
[ "$actual_steel" = "$expected_steel" ] || { echo "Steel checkout is $actual_steel, expected $expected_steel" >&2; exit 1; }

grep -F 'rev = "24cd21598c091fb88bc10a6375a1ded25e677c37"' "$repo_root/Cargo.toml" >/dev/null
grep -F 'selection-did-change' "$repo_root/host/extension.scm" >/dev/null
grep -F 'document-changed' "$repo_root/host/extension.scm" >/dev/null
grep -F '$/lean/plainGoal' "$repo_root/host/extension.scm" >/dev/null
grep -F 'new-component!' "$repo_root/host/extension.scm" >/dev/null

cd "$repo_root"
cargo check --lib
echo "Rust host check passed against Steel $actual_steel"
echo "Live Helix/Lean interaction remains an operator probe; see README.md"
