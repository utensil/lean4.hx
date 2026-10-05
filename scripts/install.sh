#!/bin/sh
# Build and install the native library and Steel module into STEEL_HOME.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
steel_home=${STEEL_HOME:-}

if [ -z "$steel_home" ]; then
    echo "STEEL_HOME must point to the Steel installation directory" >&2
    exit 64
fi

state_dir=$steel_home/.lean4-hx
backup_root=$state_dir/backups
native_target=$steel_home/native/liblean4_hx.dylib
cog_target=$steel_home/cogs/lean4-hx.scm

latest_backup() {
    latest=
    for candidate in "$backup_root"/*; do
        if [ -d "$candidate" ]; then
            latest=$candidate
        fi
    done
    [ -n "$latest" ] || return 1
    printf '%s\n' "$latest"
}

restore_file() {
    backup_file=$1
    target_file=$2
    missing_file=$3
    mkdir -p "$(dirname "$target_file")"
    if [ -f "$backup_file" ]; then
        cp -p "$backup_file" "$target_file"
    elif [ -f "$missing_file" ]; then
        rm -f "$target_file"
    else
        echo "backup entry is incomplete: $backup_file" >&2
        return 1
    fi
}

if [ "${1:-}" = "--rollback" ]; then
    [ "${2:-}" = "" ] || { echo "usage: $0 [--rollback]" >&2; exit 64; }
    backup=$(latest_backup) || {
        echo "no lean4.hx installation backup exists" >&2
        exit 1
    }
    restore_file "$backup/native" "$native_target" "$backup/native.missing"
    restore_file "$backup/cog" "$cog_target" "$backup/cog.missing"
    rm -f "$state_dir/manifest"
    printf '%s\n' "rolled back lean4.hx from $backup"
    exit 0
fi

[ "${1:-}" = "" ] || { echo "usage: $0 [--rollback]" >&2; exit 64; }

cd "$repo_root"
cargo build --release --locked
mkdir -p "$steel_home/native" "$steel_home/cogs" "$backup_root"

stamp=$(date -u '+%Y%m%dT%H%M%SZ')-$$
backup="$backup_root/$stamp"
stage_dir=$state_dir/stage.$$
mkdir -p "$backup" "$stage_dir/native" "$stage_dir/cogs"
trap 'rm -rf "$stage_dir"' EXIT HUP INT TERM

if [ -e "$native_target" ]; then
    cp -p "$native_target" "$backup/native"
else
    : >"$backup/native.missing"
fi
if [ -e "$cog_target" ]; then
    cp -p "$cog_target" "$backup/cog"
else
    : >"$backup/cog.missing"
fi

cp -p target/release/liblean4_hx.dylib "$stage_dir/native/liblean4_hx.dylib"
cp -p host/extension.scm "$stage_dir/cogs/lean4-hx.scm"
mv -f "$stage_dir/native/liblean4_hx.dylib" "$native_target"
mv -f "$stage_dir/cogs/lean4-hx.scm" "$cog_target"

revision=$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)
steel_pin=$(sed -n 's/.*rev = "\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)
helix_pin=$(sed -n 's/^helix_commit=\(.*\)$/\1/p' scripts/host-harness.sh | head -n 1)
{
    printf 'revision=%s\n' "$revision"
    printf 'steel_commit=%s\n' "$steel_pin"
    printf 'helix_commit=%s\n' "$helix_pin"
} >"$state_dir/manifest.new"
mv -f "$state_dir/manifest.new" "$state_dir/manifest"

printf '%s\n' "installed lean4.hx into $steel_home"
printf '%s\n' "backup: $backup"
