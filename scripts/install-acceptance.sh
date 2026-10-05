#!/bin/sh
# Verify install, downgrade, pin metadata, and rollback in a project-owned fixture.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
run_dir=${LEAN4_HX_INSTALL_ACCEPTANCE_DIR:-$repo_root/.acceptance-install}
steel_home=$run_dir/steel
old_native=$run_dir/old-native
old_cog=$run_dir/old-cog

rm -rf "$run_dir"
mkdir -p "$steel_home/native" "$steel_home/cogs"
printf '%s\n' old-native >"$old_native"
printf '%s\n' old-cog >"$old_cog"
cp "$old_native" "$steel_home/native/liblean4_hx.dylib"
cp "$old_cog" "$steel_home/cogs/lean4-hx.scm"

grep -F 'helix_commit=87ec539d136c6cd83aeb82697e66de9706d7af4a' "$repo_root/scripts/host-harness.sh" >/dev/null
grep -F 'rev = "24cd21598c091fb88bc10a6375a1ded25e677c37"' "$repo_root/Cargo.toml" >/dev/null

STEEL_HOME=$steel_home "$repo_root/scripts/install.sh"
[ -s "$steel_home/native/liblean4_hx.dylib" ]
[ -s "$steel_home/cogs/lean4-hx.scm" ]
grep -F 'steel_commit=24cd21598c091fb88bc10a6375a1ded25e677c37' "$steel_home/.lean4-hx/manifest" >/dev/null
grep -F 'helix_commit=87ec539d136c6cd83aeb82697e66de9706d7af4a' "$steel_home/.lean4-hx/manifest" >/dev/null

STEEL_HOME=$steel_home "$repo_root/scripts/install.sh" --rollback
cmp -s "$old_native" "$steel_home/native/liblean4_hx.dylib"
cmp -s "$old_cog" "$steel_home/cogs/lean4-hx.scm"
[ ! -e "$steel_home/.lean4-hx/manifest" ]
printf '%s\n' "install, pin, downgrade, and rollback acceptance passed"
