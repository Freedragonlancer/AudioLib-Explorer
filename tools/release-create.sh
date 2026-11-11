#!/usr/bin/env bash
# tools/release-create.sh
# Create a GitHub Release for an existing tag with the agreed naming rule:
#   title = "AudioLib Explorer <tag>"; suffixed tags (-beta.N / -rc.N) are --prerelease --latest=false.
# Drafts stay unpublished and are explicitly excluded from Latest. Resume uploads
# only missing, verified assets; --publish is a separate, explicit finalization.
# The tag's X.Y.Z must equal the Cargo.toml [workspace.package] version.
#   tools/release-create.sh <tag> [--notes-file <file>] [--draft] [--dry-run] [--] [asset ...]
#   tools/release-create.sh <tag> --resume-draft [--publish] [--dry-run] [--notes-file <file>] <all-assets...>
set -euo pipefail
die() { echo "release-create: $*" >&2; exit 1; }
[ $# -ge 1 ] || die "usage: $0 <tag> [--notes-file <file>] [--draft] [--dry-run] [--resume-draft [--publish]] [--] [asset ...]"
tag=$1; shift
[[ $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-(rc|beta)\.[0-9]+)?$ ]] || die "tag '$tag' must be vX.Y.Z, vX.Y.Z-rc.N or vX.Y.Z-beta.N"
core=${BASH_REMATCH[1]}; suffix=${BASH_REMATCH[2]}
root=$(cd "$(dirname "$0")/.." && pwd)
cargo_ver=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"");sub("version=","");print;exit}' "$root/Cargo.toml")
[ "$core" = "$cargo_ver" ] || die "tag $tag does not match Cargo.toml workspace version $cargo_ver"
notes=(--notes-file "$root/doc/Rust_Release_Notes.md"); dry=0; draft=0; resume=0; publish=0; assets=()
while [ $# -gt 0 ]; do
  case $1 in
    --notes-file)
      [ $# -ge 2 ] && [ -n "$2" ] && [[ $2 != --* ]] || die "option $1 requires a value"
      notes=(--notes-file "$2"); shift 2 ;;
    --notes-file=*) [ -n "${1#*=}" ] || die "option --notes-file requires a value"; notes=(--notes-file "${1#*=}"); shift ;;
    --dry-run) dry=1; shift ;;
    --draft) draft=1; shift ;;
    --resume-draft) resume=1; shift ;;
    --publish) publish=1; shift ;;
    --) shift; assets+=("$@"); break ;;
    -*) die "unknown option: $1" ;;
    *) assets+=("$1"); shift ;;
  esac
done
[ "$publish" = 0 ] || [ "$resume" = 1 ] || die '--publish requires --resume-draft'
[ "$publish" = 0 ] || [ "$draft" = 0 ] || die '--publish conflicts with --draft'
args=("$tag" --verify-tag --title "AudioLib Explorer $tag" "${notes[@]}")
[ -n "$suffix" ] && args+=(--prerelease)
if [ -n "$suffix" ] || [ "$draft" = 1 ]; then args+=(--latest=false); fi
[ "$draft" = 1 ] && args+=(--draft)
# Protect literal asset names beginning with '-' from the downstream CLI parser.
if [ "${assets[*]+set}" = set ]; then args+=(--); fi
# A create preview is intentionally fully offline. Resume previews need the
# existing draft's read-only identity to distinguish matching/missing assets.
if [ "$dry" = 1 ] && [ "$resume" = 0 ]; then printf '%q ' gh release create "${args[@]}" ${assets[@]+"${assets[@]}"}; echo; exit 0; fi

matches=$(gh api --paginate 'repos/{owner}/{repo}/releases?per_page=100' --jq ".[] | select(.tag_name == \"$tag\") | [.id, .draft] | @tsv") || die 'Cannot inspect authenticated releases/drafts; absence is unproven'
release_id=''; release_draft=''; release_count=0
while IFS=$'\t' read -r candidate_id candidate_draft extra; do
  [ -n "$candidate_id" ] || continue
  [[ $candidate_id =~ ^[1-9][0-9]*$ && ( $candidate_draft = true || $candidate_draft = false ) && -z $extra ]] || die 'Invalid release listing metadata'
  release_count=$((release_count+1)); release_id=$candidate_id; release_draft=$candidate_draft
