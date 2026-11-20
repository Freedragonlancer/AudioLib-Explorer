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
state=$AUDIOLIB_RELEASE_CREATE_STATE
locked=0
lock_state() {
  local attempt
  for ((attempt=0; attempt<400; attempt++)); do
    if mkdir "$state/lock" 2>/dev/null; then locked=1; return; fi
    sleep 0.05
  done
  echo 'Timed out acquiring fake GitHub state lock' >&2
  exit 96
}
unlock_state() {
  if [ "$locked" = 1 ]; then
    rmdir "$state/lock"
    locked=0
  fi
}
await_marker() {
  local file=$1 attempt
  for ((attempt=0; attempt<400; attempt++)); do
    [ ! -f "$file" ] || return 0
    sleep 0.05
  done
  printf 'Timed out waiting for mock upload barrier: %s\n' "$file" >&2
  exit 96
}
require_pinned=${AUDIOLIB_REQUIRE_API_HOST:-0}
if [ "$require_pinned" = 1 ]; then
  [ "$(pwd -P)" = "$AUDIOLIB_EXPECT_API_CWD" ] || exit 97
  lock_state
  trap unlock_state EXIT
fi
printf '%s\0' "$@" > "$AUDIOLIB_RELEASE_CREATE_CAPTURE"
record_mutation() {
  local count=0
  [ ! -f "$state/mutations" ] || count=$(cat "$state/mutations")
  printf '%s\0' "$@" > "$state/mutation-$count.argv"
  printf '%s\n' "$((count+1))" > "$state/mutations"
}
if [ "$1" = api ]; then
  argv=("$@")
  shift
  pinned=0
  if [ "${1:-}" = --hostname ]; then
    [ "${2:-}" = github.com ] || exit 97
    pinned=1
    shift 2
  fi
  [ "$pinned" = "$require_pinned" ] || exit 97
  if [ "${1:-}" = --method ]; then
    [ "$pinned" = 1 ] || exit 97
    case $2 in
      POST)
        [ "$#" = 10 ] && [ "$3" = "https://uploads.github.com/repos/{owner}/{repo}/releases/$(cat "$state/id")/assets" ] &&
          [ "$4" = -f ] && [ "$5" = "name=$(basename "$9")" ] && [ "$6" = -H ] &&
          [ "$7" = 'Content-Type: application/octet-stream' ] && [ "$8" = --input ] && [ "${10}" = --silent ] || exit 97
        [ "$(cat "$state/draft")" = true ] || exit 97
        record_mutation "${argv[@]}"
        asset=$9; name=${5#name=}
        grep -Fqx -- "$name" "$state/expected" || exit 97
        if [ "${AUDIOLIB_INDEPENDENT_FAIL_BEFORE:-}" = "$name" ]; then
          echo 'gh: Forbidden (HTTP 403)' >&2
          exit 41
        fi
        if awk -F '\t' -v name="$name" '$3 == name { found=1 } END { exit !found }' "$state/assets"; then exit 42; fi
        bytes=$(wc -c < "$asset" | tr -d '[:space:]')
        if command -v sha256sum >/dev/null 2>&1; then hash=$(sha256sum "$asset"); else hash=$(shasum -a 256 "$asset"); fi
        hash=${hash%% *}; rows=$(wc -l < "$state/assets" | tr -d '[:space:]')
        id=$((1000+rows))
        if [ -n "${AUDIOLIB_INDEPENDENT_PEER:-}" ] && [[ $name != *.sha256 ]]; then
          printf 'asset\t%s\t%s\tstarter\t0\t\n' "$id" "$name" >> "$state/assets"
          : > "$state/arrived-$name"
          unlock_state
          await_marker "$state/arrived-$AUDIOLIB_INDEPENDENT_PEER"
          if [ "${AUDIOLIB_INDEPENDENT_HOLD:-0}" = 1 ]; then await_marker "$state/release-held-upload"; fi
          lock_state
          awk -F '\t' -v name="$name" '$3 != name' "$state/assets" > "$state/assets-next"
          mv "$state/assets-next" "$state/assets"
        fi
        printf 'asset\t%s\t%s\tuploaded\t%s\tsha256:%s\n' "$id" "$name" "$bytes" "$hash" >> "$state/assets"
        [ "${AUDIOLIB_INDEPENDENT_FAIL_AFTER:-}" != "$name" ] || exit 43
        ;;
      PATCH)
        [ "$#" = 8 ] && [ "$3" = "repos/{owner}/{repo}/releases/$(cat "$state/id")" ] &&
          [ "$4" = -F ] && [ "$5" = draft=false ] && [ "$6" = -f ] && [ "$8" = --silent ] || exit 97
        latest=true; if [ "$(cat "$state/prerelease")" = true ]; then latest=false; fi
        [ "$7" = "make_latest=$latest" ] && [ "$(cat "$state/draft")" = true ] || exit 97
        record_mutation "${argv[@]}"
        printf 'false\n' > "$state/draft"
        ;;
      *) exit 97 ;;
    esac
    exit 0
  fi
  [ "${1:-}" != --paginate ] || shift
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
      [ "$pinned" = 0 ] || [ "$endpoint" = "repos/{owner}/{repo}/releases/$(cat "$state/id")" ] || exit 97
      snapshot_count=0; [ ! -f "$state/snapshots" ] || snapshot_count=$(cat "$state/snapshots")
      snapshot_count=$((snapshot_count+1)); printf '%s\n' "$snapshot_count" > "$state/snapshots"
      is_draft=$(cat "$state/draft")
      if [ "$pinned" = 1 ] && [ "$is_draft" = false ]; then : > "$state/published-snapshot"; fi
      printf 'release\t%s\t%s\t%s\t%s\t%s\n' "$(cat "$state/id")" "$(cat "$state/tag")" "$(cat "$state/title")" "$is_draft" "$(cat "$state/prerelease")"
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
[ "$1" = release ] && [ "$require_pinned" = 0 ] || exit 97
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
export AUDIOLIB_EXPECT_API_CWD=$(cd "$root" && pwd -P)

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
stage_command() { "$here/release-create.sh" "$tag" --resume-draft --stage-pairs --notes-file "$notes" "$@"; }
reject_draft() {
  local message=$1 error before after; shift
  before=$(mutation_count)
  if error=$("$@" 2>&1); then fail 'invalid draft command accepted'; fi
  after=$(mutation_count)
  [[ $error == *"$message"* && $error != *unbound* ]] || fail "wrong draft rejection: $error"
  [ "$after" = "$before" ] || fail 'invalid draft reached a mutation'
}
reject_resume() { local message=$1; shift; reject_draft "$message" resume_command "$@"; }
read_mutation() {
  local number=$1 argument
  mutation=()
  while IFS= read -r -d '' argument; do mutation+=("$argument"); done < "$state/mutation-$number.argv"
}
assert_mutation() {
  local number=$1 verb=$2
  read_mutation "$number"
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

# A cumulative subset can stage complete pairs while the draft stays unpublished.
reset_draft
out=$(stage_command --dry-run -- "${all_assets[@]:0:2}")
[[ $out == *'gh release upload'* && $out != *'gh release edit'* && $out == *'no mutations performed'* ]] || fail 'stage preview did not limit itself to uploads'
[ "$(mutation_count)" = 0 ] && [ "$(cat "$state/draft")" = true ] || fail 'stage preview mutated draft'
stage_command -- "${all_assets[1]}" "${all_assets[0]}" >/dev/null
[ "$(mutation_count)" = 2 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 2 ] && [ "$(cat "$state/draft")" = true ] || fail 'first pair did not stage without publishing'
assert_mutation 0 upload; [ "${mutation[4]}" = "${all_assets[0]}" ] || fail 'wrong first staged archive'
assert_mutation 1 upload; [ "${mutation[4]}" = "${all_assets[1]}" ] || fail 'wrong first staged checksum'

stage_command -- "${all_assets[@]:0:4}" >/dev/null
[ "$(mutation_count)" = 4 ] && [ "$(cat "$state/draft")" = true ] || fail 'next cumulative pair retransmitted or published assets'
assert_mutation 2 upload; [ "${mutation[4]}" = "${all_assets[2]}" ] || fail 'wrong next staged archive'
assert_mutation 3 upload; [ "${mutation[4]}" = "${all_assets[3]}" ] || fail 'wrong next staged checksum'
stage_command -- "${all_assets[@]:0:4}" >/dev/null
[ "$(mutation_count)" = 4 ] || fail 'stage retry retransmitted matching assets'

# Publish still requires the complete local contract and verifies staged assets.
reject_draft 'complete expected local asset set' "$here/release-create.sh" "$tag" --resume-draft --publish --notes-file "$notes" -- "${all_assets[@]:0:4}"
stage_command -- "${all_assets[@]}" >/dev/null
[ "$(mutation_count)" = 14 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 14 ] && [ "$(cat "$state/draft")" = true ] || fail 'complete staged set did not remain a draft'
assert_mutation 4 upload; [ "${mutation[4]}" = "${all_assets[4]}" ] || fail 'staging complete set retransmitted an earlier batch'
resume_command --publish >/dev/null
[ "$(mutation_count)" = 15 ] && [ "$(cat "$state/draft")" = false ] || fail 'finalization of a complete draft did more than publish once'
assert_mutation 14 edit; [ "${mutation[4]}" = --latest=true ] || fail 'staged stable release lost Latest'

# Recover an interrupted pair by uploading only its missing checksum.
reset_draft
export AUDIOLIB_RESUME_UPLOAD_AFTER_STORE_FAIL=1
if stage_command -- "${all_assets[@]:0:2}" >/dev/null 2>&1; then fail 'stage upload failure hidden'; else status=$?; fi
unset AUDIOLIB_RESUME_UPLOAD_AFTER_STORE_FAIL
[ "$status" = 43 ] && [ "$(mutation_count)" = 1 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 1 ] || fail 'interrupted stage lost its accepted archive'
stage_command -- "${all_assets[@]:0:2}" >/dev/null
[ "$(mutation_count)" = 2 ] && [ "$(cat "$state/draft")" = true ] || fail 'pair retry retransmitted or published'
assert_mutation 1 upload; [ "${mutation[4]}" = "${all_assets[1]}" ] || fail 'pair retry did not select the missing checksum'

# A stage call must supply valid pairs and all previously staged local bytes.
reset_draft
reject_draft 'at least one archive and checksum pair' stage_command --
reject_draft 'archive and checksum' stage_command -- "${all_assets[0]}"
reject_draft 'archive and checksum' stage_command -- "${all_assets[1]}"
reject_draft 'Duplicate local' stage_command -- "${all_assets[@]:0:2}" "${all_assets[0]}"
printf 'owned invalid asset\n' > "$payload/unknown.zip"
reject_draft 'Unexpected local' stage_command -- "${all_assets[@]:0:2}" "$payload/unknown.zip"

reset_draft; seed_remote 2
reject_draft 'Unexpected remote' stage_command -- "${all_assets[@]:2:2}"
reset_draft
printf 'asset\t9000\textra.zip\tuploaded\t1\tsha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' > "$state/assets"
reject_draft 'Unexpected remote' stage_command -- "${all_assets[@]:0:2}"
reset_draft; seed_remote 1
sed 's/sha256:[a-f0-9]*/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_draft 'size/SHA256 conflicts' stage_command -- "${all_assets[@]:0:2}"
reset_draft; seed_remote 1
awk 'BEGIN { FS=OFS="\t" } { $4="starter"; print }' "$state/assets" > "$state/rewrite"; mv "$state/rewrite" "$state/assets"
reject_draft 'incomplete' stage_command -- "${all_assets[@]:0:2}"
reset_draft; seed_remote 1; cp "$state/assets" "$state/duplicate"; cat "$state/duplicate" >> "$state/assets"
reject_draft 'Duplicate remote' stage_command -- "${all_assets[@]:0:2}"

reject '--stage-pairs requires --resume-draft' "$tag" --stage-pairs
reject '--stage-pairs conflicts with --publish' "$tag" --resume-draft --stage-pairs --publish
reject '--stage-pairs conflicts with --draft' "$tag" --resume-draft --stage-pairs --draft

# Normal create also checks authenticated drafts before invoking create.
reset_draft
if "$here/release-create.sh" "$tag" --notes-file "$notes" >/dev/null 2>&1; then fail 'duplicate draft creation accepted'; fi
[ "$(mutation_count)" = 0 ] || fail 'existing draft was recreated'
reject '--publish requires --resume-draft' "$tag" --publish
reject '--publish conflicts with --draft' "$tag" --resume-draft --draft --publish

independent_command() {
  (cd "$test_root" && AUDIOLIB_REQUIRE_API_HOST=1 "$here/release-create.sh" "$tag" --resume-draft --stage-independent --draft-id 12345 --notes-file "$notes" "$@")
}
publish_staged_command() {
  (cd "$test_root" && AUDIOLIB_REQUIRE_API_HOST=1 "$here/release-create.sh" "$tag" --resume-draft --publish-staged --draft-id 12345 --notes-file "$notes" "$@")
}
assert_independent_upload() {
  local number=$1 asset=$2
  read_mutation "$number"
  [ "${mutation[0]}" = api ] && [ "${mutation[1]}" = --hostname ] &&
    [ "${mutation[2]}" = github.com ] && [ "${mutation[4]}" = POST ] &&
    [ "${mutation[7]}" = "name=$(basename "$asset")" ] && [ "${mutation[11]}" = "$asset" ] ||
    fail "unexpected independent upload at mutation $number"
}

assert_pair_status() {
  local missing=$1 before out
  shift
  before=$(mutation_count)
  out=$(independent_command --pair-status "$@")
  [ "$out" = "RELEASE_PAIR_STATUS {\"tag\":\"$tag\",\"draft_id\":\"12345\",\"archive\":\"$amd_name\",\"missing\":$missing}" ] ||
    fail "wrong read-only pair status: $out"
  [ "$(mutation_count)" = "$before" ] || fail 'pair status mutated the draft'
}

probe_hash_log="$test_root/probe-hash-paths"
probe_hash_tool=$(command -v sha256sum || command -v shasum)
mkdir "$test_root/hash-probe-bin"
cat > "$test_root/hash-probe-bin/sha256sum" <<'HASH_PROBE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$1" >> "$AUDIOLIB_TEST_HASH_LOG"
case "$AUDIOLIB_TEST_HASH_TOOL" in
  */shasum) exec "$AUDIOLIB_TEST_HASH_TOOL" -a 256 "$@" ;;
  *) exec "$AUDIOLIB_TEST_HASH_TOOL" "$@" ;;
