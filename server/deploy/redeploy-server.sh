#!/bin/sh
# deploy/redeploy-server.sh - generic deploy/upgrade script for audiolib-server.
#
# Replaces the server binary atomically (install + mv), restarts the service, verifies
# /api/v1/health and rolls back automatically when verification fails.
#
# Usage:  redeploy-server.sh /path/to/new/audiolib-server
#
# Configuration (environment variables, all optional):
#   INSTALL_DIR   directory holding the installed binary      (default /opt/audiolib)
#   BINARY        installed binary name                       (default audiolib-server)
#   SERVICE_NAME  systemd unit or launchd label               (default audiolib-server)
#   DATA_DIR      directory with library.db                   (default /var/lib/audiolib)
#   LIBRARY_DB    library file name inside DATA_DIR           (default library.db)
#   BACKUP_DIR    where backups go                            (default $DATA_DIR/backups)
#   HOST / PORT   address the server answers on locally       (default 127.0.0.1 / 8790)
#   TOKEN         bearer token for /scans/current when auth requires one (optional;
#                 prefer exporting AUDIOLIB_SERVER_TOKEN, which is used as a fallback)
#   EXPECT_API_VERSION     expected api_version               (default v1)
#   EXPECT_SCHEMA_VERSION  expected schema_version            (optional, integer)
#   EXPECT_COMMIT          expected commit (short hash)       (optional)
#   SERVICE_MANAGER        systemd | launchd | none           (default: auto-detect)
#   WAIT_SECS              seconds to wait for health "ok"    (default 60)
#   UPDATE_CHANNEL / UPDATE_VERSION  optional paired update markers (stable|beta,
#                 full SemVer without v). Restored before a rollback restart.
#
# Run it in a maintenance window. It never deletes data files and never uses cp on the
# running binary.
set -eu

INSTALL_DIR="${INSTALL_DIR:-/opt/audiolib}"
BINARY="${BINARY:-audiolib-server}"
SERVICE_NAME="${SERVICE_NAME:-audiolib-server}"
DATA_DIR="${DATA_DIR:-/var/lib/audiolib}"
LIBRARY_DB="${LIBRARY_DB:-library.db}"
BACKUP_DIR="${BACKUP_DIR:-$DATA_DIR/backups}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8790}"
WAIT_SECS="${WAIT_SECS:-60}"
EXPECT_API_VERSION="${EXPECT_API_VERSION:-v1}"
BASE="http://$HOST:$PORT/api/v1"
TOKEN="${TOKEN:-${AUDIOLIB_SERVER_TOKEN:-}}"

die() { echo "error: $*" >&2; exit 1; }
log() { echo "[redeploy] $*"; }

[ $# -eq 1 ] || die "usage: $0 /path/to/new/$BINARY"
NEW="$1"
[ -f "$NEW" ] && [ -x "$NEW" ] || die "$NEW is not an executable file"
CUR="$INSTALL_DIR/$BINARY"
[ -f "$CUR" ] || die "$CUR not found (set INSTALL_DIR/BINARY)"
command -v curl >/dev/null 2>&1 || die "curl is required"
UPDATE_METADATA=0
if [ -n "${UPDATE_CHANNEL:-}${UPDATE_VERSION:-}" ]; then
    case "${UPDATE_CHANNEL:-}" in stable|beta) ;; *) die "UPDATE_CHANNEL must be stable or beta" ;; esac
    printf '%s\n' "${UPDATE_VERSION:-}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-(beta|rc)\.(0|[1-9][0-9]*)|-0)?$' || die "invalid UPDATE_VERSION"
    case "$UPDATE_CHANNEL:$UPDATE_VERSION" in stable:*-*) die "stable channel cannot install a prerelease marker" ;; esac
    new_version="$("$NEW" --version)"
    [ "$new_version" = "audiolib-server ${UPDATE_VERSION%%-*}" ] || die "update marker differs from the new binary version"
    for marker in update-channel.txt update-version.txt; do
        [ ! -L "$INSTALL_DIR/$marker" ] || die "linked update marker is not supported"
        [ ! -e "$INSTALL_DIR/$marker" ] || [ -f "$INSTALL_DIR/$marker" ] || die "update marker is not a regular file"
    done
    UPDATE_METADATA=1
