#!/bin/sh
# Exercise the Steel extension inside the pinned Helix binary through a PTY.
# This is deliberately a live host test: it opens, edits, reloads, restarts
# the LSP, closes and reopens a document, and checks the native callback.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$repo_root/tests/host/project
host_tmp=${TMPDIR:-/tmp}
helix_dir=${HOST_HELIX_DIR:-$host_tmp/lean4hx-helix}
steel_dir=${HOST_STEEL_DIR:-$host_tmp/lean4hx-steel}

expected_helix=df595c7dc5729e2712c79dd2e35977e3474b3ec6
expected_steel=24cd21598c091fb88bc10a6375a1ded25e677c37
[ -d "$helix_dir/.git" ] || { echo "set HOST_HELIX_DIR to the pinned Helix checkout" >&2; exit 1; }
[ -d "$steel_dir/.git" ] || { echo "set HOST_STEEL_DIR to the pinned Steel checkout" >&2; exit 1; }
[ "$(git -C "$helix_dir" rev-parse HEAD)" = "$expected_helix" ] || {
    echo "Helix checkout is not pinned at $expected_helix" >&2; exit 1;
}
[ "$(git -C "$steel_dir" rev-parse HEAD)" = "$expected_steel" ] || {
    echo "Steel checkout is not pinned at $expected_steel" >&2; exit 1;
}

hx_bin=${HOST_HELIX_BIN:-$helix_dir/target/debug/hx}
[ -x "$hx_bin" ] || { echo "build pinned Helix first: $hx_bin" >&2; exit 1; }
command -v expect >/dev/null 2>&1 || { echo "expect is required for PTY acceptance" >&2; exit 1; }
elan_bin=$(command -v elan || true)
[ -n "$elan_bin" ] || { echo "elan is required for PTY acceptance" >&2; exit 1; }
elan_dir=$(CDPATH= cd -- "$(dirname "$elan_bin")" && pwd)

cd "$repo_root"
cargo build --release --locked

run_dir=$(mktemp -d "$host_tmp/lean4hx-host-acceptance.XXXXXX")
runtime_dir=$run_dir/runtime
config_dir=$run_dir/config
helix_config=$config_dir/helix
steel_home=$run_dir/steel
project_dir=$run_dir/project
mkdir -p "$helix_config" "$steel_home/cogs" "$steel_home/native" "$project_dir"
mkdir -p "$config_dir/helix"
cp -R "$helix_dir/runtime/." "$runtime_dir/"
cp "$fixture/Main.lean" "$fixture/lakefile.toml" "$fixture/.lean-toolchain" "$project_dir/"
cp "$repo_root/host/extension.scm" "$steel_home/cogs/lean4-hx.scm"
cp "$repo_root/host/extension.scm" "$helix_config/lean4-hx.scm"
cp "$repo_root/target/release/liblean4_hx.dylib" "$steel_home/native/liblean4_hx.dylib"
: >"$helix_config/helix.scm"
: >"$steel_home/helix.scm"
cat >"$helix_config/config.toml" <<'EOF'
[editor]
enable-steel = true
EOF
cp "$helix_config/config.toml" "$config_dir/config.toml"
cat >"$config_dir/helix/languages.toml" <<'EOF'
[[language]]
name = "lean"
language-servers = ["lean"]

[language-server.lean]
command = "elan"
args = ["run", "v4.34.0", "lean", "--server"]
EOF
cat >"$helix_config/init.scm" <<'EOF'
(require "lean4-hx.scm")
(lean4-hx-install!)
EOF
cp "$helix_config/init.scm" "$steel_home/init.scm"