esac
HASH_PROBE
chmod +x "$test_root/hash-probe-bin/sha256sum"
trace_pair_hashes() {
  : > "$probe_hash_log"
  PATH="$test_root/hash-probe-bin:$PATH" AUDIOLIB_TEST_HASH_TOOL="$probe_hash_tool" \
    AUDIOLIB_TEST_HASH_LOG="$probe_hash_log" "$@"
}
assert_sidecar_hash_only() {
  if grep -Fqx -- "$amd64" "$probe_hash_log"; then fail 'pending pinned status hashed the archive'; fi
  grep -Fqx -- "$amd64.sha256" "$probe_hash_log" || fail 'pinned status did not hash the checksum file'
}

timing_field() {
  local pattern="\"$2\":([0-9]+)"
  [[ $1 =~ $pattern ]] || fail "missing integer timing field: $2"
  printf '%s' "${BASH_REMATCH[1]}"
}
assert_upload_timing() {
  local log=$1 archive=$2 attempts=$3 failures=$4 code=$5 intake=$6 failed_asset=${7:-}
  local line local_ms archive_ms checksum_ms
  [ "$(grep -c '^RELEASE_UPLOAD_TIMING ' "$log")" = 1 ] || fail "missing or duplicated timing receipt: $log"
  line=$(grep '^RELEASE_UPLOAD_TIMING ' "$log")
  [[ $line == *"\"archive\":\"$archive\""* && $line == *'"draft_id":"12345"'* ]] || fail 'timing receipt has the wrong identity'
  [[ $line == *"\"failed_post_asset\":\"$failed_asset\""* ]] || fail 'timing receipt identified the wrong terminal failed POST'
  [ "$(timing_field "$line" post_attempts)" = "$attempts" ] &&
    [ "$(timing_field "$line" post_failures)" = "$failures" ] &&
    [ "$(timing_field "$line" exit_code)" = "$code" ] &&
    [ "$(timing_field "$line" intake_read_hash_ms)" = "$intake" ] || fail 'timing receipt has the wrong operation counts or outcome'
  local_ms=$(timing_field "$line" local_read_hash_ms)
  archive_ms=$(timing_field "$line" post_archive_ms)
  checksum_ms=$(timing_field "$line" post_checksum_ms)
  [ "$(timing_field "$line" read_hash_ms)" -eq "$((intake + local_ms))" ] ||
    fail 'timing receipt read/hash total disagrees with its components'
  [ "$(timing_field "$line" post_ms)" -eq "$((archive_ms + checksum_ms))" ] ||
    fail 'timing receipt POST total disagrees with its components'
  [ "$local_ms" -gt 0 ] && [ "$(timing_field "$line" remote_verify_ms)" -gt 0 ] ||
    fail 'timing receipt omitted local or remote reads'
  if [ "$attempts" -gt 0 ] && [ "$((archive_ms + checksum_ms))" -eq 0 ]; then
    fail 'timing receipt omitted attempted POSTs'
  fi
}

