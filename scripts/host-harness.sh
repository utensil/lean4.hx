#!/bin/sh
# Reproducible host check for the pinned public Helix and Steel inputs.

set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
pin_file=$repo_root/HOST-PINS.toml

if [ ! -f "$pin_file" ]; then
    echo "missing pin file: $pin_file" >&2
    exit 1
fi

pin_value() {
    section=$1
    key=$2
    awk -v wanted_section="[$section]" -v wanted_key="$key" '
        $0 == wanted_section { active = 1; next }
        /^\[/ { active = 0 }
        active && $1 == wanted_key {
            sub(/^[^\"]*\"/, "")
            sub(/\".*$/, "")
            print
            exit
        }
    ' "$pin_file"
}

lean_toolchain=$(sed -n '1p' "$repo_root/.lean-toolchain")
rust_toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$repo_root/rust-toolchain.toml")
helix_url=$(pin_value helix repository)
helix_branch=$(pin_value helix branch)
helix_commit=$(pin_value helix commit)
steel_url=$(pin_value steel repository)
steel_branch=$(pin_value steel branch)
steel_commit=$(pin_value steel commit)

for value in "$lean_toolchain" "$rust_toolchain" "$helix_url" "$helix_branch" \
    "$helix_commit" "$steel_url" "$steel_branch" "$steel_commit"; do
    if [ -z "$value" ]; then
        echo "an expected pin is missing from $pin_file or the toolchain files" >&2
        exit 1
    fi
done

case $helix_branch:$steel_branch in
    lean-dev:lean-dev) ;;
    *) echo "dependency branches must both be lean-dev" >&2; exit 1 ;;
esac

cache_root=${HOST_HARNESS_CACHE_DIR:-${TMPDIR:-/tmp}/lean4-hx-host-harness}
mkdir -p "$cache_root"

clone_exact() {
    name=$1
    url=$2
    branch=$3
    commit=$4
    checkout=$cache_root/$name

    if [ -d "$checkout/.git" ]; then
        origin=$(git -C "$checkout" remote get-url origin)
        if [ "$origin" != "$url" ]; then
            echo "$name cache has origin $origin; expected $url" >&2
            exit 1
        fi
        git -C "$checkout" fetch --force origin "+refs/heads/$branch:refs/remotes/origin/$branch"
    else
        git clone --single-branch --branch "$branch" "$url" "$checkout"
    fi

    remote_head=$(git -C "$checkout" rev-parse "refs/remotes/origin/$branch")
    if [ "$remote_head" != "$commit" ]; then
        echo "$name $branch is $remote_head; expected $commit" >&2
        exit 1
    fi
    git -C "$checkout" checkout --detach --force "$commit"
    actual=$(git -C "$checkout" rev-parse HEAD)
    if [ "$actual" != "$commit" ]; then
        echo "$name checkout is $actual; expected $commit" >&2
        exit 1
    fi
    printf '%s %s\n' "$name" "$actual"
}

clone_exact helix "$helix_url" "$helix_branch" "$helix_commit"
clone_exact steel "$steel_url" "$steel_branch" "$steel_commit"

if ! grep -F "$steel_commit" "$cache_root/helix/Cargo.lock" >/dev/null 2>&1; then
    echo "Helix Cargo.lock does not contain the pinned Steel commit $steel_commit" >&2
    exit 1
fi

if ! command -v elan >/dev/null 2>&1; then
    echo "elan is required to verify Lean $lean_toolchain" >&2
    exit 1
fi
lean_version=$(cd "$repo_root" && elan run "$lean_toolchain" lean --version)
case $lean_version in
    *"Lean (version 4.34.0"*) ;;
    *) echo "unexpected Lean version: $lean_version" >&2; exit 1 ;;
esac
printf '%s\n' "$lean_version"

rustup_bin=$(command -v rustup || true)
if [ -z "$rustup_bin" ]; then
    rustup_candidate=${CARGO_HOME:-${HOME:-}/.cargo}/bin/rustup
    if [ -x "$rustup_candidate" ]; then
        rustup_bin=$rustup_candidate
        PATH=$(dirname "$rustup_bin"):$PATH
        export PATH
    fi
fi
if [ -z "$rustup_bin" ] || ! command -v rustup >/dev/null 2>&1; then
    echo "rustup is required to verify Rust $rust_toolchain" >&2
    exit 1
fi
rust_runner=$rust_toolchain
rust_version=$(rustup run "$rust_runner" rustc --version 2>/dev/null || true)
case $rust_version in
    "rustc 1.100.0-nightly"*) ;;
    *)
        rust_runner=
        for candidate in $(rustup toolchain list | sed 's/[[:space:]].*//'); do
            candidate_version=$(rustup run "$candidate" rustc --version 2>/dev/null || true)
            case $candidate_version in
                "rustc 1.100.0-nightly"*)
                    rust_runner=$candidate
                    rust_version=$candidate_version
                    break
                    ;;
            esac
        done
        if [ -z "$rust_runner" ]; then
            echo "no installed rustup toolchain reports Rust 1.100.0-nightly" >&2
            exit 1
        fi
        ;;
esac
printf '%s\n' "$rust_version"
rustup run "$rust_runner" cargo --version

cd "$cache_root/helix"
export RUSTUP_TOOLCHAIN=$rust_runner
rust_cargo=$(rustup which --toolchain "$rust_runner" cargo)
rust_bin=$(dirname "$rust_cargo")
PATH=$rust_bin:$PATH
export PATH
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo check --locked -p helix-term --features steel
