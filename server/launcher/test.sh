#!/bin/sh
# server/launcher/test.sh
# Checks exit-status propagation and signal forwarding of bin/audiolib-launcher.
set -u
L="$(cd "$(dirname "$0")" && pwd)/bin/audiolib-launcher"
fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: expected $2, got $3"; fail=1; fi; }

"$L" /bin/sh -c 'exit 3'; check "exit status" 3 $?
"$L" relative/path >/dev/null 2>&1; check "relative path rejected" 64 $?
"$L" /nonexistent/program 2>/dev/null; check "spawn failure" 127 $?
out="$(LAUNCHER_TEST=1 "$L" /usr/bin/env | grep -c '^LAUNCHER_TEST=1$')" ; check "environment kept" 1 "$out"

for sig in TERM INT HUP; do
    LAUNCHER_TEST=1 "$L" /bin/sh -c "trap 'exit 42' $sig; while :; do sleep 0.05; done" &
    pid=$!
    sleep 0.5
    kill -s "$sig" "$pid"
    wait "$pid"; check "SIG$sig forwarded, child status returned" 42 $?
done

# A child killed by a signal makes the launcher die by the same signal.
"$L" /bin/sh -c 'kill -s KILL $$'; check "child signal re-raised" 137 $?

# The child's parent is the launcher (spawned, not exec'd).
"$L" /bin/sh -c 'ps -o comm= -p $PPID' | grep -q audiolib-launcher; check "launcher is the parent" 0 $?
exit $fail
