#!/bin/sh
# Exercise selected two-file WorkspaceEdits through the pinned Helix host.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$repo_root/tests/host/project
host_tmp=${TMPDIR:-/tmp}
helix_dir=${HOST_HELIX_DIR:-$host_tmp/lean4hx-helix}
steel_dir=${HOST_STEEL_DIR:-$host_tmp/lean4hx-steel}
expected_helix=87ec539d136c6cd83aeb82697e66de9706d7af4a
expected_steel=24cd21598c091fb88bc10a6375a1ded25e677c37

[ "$(git -C "$helix_dir" rev-parse HEAD)" = "$expected_helix" ] || exit 1
[ "$(git -C "$steel_dir" rev-parse HEAD)" = "$expected_steel" ] || exit 1
hx_bin=${HOST_HELIX_BIN:-$helix_dir/target/release/hx}
[ -x "$hx_bin" ] || exit 1
command -v expect >/dev/null 2>&1 || exit 1

acceptance_root=${LEAN4_HX_ACCEPTANCE_DIR:-$repo_root/.acceptance-workspace}
run_dir=$acceptance_root/current
runtime_dir=$helix_dir/runtime
config_dir=$run_dir/config
helix_config=$config_dir/helix
steel_home=$run_dir/steel
project_dir=$run_dir/project
mkdir -p "$helix_config" "$steel_home/cogs" "$steel_home/native" "$project_dir"
cp "$fixture/Main.lean" "$fixture/Second.lean" "$fixture/lakefile.toml" "$fixture/.lean-toolchain" "$project_dir/"
cp "$repo_root/host/extension.scm" "$steel_home/cogs/lean4-hx.scm"
cp "$repo_root/host/extension.scm" "$helix_config/lean4-hx.scm"
cp "$repo_root/tests/host/workspace-edit-server.py" "$run_dir/server.py"
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
set env(LEAN4_HX_WORKSPACE_ROOT) "$project_dir"
set env(LEAN4_HX_TRACE) "1"
cd "$project_dir"
spawn "$hx_bin" Main.lean
expect -exact "LEAN4_HX_INSTALL active=true"
expect -exact "LEAN4_HX_OPEN"
send ":open Second.lean"
send "\\r"
sleep 1
send ":open Main.lean"
send "\\r"
sleep 1
send ":lean4-hx-code-action!"
send "\\r"
expect -exact "Apply two-file fixture"
send "\\r"
sleep 1
send ":write-all"
send "\\r"
sleep 1
send ":lean4-hx-code-action!"
send "\\r"
expect -exact "Reject stale two-file fixture"
send "\\r"
sleep 1
send ":quit!"
send "\\r"
expect {
    eof {}
    timeout { catch {close}; catch {wait}; exit 0 }
}
EOF

LEAN4_HX_WORKSPACE_ROOT="$project_dir" expect "$pty_script"

grep -F 'exact rfl' "$project_dir/Main.lean" >/dev/null
grep -F 'def helper : Nat := 1' "$project_dir/Second.lean" >/dev/null
if grep -F 'def helper : Nat := 2' "$project_dir/Second.lean" >/dev/null; then
    echo "stale WorkspaceEdit mutated the second document" >&2
    exit 1
fi
if grep -F 'rfl' "$project_dir/Main.lean" >/dev/null && ! grep -F 'exact rfl' "$project_dir/Main.lean" >/dev/null; then
    echo "stale WorkspaceEdit mutated the first document" >&2
    exit 1
fi
if grep -E 'Sync job failed|panicked at|thread .* panicked' "$pty_log" >/dev/null; then
    echo "workspace edit PTY reported a host failure" >&2
    exit 1
fi
echo "two-file workspace edit acceptance passed"
