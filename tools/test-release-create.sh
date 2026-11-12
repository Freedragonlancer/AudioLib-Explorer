#!/usr/bin/env bash
# tools/test-release-create.sh — isolated argv/draft-resume checks; never contacts GitHub.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
v=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"");sub("version=","");print;exit}' "$root/Cargo.toml")
temp_parent=$(cd "${TMPDIR:-/tmp}" && pwd -P)
test_root=$(mktemp -d "$temp_parent/audiolib-release-create.XXXXXX")
cleanup() {
  case $test_root in
    "$temp_parent"/audiolib-release-create.*) rm -rf -- "$test_root" ;;
    *) echo "Refusing cleanup outside owned test root" >&2 ;;
  esac
}
trap cleanup EXIT
mkdir "$test_root/bin"
cat > "$test_root/bin/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\0' "$@" > "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
state=$AUDIOLIB_RELEASE_CREATE_STATE
record_mutation() {
  local count=0
  [ ! -f "$state/mutations" ] || count=$(cat "$state/mutations")
  printf '%s\0' "$@" > "$state/mutation-$count.argv"
  printf '%s\n' "$((count+1))" > "$state/mutations"
}
if [ "$1" = api ]; then
  shift; [ "${1:-}" != --paginate ] || shift
  endpoint=$1
  case $endpoint in
    'repos/{owner}/{repo}/releases?per_page=100')
      [ "${AUDIOLIB_RELEASE_LIST_FAIL:-0}" = 0 ] || exit 19
      [ -f "$state/id" ] || exit 0
      printf '%s\t%s\n' "$(cat "$state/id")" "$(cat "$state/draft")"
      if [ "${AUDIOLIB_RELEASE_DUPLICATE:-0}" = 1 ]; then printf '9999\ttrue\n'; fi
      ;;
    repos/\{owner\}/\{repo\}/git/ref/tags/*)
      printf '%s\n' "${AUDIOLIB_RESUME_REMOTE_TAG:-1111111111111111111111111111111111111111}"
      ;;
    repos/\{owner\}/\{repo\}/releases/*)
      [ -f "$state/id" ] || exit 44
      snapshot_count=0; [ ! -f "$state/snapshots" ] || snapshot_count=$(cat "$state/snapshots")
      snapshot_count=$((snapshot_count+1)); printf '%s\n' "$snapshot_count" > "$state/snapshots"
      printf 'release\t%s\t%s\t%s\t%s\t%s\n' "$(cat "$state/id")" "$(cat "$state/tag")" "$(cat "$state/title")" "$(cat "$state/draft")" "$(cat "$state/prerelease")"
      cat "$state/assets"
      printf 'release-create-body-boundary\n'
      if [ "$snapshot_count" -ge 2 ] && [ "${AUDIOLIB_RESUME_NOTES_FILE_RACE:-0}" = 1 ]; then
        printf 'changed concurrently\n' > "$AUDIOLIB_RESUME_NOTES_PATH"
        cat "$AUDIOLIB_RESUME_NOTES_PATH"
      elif [ "$snapshot_count" -ge 2 ] && [ "${AUDIOLIB_RESUME_NOTES_RACE:-0}" = 1 ]; then printf 'changed concurrently\n'; else cat "$state/notes"; fi
      ;;
    *) echo "Unexpected fake gh API route" >&2; exit 97 ;;
  esac
  exit 0
fi
[ "$1" = release ] || exit 97
case $2 in
  create)
    record_mutation "$@"
    exit "${AUDIOLIB_RELEASE_CREATE_EXIT:-0}"
    ;;
  upload)
    [ "$#" = 5 ] && [ "$4" = -- ] || exit 97
    record_mutation "$@"
    asset=$5; name=$(basename "$asset")
    [ "${AUDIOLIB_RESUME_UPLOAD_FAIL:-0}" = 0 ] || exit 41
    bytes=$(wc -c < "$asset" | tr -d '[:space:]')
    if command -v sha256sum >/dev/null 2>&1; then hash=$(sha256sum "$asset"); else hash=$(shasum -a 256 "$asset"); fi
    hash=${hash%% *}; rows=$(wc -l < "$state/assets" | tr -d '[:space:]')
    printf 'asset\t%s\t%s\tuploaded\t%s\tsha256:%s\n' "$((1000+rows))" "$name" "$bytes" "$hash" >> "$state/assets"
    [ "${AUDIOLIB_RESUME_UPLOAD_AFTER_STORE_FAIL:-0}" = 0 ] || exit 43
    ;;
  edit)
    [ "$#" = 5 ] && [ "$4" = --draft=false ] || exit 97
    record_mutation "$@"; printf 'false\n' > "$state/draft"
    ;;
  *) echo 'Unexpected fake gh mutation' >&2; exit 97 ;;
esac
FAKE_GH
cat > "$test_root/bin/git" <<'FAKE_GIT'
#!/usr/bin/env bash
set -euo pipefail
[ "$#" = 5 ] && [ "$1" = -C ] && [ "$3" = rev-parse ] && [ "$4" = --verify ] || exit 97
[ "${AUDIOLIB_RESUME_LOCAL_TAG_FAIL:-0}" = 0 ] || exit 12
printf '%s\n' 1111111111111111111111111111111111111111
FAKE_GIT
cat > "$test_root/bin/pwsh" <<'FAKE_PWSH'
#!/usr/bin/env bash
set -euo pipefail
[ "$#" = 7 ] && [ "$1" = -NoLogo ] && [ "$2" = -NoProfile ] && [ "$3" = -File ] && [ "$5" = -Tag ] && [ "$7" = -ListExpectedAssets ] || exit 97
[ "${AUDIOLIB_RESUME_EXPECTED_FAIL:-0}" = 0 ] || exit 13
cat "$AUDIOLIB_RELEASE_CREATE_STATE/expected"
FAKE_PWSH
chmod +x "$test_root/bin/gh" "$test_root/bin/git" "$test_root/bin/pwsh"
export PATH="$test_root/bin:$PATH"
export AUDIOLIB_RELEASE_CREATE_CAPTURE="$test_root/gh.argv"
export AUDIOLIB_RELEASE_CREATE_STATE="$test_root/state"
mkdir "$AUDIOLIB_RELEASE_CREATE_STATE"
export AUDIOLIB_RELEASE_CREATE_EXIT=0

fail() { echo "FAIL: $*" >&2; exit 1; }
run() {
  rm -f -- "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
  "$here/release-create.sh" "$@"
  [ -f "$AUDIOLIB_RELEASE_CREATE_CAPTURE" ] || fail "gh was not invoked"
  received=()
  while IFS= read -r -d '' argument; do received+=("$argument"); done < "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
  [ "${received[0]}" = release ] && [ "${received[1]}" = create ] || fail "wrong gh command"
}
assert_value() {
  local option=$1 expected=$2 i
  for ((i=3; i<${#received[@]}; i++)); do
    [ "${received[i]}" != -- ] || break
    if [ "${received[i]}" = "$option" ]; then
      [ "${received[i+1]}" = "$expected" ] || fail "$option value changed"
      return
    fi
  done
  fail "missing $option"
}
assert_flag_count() {
  local option=$1 expected=$2 count=0 i
  for ((i=3; i<${#received[@]}; i++)); do
    case ${received[i]} in
      --) break ;;
      --title|--notes-file) i=$((i+1)) ;;
      *) if [ "${received[i]}" = "$option" ]; then count=$((count+1)); fi ;;
    esac
  done
  [ "$count" = "$expected" ] || fail "$option count $count != $expected"
}
assert_assets() {
  local start=-1 i
  for ((i=3; i<${#received[@]}; i++)); do
    if [ "${received[i]}" = -- ]; then start=$((i+1)); break; fi
  done
  [ "$start" -ge 0 ] || fail "missing downstream asset separator"
  [ "$((${#received[@]}-start))" = "$#" ] || fail "asset count changed"
  for argument in "$@"; do
    [ "${received[start]}" = "$argument" ] || fail "asset argument changed"
    start=$((start+1))
  done
}
assert_identity() {
  local tag=$1
  [ "${received[2]}" = "$tag" ] || fail "tag changed"
  assert_flag_count --verify-tag 1
  assert_value --title "AudioLib Explorer $tag"
}
reject() {
  local message=$1 error
  shift
  rm -f -- "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
  if error=$("$here/release-create.sh" "$@" 2>&1); then fail "invalid input accepted"; fi
  [[ $error == *"$message"* && $error != *unbound* ]] || fail "wrong rejection: $error"
  [ ! -e "$AUDIOLIB_RELEASE_CREATE_CAPTURE" ] || fail "invalid input reached gh"
}

for suffix in '' '-beta.3' '-rc.2'; do
  tag="v$v$suffix"
  for draft in no yes; do
    if [ "$draft" = yes ]; then run "$tag" --draft; else run "$tag"; fi
    assert_identity "$tag"
    assert_value --notes-file "$root/doc/Rust_Release_Notes.md"
    if [ "$draft" = yes ]; then assert_flag_count --draft 1; else assert_flag_count --draft 0; fi
    if [ -n "$suffix" ]; then assert_flag_count --prerelease 1; else assert_flag_count --prerelease 0; fi
    if [ -n "$suffix" ] || [ "$draft" = yes ]; then
      assert_flag_count --latest=false 1
    else
      assert_flag_count --latest=false 0
    fi
    assert_flag_count --latest=true 0
  done
done

notes="$test_root/release notes.md"
printf 'Owned test notes\n' > "$notes"
run "v$v" --notes-file "$notes" "$test_root/client package.zip#Client package" "$test_root/server.tar.gz"
assert_identity "v$v"
assert_value --notes-file "$notes"
assert_assets "$test_root/client package.zip#Client package" "$test_root/server.tar.gz"
run "v$v" "--notes-file=$notes" --draft -- --draft --dry-run 'literal asset.zip'
assert_value --notes-file "$notes"
assert_flag_count --draft 1
assert_flag_count --latest=false 1
assert_assets --draft --dry-run 'literal asset.zip'
run "v$v" -- --draft
assert_flag_count --draft 0
assert_flag_count --latest=false 0
assert_assets --draft

rm -f -- "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
out=$("$here/release-create.sh" "v$v" --draft --dry-run --notes-file "$notes" 'client package.zip')
[[ $out == gh\ release\ create\ * && $out == *--draft* && $out == *--latest=false* ]] || fail "draft preview lost publication flags"
[ ! -e "$AUDIOLIB_RELEASE_CREATE_CAPTURE" ] || fail "--dry-run invoked gh"

reject 'requires a value' "v$v" --notes-file
reject 'requires a value' "v$v" --notes-file ''
reject 'requires a value' "v$v" --notes-file=
reject 'requires a value' "v$v" --notes-file --draft
reject 'unknown option: --bogus' "v$v" --draft --bogus
reject 'unknown option: --draft=true' "v$v" --draft=true
reject 'unknown option: --latest' "v$v" --latest
reject 'does not match Cargo.toml' v0.0.1 --draft
reject 'must be vX.Y.Z' 0.1.0 --draft
reject 'usage:'

export AUDIOLIB_RELEASE_CREATE_EXIT=23
if "$here/release-create.sh" "v$v" --draft; then fail "gh failure was hidden"; else status=$?; fi
[ "$status" = 23 ] || fail "gh exit status changed"
export AUDIOLIB_RELEASE_CREATE_EXIT=0

state=$AUDIOLIB_RELEASE_CREATE_STATE
payload="$test_root/accepted payload"
mkdir "$payload"
hash_file() {
  local hash
  if command -v sha256sum >/dev/null 2>&1; then hash=$(sha256sum "$1"); else hash=$(shasum -a 256 "$1"); fi
  printf '%s' "${hash%% *}"
}
reset_draft() {
  local suffix=${1:-} archive hash
  rm -f -- "$state/"*
  tag="v$v$suffix"
  printf '12345\n' > "$state/id"; printf 'true\n' > "$state/draft"
  printf '%s\n' "$tag" > "$state/tag"; printf 'AudioLib Explorer %s\n' "$tag" > "$state/title"
  if [ -n "$suffix" ]; then printf 'true\n' > "$state/prerelease"; else printf 'false\n' > "$state/prerelease"; fi
  cp "$notes" "$state/notes"; : > "$state/assets"; : > "$state/expected"
  all_assets=()
  for archive in "AudioLibExplorer-$tag-win-x64-portable.zip" "AudioLibExplorer-$tag-macos-arm64.zip" "AudioLibServer-$tag-macos-arm64.tar.gz" "AudioLibServer-$tag-linux-x64.tar.gz" "AudioLibServer-$tag-docker-linux-amd64.tar.gz" "AudioLibServer-$tag-docker-linux-arm64.tar.gz" "AudioLibServer-$tag-win-x64.zip"; do
    printf 'accepted fixture %s\n' "$archive" > "$payload/$archive"
    hash=$(hash_file "$payload/$archive")
    printf '%s  %s\n' "$hash" "$archive" > "$payload/$archive.sha256"
    printf '%s\n%s\n' "$archive" "$archive.sha256" >> "$state/expected"
    all_assets+=("$payload/$archive" "$payload/$archive.sha256")
  done
}
seed_remote() {
  local count=$1 index name hash bytes
  : > "$state/assets"
  for ((index=0; index<count; index++)); do
    name=$(basename "${all_assets[index]}"); bytes=$(wc -c < "${all_assets[index]}" | tr -d '[:space:]'); hash=$(hash_file "${all_assets[index]}")
    printf 'asset\t%s\t%s\tuploaded\t%s\tsha256:%s\n' "$((1000+index))" "$name" "$bytes" "$hash" >> "$state/assets"
  done
}
mutation_count() { if [ -f "$state/mutations" ]; then cat "$state/mutations"; else printf 0; fi; }
resume_command() { "$here/release-create.sh" "$tag" --resume-draft --notes-file "$notes" "$@" -- "${all_assets[@]}"; }
reject_resume() {
  local message=$1 error before after; shift
  before=$(mutation_count)
  if error=$(resume_command "$@" 2>&1); then fail 'invalid draft resume accepted'; fi
  after=$(mutation_count)
  [[ $error == *"$message"* && $error != *unbound* ]] || fail "wrong resume rejection: $error"
  [ "$after" = "$before" ] || fail 'invalid draft reached a mutation'
}
assert_mutation() {
  local number=$1 verb=$2
  mutation=()
  while IFS= read -r -d '' argument; do mutation+=("$argument"); done < "$state/mutation-$number.argv"
  [ "${mutation[0]}" = release ] && [ "${mutation[1]}" = "$verb" ] || fail 'wrong resume mutation'
  for argument in "${mutation[@]}"; do [ "$argument" != --clobber ] || fail 'resume clobbered an asset'; done
}

# An existing draft can be found by authenticated listing while the tag endpoint
# is unavailable: the fake has no releases/tags API route at all.
reset_draft
seed_remote 12
out=$(resume_command)
[[ $out == *'left unpublished'* ]] || fail 'resume published without explicit request'
[ "$(mutation_count)" = 2 ] || fail 'matching assets were uploaded again'
[ "$(cat "$state/draft")" = true ] || fail 'draft did not remain unpublished'
assert_mutation 0 upload; [ "${mutation[4]}" = "${all_assets[12]}" ] || fail 'wrong first missing asset'
assert_mutation 1 upload; [ "${mutation[4]}" = "${all_assets[13]}" ] || fail 'wrong second missing asset'
before=$(mutation_count); resume_command >/dev/null
[ "$(mutation_count)" = "$before" ] || fail 'complete draft was uploaded again'

for suffix in '' '-beta.3' '-rc.2'; do
  reset_draft "$suffix"; seed_remote 14
  resume_command --publish >/dev/null
  [ "$(mutation_count)" = 1 ] && [ "$(cat "$state/draft")" = false ] || fail 'explicit publish did not finalize once'
  assert_mutation 0 edit
  if [ -n "$suffix" ]; then [ "${mutation[4]}" = --latest=false ] || fail 'prerelease became Latest'; else [ "${mutation[4]}" = --latest=true ] || fail 'stable release lost Latest'; fi
done

reset_draft; seed_remote 12
out=$(resume_command --publish --dry-run)
[[ $out == *'gh release upload'* && $out == *'gh release edit'* && $out == *'no mutations performed'* ]] || fail 'resume preview incomplete'
[ "$(mutation_count)" = 0 ] || fail 'resume preview mutated draft'

reset_draft; seed_remote 14
printf 'false\n' > "$state/draft"; reject_resume 'unpublished draft'
reset_draft; printf 'Other title\n' > "$state/title"; reject_resume 'tag/title/ID'
reset_draft; printf 'v0.0.0\n' > "$state/tag"; reject_resume 'tag/title/ID'
reset_draft; printf 'true\n' > "$state/prerelease"; reject_resume 'prerelease flag'
reset_draft; printf 'Different notes\n' > "$state/notes"; reject_resume 'notes differ'
reset_draft; seed_remote 1
sed 's/sha256:[a-f0-9]*/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_resume 'size/SHA256 conflicts'
reset_draft; seed_remote 1
sed 's/sha256:[a-f0-9]*$/missing/' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_resume 'trustworthy SHA256'
reset_draft; seed_remote 1
awk 'BEGIN { FS=OFS="\t" } { $4="starter"; print }' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_resume 'incomplete'
reset_draft; seed_remote 1
awk 'BEGIN { FS=OFS="\t" } { $5=$5+1; print }' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_resume 'size/SHA256 conflicts'
reset_draft; seed_remote 1; cat "$state/assets" > "$state/duplicate"; cat "$state/duplicate" >> "$state/assets"; reject_resume 'Duplicate remote'
reset_draft; printf 'asset\t9000\textra.zip\tuploaded\t1\tsha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' > "$state/assets"; reject_resume 'Unexpected remote'
reset_draft; export AUDIOLIB_RELEASE_DUPLICATE=1; reject_resume 'Multiple same-tag'; unset AUDIOLIB_RELEASE_DUPLICATE
reset_draft; export AUDIOLIB_RELEASE_LIST_FAIL=1; reject_resume 'absence is unproven'; unset AUDIOLIB_RELEASE_LIST_FAIL
reset_draft; export AUDIOLIB_RESUME_REMOTE_TAG=2222222222222222222222222222222222222222; reject_resume 'tag object differs'; unset AUDIOLIB_RESUME_REMOTE_TAG
reset_draft; export AUDIOLIB_RESUME_LOCAL_TAG_FAIL=1; reject_resume 'frozen local tag'; unset AUDIOLIB_RESUME_LOCAL_TAG_FAIL
reset_draft; export AUDIOLIB_RESUME_EXPECTED_FAIL=1; reject_resume 'official expected asset set'; unset AUDIOLIB_RESUME_EXPECTED_FAIL