reject '--timings requires --stage-independent' "v$v" --resume-draft --timings
reject 'requires an integer number of milliseconds' "v$v" --resume-draft --stage-independent --draft-id 12345 --timings --intake-read-hash-ms -1
reject '--intake-read-hash-ms requires --timings' "v$v" --resume-draft --stage-independent --draft-id 12345 --intake-read-hash-ms 14
reject '--pair-status requires --stage-independent' "v$v" --resume-draft --pair-status
reject '--accepted-sha256 requires --stage-independent' "v$v" --resume-draft --accepted-sha256 "$(printf '%064d' 0)"
reject 'requires a 64-digit hexadecimal digest' "v$v" --resume-draft --stage-independent --draft-id 12345 --accepted-sha256 invalid
reject 'may only be supplied once' "v$v" --resume-draft --stage-independent --draft-id 12345 \
  --accepted-sha256 "$(printf '%064d' 0)" --accepted-sha256 "$(printf '%064d' 1)"

# Both Docker uploads start before either archive finishes. Leave ARM64 in
# starter state until the AMD64 process has completed its final verification.
reset_draft
amd64=${all_assets[8]}; arm64=${all_assets[10]}
amd_name=$(basename "$amd64"); arm_name=$(basename "$arm64")
amd_accepted=$(hash_file "$amd64"); wrong_accepted=$(printf '%064d' 0)
trace_pair_hashes assert_pair_status 2 --timings --accepted-sha256 "$(printf '%s' "$amd_accepted" | tr 'a-f' 'A-F')" -- "$amd64.sha256" "$amd64"
assert_sidecar_hash_only
trace_pair_hashes assert_pair_status 2 -- "$amd64" "$amd64.sha256"
grep -Fqx -- "$amd64" "$probe_hash_log" || fail 'unpinned status did not hash the archive'
reject_draft 'Local archive SHA256 differs' independent_command --accepted-sha256 "$wrong_accepted" -- "$amd64" "$amd64.sha256"
reject_draft 'Local archive checksum differs' independent_command --pair-status --accepted-sha256 "$wrong_accepted" -- "$amd64" "$amd64.sha256"
out=$(independent_command --timings --dry-run -- "$amd64.sha256" "$amd64")
[[ $out != *'RELEASE_UPLOAD_TIMING '* ]] || fail 'dry-run emitted actual upload timings'
[[ $out == *'gh api --hostname github.com --method POST'* && $out == *'no mutations performed'* ]] ||
  fail 'independent preview did not show a pinned upload'
