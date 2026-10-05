#!/bin/sh
# Verify that a definition callback becomes an actual editor selection.

set -eu
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
helix_dir=${HOST_HELIX_DIR:?set HOST_HELIX_DIR to the pinned Helix checkout}
steel_dir=${HOST_STEEL_DIR:?set HOST_STEEL_DIR to the pinned Steel checkout}
hx_bin=${HOST_HELIX_BIN:-$helix_dir/target/release/hx}
acceptance_root=${LEAN4_HX_ACCEPTANCE_DIR:-$repo_root/.acceptance-navigation}
run_dir=$acceptance_root/current
runtime_dir=$helix_dir/runtime
config_dir=$run_dir/config
helix_config=$config_dir/helix
steel_home=$run_dir/steel
project_dir=$run_dir/project
expected_helix=87ec539d136c6cd83aeb82697e66de9706d7af4a
expected_steel=24cd21598c091fb88bc10a6375a1ded25e677c37
navigation_relation=${LEAN4_HX_NAVIGATION_RELATION:-same-file}
navigation_start_character=${LEAN4_HX_NAVIGATION_START_CHARACTER:-0}

[ "$(git -C "$helix_dir" rev-parse HEAD)" = "$expected_helix" ]
[ "$(git -C "$steel_dir" rev-parse HEAD)" = "$expected_steel" ]
[ -x "$hx_bin" ]
command -v expect >/dev/null 2>&1

mkdir -p "$helix_config" "$steel_home/cogs" "$steel_home/native" "$project_dir"
cp "$repo_root/tests/host/project/Main.lean" "$project_dir/Main.lean"
cp "$repo_root/tests/host/project/Term.lean" "$project_dir/Term.lean"
printf 'αtarget\n' >"$project_dir/Unicode.lean"
cp "$repo_root/tests/host/project/lakefile.toml" "$project_dir/lakefile.toml"
cp "$repo_root/tests/host/project/.lean-toolchain" "$project_dir/.lean-toolchain"
cp "$repo_root/host/extension.scm" "$steel_home/cogs/lean4-hx.scm"
cp "$repo_root/host/extension.scm" "$helix_config/lean4-hx.scm"
cp "$repo_root/tests/host/navigation-server.py" "$run_dir/server.py"
cp "$repo_root/target/release/liblean4_hx.dylib" "$steel_home/native/liblean4_hx.dylib"
chmod +x "$run_dir/server.py"
: >"$helix_config/helix.scm"
: >"$steel_home/helix.scm"
cat >"$helix_config/config.toml" <<'EOF'
[editor]
enable-steel = true
EOF
cat >"$config_dir/helix/languages.toml" <<EOF
[[language]]
name = "lean"
language-servers = ["lean"]

[language-server.lean]
command = "python3"
args = ["$run_dir/server.py"]
EOF
cat >"$helix_config/init.scm" <<'EOF'
(require "lean4-hx.scm")
(lean4-hx-install!)
EOF
cp "$helix_config/init.scm" "$steel_home/init.scm"

pty_script=$run_dir/acceptance.exp
pty_log=$run_dir/pty.log
: >"$pty_log"
cat >"$pty_script" <<EOF
log_user 0
log_file -a "$pty_log"
set timeout 20
set env(HELIX_RUNTIME) "$runtime_dir"
set env(XDG_CONFIG_HOME) "$config_dir"
set env(HELIX_STEEL_CONFIG) "$helix_config"
set env(STEEL_HOME) "$steel_home"
set env(LEAN4_HX_NAVIGATION_ROOT) "$project_dir"
set env(LEAN4_HX_NAVIGATION_TARGET) "${LEAN4_HX_NAVIGATION_TARGET:-Main.lean}"
set env(LEAN4_HX_NAVIGATION_START_CHARACTER) "$navigation_start_character"
set env(LEAN4_HX_NAVIGATION_END_CHARACTER) "${LEAN4_HX_NAVIGATION_END_CHARACTER:-5}"
set env(LEAN4_HX_TRACE) "1"
cd "$project_dir"
spawn "$hx_bin" Main.lean
expect -exact "LEAN4_HX_INSTALL active=true"
expect -exact "LEAN4_HX_OPEN"
expect -exact "LEAN4_HX_NAVIGATION $navigation_relation"
send ":lean4-hx-apply-navigation!"
send "\\r"
expect -exact "LEAN4_HX_NAVIGATION_APPLIED"
send ":quit!"
send "\\r"
expect eof
EOF

LEAN4_HX_NAVIGATION_ROOT="$project_dir" expect "$pty_script"
grep -F "LEAN4_HX_NAVIGATION $navigation_relation" "$pty_log" >/dev/null
grep -F "LEAN4_HX_NAVIGATION_APPLIED" "$pty_log" >/dev/null
grep -F "character=$navigation_start_character" "$pty_log" >/dev/null
if grep -E 'Sync job failed|panicked at|thread .* panicked' "$pty_log" >/dev/null; then
    exit 1
fi
echo "editor navigation acceptance passed"