elif [ -e "$INSTALL_DIR/update-version.txt" ] || [ -L "$INSTALL_DIR/update-version.txt" ]; then
    [ ! -L "$INSTALL_DIR/update-version.txt" ] && [ -f "$INSTALL_DIR/update-version.txt" ] || die "update version marker is not a regular file"
    old_version="$(cat "$INSTALL_DIR/update-version.txt")"
    new_version="$("$NEW" --version)"
    [ "$new_version" = "audiolib-server ${old_version%%-*}" ] || die "existing update marker would become stale; set both UPDATE_CHANNEL and UPDATE_VERSION for this deployment"
fi

if [ -z "${SERVICE_MANAGER:-}" ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        SERVICE_MANAGER=systemd
    elif command -v launchctl >/dev/null 2>&1; then
        SERVICE_MANAGER=launchd
    else
        SERVICE_MANAGER=none
    fi
fi

restart_service() {
    case "$SERVICE_MANAGER" in
    systemd) systemctl restart "$SERVICE_NAME" ;;
    launchd) launchctl kickstart -k "gui/$(id -u)/$SERVICE_NAME" ;;
    *) log "SERVICE_MANAGER=none: restart the service yourself now, then press Enter"; read -r _ ;;
    esac
}

curl_auth() {
    if [ -n "$TOKEN" ]; then
        curl -fsS --max-time 10 -H "Authorization: Bearer $TOKEN" "$@"
    else
        curl -fsS --max-time 10 "$@"
    fi
}

# Extract a JSON field without jq: string or number value of "key".
json_field() {
    printf '%s' "$1" | sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*/\1/p" | head -n 1
}

wait_health() {
    i=0
    while [ "$i" -lt "$WAIT_SECS" ]; do
        body="$(curl -fsS --max-time 5 "$BASE/health" 2>/dev/null || true)"
        if [ -n "$body" ] && [ "$(json_field "$body" status)" = "ok" ]; then
            HEALTH="$body"
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    return 1
}

verify_health() {
    v="$(json_field "$HEALTH" version)"
    api="$(json_field "$HEALTH" api_version)"
    schema="$(json_field "$HEALTH" schema_version)"
    commit="$(json_field "$HEALTH" commit)"
    log "health: version=$v api_version=$api schema_version=$schema commit=$commit"
    [ -n "$v" ] || { log "version missing"; return 1; }
    [ "$api" = "$EXPECT_API_VERSION" ] || { log "api_version is not $EXPECT_API_VERSION"; return 1; }
    [ -n "$schema" ] || { log "schema_version missing"; return 1; }
    [ -n "$commit" ] && [ "$commit" != "unknown" ] || { log "commit missing or unknown"; return 1; }
    if [ -n "${EXPECT_SCHEMA_VERSION:-}" ] && [ "$schema" != "$EXPECT_SCHEMA_VERSION" ]; then
        log "schema_version is not $EXPECT_SCHEMA_VERSION"; return 1
    fi
    if [ -n "${EXPECT_COMMIT:-}" ] && [ "$commit" != "$EXPECT_COMMIT" ]; then
        log "commit is not $EXPECT_COMMIT"; return 1
    fi
    return 0
}

# 1. No scan may be running.
log "checking for a running scan"
if scan="$(curl_auth "$BASE/scans/current" 2>/dev/null)"; then
    compact="$(printf '%s' "$scan" | tr -d ' \t\r\n')"
    case "$compact" in
    '{"scan":null}') ;;
    *'"active":true'*) die "a scan is active; wait for it or cancel it first" ;;
    esac
else
    log "warning: could not query /scans/current (server down, or a token is required: set TOKEN)"
fi
if pgrep -fl "audiolib.*(db rebuild-search|scan)" >/dev/null 2>&1; then
    die "an audiolib maintenance command is running; wait for it to finish"
fi

# 2. Back up the library and the old binary.
TS="$(date +%Y%m%d-%H%M%S)"
DEST="$BACKUP_DIR/$TS"
mkdir -p "$DEST"
cp -p "$CUR" "$DEST/$BINARY.bin"
DB="$DATA_DIR/$LIBRARY_DB"
if [ -f "$DB" ]; then
    log "backing up $DB to $DEST (needs free space for the library plus its WAL)"
    # `cp -c` (APFS clone) is instant on macOS; fall back to a plain copy elsewhere.
    for f in "$DB" "$DB-wal" "$DB-shm"; do
        [ -f "$f" ] || continue
        cp -c "$f" "$DEST/" 2>/dev/null || cp -p "$f" "$DEST/" || die "backup of $f failed"
    done
