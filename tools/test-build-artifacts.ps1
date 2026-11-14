# tools/test-build-artifacts.ps1
# Destructive cases use only synthetic caches beneath this invocation's owned
# temporary Git checkout. No real cache, SDK, application or library is changed.
#Requires -Version 7.3
$ErrorActionPreference='Stop'
$taskRoot=Join-Path ([IO.Path]::GetTempPath()) ('AudioLibCacheTest-'+[guid]::NewGuid().ToString('N'))
$taskTarget=Join-Path $taskRoot 'target'
$taskCleaner=Join-Path $PSScriptRoot 'clean-build-artifacts.ps1'
$taskSignature="Signature: 8a477f597d28d172789f06886806bc55`n# synthetic Cargo fixture"
New-Item -ItemType Directory -Path $taskRoot | Out-Null
& git -C $taskRoot init -q
if($LASTEXITCODE -ne 0){throw 'Fixture Git initialization failed.'}
& git -C $taskRoot config core.autocrlf false
[IO.File]::WriteAllText((Join-Path $taskRoot 'Cargo.toml'),"[workspace]`nmembers=[`"crates/audiolib-core`"]`n")
. (Join-Path $PSScriptRoot 'build-cache.ps1')
$global:taskCacheTestActive=$false
$global:taskCacheTestProbeCount=0
$global:taskCacheTestStartAt=0
function Get-Process {$global:taskCacheTestProbeCount++;if($global:taskCacheTestActive -or ($global:taskCacheTestStartAt -gt 0 -and $global:taskCacheTestProbeCount -ge $global:taskCacheTestStartAt)){[pscustomobject]@{ProcessName='rustc';Id=123;Path=(Join-Path $taskTarget 'debug/rustc.exe')}}}
function New-Cache([string]$Path){
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $Path 'CACHEDIR.TAG'),$taskSignature)
    [IO.File]::WriteAllText((Join-Path $Path '.rustc_info.json'),'{}')
    foreach($name in @('debug/deps/test.exe','debug/incremental/session/cache.bin','x86_64-pc-windows-msvc/debug/deps/test.exe','x86_64-pc-windows-msvc/release/app.exe','doc/index.html','tmp/temp.bin')){
        $file=Join-Path $Path $name
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($file)) -Force | Out-Null
        [IO.File]::WriteAllBytes($file,[byte[]]::new(256))
    }
}
function Invoke-Clean([hashtable]$Options=@{},[int]$ExpectedExit=0){
    $parameters=@{Root=$taskRoot;TargetDirectory=$taskTarget;Action='Run';Mode='Debug';MaxGiB=0}
    foreach($key in $Options.Keys){$parameters[$key]=$Options[$key]}
    $output=& $taskCleaner @parameters *>&1
    if($LASTEXITCODE -ne $ExpectedExit){throw "Unexpected cleanup result: $($output|Out-String)"}
    $json=@($output|ForEach-Object{$_.ToString()}|Where-Object{$_ -match '^\{'})
    if($json.Count -ne 1){throw 'Cleanup did not produce one compact JSON record.'}
    $json[0]|ConvertFrom-Json
}
New-Cache $taskTarget
$preview=Invoke-Clean @{Action='Plan'}
if($preview.status -ne 'planned' -or $preview.removed_bytes -ne 0 -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'Preview changed artifacts.'}
$underBudget=Invoke-Clean @{MaxGiB=40}
if($underBudget.removed_bytes -ne 0 -or !$underBudget.budget_met){throw 'Small cache was needlessly removed.'}
$whatif=Invoke-Clean @{WhatIf=$true}
if($whatif.removed_bytes -ne 0 -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'WhatIf deleted artifacts.'}
$lease=Enter-AudioLibBuildLease $taskRoot
try{$blocked=Invoke-Clean @{} 2;if($blocked.status -ne 'blocked'){throw 'Active build lease was ignored.'}}finally{$lease.Dispose()}
$global:taskCacheTestActive=$true
$blocked=Invoke-Clean @{} 2
$global:taskCacheTestActive=$false
if($blocked.status -ne 'blocked'){throw 'Active compiler was ignored.'}
$global:taskCacheTestProbeCount=0
$global:taskCacheTestStartAt=2
try{
    $blocked=Invoke-Clean @{} 2
    if($blocked.removed_bytes -ne 0 -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'A compiler starting after inventory was ignored.'}
}finally{$global:taskCacheTestStartAt=0}
$incremental=Invoke-Clean @{Mode='Incremental'}
if($incremental.removed_bytes -ne 256 -or (Test-Path -LiteralPath (Join-Path $taskTarget 'debug/incremental')) -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'Incremental cleanup exceeded its boundary.'}
$age=Invoke-Clean @{OlderThanDays=30}
if($age.removed_bytes -ne 0){throw 'Recent cache ignored retention age.'}
Write-Host 'PASS: preview/WhatIf/budget/age, shared build leases and active compilers; incremental cleanup preserves compiled outputs.'
$outside=Join-Path $taskRoot 'outside'
New-Item -ItemType Directory -Path $outside | Out-Null
[IO.File]::WriteAllText((Join-Path $outside 'keep.txt'),'outside untouched')
$link=Join-Path $taskTarget 'debug/linked'
if($IsWindows){New-Item -ItemType Junction -Path $link -Target $outside | Out-Null}else{$null=[IO.Directory]::CreateSymbolicLink($link,$outside)}
try{
    $null=Invoke-Clean @{} 1
    if([IO.File]::ReadAllText((Join-Path $outside 'keep.txt')) -cne 'outside untouched' -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'Linked tree escaped cleanup scope.'}
}finally{
    if(![IO.Path]::GetFullPath($link).StartsWith($taskTarget+[IO.Path]::DirectorySeparatorChar,[StringComparison]::Ordinal)){throw 'Fixture cleanup escaped owned target.'}
    [IO.Directory]::Delete($link)
}
[IO.File]::WriteAllText((Join-Path $taskTarget 'working.db'),'not a cache')
$null=Invoke-Clean @{} 1
Remove-Item -LiteralPath (Join-Path $taskTarget 'working.db')
Remove-Item -LiteralPath (Join-Path $taskTarget 'CACHEDIR.TAG')
$null=Invoke-Clean @{} 1
[IO.File]::WriteAllText((Join-Path $taskTarget 'CACHEDIR.TAG'),$taskSignature)
foreach($protected in @($taskRoot,(Join-Path $taskRoot 'local_release'),(Join-Path $taskRoot 'tools/.build-tools'),(Join-Path $taskRoot 'data'))){$null=Invoke-Clean @{TargetDirectory=$protected} 1}
$debug=Invoke-Clean
if((Test-Path -LiteralPath (Join-Path $taskTarget 'debug')) -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'x86_64-pc-windows-msvc/release/app.exe')) -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'CACHEDIR.TAG'))){throw 'Debug cleanup removed release/root markers or retained debug outputs.'}
$all=Invoke-Clean @{Mode='All'}
if((Test-Path -LiteralPath (Join-Path $taskTarget 'x86_64-pc-windows-msvc/release')) -or (Test-Path -LiteralPath (Join-Path $taskTarget 'doc')) -or !(Test-Path -LiteralPath (Join-Path $taskTarget '.rustc_info.json'))){throw 'Explicit All scope is incorrect.'}
Write-Host 'PASS: markers/layout/links/protected paths checked; debug preserves release; explicit All only removes Cargo profiles/docs/tmp.'
New-Cache $taskTarget
foreach($name in @('debug/app.exe','release/app.exe','release/app.pdb','release/libapp.rlib','release/.cargo-lock','x86_64-pc-windows-msvc/release/app.pdb','x86_64-pc-windows-msvc/release/libapp.rlib')){
    $file=Join-Path $taskTarget $name
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($file)) -Force | Out-Null
    [IO.File]::WriteAllBytes($file,[byte[]]::new(256))
}
foreach($profile in @('debug','release','x86_64-pc-windows-msvc/debug','x86_64-pc-windows-msvc/release')){
    foreach($temporary in @('build','deps','.fingerprint','incremental','examples')){
        $file=Join-Path $taskTarget ($profile+'/'+$temporary+'/temporary.bin')
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($file)) -Force | Out-Null
        [IO.File]::WriteAllBytes($file,[byte[]]::new(256))
    }
}
$targetTemporary=Join-Path $taskTarget 'x86_64-pc-windows-msvc/tmp'
New-Item -ItemType Directory -Path $targetTemporary -Force | Out-Null
[IO.File]::WriteAllBytes((Join-Path $targetTemporary 'temporary.bin'),[byte[]]::new(256))
$intermediatePreview=Invoke-Clean @{Action='Plan';Mode='Intermediate'}
if($intermediatePreview.status -ne 'planned' -or $intermediatePreview.removed_bytes -ne 0 -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'release/deps/temporary.bin'))){throw 'Intermediate preview changed artifacts.'}
$intermediate=Invoke-Clean @{Mode='Intermediate'}
foreach($profile in @('debug','release','x86_64-pc-windows-msvc/debug','x86_64-pc-windows-msvc/release')){
    foreach($temporary in @('build','deps','.fingerprint','incremental','examples')){
        if(Test-Path -LiteralPath (Join-Path $taskTarget ($profile+'/'+$temporary))){throw 'Intermediate cleanup retained rebuildable profile files.'}
    }
}
foreach($name in @('debug/app.exe','release/app.exe','release/app.pdb','release/libapp.rlib','release/.cargo-lock','x86_64-pc-windows-msvc/release/app.exe','x86_64-pc-windows-msvc/release/app.pdb','x86_64-pc-windows-msvc/release/libapp.rlib','doc/index.html','CACHEDIR.TAG','.rustc_info.json')){
    if(!(Test-Path -LiteralPath (Join-Path $taskTarget $name))){throw 'Intermediate cleanup removed a final artifact, documentation or marker.'}
}
if($intermediate.removed_bytes -le 0 -or (Test-Path -LiteralPath (Join-Path $taskTarget 'tmp')) -or (Test-Path -LiteralPath $targetTemporary)){throw 'Intermediate cleanup did not reclaim owned temporary files.'}
Write-Host 'PASS: intermediate cleanup removes host/target compilation caches and tmp while preserving final binaries, libraries, PDBs, docs and markers.'
$external=Join-Path $taskRoot 'external-target'
New-Cache $external
$preview=Invoke-Clean @{Action='Plan';TargetDirectory=$external}
if($preview.ownership -ne 'unregistered-preview-only'){throw 'External preview silently claimed ownership.'}
$null=Invoke-Clean @{TargetDirectory=$external} 1
$null=Invoke-Clean @{Action='Register';TargetDirectory=$external} 1
$registered=Invoke-Clean @{Action='Register';TargetDirectory=$external;AdoptExisting=$true}
if($registered.status -ne 'registered'){throw 'Explicit cache registration failed.'}
$marker=Join-Path $external '.audiolib-build-owner.json'
$markerAttributes=[IO.File]::GetAttributes($marker)
try{
    # Dotfiles are hidden on Unix; Windows requires the real Hidden attribute.
    if($IsWindows){[IO.File]::SetAttributes($marker,($markerAttributes -bor [IO.FileAttributes]::Hidden))}
    if(!((Get-Item -LiteralPath $marker -Force).Attributes -band [IO.FileAttributes]::Hidden)){throw 'Ownership regression requires a genuinely hidden marker.'}
    $hiddenPreview=Invoke-Clean @{Action='Plan';TargetDirectory=$external}
    if($hiddenPreview.status -ne 'planned' -or $hiddenPreview.ownership -ne 'registered' -or $hiddenPreview.removed_bytes -ne 0){throw 'Hidden cache ownership was not recognized safely during preview.'}
    $hiddenRun=Invoke-Clean @{TargetDirectory=$external}
    if($hiddenRun.status -ne 'passed' -or $hiddenRun.ownership -ne 'registered' -or $hiddenRun.removed_bytes -le 0 -or !(Test-Path -LiteralPath $marker) -or !(Test-Path -LiteralPath (Join-Path $external 'x86_64-pc-windows-msvc/release/app.exe'))){throw 'Hidden-marker cleanup lost ownership or exceeded the debug boundary.'}
}finally{
    if($IsWindows -and (Test-Path -LiteralPath $marker)){[IO.File]::SetAttributes($marker,$markerAttributes)}
}
@{schema_version=1;owner_id='foreign';host=[Environment]::MachineName}|ConvertTo-Json|Set-Content -LiteralPath $marker -Encoding utf8
$null=Invoke-Clean @{TargetDirectory=$external;Mode='All'} 1
if(!(Test-Path -LiteralPath (Join-Path $external 'x86_64-pc-windows-msvc/release/app.exe'))){throw 'Foreign-owned cache was deleted.'}
Write-Host 'PASS: external caches require explicit adoption; hidden owner markers support preview/cleanup; foreign ownership refused.'
$precreated=Join-Path $taskRoot 'precreated-target'
$registered=Invoke-Clean @{Action='Register';TargetDirectory=$precreated}
if($registered.status -ne 'registered' -or (Test-Path -LiteralPath (Join-Path $precreated 'CACHEDIR.TAG'))){throw 'Fresh registration unexpectedly fabricated a Cargo tag.'}
function New-UntaggedCargoEvidence([string]$Path){
    foreach($name in @('debug/.fingerprint/owned_fixture-0123456789abcdef','debug/deps')){New-Item -ItemType Directory -Path (Join-Path $Path $name) -Force|Out-Null}
    $state=@{rustc_fingerprint=[ulong]123456789;outputs=@{'123456789'=@{success=$true;status='exit status: 0';code=0;stdout="rustc 1.97.1 (ownedfixture 2026-07-14)`nbinary: rustc`nhost: x86_64-pc-windows-msvc`nrelease: 1.97.1`n";stderr=''}};successes=@{}}
    $state|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $Path '.rustc_info.json') -Encoding utf8
    [IO.File]::WriteAllBytes((Join-Path $Path 'debug/deps/libowned_fixture-0123456789abcdef.rmeta'),[byte[]]::new(256))
}
New-UntaggedCargoEvidence $precreated
New-Item -ItemType Directory -Path (Join-Path $precreated 'x86_64-unknown-linux-gnu')|Out-Null
$retainedTripleTag=Join-Path $precreated 'x86_64-unknown-linux-gnu/CACHEDIR.TAG'
[IO.File]::WriteAllText($retainedTripleTag,$taskSignature)
$preview=Invoke-Clean @{Action='Plan';TargetDirectory=$precreated}
if($preview.status -ne 'planned' -or $preview.ownership -ne 'registered' -or $preview.cache_evidence -ne 'registered-cargo-state' -or $preview.removed_bytes -ne 0){throw 'Pre-registered Cargo outputs without a tag were not recognized during preview.'}
$precreatedOwner=Join-Path $precreated '.audiolib-build-owner.json'
$validOwner=[IO.File]::ReadAllText($precreatedOwner)
Remove-Item -LiteralPath $precreatedOwner
$null=Invoke-Clean @{Action='Plan';TargetDirectory=$precreated} 1
[IO.File]::WriteAllText($precreatedOwner,$validOwner)
@{schema_version=1;owner_id='foreign';host=[Environment]::MachineName}|ConvertTo-Json|Set-Content -LiteralPath $precreatedOwner -Encoding utf8
$null=Invoke-Clean @{TargetDirectory=$precreated} 1
[IO.File]::WriteAllText($precreatedOwner,$validOwner)
$infoPath=Join-Path $precreated '.rustc_info.json'
$validInfo=[IO.File]::ReadAllText($infoPath)
foreach($invalidInfo in @('{}','{"rustc_fingerprint":"123","outputs":{}}',(' ' * 65537))){
    [IO.File]::WriteAllText($infoPath,$invalidInfo)
    $null=Invoke-Clean @{TargetDirectory=$precreated} 1
}
[IO.File]::WriteAllText($infoPath,$validInfo)
$global:taskCacheTestActive=$true
try{$blocked=Invoke-Clean @{TargetDirectory=$precreated} 2;if($blocked.status -ne 'blocked'){throw 'Untagged Cargo evidence bypassed the active compiler guard.'}}finally{$global:taskCacheTestActive=$false}
$cleaned=Invoke-Clean @{TargetDirectory=$precreated;Mode='All'}
if($cleaned.status -ne 'passed' -or $cleaned.removed_bytes -ne 256 -or (Test-Path -LiteralPath (Join-Path $precreated 'debug')) -or (Test-Path -LiteralPath (Join-Path $precreated 'CACHEDIR.TAG')) -or !(Test-Path -LiteralPath $precreatedOwner)){throw 'Pre-registered untagged Cargo cleanup exceeded its ownership boundary.'}
$emptyPreview=Invoke-Clean @{Action='Plan';TargetDirectory=$precreated;Mode='All'}
$emptyRun=Invoke-Clean @{TargetDirectory=$precreated;Mode='All'}
if($emptyPreview.status -ne 'planned' -or $emptyRun.status -ne 'passed' -or $emptyRun.removed_bytes -ne 0 -or $emptyRun.before_bytes -ne 0 -or [IO.File]::ReadAllText($retainedTripleTag) -cne $taskSignature){throw 'Already-cleared registered Cargo state with a retained target-triple tag was not idempotent.'}
foreach($invalidTag in @('not a Cargo tag',($taskSignature+(' ' * 4097)))){
    [IO.File]::WriteAllText($retainedTripleTag,$invalidTag)
    $null=Invoke-Clean @{Action='Plan';TargetDirectory=$precreated;Mode='All'} 1
}
[IO.File]::WriteAllText($retainedTripleTag,$taskSignature)
$tripleExtra=Join-Path $precreated 'x86_64-unknown-linux-gnu/keep.txt'
[IO.File]::WriteAllText($tripleExtra,'a valid tag does not authorize unrelated data')
$null=Invoke-Clean @{TargetDirectory=$precreated;Mode='All'} 1
if(!(Test-Path -LiteralPath $tripleExtra)){throw 'Retained target-triple tag authorized unrelated data.'}
Remove-Item -LiteralPath $tripleExtra
$notCargo=Join-Path $taskRoot 'registered-noncargo-target'
$null=Invoke-Clean @{Action='Register';TargetDirectory=$notCargo}
New-Item -ItemType Directory -Path (Join-Path $notCargo 'debug')|Out-Null
[IO.File]::WriteAllText((Join-Path $notCargo 'debug/notes.txt'),'owned registration is not Cargo artifact evidence')
$null=Invoke-Clean @{TargetDirectory=$notCargo} 1
if(!(Test-Path -LiteralPath (Join-Path $notCargo 'debug/notes.txt'))){throw 'Owner-only non-Cargo files were removed.'}
[IO.File]::WriteAllText((Join-Path $notCargo '.rustc_info.json'),$validInfo)
$null=Invoke-Clean @{TargetDirectory=$notCargo} 1
Remove-Item -LiteralPath (Join-Path $notCargo 'debug/notes.txt')
New-Item -ItemType Directory -Path (Join-Path $notCargo 'x86_64-unknown-linux-gnu')|Out-Null
[IO.File]::WriteAllText((Join-Path $notCargo 'x86_64-unknown-linux-gnu/notes.txt'),'a populated target container is not cleared Cargo state')
$null=Invoke-Clean @{TargetDirectory=$notCargo} 1
Write-Host 'PASS: pre-registered genuine-shaped Cargo state supports missing tags; unowned/foreign/invalid/non-Cargo state and active compilers remain blocked.'
$untaggedIntermediate=Join-Path $taskRoot 'untagged-intermediate-target'
$null=Invoke-Clean @{Action='Register';TargetDirectory=$untaggedIntermediate}
New-UntaggedCargoEvidence $untaggedIntermediate
foreach($name in @('debug/app.exe','debug/app.pdb','x86_64-pc-windows-msvc/release/app.exe','x86_64-pc-windows-msvc/release/app.pdb')){
    $file=Join-Path $untaggedIntermediate $name
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($file)) -Force|Out-Null
    [IO.File]::WriteAllBytes($file,[byte[]]::new(256))
}
$firstIntermediate=Invoke-Clean @{TargetDirectory=$untaggedIntermediate;Mode='Intermediate'}
if($firstIntermediate.status -ne 'passed' -or $firstIntermediate.removed_bytes -ne 256){throw 'Untagged intermediate cleanup did not remove compilation artifacts.'}
$retainedExecutable=Join-Path $untaggedIntermediate 'debug/app.exe'
$retainedHash=(Get-FileHash -LiteralPath $retainedExecutable).Hash
$retainedOwner=Join-Path $untaggedIntermediate '.audiolib-build-owner.json'
$ownerBeforePreview=[IO.File]::ReadAllText($retainedOwner)
$retainedPreview=Invoke-Clean @{Action='Plan';TargetDirectory=$untaggedIntermediate;Mode='Intermediate'}
if($retainedPreview.status -ne 'planned' -or $retainedPreview.removed_bytes -ne 0 -or [IO.File]::ReadAllText($retainedOwner) -cne $ownerBeforePreview){throw 'Post-intermediate preview failed or changed registered ownership.'}
$null=Invoke-Clean @{TargetDirectory=$untaggedIntermediate;Mode='All'} 1
$temporary=Join-Path $untaggedIntermediate 'tmp/compiler.tmp'
New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($temporary)) -Force|Out-Null
[IO.File]::WriteAllBytes($temporary,[byte[]]::new(128))
$retainedInfo=Join-Path $untaggedIntermediate '.rustc_info.json'
$savedInfo=[IO.File]::ReadAllText($retainedInfo)
try{
    [IO.File]::WriteAllText($retainedInfo,($savedInfo+' '))
    $null=Invoke-Clean @{TargetDirectory=$untaggedIntermediate;Mode='Intermediate'} 1
    if(!(Test-Path -LiteralPath $temporary)){throw 'Stale post-intermediate evidence authorized temporary-file deletion.'}
}finally{[IO.File]::WriteAllText($retainedInfo,$savedInfo)}
$retainedRun=Invoke-Clean @{TargetDirectory=$untaggedIntermediate;Mode='Intermediate'}
if($retainedRun.status -ne 'passed' -or $retainedRun.removed_bytes -ne 128 -or (Test-Path -LiteralPath $temporary) -or
    (Get-FileHash -LiteralPath $retainedExecutable).Hash -cne $retainedHash -or (Test-Path -LiteralPath (Join-Path $untaggedIntermediate 'CACHEDIR.TAG'))){throw 'Repeated untagged intermediate cleanup changed final executables or fabricated a Cargo tag.'}