[ "$(mutation_count)" = 0 ] || fail 'independent preview mutated the draft'
AUDIOLIB_INDEPENDENT_PEER=$arm_name AUDIOLIB_INDEPENDENT_FAIL_AFTER=$amd_name \
  independent_command --timings --accepted-sha256 "$amd_accepted" -- "$amd64.sha256" "$amd64" > "$test_root/amd64.log" 2>&1 &
amd_pid=$!
AUDIOLIB_INDEPENDENT_PEER=$amd_name AUDIOLIB_INDEPENDENT_HOLD=1 \
  independent_command --timings -- "$arm64" "$arm64.sha256" > "$test_root/arm64.log" 2>&1 &
arm_pid=$!
amd_status=0; wait "$amd_pid" || amd_status=$?
foreign_starter=0
if awk -F '\t' -v name="$arm_name" '$3 == name && $4 == "starter" { found=1 } END { exit !found }' "$state/assets"; then
  foreign_starter=1
fi
: > "$state/release-held-upload"
arm_status=0; wait "$arm_pid" || arm_status=$?
if [ "$amd_status" != 0 ] || [ "$arm_status" != 0 ]; then
  cat "$test_root/amd64.log" "$test_root/arm64.log" >&2
  fail 'overlapping independent Docker uploads failed'