# Missing local assets and duplicate basenames cannot define a smaller contract.
reset_draft
saved=${all_assets[13]}; unset 'all_assets[13]'; reject_resume 'complete expected local asset set'
all_assets+=("$saved"); all_assets+=("${all_assets[0]}"); reject_resume 'Duplicate local'
reset_draft; printf 'bad checksum\n' > "${all_assets[1]}"; reject_resume 'Malformed local checksum'
reset_draft
hash=$(hash_file "${all_assets[0]}"); printf '%s  %s\r\n' "$hash" "$(basename "${all_assets[0]}")" > "${all_assets[1]}"
seed_remote 14; resume_command >/dev/null; [ "$(mutation_count)" = 0 ] || fail 'CRLF checksum triggered upload'

# Network failure after one server-side accepted upload must not lose that asset.
reset_draft; seed_remote 12
export AUDIOLIB_RESUME_UPLOAD_AFTER_STORE_FAIL=1
if resume_command >/dev/null 2>&1; then fail 'upload failure hidden'; else status=$?; fi
[ "$status" = 43 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 13 ] || fail 'partial successful asset lost'
unset AUDIOLIB_RESUME_UPLOAD_AFTER_STORE_FAIL
resume_command >/dev/null
[ "$(mutation_count)" = 2 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 14 ] || fail 'retry retransmitted completed asset'
assert_mutation 1 upload; [ "${mutation[4]}" = "${all_assets[13]}" ] || fail 'retry selected an existing asset'

