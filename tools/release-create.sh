#!/usr/bin/env bash
# tools/release-create.sh
# Create a GitHub Release for an existing tag with the agreed naming rule:
#   title = "AudioLib Explorer <tag>"; suffixed tags (-beta.N / -rc.N) are --prerelease --latest=false.
# The tag's X.Y.Z must equal the Cargo.toml [workspace.package] version.
#   tools/release-create.sh <tag> [--notes-file <file>] [--dry-run] [asset ...]
set -euo pipefail
die() { echo "release-create: $*" >&2; exit 1; }
[ $# -ge 1 ] || die "usage: $0 <tag> [--notes-file <file>] [--dry-run] [asset ...]"
tag=$1; shift
[[ $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-(rc|beta)\.[0-9]+)?$ ]] || die "tag '$tag' must be vX.Y.Z, vX.Y.Z-rc.N or vX.Y.Z-beta.N"
core=${BASH_REMATCH[1]}; suffix=${BASH_REMATCH[2]}
root=$(cd "$(dirname "$0")/.." && pwd)
cargo_ver=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"");sub("version=","");print;exit}' "$root/Cargo.toml")
[ "$core" = "$cargo_ver" ] || die "tag $tag does not match Cargo.toml workspace version $cargo_ver"
notes=(--notes-file "$root/doc/Rust_Release_Notes.md"); dry=0; assets=()
while [ $# -gt 0 ]; do
  case $1 in
    --notes-file) notes=(--notes-file "$2"); shift 2 ;;
    --dry-run) dry=1; shift ;;
    *) assets+=("$1"); shift ;;
  esac
done
args=("$tag" --verify-tag --title "AudioLib Explorer $tag" "${notes[@]}")
[ -n "$suffix" ] && args+=(--prerelease --latest=false)
if [ $dry = 1 ]; then printf '%q ' gh release create "${args[@]}" ${assets[@]+"${assets[@]}"}; echo; exit 0; fi
gh release create "${args[@]}" ${assets[@]+"${assets[@]}"}
