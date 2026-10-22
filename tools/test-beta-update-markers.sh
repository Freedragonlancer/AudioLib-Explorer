#!/bin/bash
# tools/test-beta-update-markers.sh
# Execute the real beta/redeploy scripts with fake services and an owned fixture.
set -eu
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/audiolib-beta-markers.XXXXXX")"
case "$TEST_ROOT" in "${TMPDIR:-/tmp}"/audiolib-beta-markers.*) ;; *) exit 2 ;; esac
cleanup() { rm -rf -- "$TEST_ROOT"; }
trap cleanup EXIT
mkdir -p "$TEST_ROOT/repo/tools" "$TEST_ROOT/repo/server/deploy" "$TEST_ROOT/mock-bin"
cp "$SOURCE/tools/beta-refresh.sh" "$TEST_ROOT/repo/tools/"
cp "$SOURCE/server/deploy/redeploy-server.sh" "$TEST_ROOT/repo/server/deploy/"
cat > "$TEST_ROOT/mock-bin/git" <<'EOF'
#!/bin/sh
case " $* " in
    *" rev-parse "*) printf '%s\n' f54f036 ;;
    *" tag "*) printf '%s\n' "${MOCK_TAG:-}" ;;
esac
EOF
cat > "$TEST_ROOT/mock-bin/curl" <<'EOF'
#!/bin/sh
case " $* " in
    *'/scans/current'*) printf '%s\n' '{"scan":null}'; exit 0 ;;
esac
number="$(cat "$BETA_APP_DIR/start-count")"
version="$("$BETA_APP_DIR/bin/audiolib-server" --version)"; version="${version#audiolib-server }"
commit=f54f036
if [ "${MOCK_REDEPLOY_FAIL:-0}" = 1 ] && [ "$number" = 1 ]; then commit=wrong; fi
printf '{"status":"ok","version":"%s","commit":"%s","api_version":"v1","schema_version":6}\n' "$version" "$commit"
EOF
cat > "$TEST_ROOT/mock-bin/launchctl" <<'EOF'
#!/bin/sh
number=0
if [ -f "$BETA_APP_DIR/start-count" ]; then number="$(cat "$BETA_APP_DIR/start-count")"; fi
number=$((number + 1)); printf '%s\n' "$number" > "$BETA_APP_DIR/start-count"
version="$("$BETA_APP_DIR/bin/audiolib-server" --version)"; version="${version#audiolib-server }"
channel='<missing>'; installed='<missing>'
if [ -f "$BETA_APP_DIR/bin/update-channel.txt" ]; then channel="$(cat "$BETA_APP_DIR/bin/update-channel.txt")"; fi
if [ -f "$BETA_APP_DIR/bin/update-version.txt" ]; then installed="$(cat "$BETA_APP_DIR/bin/update-version.txt")"; fi
printf '%s|%s|%s|%s\n' "$number" "$version" "$channel" "$installed" >> "$BETA_APP_DIR/start-events"
if [ "$installed" != '<missing>' ] && [ "${installed%%-*}" != "$version" ]; then
    echo 'UpdateClient would reject the marker at process startup' >&2; exit 8