# A concurrent metadata change prevents final publication after the upload.
reset_draft; seed_remote 12; export AUDIOLIB_RESUME_NOTES_RACE=1
if resume_command --publish >/dev/null 2>&1; then fail 'concurrent notes change published'; fi
[ "$(mutation_count)" = 2 ] && [ "$(cat "$state/draft")" = true ] || fail 'changed notes reached publication'
unset AUDIOLIB_RESUME_NOTES_RACE
reset_draft; seed_remote 14
original_notes=$(cat "$notes")
export AUDIOLIB_RESUME_NOTES_FILE_RACE=1 AUDIOLIB_RESUME_NOTES_PATH="$notes"
if resume_command --publish >/dev/null 2>&1; then fail 'concurrent matching local/remote notes change published'; fi
[ "$(mutation_count)" = 0 ] && [ "$(cat "$state/draft")" = true ] || fail 'changed reviewed notes reached publication'
unset AUDIOLIB_RESUME_NOTES_FILE_RACE AUDIOLIB_RESUME_NOTES_PATH
printf '%s\n' "$original_notes" > "$notes"

# Normal create also checks authenticated drafts before invoking create.
reset_draft
if "$here/release-create.sh" "$tag" --notes-file "$notes" >/dev/null 2>&1; then fail 'duplicate draft creation accepted'; fi
[ "$(mutation_count)" = 0 ] || fail 'existing draft was recreated'
reject '--publish requires --resume-draft' "$tag" --publish
reject '--publish conflicts with --draft' "$tag" --resume-draft --draft --publish

echo 'release-create tests passed (offline fake gh/pwsh/git; legacy create, draft discovery, exact-set resume, conflicts, partial retry, explicit publish)'