else
    log "warning: $DB not found, library backup skipped"
fi
cp -p "$CUR" "$CUR.prev"
if [ "$UPDATE_METADATA" = 1 ]; then
    OLD_CHANNEL_PRESENT=0; OLD_VERSION_PRESENT=0
    [ ! -f "$INSTALL_DIR/update-channel.txt" ] || OLD_CHANNEL_PRESENT=1
    [ ! -f "$INSTALL_DIR/update-version.txt" ] || OLD_VERSION_PRESENT=1
    for marker in update-channel.txt update-version.txt; do
        if [ -f "$INSTALL_DIR/$marker" ]; then cp -p "$INSTALL_DIR/$marker" "$DEST/$marker"; fi
    done
fi

write_marker() {
    marker="$1"; value="$2"
    staged="$(mktemp "$INSTALL_DIR/.$marker.new.XXXXXX")" || return 1
    printf '%s\n' "$value" > "$staged" && chmod 644 "$staged" && mv -f "$staged" "$INSTALL_DIR/$marker" || {
        rm -f "$staged"; return 1;
    }
}
restore_markers() {
    [ "$UPDATE_METADATA" = 1 ] || return 0
    for marker in update-channel.txt update-version.txt; do
        [ ! -L "$INSTALL_DIR/$marker" ] || return 1
        original_present="$OLD_VERSION_PRESENT"
        [ "$marker" != update-channel.txt ] || original_present="$OLD_CHANNEL_PRESENT"
        if [ "$original_present" = 1 ]; then
            staged="$(mktemp "$INSTALL_DIR/.$marker.new.XXXXXX")" || return 1
            cp -p "$DEST/$marker" "$staged" && mv -f "$staged" "$INSTALL_DIR/$marker" || {
                rm -f "$staged"; return 1;
            }
        else
            # Remove only this exact script-owned marker when none existed before.
            rm -f "$INSTALL_DIR/$marker" || return 1
        fi
    done
}

TRANSACTION_STARTED=0; COMMITTED=0; ROLLBACK_DONE=0
rollback() {
    ROLLBACK_DONE=1
    log "ROLLBACK: restoring previous binary"
    install -m 755 "$CUR.prev" "$CUR.new" && mv -f "$CUR.new" "$CUR" || {
        log "binary rollback failed; backup retained at $DEST"; return 1;
    }
    # The old process reads its release identity once during startup, so restore
    # both markers before restarting it, rather than after this script returns.
    restore_markers || { log "marker rollback failed; backup retained at $DEST"; return 1; }
    restart_service || true
    if wait_health; then
        log "rolled back; health is ok again (version $(json_field "$HEALTH" version))"
    else
        log "rolled-back server is still not healthy; library backup: $DEST"
    fi
    log "If a schema migration already ran, an older binary refuses the newer library:"
    log "restore the library files from $DEST together with the old binary (see README)."
}
on_exit() {
    result=$?
    if [ "$TRANSACTION_STARTED" = 1 ] && [ "$COMMITTED" = 0 ] && [ "$ROLLBACK_DONE" = 0 ]; then rollback || true; fi
    exit "$result"
}
trap on_exit EXIT

# 3. Replace the binary and its optional release identity before starting it.
log "installing new binary"
TRANSACTION_STARTED=1
install -m 755 "$NEW" "$CUR.new"
mv -f "$CUR.new" "$CUR"
if [ "$UPDATE_METADATA" = 1 ]; then
    write_marker update-channel.txt "$UPDATE_CHANNEL"
    write_marker update-version.txt "$UPDATE_VERSION"
fi

# 4. Restart and verify.
restart_service || { rollback; die "restart failed"; }
if wait_health && verify_health; then
    COMMITTED=1
    log "OK: new binary is running (previous one kept as $CUR.prev, backups in $DEST)"
    exit 0
fi
rollback
die "health verification failed; rolled back"
