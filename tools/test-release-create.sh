#!/usr/bin/env bash
# tools/test-release-create.sh — dry-run checks for tools/release-create.sh naming rule.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
v=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"");sub("version=","");print;exit}' "$here/../Cargo.toml")
out=$("$here/release-create.sh" "v$v" --dry-run)
[[ $out == *"AudioLib\\ Explorer\\ v$v "* && $out != *--prerelease* ]] || { echo "FAIL stable: $out"; exit 1; }
out=$("$here/release-create.sh" "v$v-beta.3" --dry-run)
[[ $out == *"AudioLib\\ Explorer\\ v$v-beta.3 "* && $out == *"--prerelease --latest=false"* ]] || { echo "FAIL beta: $out"; exit 1; }
if "$here/release-create.sh" "v0.0.1" --dry-run 2>/dev/null; then echo "FAIL: Cargo mismatch accepted"; exit 1; fi
if "$here/release-create.sh" "0.1.0" --dry-run 2>/dev/null; then echo "FAIL: bad tag accepted"; exit 1; fi
echo "release-create tests passed"
