# Rust Debug Mode and Bug Reports

The desktop File menu provides a session-only **Debug mode** and a bug-report window. Debug mode starts off on each application launch. Enable it before reproducing a problem, then open the report window to inspect and copy the diagnostic report or continue to the repository's GitHub issue form.

## Reproduce and report

1. Choose **File → Debug mode** to begin recording the current session.
2. Reproduce the problem. For playback or unexpected refreshes, keep the same data source, search and playback settings used when the problem occurred.
3. Open **File → Report a bug**. Describe the actions, expected result and actual result; review both the description and the diagnostic preview before sharing them.
4. Copy the report for a local record or use **Open GitHub issue** to open a prefilled issue for [Freedragonlancer/AudioLib-Explorer](https://github.com/Freedragonlancer/AudioLib-Explorer/issues).
5. Review the issue in the browser, sign in to GitHub if needed and click GitHub's final submit button. Opening the browser does not mean an issue has been created.

Descriptions are written by the user and can contain private information. Check them for private file names, source paths, server addresses, customer information and credentials before publishing. Issues in the public repository are public.

Turning Debug mode off stops further diagnostic recording. It does not publish recorded data. Debug mode is not saved as a preference, and the next launch starts with recording disabled.

## Local diagnostic data

Debug event logs and generated reports are stored beneath `config.paths.logs/debug`. With the default configuration this is `%LOCALAPPDATA%/AudioLibExplorer/v1/logs/debug`; an existing `AUDIOLIB_DATA_DIR` selects that data directory's logs instead. Installing updated application files does not move this runtime data.

The recorder uses a bounded set of structured events and a fixed field allowlist. It records changes in sampled UI/playback state, along with explicit remote-resync and library-view-reset events. This makes it possible to relate a playback transition to a subsequent synchronization or view reset without recording library content. Event history and report size are capped, so a long recording retains limited context.

Diagnostic fields describe application version, platform, local/server mode, playback state, audio generations/session identity, visible result count, page and non-identifying audio configuration. Query content is excluded; a query's length or the presence of filters can describe the state without exporting the text. Playback configuration can describe the backend, sample rate and buffer size without exporting device identities or source paths. The UI samples changes every 250ms; reports retain at most 128 events. Two fixed session log files rotate at approximately 256 KiB each; `Debug-report.txt` is overwritten when generating another report.

The report does not bundle ordinary application logs. It excludes local paths, audio/file names, server URLs, access tokens, WebDAV passwords, Metadata, user annotations and search text. It does not include WAV assets, working libraries or preferences. The structured recorder must preserve this allowlist when new diagnostic fields are added; arbitrary error strings or a complete debug dump of settings are not safe additions.

## Capture limits

The recorder observes state outside the realtime audio callback. Audio callbacks must not write logs, allocate report data, do database work or block on diagnostic synchronization. GUI state is sampled, so very brief transitions between samples can be missed. The explicit resync/reset events supplement these samples but do not provide a complete trace of every worker operation.

Enable Debug mode **before** reproducing a failure. An empty report after enabling it cannot reconstruct earlier activity, and the bounded history may omit older events. The report is diagnostic evidence, not proof that a particular action caused a failure. Recording is not an automatic crash-report service, and diagnostic persistence can be affected by a sudden process or machine failure.

## GitHub handoff

The desktop creates a bounded, URL-encoded issue title/body and opens the fixed repository's HTTPS issue form. GitHub supports this prefill through its [issue URL query parameters](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/creating-an-issue). The handoff limits the prefilled payload because excessively long issue URLs can be rejected. Use the copied or local report when additional context is needed.

The application does not hold a GitHub personal access token and does not submit an issue through a maintainer account. GitHub's browser session performs authentication and the final submission. If the browser cannot open, the network is unavailable or the user chooses not to submit, the report remains local and can be copied for later use. The application cannot confirm creation or an issue number from a browser handoff.

## Playback and refresh investigation

Server-side playback history no longer changes the library revision (plays reply with `revision == revision_before`); older servers did, which triggered automatic snapshot synchronization. A same-library snapshot refresh preserves search, filters, sort, folder expansion and page. Automatic admission waits while an audition is Loading, Playing or Paused, and duplicate write-triggered resync requests are coalesced. Local write requests, replies and cache application are serialized so this client's out-of-order replies cannot create an avoidable revision gap; delayed listener callbacks cannot reduce the current revision. Real external changes still require synchronization. Source changes, snapshot publication and close drain admitted playback-history writes through their reply and cache settlement; queued unsent writes can cancel. Snapshot synchronization keeps new playback-history writes gated until the cache and provider publication agree. A manual sync or a download admitted before a new audition can still stop that audition when applying its snapshot. Changing the selected data source or replacing the server library retains the full reset behavior. Debug resync/reset and fixed cache-failure categories distinguish these paths.

Once the close drain is admitted, it cancels owned snapshot copying, integrity validation, index upgrades and final statistics, including offline preparation, then drains that work before exiting. A library operation or data-source switch already in progress settles first, and the deferred close is handled afterward. Cancellation cleans only this preparation's staging and preserves the original cache and reader. Already-requested Metadata writes still wait for their confirmed result or existing network timeout; save failures preserve drafts for retry. The recorder includes the closing, library-busy and draft-dirty flags without including any draft text.

For implementation boundaries, see [Rust Architecture](Rust_Architecture.md), [Remote Mode](Rust_Remote_Mode.md) and [Library Management](Rust_Library_Management.md). Diagnostic collection does not change the primary search target: **P95 latency <500ms for 1M+ records**.

## Focused verification (2026-10-13, 0.4.4)

The isolated `snapshot_refresh_tests` regressions preserve a filtered 405-record result set's folder and page during a real same-library provider replacement, retain the full reset on a changed endpoint/library, and verify that automatic resync waits for Loading audio and coalesces duplicate requests. The audio source is a controlled test opener; this does not measure physical playback latency.

The remote writer regression uses independent concurrent writers and checks that each reply advances the cache before the next request is sent. It also checks cancellation before queued playback dispatch and a genuine external revision gap requiring resync. Owned preparation regressions cancel copy, validation, upgrade and publication, checking original snapshot/cache/active bytes, open reader identity and absence of owned staging leftovers. A running SQLite statement cancellation regression returns within one second and verifies that the connection remains usable afterward.

Diagnostic regressions check exclusion of raw audio/error/identity fields and search/Metadata/path fields, UTF-8 query encoding and oversized URL fallback, bounded history/log rotation, and independent sampling of snapshot synchronization and data-source application. Four additional real HTTP regressions delay a sent play reply while switching profiles, editing the active endpoint, swapping the provider or closing; a second queued play cancels, and another endpoint's snapshot retains its own Metadata and playback history. Full release, portable and fixed-install validation results are recorded separately in [Rust Validation](Rust_Validation.md).
