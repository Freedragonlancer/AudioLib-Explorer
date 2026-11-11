#!/usr/bin/env bash
# tools/test-release-create.sh — isolated argv/preview checks; never contacts GitHub.
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
exit "${AUDIOLIB_RELEASE_CREATE_EXIT:-0}"
FAKE_GH
chmod +x "$test_root/bin/gh"
export PATH="$test_root/bin:$PATH"
export AUDIOLIB_RELEASE_CREATE_CAPTURE="$test_root/gh.argv"
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
echo 'release-create tests passed (offline fake gh; stable/beta/rc, draft, preview, assets, failures)'
