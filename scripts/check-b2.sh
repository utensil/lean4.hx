#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
pin_file=$repo_root/HOST-PINS.toml
host_tmp=${TMPDIR:-/tmp}
helix_dir=${B2_HELIX_DIR:-$host_tmp/lean4hx-helix}
steel_dir=${B2_STEEL_DIR:-$host_tmp/lean4hx-steel}

pin_value() {
    section=$1
    key=$2
    awk -v wanted_section="[$section]" -v wanted_key="$key" '
        $0 == wanted_section { active = 1; next }
        /^\[/ { active = 0 }
        active && $1 == wanted_key { sub(/^[^\"]*\"/, ""); sub(/\".*$/, ""); print; exit }
    ' "$pin_file"
}

expected_helix=$(pin_value helix commit)
expected_steel=$(pin_value steel commit)
[ -d "$helix_dir/.git" ] || { echo "set B2_HELIX_DIR to a detached pinned Helix checkout" >&2; exit 1; }
[ -d "$steel_dir/.git" ] || { echo "set B2_STEEL_DIR to a detached pinned Steel checkout" >&2; exit 1; }
actual_helix=$(git -C "$helix_dir" rev-parse HEAD)
actual_steel=$(git -C "$steel_dir" rev-parse HEAD)
[ "$actual_helix" = "$expected_helix" ] || { echo "Helix checkout is $actual_helix, expected $expected_helix" >&2; exit 1; }
[ "$actual_steel" = "$expected_steel" ] || { echo "Steel checkout is $actual_steel, expected $expected_steel" >&2; exit 1; }

grep -F 'rev = "24cd21598c091fb88bc10a6375a1ded25e677c37"' "$repo_root/Cargo.toml" >/dev/null
grep -F 'selection-did-change' "$repo_root/b2/b2.scm" >/dev/null
grep -F 'post-insert-char' "$repo_root/b2/b2.scm" >/dev/null
grep -F '$/lean/plainGoal' "$repo_root/b2/b2.scm" >/dev/null
grep -F 'new-component!' "$repo_root/b2/b2.scm" >/dev/null

cd "$repo_root"
cargo check --lib
echo "B2 Rust cdylib check passed against Steel $actual_steel"
echo "Live Helix/Lean interaction remains an operator probe; see b2/README.md"