pty_script=$run_dir/acceptance.exp
pty_log=$run_dir/pty.log
cat >"$pty_script" <<EOF
log_user 0
log_file -a "$pty_log"
set timeout 20
set env(HELIX_RUNTIME) "$runtime_dir"
set env(XDG_CONFIG_HOME) "$config_dir"
set env(HELIX_STEEL_CONFIG) "$helix_config"
set env(STEEL_HOME) "$steel_home"
set env(LEAN4_HX_TRACE) "1"
set env(PATH) "$elan_dir:\$env(PATH)"
proc need {pattern} {
    expect {
        -exact \$pattern {}
        timeout { puts stderr "timed out waiting for \$pattern"; exit 1 }
        eof { puts stderr "Helix exited before \$pattern"; exit 1 }
    }
}
cd "$project_dir"
spawn "$hx_bin" Main.lean
need "LEAN4_HX_INSTALL active=true"
need "LEAN4_HX_OPEN"
sleep 2
need "LEAN4_HX_REQUEST line="
send "j"
sleep 2
need "LEAN4_HX_GOAL_AVAILABLE"
need "LEAN4_HX_TAGGED_RENDER_READY"
send "j"
send "j"
sleep 2
send ":lean4-hx-code-action!"
send "\\r"
need "Update #guard_msgs with generated message"
send "\\r"
sleep 2
send ":write"
send "\\r"
sleep 1
send "\\t"
need "LEAN4_HX_GOAL_FOCUS focused=true"
send "n"
need "LEAN4_HX_GOAL_SELECTED index="
send "p"
need "LEAN4_HX_GOAL_SELECTED index="
send "\\033"
need "LEAN4_HX_GOAL_FOCUS focused=false"
send "i"
send " "
send "\\033"
need "LEAN4_HX_DOCUMENT_CHANGED version="
sleep 2
need "LEAN4_HX_CALLBACK result=reply"
send ":lsp-restart"
send "\\r"
sleep 5
send ":buffer-close!"
send "\\r"
need "LEAN4_HX_CLOSE"
send ":open Main.lean"
send "\\r"
need "LEAN4_HX_OPEN"
sleep 2
need "LEAN4_HX_REQUEST line="
send "j"
sleep 2
need "LEAN4_HX_GOAL_AVAILABLE"
send ":lean4-hx-remove-component!"
send "\\r"
need "LEAN4_HX_REMOVE active=false"
send ":quit!"
send "\\r"
expect eof
EOF

if ! expect "$pty_script"; then
    echo "PTY acceptance failed; preserved evidence at $run_dir" >&2
    exit 1
fi

for marker in \
    'LEAN4_HX_INSTALL active=true' \
    'LEAN4_HX_OPEN' \
    'LEAN4_HX_DOCUMENT_CHANGED version=' \
    'LEAN4_HX_CLOSE' \
    'LEAN4_HX_REQUEST line=' \
    'LEAN4_HX_CALLBACK result=reply' \
    'LEAN4_HX_GOAL_RENDER_READY lines=' \
    'LEAN4_HX_GOAL_AVAILABLE' \
    'LEAN4_HX_TAGGED_RENDER_READY' \
    'LEAN4_HX_GOAL_FOCUS focused=true' \
    'LEAN4_HX_GOAL_FOCUS focused=false' \
    'LEAN4_HX_GOAL_SELECTED index=' \
    'LEAN4_HX_RPC action=keepAlive' \
    'LEAN4_HX_RPC action=release refs=' \
    'LEAN4_HX_REMOVE active=false'; do
    grep -F "$marker" "$pty_log" >/dev/null || {
        echo "missing PTY marker: $marker; preserved evidence at $run_dir" >&2
        exit 1
    }
done
connect_line=$(grep -n -m1 'LEAN4_HX_RPC action=connect' "$pty_log" | cut -d: -f1)
interactive_line=$(grep -n -m1 'LEAN4_HX_RPC action=interactive-goals' "$pty_log" | cut -d: -f1)
[ -n "$connect_line" ] && [ -n "$interactive_line" ] && [ "$connect_line" -lt "$interactive_line" ] || {
    echo "tagged RPC did not complete after connect; preserved evidence at $run_dir" >&2
    exit 1
}
# The marker is emitted only after the rich callback has populated the styled
# snapshot. Require the actual ANSI styles in the PTY transcript as well.
cyan_escape=$(printf '\033[36')
bold_goal_escape=$(printf '\033[34;1m')
grep -F "$cyan_escape" "$pty_log" >/dev/null || {
    echo "missing cyan tagged style in PTY transcript; preserved evidence at $run_dir" >&2
    exit 1
}
grep -F "$bold_goal_escape" "$pty_log" >/dev/null || {
    echo "missing bold goal style in PTY transcript; preserved evidence at $run_dir" >&2
    exit 1
}
if grep -E 'error\[E[0-9]+\]|BadSyntax|Sync job failed|panicked at|thread .* panicked' "$pty_log" >/dev/null; then
    echo "PTY log contains a host failure; preserved evidence at $run_dir" >&2
    exit 1
fi
grep -F 'info: 42' "$project_dir/Main.lean" >/dev/null || {
    echo "code action did not update the guard message; preserved evidence at $run_dir" >&2
    exit 1
}

echo "host PTY acceptance passed"
echo "Helix: $(git -C "$helix_dir" rev-parse HEAD)"
echo "Steel: $(git -C "$steel_dir" rev-parse HEAD)"
echo "evidence: $pty_log"
tr '\r' '\n' <"$pty_log" |
    grep -F 'LEAN4_HX_' |
    sed 's/.*\(LEAN4_HX_[A-Z_]*[^[:cntrl:]]*\).*/\1/'
