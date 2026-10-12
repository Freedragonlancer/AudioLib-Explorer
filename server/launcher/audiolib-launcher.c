// server/launcher/audiolib-launcher.c
// Stable launchd entry point for audiolib-server on macOS.
//
// macOS TCC records ad-hoc signed binaries by code hash (cdhash), so every rebuild of
// audiolib-server used to trigger a new "access files on a removable/network volume"
// prompt. launchd starts this tiny launcher instead; it is compiled and signed once locally
// (build.sh), so its cdhash never changes. It posix_spawn()s
// (does not exec) the given program, e.g. `/bin/sh .../bin/run.sh`, which makes the
// launcher the TCC "responsible process" of the child: permissions granted to the
// launcher keep working when only audiolib-server is replaced.
//
// Usage: audiolib-launcher <absolute program path> [args...]
// SIGTERM, SIGINT and SIGHUP are forwarded to the child; the launcher waits and exits
// with the child's exit status (or re-raises the child's terminating signal).
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static volatile sig_atomic_t child_pid = 0;

static void forward_signal(int sig) {
    if (child_pid > 0) {
        kill((pid_t)child_pid, sig);
    }
}

int main(int argc, char *argv[]) {
    static const int forwarded[] = {SIGTERM, SIGINT, SIGHUP};
    const size_t count = sizeof(forwarded) / sizeof(forwarded[0]);
    if (argc < 2 || argv[1][0] != '/') {
        fprintf(stderr, "usage: audiolib-launcher <absolute program path> [args...]\n");
        return 64;
    }

    // Block the forwarded signals until the child pid is known, so none is lost.
    sigset_t blocked, original;
    sigemptyset(&blocked);
    for (size_t i = 0; i < count; i++) {
        sigaddset(&blocked, forwarded[i]);
    }
    sigprocmask(SIG_BLOCK, &blocked, &original);

    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = forward_signal;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    for (size_t i = 0; i < count; i++) {
        sigaction(forwarded[i], &action, NULL);
    }

    // The child starts with the original mask and default dispositions.
    posix_spawnattr_t attributes;
    posix_spawnattr_init(&attributes);
    posix_spawnattr_setsigmask(&attributes, &original);
    posix_spawnattr_setsigdefault(&attributes, &blocked);
    posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF);

    pid_t pid = 0;
    int rc = posix_spawn(&pid, argv[1], NULL, &attributes, &argv[1], environ);
    posix_spawnattr_destroy(&attributes);
    if (rc != 0) {
        fprintf(stderr, "audiolib-launcher: cannot start %s: %s\n", argv[1], strerror(rc));
        return 127;
    }
    child_pid = pid;
    sigprocmask(SIG_SETMASK, &original, NULL);

    int status = 0;
    for (;;) {
        pid_t done = waitpid(pid, &status, 0);
        if (done == pid) {
            break;
        }
        if (done < 0 && errno != EINTR) {
            perror("audiolib-launcher: waitpid");
            return 1;
        }
    }
    if (WIFEXITED(status)) {
        return WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status)) {
        int sig = WTERMSIG(status);
        signal(sig, SIG_DFL);
        sigset_t unblock;
        sigemptyset(&unblock);
        sigaddset(&unblock, sig);
        sigprocmask(SIG_UNBLOCK, &unblock, NULL);
        raise(sig);
        return 128 + sig;
    }
    return 1;
}