fi
EOF
cat > "$TEST_ROOT/mock-bin/pgrep" <<'EOF'
#!/bin/sh
exit 1
EOF
cat > "$TEST_ROOT/server-binary" <<'EOF'
#!/bin/sh
[ "$#" -eq 1 ] && [ "$1" = '--version' ] || exit 9
printf '%s\n' 'audiolib-server 0.6.2'
EOF
chmod +x "$TEST_ROOT/mock-bin/git" "$TEST_ROOT/mock-bin/curl" "$TEST_ROOT/mock-bin/launchctl" "$TEST_ROOT/mock-bin/pgrep" "$TEST_ROOT/server-binary"
export PATH="$TEST_ROOT/mock-bin:$PATH"
run_case() {
    name="$1"; tag="$2"; expected="$3"; fail="$4"; old_markers="$5"
    app="$TEST_ROOT/$name/app"; data="$TEST_ROOT/$name/data"; work="$TEST_ROOT/$name/work"
    mkdir -p "$app/bin" "$data" "$work"
    printf '%s\n' 'bind = "127.0.0.1:8791"' > "$app/config.toml"
    printf '%s\n' 'synthetic-token' > "$app/token"
    printf '%s\n' '#!/bin/sh' 'printf "%s\n" "audiolib-server 0.6.1"' > "$app/bin/audiolib-server"
    chmod +x "$app/bin/audiolib-server"
    if [ "$old_markers" = 1 ]; then
        printf '%s\n' stable > "$app/bin/update-channel.txt"
        printf '%s\n' 0.6.1 > "$app/bin/update-version.txt"
    fi
    status=0
    BETA_APP_DIR="$app" BETA_DATA_DIR="$data" SERVICE_NAME=synthetic.beta WORK_DIR="$work" \
        MOCK_TAG="$tag" MOCK_REDEPLOY_FAIL="$fail" \
        bash "$TEST_ROOT/repo/tools/beta-refresh.sh" --no-build "$TEST_ROOT/server-binary" >/dev/null 2>&1 || status=$?
    grep -Fx "1|0.6.2|beta|$expected" "$app/start-events" >/dev/null || { cat "$work"/beta-refresh-*.log >&2; echo 'Candidate startup did not see its exact new markers' >&2; exit 1; }
    if [ "$fail" = 1 ]; then
        [ "$status" -ne 0 ] || { echo 'Failed deployment was accepted' >&2; exit 1; }
        if [ "$old_markers" = 1 ]; then
            grep -Fx '2|0.6.1|stable|0.6.1' "$app/start-events" >/dev/null &&
                [ "$(cat "$app/bin/update-channel.txt")" = stable ] && [ "$(cat "$app/bin/update-version.txt")" = 0.6.1 ] || {
                echo 'Rollback process started with mismatched release markers' >&2; exit 1;
            }
        else
            grep -Fx '2|0.6.1|<missing>|<missing>' "$app/start-events" >/dev/null &&
                [ ! -e "$app/bin/update-channel.txt" ] && [ ! -e "$app/bin/update-version.txt" ] || {
                echo 'Rollback invented markers for a previously unmarked install' >&2; exit 1;
            }
        fi
    else
        [ "$status" -eq 0 ] && [ "$(cat "$app/bin/update-channel.txt")" = beta ] &&
            [ "$(cat "$app/bin/update-version.txt")" = "$expected" ] || {
            cat "$work"/beta-refresh-*.log >&2; echo 'Beta metadata deployment failed' >&2; exit 1;
        }
    fi
}
run_case development '' 0.6.2-0 0 1
run_case candidate v0.6.2-beta.12 0.6.2-beta.12 0 1
run_case stable v0.6.2 0.6.2 0 1
run_case wrong-version v0.6.3-beta.1 0.6.2-0 0 1
run_case rollback v0.6.2-beta.12 0.6.2-beta.12 1 1
run_case unmarked-rollback v0.6.2-beta.12 0.6.2-beta.12 1 0
# POSIX hosts must let an unprivileged service account read public markers
# written by a root-owned deployment. Git Bash emulates rather than enforces it.
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) ;; *)
    for marker in update-channel.txt update-version.txt; do
        mode="$(ls -l "$TEST_ROOT/candidate/app/bin/$marker" | cut -c 1-10)"
        [ "${mode:7:1}" = r ] || { echo 'New public marker is not world-readable' >&2; exit 1; }
    done
esac
# A caller which does not manage markers may still update an unmarked install.
app="$TEST_ROOT/direct/app"; data="$TEST_ROOT/direct/data"; work="$TEST_ROOT/direct/work"
mkdir -p "$app/bin" "$data" "$work"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" "audiolib-server 0.6.1"' > "$app/bin/audiolib-server"
chmod +x "$app/bin/audiolib-server"
BETA_APP_DIR="$app" INSTALL_DIR="$app/bin" DATA_DIR="$data" BACKUP_DIR="$work/backups" \
    SERVICE_MANAGER=launchd SERVICE_NAME=synthetic.beta EXPECT_COMMIT=f54f036 \
    sh "$TEST_ROOT/repo/server/deploy/redeploy-server.sh" "$TEST_ROOT/server-binary" >/dev/null 2>&1
grep -Fx '1|0.6.2|<missing>|<missing>' "$app/start-events" >/dev/null || { echo 'Unmarked generic deployment changed its existing behavior' >&2; exit 1; }
cp "$app/bin/audiolib-server.prev" "$app/bin/audiolib-server"
printf '%s\n' stable > "$app/bin/update-channel.txt"
printf '%s\n' 0.6.1 > "$app/bin/update-version.txt"
before="$(cksum "$app/bin/audiolib-server")"
status=0
BETA_APP_DIR="$app" INSTALL_DIR="$app/bin" DATA_DIR="$data" BACKUP_DIR="$work/backups" \
    SERVICE_MANAGER=launchd SERVICE_NAME=synthetic.beta EXPECT_COMMIT=f54f036 \
    sh "$TEST_ROOT/repo/server/deploy/redeploy-server.sh" "$TEST_ROOT/server-binary" >"$work/rejected.log" 2>&1 || status=$?
[ "$status" -ne 0 ] && grep -F 'set both UPDATE_CHANNEL and UPDATE_VERSION' "$work/rejected.log" >/dev/null &&
    [ "$before" = "$(cksum "$app/bin/audiolib-server")" ] && [ "$(cat "$app/start-count")" = 1 ] || {
    echo 'A binary-only base-version change left stale metadata or restarted the service' >&2; exit 1;
}
printf '%s\n' 'Beta update metadata regression passed: candidate/rollback startup identities, unmarked rollback and generic deployment, stale-marker preflight, development baseline and mismatched-tag rejection.'