fi
[ "$foreign_starter" = 1 ] && [ -f "$state/arrived-$amd_name" ] && [ -f "$state/arrived-$arm_name" ] ||
  fail 'Docker uploads did not overlap through verification'
[ "$(mutation_count)" = 4 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 4 ] &&
  [ "$(cat "$state/draft")" = true ] || fail 'concurrent stage changed the draft or retransmitted an archive'
read_mutation 0
if [ "${mutation[11]}" = "$amd64" ]; then
  assert_independent_upload 0 "$amd64"; assert_independent_upload 1 "$arm64"
else
  assert_independent_upload 0 "$arm64"; assert_independent_upload 1 "$amd64"
fi
assert_independent_upload 2 "$amd64.sha256"
assert_independent_upload 3 "$arm64.sha256"
assert_upload_timing "$test_root/amd64.log" "$amd_name" 2 1 0 0
assert_upload_timing "$test_root/arm64.log" "$arm_name" 2 0 0 0

# The archive POST reports an error after GitHub stores it. The later checksum
# POST fails before storage. The existing ARM64 pair and AMD64 archive survive.
reset_draft
independent_command -- "$arm64" "$arm64.sha256" >/dev/null
status=0
AUDIOLIB_INDEPENDENT_FAIL_AFTER="$amd_name" AUDIOLIB_INDEPENDENT_FAIL_BEFORE="$amd_name.sha256" \
  independent_command --timings -- "$amd64" "$amd64.sha256" > "$test_root/interrupted.log" 2>&1 || status=$?