foreach($name in @('debug/app.pdb','x86_64-pc-windows-msvc/release/app.exe','x86_64-pc-windows-msvc/release/app.pdb')){
    if(!(Test-Path -LiteralPath (Join-Path $untaggedIntermediate $name))){throw 'Repeated untagged intermediate cleanup removed retained EXE/PDB outputs.'}
}
Write-Host 'PASS: untagged Intermediate cleanup retains EXE/PDBs across repeat Plan/Run, removes new Cargo tmp, rejects changed compiler evidence and does not authorize All.'
$realCompilers=@(Microsoft.PowerShell.Management\Get-Process | Where-Object ProcessName -match '^(cargo|rustc|rustdoc|clippy-driver|clang|clang\+\+|gcc|g\+\+|cc1|cc1plus|link|lld|lld-link|cl)$')
if($realCompilers.Count){Write-Host 'Pipeline cleanup integration: not-run (another real compiler is active).'}else{
    $tools=Join-Path $taskRoot 'tools'
    New-Item -ItemType Directory -Path $tools -Force | Out-Null
    foreach($name in @('clean-build-artifacts.ps1','build-cache.ps1')){Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $tools}
    [IO.File]::WriteAllText((Join-Path $taskRoot '.gitignore'),"target/`ndist/`n.rust-validation/`n*.log`n")
    [IO.File]::WriteAllText((Join-Path $tools 'deliver.ps1'),@'
if(Test-Path -LiteralPath .rust-validation/fail-delivery){throw 'Expected delivery failure.'}
New-Item -ItemType Directory -Path dist -Force | Out-Null
[IO.File]::WriteAllText((Join-Path (Get-Location) 'dist/accepted.txt'),'accepted payload')
'@)
    $plan=@{schema_version=1;name='cleanup-integration';host='any';stages=@(@{id='deliver';reuse='never';script='tools/deliver.ps1';outputs=@('dist/accepted.txt')})}
    $planPath=Join-Path $taskRoot 'pipeline.json'
    $plan|ConvertTo-Json -Depth 6|Set-Content -LiteralPath $planPath -Encoding utf8
    $reusePlan=@{schema_version=1;name='cleanup-reuse';host='any';stages=@(@{id='build';script='tools/deliver.ps1';outputs=@('${TargetDirectory}/x86_64-pc-windows-msvc/release/app.exe','dist/accepted.txt')})}
    $reusePlanPath=Join-Path $taskRoot 'reuse-pipeline.json'
    $reusePlan|ConvertTo-Json -Depth 6|Set-Content -LiteralPath $reusePlanPath -Encoding utf8
    & git -C $taskRoot -c user.name=CacheTest -c user.email=cache@example.invalid add .
    & git -C $taskRoot -c user.name=CacheTest -c user.email=cache@example.invalid commit -qm fixture
    if($LASTEXITCODE -ne 0){throw 'Could not commit pipeline fixture.'}
    $savedTarget=$env:CARGO_TARGET_DIR
    try{
        $env:CARGO_TARGET_DIR=$taskTarget
        New-Cache $taskTarget
        $runner=Join-Path $PSScriptRoot 'release-pipeline.ps1'
        $run=Join-Path $taskRoot '.rust-validation/release-pipeline/cleanup'
        $pwsh=Join-Path $PSHOME ('pwsh'+$(if($IsWindows){'.exe'}else{''}))
        & $pwsh -NoProfile -File $runner -Root $taskRoot -PlanFile $planPath -RunDirectory $run -Tag v0.0.1 -Action Run -CleanBuildArtifacts -BuildCacheLimitGiB 0 *> (Join-Path $taskRoot 'pipeline-success.log')
        if($LASTEXITCODE -ne 0){throw 'Pipeline post-delivery cleanup failed; inspect owned pipeline-success.log.'}
        if((Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps')) -or (Test-Path -LiteralPath (Join-Path $taskTarget 'tmp')) -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'x86_64-pc-windows-msvc/release/app.exe'))){throw 'Pipeline cleanup removed wrong outputs.'}
        New-Cache $taskTarget
        $reuseRun=Join-Path $taskRoot '.rust-validation/release-pipeline/reuse'
        & $pwsh -NoProfile -File $runner -Root $taskRoot -PlanFile $reusePlanPath -RunDirectory $reuseRun -Tag v0.0.1 -Action Run *> (Join-Path $taskRoot 'pipeline-default-cleanup.log')
        if($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps')) -or (Test-Path -LiteralPath (Join-Path $taskTarget 'tmp'))){throw 'Pipeline default successful build did not clean intermediate files.'}
        New-Item -ItemType Directory -Path (Join-Path $taskTarget 'tmp') -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $taskTarget 'tmp/unchanged-result.tmp'),'Do not rescan when all builds are reused.')
        & $pwsh -NoProfile -File $runner -Root $taskRoot -PlanFile $reusePlanPath -RunDirectory $reuseRun -Tag v0.0.1 -Action Run *> (Join-Path $taskRoot 'pipeline-default-reuse.log')
        if($LASTEXITCODE -ne 0){throw 'Pipeline could not resume after successful intermediate cleanup.'}
        $reuseState=Get-Content -LiteralPath (Join-Path $reuseRun 'state.json') -Raw|ConvertFrom-Json
        if($reuseState.stages.build.status -ne 'reused' -or $reuseState.stages.'build-cache-cleanup'.status -ne 'reused' -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'tmp/unchanged-result.tmp'))){throw 'Unchanged pipeline results rebuilt payloads or repeated automatic cache maintenance.'}
        New-Cache $taskTarget
        [IO.File]::WriteAllText((Join-Path $taskRoot '.rust-validation/fail-delivery'),'fail')
        & $pwsh -NoProfile -File $runner -Root $taskRoot -PlanFile $planPath -RunDirectory $run -Tag v0.0.1 -Action Run -CleanBuildArtifacts -BuildCacheLimitGiB 0 *> (Join-Path $taskRoot 'pipeline-failure.log')
        if($LASTEXITCODE -eq 0 -or !(Test-Path -LiteralPath (Join-Path $taskTarget 'debug/deps/test.exe'))){throw 'Failed delivery triggered cleanup or was accepted.'}
        $probe=Enter-AudioLibBuildLease $taskRoot -Exclusive
        $probe.Dispose()
    }finally{$env:CARGO_TARGET_DIR=$savedTarget}
    Write-Host 'PASS: pipeline cleans after success, resumes accepted binaries without rescanning unchanged caches, and retains failed-delivery evidence.'
}
Write-Host "Build artifact cleanup regressions passed. Owned evidence: $taskRoot"
