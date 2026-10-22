#!/bin/bash
# tools/test-beta-update-markers.sh
# Exercise beta-refresh metadata and rollback using an owned fake deployment.
set -eu
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/audiolib-beta-markers.XXXXXX")"
case "$TEST_ROOT" in "${TMPDIR:-/tmp}"/audiolib-beta-markers.*) ;; *) exit 2 ;; esac
cleanup() { rm -rf -- "$TEST_ROOT"; }
trap cleanup EXIT
mkdir -p "$TEST_ROOT/repo/tools" "$TEST_ROOT/repo/server/deploy" "$TEST_ROOT/mock-bin"
cp "$SOURCE/tools/beta-refresh.sh" "$TEST_ROOT/repo/tools/"
cat > "$TEST_ROOT/mock-bin/git" <<'EOF'
#!/bin/sh
case " $* " in
    *" rev-parse "*) printf '%s\n' f54f036 ;;
    *" tag "*) printf '%s\n' "${MOCK_TAG:-}" ;;
esac
EOF
cat > "$TEST_ROOT/mock-bin/curl" <<'EOF'
#!/bin/sh
printf '%s\n' '{"status":"ok","commit":"f54f036"}'
EOF
cat > "$TEST_ROOT/server-binary" <<'EOF'
#!/bin/sh
[ "$#" -eq 1 ] && [ "$1" = '--version' ] || exit 9
printf '%s\n' 'audiolib-server 0.6.2'
EOF
cat > "$TEST_ROOT/repo/server/deploy/redeploy-server.sh" <<'EOF'
#!/bin/sh
[ "$(cat "$INSTALL_DIR/update-channel.txt")" = beta ] || exit 8
[ "$(cat "$INSTALL_DIR/update-version.txt")" = "$EXPECTED_VERSION" ] || exit 8
[ "${MOCK_REDEPLOY_FAIL:-0}" != 1 ] || exit 7
EOF
chmod +x "$TEST_ROOT/mock-bin/git" "$TEST_ROOT/mock-bin/curl" "$TEST_ROOT/server-binary"
export PATH="$TEST_ROOT/mock-bin:$PATH"
run_case() {
    name="$1"; tag="$2"; expected="$3"; fail="$4"
    app="$TEST_ROOT/$name/app"; data="$TEST_ROOT/$name/data"; work="$TEST_ROOT/$name/work"
    mkdir -p "$app/bin" "$data" "$work"
    printf '%s\n' 'bind = "127.0.0.1:8791"' > "$app/config.toml"
    printf '%s\n' 'synthetic-token' > "$app/token"
    printf '%s\n' stable > "$app/bin/update-channel.txt"
    printf '%s\n' 0.6.1 > "$app/bin/update-version.txt"
    status=0
    BETA_APP_DIR="$app" BETA_DATA_DIR="$data" SERVICE_NAME=synthetic.beta WORK_DIR="$work" \
        MOCK_TAG="$tag" EXPECTED_VERSION="$expected" MOCK_REDEPLOY_FAIL="$fail" \
        bash "$TEST_ROOT/repo/tools/beta-refresh.sh" --no-build "$TEST_ROOT/server-binary" >/dev/null 2>&1 || status=$?
    if [ "$fail" = 1 ]; then
        [ "$status" -ne 0 ] || { echo 'Failed deployment was accepted' >&2; exit 1; }
        [ "$(cat "$app/bin/update-channel.txt")" = stable ] && [ "$(cat "$app/bin/update-version.txt")" = 0.6.1 ] || {
            echo 'Failed deployment lost previous channel/version markers' >&2; exit 1;
        }
    else
        [ "$status" -eq 0 ] && [ "$(cat "$app/bin/update-channel.txt")" = beta ] &&
            [ "$(cat "$app/bin/update-version.txt")" = "$expected" ] || {
            cat "$work"/beta-refresh-*.log >&2; echo 'Beta metadata deployment failed' >&2; exit 1;
        }
    fi
}
run_case development '' 0.6.2-0 0
run_case candidate v0.6.2-beta.12 0.6.2-beta.12 0
run_case stable v0.6.2 0.6.2 0
run_case wrong-version v0.6.3-beta.1 0.6.2-0 0
run_case rollback v0.6.2-beta.12 0.6.2-beta.12 1
printf '%s\n' 'Beta update metadata regression passed: development baseline, exact beta/stable tags, mismatched-tag rejection and failed-deploy restoration.'