[ "$status" = 41 ] && [ "$(mutation_count)" = 4 ] && [ "$(wc -l < "$state/assets" | tr -d '[:space:]')" = 3 ] ||
  fail 'independent checksum request did not fail with only the archive stored'
assert_independent_upload 2 "$amd64"
assert_independent_upload 3 "$amd64.sha256"
assert_upload_timing "$test_root/interrupted.log" "$amd_name" 2 2 41 0 "$amd_name.sha256"
trace_pair_hashes assert_pair_status 1 --accepted-sha256 "$amd_accepted" -- "$amd64" "$amd64.sha256"
assert_sidecar_hash_only
cp "$state/assets" "$state/before-retry"
reject_draft 'complete verified draft' publish_staged_command -- "${all_assets[@]}"
independent_command --timings --intake-read-hash-ms 123 -- "$amd64" "$amd64.sha256" > "$test_root/retry.log"
assert_upload_timing "$test_root/retry.log" "$amd_name" 1 0 0 123
grep -q '"post_archive_ms":0,' "$test_root/retry.log" || fail 'timing charged an archive upload on checksum-only retry'
[ "$(mutation_count)" = 5 ] && [ "$(cat "$state/draft")" = true ] || fail 'independent retry retransmitted or published'
assert_independent_upload 4 "$amd64.sha256"
while IFS= read -r row; do
  grep -Fqx -- "$row" "$state/assets" || fail 'retry changed an already stored asset'
done < "$state/before-retry"
independent_command --timings -- "$amd64" "$amd64.sha256" > "$test_root/complete.log"
assert_upload_timing "$test_root/complete.log" "$amd_name" 0 0 0 0
grep -q '"post_ms":0,' "$test_root/complete.log" || fail 'timing charged a POST for verified existing assets'
[ "$(mutation_count)" = 5 ] || fail 'complete independent pair was retransmitted'
trace_pair_hashes assert_pair_status 0 --accepted-sha256 "$amd_accepted" -- "$amd64" "$amd64.sha256"
[ "$(grep -Fxc -- "$amd64" "$probe_hash_log")" = 1 ] || fail 'complete pinned status must verify the archive once'
archive_text=$(cat "$amd64")
printf 'z%s\n' "${archive_text#?}" > "$amd64"
reject_draft 'Local accepted asset changed' independent_command --pair-status --accepted-sha256 "$amd_accepted" -- "$amd64" "$amd64.sha256"
printf '%s\n' "$archive_text" > "$amd64"

