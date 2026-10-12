<!-- server/launcher/README.md -->
# audiolib-launcher

> **macOS only, optional** - it exists to avoid repeated TCC (privacy) prompts for volume
> access when the server binary is replaced. Linux, Windows and Docker deployments do not need it.

Tiny C launcher that launchd starts instead of `audiolib-server`, so macOS privacy (TCC)
grants survive server rebuilds.

TCC identifies ad-hoc signed programs by their code hash (cdhash); every rebuild of
`audiolib-server` therefore used to trigger a new "access files on a removable / network
volume" prompt. The launcher is compiled and ad-hoc signed **once on your machine** with `./build.sh`
(identifier `io.github.audiolib.launcher`; the binary is not part of the repository, and its
CDHash depends on your compiler and is printed by `build.sh`). It `posix_spawn`s (does not `exec`) its
arguments, e.g. `/bin/sh …/bin/run.sh`, so it stays the child's TCC *responsible process*:
grants given to the launcher cover the server binary no matter how often it is replaced.

- Usage: `audiolib-launcher <absolute program> [args…]`; the environment is passed on.
- SIGTERM, SIGINT and SIGHUP are forwarded to the child; the launcher waits and exits with
  the child's status (or dies by the child's terminating signal).
- `./test.sh` checks status propagation, signal forwarding and the parent relationship.
- `./build.sh` builds and signs it into `bin/` (git-ignored). Only do that when the C source really changes: a
  new binary has a new cdhash and macOS will prompt once more.

Verify after deploying (see `../deploy/README.md`):

```sh
/usr/bin/log show --last 15m --style compact \
  --predicate 'subsystem == "com.apple.TCC" AND eventMessage CONTAINS "AUTHREQ_ATTRIBUTION"' \
  | grep -i audiolib
```

The attribution should name `audiolib-launcher` as `responsible` (and `audiolib-server` as
`accessing`). A later deploy that replaces only `bin/audiolib-server` must not prompt again.