done <<< "$matches"
[ "$release_count" -le 1 ] || die 'Multiple same-tag releases found; inspect them before recovery'
if [ "$resume" = 0 ]; then
  [ "$release_count" = 0 ] || die "Release/draft $release_id already exists; inspect it before using --resume-draft"
  gh release create "${args[@]}" ${assets[@]+"${assets[@]}"}
  exit $?
fi
[ "$release_count" = 1 ] && [ "$release_draft" = true ] || die '--resume-draft requires one existing unpublished draft'
[ "${assets[*]+set}" = set ] || die 'Resume requires the complete expected local asset set'
[ -f "${notes[1]}" ] && [ ! -L "${notes[1]}" ] || die 'Resume notes must be a regular, unlinked file'

expected_text=$(pwsh -NoLogo -NoProfile -File "$root/tools/verify-release-assets.ps1" -Tag "$tag" -ListExpectedAssets) || die 'Cannot obtain the official expected asset set (resume requires PowerShell 7)'
expected_names=(); expected_count=0
while IFS= read -r expected_name; do
  expected_name=${expected_name%$'\r'}
  [ -n "$expected_name" ] || continue
  [[ $expected_name != */* && $expected_name != *\\* && $expected_name != *$'\t'* ]] || die 'Invalid expected asset basename'
  expected_names+=("$expected_name"); expected_count=$((expected_count+1))
done <<< "$expected_text"
[ "$expected_count" -gt 0 ] || die 'Official expected asset set is empty'

sha256_file() {
  local value
  if command -v sha256sum >/dev/null 2>&1; then value=$(sha256sum "$1") || return
  elif command -v shasum >/dev/null 2>&1; then value=$(shasum -a 256 "$1") || return
  else die 'Resume requires sha256sum or shasum'; fi
  value=${value%% *}; value=$(printf '%s' "$value" | tr 'A-F' 'a-f')
  [[ $value =~ ^[a-f0-9]{64}$ ]] || die 'Invalid local SHA256 result'
  printf '%s' "$value"
}
file_size() { wc -c < "$1" | tr -d '[:space:]'; }
reviewed_notes=$(cat "${notes[1]}")
reviewed_notes_hash=$(sha256_file "${notes[1]}") || die 'Cannot hash reviewed notes'
verify_notes() {
  [ -f "${notes[1]}" ] && [ ! -L "${notes[1]}" ] && [ "$(sha256_file "${notes[1]}")" = "$reviewed_notes_hash" ] || die 'Reviewed local notes changed during resume'
}
local_names=(); local_paths=(); local_hashes=(); local_sizes=(); local_count=0
find_local() {
  local wanted=$1 index
  found_index=-1
  for ((index=0; index<local_count; index++)); do
    if [ "${local_names[index]}" = "$wanted" ]; then found_index=$index; return; fi
  done
}
for asset in "${assets[@]}"; do
  [[ $asset != *'#'* ]] || die 'Resume requires plain artifact paths, without gh display-label suffixes'
  [ -f "$asset" ] && [ ! -L "$asset" ] || die "Resume asset must be a regular, unlinked file: $asset"
  asset_dir=$(cd "$(dirname "$asset")" && pwd -P) || die 'Cannot resolve local artifact directory'
  asset_name=$(basename "$asset"); asset_path="$asset_dir/$asset_name"
  expected=0
  for ((index=0; index<expected_count; index++)); do [ "${expected_names[index]}" != "$asset_name" ] || expected=1; done
  [ "$expected" = 1 ] || die "Unexpected local asset: $asset_name"
  find_local "$asset_name"; [ "$found_index" = -1 ] || die "Duplicate local asset: $asset_name"
  asset_size=$(file_size "$asset_path") || die 'Cannot read local artifact size'
  [[ $asset_size =~ ^[0-9]+$ ]] && [ "$asset_size" -gt 0 ] || die "Empty or invalid local asset: $asset_name"
  asset_hash=$(sha256_file "$asset_path") || die 'Cannot hash local artifact'
  local_names+=("$asset_name"); local_paths+=("$asset_path"); local_sizes+=("$asset_size"); local_hashes+=("$asset_hash"); local_count=$((local_count+1))
done
[ "$local_count" = "$expected_count" ] || die 'Resume requires the complete expected local asset set'
for ((index=0; index<local_count; index++)); do
  case ${local_names[index]} in
    *.sha256)
      checksum=$(cat "${local_paths[index]}"); checksum=${checksum%$'\r'}
      [[ $checksum != *$'\n'* && $checksum != *$'\r'* ]] || die 'Checksum must contain one archive/hash line'
      IFS=$' \t' read -r declared_hash declared_name extra <<< "$checksum"
      [[ $declared_hash =~ ^[A-Fa-f0-9]{64}$ && -z $extra ]] || die 'Malformed local checksum sidecar'
      declared_name=${declared_name#\*}; archive_name=${local_names[index]%.sha256}
      [ "$declared_name" = "$archive_name" ] || die 'Local checksum basename differs from its archive'
      find_local "$archive_name"; [ "$found_index" -ge 0 ] || die 'Local checksum has no matching archive'
      declared_hash=$(printf '%s' "$declared_hash" | tr 'A-F' 'a-f')
      [ "$declared_hash" = "${local_hashes[found_index]}" ] || die 'Local archive checksum differs'
      ;;
  esac
done

tag_object=$(git -C "$root" rev-parse --verify "refs/tags/$tag") || die 'Resume requires the frozen local tag'
[[ $tag_object =~ ^[a-f0-9]{40,64}$ ]] || die 'Invalid local tag object'
verify_tag() {
  local remote_object
  remote_object=$(gh api "repos/{owner}/{repo}/git/ref/tags/$tag" --jq '.object.sha') || die 'Cannot verify the remote tag'
  [ "$remote_object" = "$tag_object" ] || die 'Remote tag object differs from the frozen local tag'
}
verify_tag

umask 077
temp_parent=$(cd "${TMPDIR:-/tmp}" && pwd -P) || die 'Cannot resolve temporary parent'
resume_root=$(mktemp -d "$temp_parent/audiolib-release-resume.XXXXXX") || die 'Cannot create owned resume evidence'
cleanup() {
  case $resume_root in
    "$temp_parent"/audiolib-release-resume.*) rm -f -- "$resume_root/snapshot"; rmdir -- "$resume_root" ;;
    *) echo 'Refusing resume temporary cleanup outside owned root' >&2 ;;
  esac
}
trap cleanup EXIT
snapshot="$resume_root/snapshot"; body_boundary='release-create-body-boundary'
remote_names=(); remote_sizes=(); remote_hashes=(); remote_ids=(); remote_count=0
fetch_snapshot() {
  local kind rid rtag title is_draft is_pre extra name state size digest boundary_seen=0 index
  gh api "repos/{owner}/{repo}/releases/$release_id" --jq '(["release", .id, .tag_name, .name, .draft, .prerelease] | @tsv), (.assets[] | ["asset", .id, .name, .state, .size, (.digest // "")] | @tsv), "release-create-body-boundary", (.body // "")' > "$snapshot" || die 'Cannot inspect draft identity/assets'
  IFS=$'\t' read -r kind rid rtag title is_draft is_pre extra < "$snapshot"
  [ "$kind" = release ] && [ "$rid" = "$release_id" ] && [ "$rtag" = "$tag" ] && [ "$title" = "AudioLib Explorer $tag" ] && [ -z "$extra" ] || die 'Existing release tag/title/ID differs'
  [ "$is_draft" = "$expected_draft" ] || die 'Existing release is not in the expected draft/public state'
  [ "$is_pre" = "$expected_prerelease" ] || die 'Existing release prerelease flag differs'
  remote_names=(); remote_sizes=(); remote_hashes=(); remote_ids=(); remote_count=0
  while IFS=$'\t' read -r kind rid name state size digest extra; do
    if [ "$kind" = "$body_boundary" ]; then boundary_seen=1; break; fi
    [ "$kind" = asset ] && [[ $rid =~ ^[1-9][0-9]*$ && $size =~ ^[0-9]+$ && -z $extra ]] || die 'Invalid remote asset metadata'
    [ "$state" = uploaded ] || die 'Remote asset is incomplete; inspect the original uploader before recovery'
    for ((index=0; index<remote_count; index++)); do
      [ "${remote_names[index]}" != "$name" ] && [ "${remote_ids[index]}" != "$rid" ] || die 'Duplicate remote asset name/ID'
    done
    find_local "$name"; [ "$found_index" -ge 0 ] || die "Unexpected remote asset: $name"
    [[ $digest =~ ^sha256:[A-Fa-f0-9]{64}$ ]] || die "Remote asset lacks a trustworthy SHA256 digest: $name"
    digest=$(printf '%s' "${digest#sha256:}" | tr 'A-F' 'a-f')
    [ "$size" = "${local_sizes[found_index]}" ] && [ "$digest" = "${local_hashes[found_index]}" ] || die "Remote asset size/SHA256 conflicts: $name"
    remote_names+=("$name"); remote_ids+=("$rid"); remote_sizes+=("$size"); remote_hashes+=("$digest"); remote_count=$((remote_count+1))
  done < <(sed '1d' "$snapshot")
  [ "$boundary_seen" = 1 ] || die 'Remote release body is missing'
  remote_notes=$(sed "1,/^$body_boundary\$/d" "$snapshot")
  [ "$remote_notes" = "$reviewed_notes" ] || die 'Existing draft notes differ from the reviewed notes file'
}
find_remote() {
  local wanted=$1 index
  remote_found=0
  for ((index=0; index<remote_count; index++)); do [ "${remote_names[index]}" != "$wanted" ] || remote_found=1; done
}
verify_local() {
  [ -f "${local_paths[$1]}" ] && [ ! -L "${local_paths[$1]}" ] && [ "$(file_size "${local_paths[$1]}")" = "${local_sizes[$1]}" ] && [ "$(sha256_file "${local_paths[$1]}")" = "${local_hashes[$1]}" ] || die 'Local accepted asset changed during resume'
}
expected_draft=true; expected_prerelease=false; [ -z "$suffix" ] || expected_prerelease=true
verify_notes
fetch_snapshot
missing=0
for ((index=0; index<local_count; index++)); do
  find_remote "${local_names[index]}"
  [ "$remote_found" = 0 ] || continue
  missing=$((missing+1)); verify_local "$index"
  if [ "$dry" = 1 ]; then printf '%q ' gh release upload "$tag" -- "${local_paths[index]}"; echo
  else gh release upload "$tag" -- "${local_paths[index]}" || exit $?; fi
done
if [ "$dry" = 1 ]; then
  if [ "$publish" = 1 ]; then
    echo 'After upload, re-verify the complete draft before this publication command:'
    if [ -n "$suffix" ]; then latest=false; else latest=true; fi
    printf '%q ' gh release edit "$tag" --draft=false "--latest=$latest"; echo
  fi
  printf 'Draft resume preview: existing=%s missing=%s; no mutations performed.\n' "$remote_count" "$missing"
  exit 0
fi
for ((index=0; index<local_count; index++)); do verify_local "$index"; done
verify_notes
fetch_snapshot
[ "$remote_count" = "$local_count" ] || die 'Draft asset set remains incomplete after uploads'
verify_tag
if [ "$publish" = 1 ]; then
  if [ -n "$suffix" ]; then latest=false; else latest=true; fi
  gh release edit "$tag" --draft=false "--latest=$latest" || exit $?
  verify_tag; verify_notes
  expected_draft=false; fetch_snapshot
  [ "$remote_count" = "$local_count" ] || die 'Published asset set differs'
  printf 'Published resumed release %s (%s verified assets).\n' "$tag" "$remote_count"
else
  printf 'Draft %s resumed and verified (%s assets); left unpublished. Use --resume-draft --publish to finalize.\n' "$release_id" "$remote_count"
fi