# Reject ambiguous scope, conflicting assets and a still-uploading owned asset.
reset_draft
reject_draft 'exactly one archive and checksum pair' independent_command -- "$amd64"
TMPDIR="$test_root/missing-scratch" reject_draft 'exactly one archive and checksum pair' \
  independent_command --dry-run --timings -- "$amd64"
reject_draft 'exactly one archive and checksum pair' independent_command -- "$amd64" "$amd64.sha256" "$arm64" "$arm64.sha256"
reject_draft 'archive and checksum' independent_command -- "$amd64" "$arm64.sha256"
reject_draft 'draft ID differs' env AUDIOLIB_REQUIRE_API_HOST=1 "$here/release-create.sh" "$tag" --resume-draft --stage-independent --draft-id 54321 --notes-file "$notes" -- "$amd64" "$amd64.sha256"
printf 'asset\t9000\t%s\tstarter\t0\t\n' "$amd_name" > "$state/assets"
reject_draft 'Remote asset is incomplete' independent_command -- "$amd64" "$amd64.sha256"
reject_draft 'Draft asset is in starter state' independent_command --pair-status -- "$amd64" "$amd64.sha256"
printf 'asset\t9000\t%s\tstarter\t0\t\n' "$arm_name" > "$state/assets"
reject_draft 'Draft asset is in starter state' independent_command --pair-status -- "$amd64" "$amd64.sha256"
printf 'asset\t9000\tunknown.zip\tstarter\t0\t\n' > "$state/assets"
reject_draft 'Unexpected remote' independent_command -- "$amd64" "$amd64.sha256"
printf 'asset\t9000\t%s\tuploaded\t%s\tsha256:%064d\n' "$amd_name" "$(wc -c < "$amd64" | tr -d '[:space:]')" 0 > "$state/assets"
reject_draft 'size/SHA256 conflicts' independent_command -- "$amd64" "$amd64.sha256"
reject_draft 'size/SHA256 conflicts' independent_command --pair-status --accepted-sha256 "$amd_accepted" -- "$amd64" "$amd64.sha256"

# Finalization alone may mutate a fully staged, verified draft, once.
for suffix in '' '-rc.2'; do
  reset_draft "$suffix"; seed_remote 14
  reject_draft 'complete expected local asset set' publish_staged_command -- "${all_assets[@]:8:4}"
  if [ -z "$suffix" ]; then
    cp "$state/assets" "$state/all-uploaded"
    awk 'BEGIN { FS=OFS="\t" } NR == 1 { $4="starter"; $5=0; $6="" } { print }' "$state/all-uploaded" > "$state/assets"
    reject_draft 'Remote asset is incomplete' publish_staged_command -- "${all_assets[@]}"
    mv "$state/all-uploaded" "$state/assets"
  fi
  out=$(publish_staged_command --dry-run -- "${all_assets[@]}")
  [[ $out == *'gh api --hostname github.com --method PATCH'* && $out != *' --method POST '* && $out == *'no mutations performed'* ]] ||
    fail 'staged publication preview did not verify without uploading'
  [ "$(mutation_count)" = 0 ] || fail 'staged publication preview mutated the draft'
  publish_staged_command -- "${all_assets[@]}" >/dev/null
  [ "$(cat "$state/draft")" = false ] && [ "$(mutation_count)" = 1 ] &&
    [ -f "$state/published-snapshot" ] || fail 'finalization did not publish and reverify the existing draft exactly once'
  read_mutation 0
  latest=true; if [ -n "$suffix" ]; then latest=false; fi
  [ "${mutation[0]}" = api ] && [ "${mutation[2]}" = github.com ] && [ "${mutation[4]}" = PATCH ] &&
    [ "${mutation[5]}" = 'repos/{owner}/{repo}/releases/12345' ] &&
    [ "${mutation[9]}" = "make_latest=$latest" ] || fail 'wrong staged finalization mutation'
  reject_draft 'unpublished draft' publish_staged_command -- "${all_assets[@]}"
done

echo 'release-create tests passed (offline fake gh/pwsh/git; legacy resume, cumulative and independent pairs, read-only pair status, accepted digests, interrupted retry, segmented upload timing, exact staged publication)'
