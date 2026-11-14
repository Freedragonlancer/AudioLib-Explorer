# tools/test-validator-build-routing.ps1
# Execute only validators' parsed path-selection prefix with an owned fake root.
# No validation body, native app, library, ACL change or Rust build is executed.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-cache.ps1')
$taskFixture = Join-Path ([IO.Path]::GetTempPath()) ('AudioLibValidatorRoutingTest-' + [guid]::NewGuid().ToString('N'))
$taskTools = Join-Path $taskFixture 'tools'
$taskLocalData = Join-Path $taskFixture 'local-app-data'
$taskSavedTarget = $env:CARGO_TARGET_DIR
$taskSavedLocalData = $env:LOCALAPPDATA
try {
    New-Item -ItemType Directory -Path $taskTools -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-cache.ps1') -Destination $taskTools
    $env:LOCALAPPDATA = $taskLocalData
    foreach ($script in @('validate-access-denial.ps1','validate-library-management.ps1','validate-playback-ipc.ps1','validate-remote-resume.ps1','validate-review-fixes.ps1','validate-runtime.ps1')) {
        $path = Join-Path $PSScriptRoot $script
        # Decode UTF-8 consistently even on Windows PowerShell's ANSI-default
        # ParseFile path; the supported validator runtime is pwsh.
        $source = [IO.File]::ReadAllText($path)
        $parseErrors = $null
        $tokens = $null
        $ast = [Management.Automation.Language.Parser]::ParseInput($source,[ref]$tokens,[ref]$parseErrors)
        if ($parseErrors.Count) { throw "Invalid PowerShell syntax in $script`: $parseErrors" }
        $assignments = @($ast.FindAll({param($node)
            $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left -is [Management.Automation.Language.VariableExpressionAst]
        },$true))
        $root = $assignments | Where-Object { $_.Left.VariablePath.UserPath -in @('taskRoot','taskRepo') } | Select-Object -First 1
        $binary = $assignments | Where-Object { $_.Left.VariablePath.UserPath -in @('binaryRoot','taskBinary') } | Select-Object -First 1
        if (!$root -or !$binary -or $binary.Extent.StartOffset -le $root.Extent.StartOffset) { throw "Could not isolate $script path routing without running its validator." }
        $prefix = $source.Substring($root.Extent.StartOffset,$binary.Extent.EndOffset - $root.Extent.StartOffset)
        $selection = [scriptblock]::Create('param($Tools,[string]$BinaryDirectory)' + "`n" + '$PSScriptRoot=$Tools' + "`n" + $prefix + "`n" + 'Get-Variable -Name ' + $binary.Left.VariablePath.UserPath + ' -ValueOnly')
        foreach ($case in @(
            @{Target=$null;Binary=$null},
            @{Target='explicit target';Binary=$null},
            @{Target=$null;Binary='explicit binaries'},
            @{Target='explicit target';Binary=(Join-Path $taskFixture 'absolute binaries')}
        )) {
            [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR',$case.Target,'Process')
            $expected = if ($case.Binary) {
                if ([IO.Path]::IsPathRooted($case.Binary)) { $case.Binary } else { Join-Path $taskFixture $case.Binary }
            } else { Join-Path (Get-AudioLibTargetDirectory $taskFixture) 'x86_64-pc-windows-msvc/release' }
            $result = & $selection $taskTools $case.Binary
            if ((Get-AudioLibCanonicalPath $result) -cne (Get-AudioLibCanonicalPath $expected)) { throw "$script selected incorrect binaries for target=$($case.Target); binary=$($case.Binary)." }
            if (Test-Path -LiteralPath $expected) { throw "$script read-only routing created a binary/cache directory." }
        }
        Write-Host "PASS: $script routes default/local/explicit binaries without running validation."
    }
    Write-Host "Validator build routing passed. Owned evidence: $taskFixture"
} finally {
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR',$taskSavedTarget,'Process')
    [Environment]::SetEnvironmentVariable('LOCALAPPDATA',$taskSavedLocalData,'Process')
}
