#!/bin/sh
# Exercise the Steel extension inside the pinned Helix binary through a PTY.
# This is deliberately a live host test: it opens, edits, reloads, restarts
# the LSP, closes and reopens a document, and checks the native callback.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
pin_file=$repo_root/HOST-PINS.toml
fixture=$repo_root/tests/host/project
host_tmp=${TMPDIR:-/tmp}
helix_dir=${HOST_HELIX_DIR:-$host_tmp/lean4hx-helix}
steel_dir=${HOST_STEEL_DIR:-$host_tmp/lean4hx-steel}

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
set timeout 90
set env(HELIX_RUNTIME) "$runtime_dir"
set env(XDG_CONFIG_HOME) "$config_dir"
set env(HELIX_STEEL_CONFIG) "$helix_config"
set env(STEEL_HOME) "$steel_home"
set env(PATH) "/Users/utensil/.elan/bin:\$env(PATH)"
cd "$project_dir"
spawn "$hx_bin" Main.lean
expect "LEAN4_HX_INSTALL"
expect "LEAN4_HX_CALLBACK result=reply"
send "i"
send "X"
send "\\033"
sleep 2
send ":reload"
send "\\r"
sleep 3
send ":lsp-restart"
send "\\r"
sleep 10
send ":buffer-close!"
send "\\r"
expect "LEAN4_HX_CLOSE"
send ":open Main.lean"
send "\\r"
sleep 1
send "\\r"
expect "LEAN4_HX_OPEN"
expect "LEAN4_HX_CALLBACK result=reply"
send ":lean4-hx-remove-component!"
send "\\r"
expect "LEAN4_HX_REMOVE active=false"
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
    'LEAN4_HX_INSERT' \
    'LEAN4_HX_CLOSE' \
    'LEAN4_HX_REQUEST line=0 character=0 uri=file://' \
    'LEAN4_HX_CALLBACK result=reply' \
    'LEAN4_HX_REMOVE active=false'; do
    grep -F "$marker" "$pty_log" >/dev/null || {
        echo "missing PTY marker: $marker; preserved evidence at $run_dir" >&2
        exit 1
    }
done
if grep -E 'BadSyntax|Sync job failed|panicked at|thread .* panicked' "$pty_log" >/dev/null; then
    echo "PTY log contains a host failure; preserved evidence at $run_dir" >&2
    exit 1
fi

echo "host PTY acceptance passed"
echo "Helix: $(git -C "$helix_dir" rev-parse HEAD)"
echo "Steel: $(git -C "$steel_dir" rev-parse HEAD)"
echo "evidence: $pty_log"
tr '\r' '\n' <"$pty_log" |
    grep -F 'LEAN4_HX_' |
    sed 's/.*\(LEAN4_HX_[A-Z_]*[^[:cntrl:]]*\).*/\1/'
