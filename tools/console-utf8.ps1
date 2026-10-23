# tools/console-utf8.ps1
# Dot-sourced by tools/validate-*.ps1. Native tools (AudioLibCli) write UTF-8, but
# PowerShell decodes their piped output with the console code page (for example
# GB2312/936), which mangles non-ASCII paths such as `ÉCHO`. Switch to UTF-8 for the
# script's lifetime; callers restore the previous encodings in `finally`.
$script:AudioLibSavedConsoleOutputEncoding = [Console]::OutputEncoding
$script:AudioLibSavedOutputEncoding = $OutputEncoding
$script:AudioLibUtf8 = [Text.UTF8Encoding]::new($false)
try { [Console]::OutputEncoding = $script:AudioLibUtf8 } catch { Write-Verbose "Console output encoding unchanged: $_" }
$script:OutputEncoding = $script:AudioLibUtf8

function Restore-AudioLibConsoleEncoding {
    try { [Console]::OutputEncoding = $script:AudioLibSavedConsoleOutputEncoding } catch { Write-Verbose "Console output encoding not restored: $_" }
    $script:OutputEncoding = $script:AudioLibSavedOutputEncoding
}
