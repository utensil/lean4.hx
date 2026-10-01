#!/bin/sh
# Refresh the vendored Unicode corpus from a pinned upstream revision.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
destination="$repo_root/src/unicode-abbreviations.json"
revision=dead846a035f42dc13beb7619ac779538e6ddf6e
url="https://raw.githubusercontent.com/leanprover-community/vscode-lean4/$revision/lean4-unicode-input/src/abbreviations.json"
expected_sha256=fa5e317d4d481c8bf12d6438e489376931c18438dd0a68af3683a4cb9e92b5b0
temporary=$(mktemp "$repo_root/src/.unicode-abbreviations.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM

curl -fsSL --retry 3 "$url" -o "$temporary"
python3 - "$temporary" <<'PY'
import json
import pathlib
import sys

value = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if not isinstance(value, dict) or not value or not all(
    isinstance(key, str) and isinstance(template, str)
    for key, template in value.items()
):
    raise SystemExit("upstream corpus is not a non-empty string map")
PY

actual_sha256=$(shasum -a 256 "$temporary" | awk '{print $1}')
[ "$actual_sha256" = "$expected_sha256" ] || {
    echo "upstream corpus checksum mismatch: $actual_sha256" >&2
    exit 1
}

cp "$temporary" "$destination"
echo "updated $destination from $revision ($actual_sha256)"
