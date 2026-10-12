#!/bin/sh
# server/launcher/build.sh
# macOS only (optional). Builds and ad-hoc signs bin/audiolib-launcher locally; the binary is
# not committed. Rebuild ONLY when the C source really changes: a new binary has a new
# cdhash and macOS will ask for volume access permissions once more.
set -eu
cd "$(dirname "$0")"
mkdir -p bin
clang -O2 -Wall -Wextra -Werror -arch arm64 -mmacosx-version-min=13.0 \
    -o bin/audiolib-launcher audiolib-launcher.c
codesign --force --sign - --identifier io.github.audiolib.launcher bin/audiolib-launcher
codesign -dvvv bin/audiolib-launcher 2>&1 | grep -E '^(Identifier|CDHash|Signature)='
