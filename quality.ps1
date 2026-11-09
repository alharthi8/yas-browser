param(
    [string]$TargetDirectory = $env:CARGO_TARGET_DIR,
    [string]$ReportPath = (Join-Path $PSScriptRoot 'QUALITY_GATE_RESULTS.json'),
    [string]$EvidenceDirectory,
    [string]$AuditExecutable,
    [string]$AdvisoryDatabase
)
# Build/test tools only. Never starts Yas, installs tools, or opens browser data.
$ErrorActionPreference = 'Stop'
if (!$TargetDirectory) { $TargetDirectory = Join-Path $PSScriptRoot 'target' }
if (!$EvidenceDirectory) { $EvidenceDirectory = Join-Path $PSScriptRoot ('quality-evidence\' + [guid]::NewGuid().ToString('N')) }
if ([bool]$AuditExecutable -ne [bool]$AdvisoryDatabase) { throw 'For the optional advisory audit, pass both -AuditExecutable and -AdvisoryDatabase.' }
if ($AuditExecutable -and !(Test-Path -LiteralPath $AuditExecutable -PathType Leaf)) { throw 'Explicit audit executable does not exist. No tools are installed automatically.' }
if ($AdvisoryDatabase -and (Test-Path -LiteralPath $AdvisoryDatabase)) { throw 'Choose a new advisory database path so this audit fetches a fresh database.' }
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
$EvidenceDirectory = [IO.Path]::GetFullPath($EvidenceDirectory)
$ReportPath = [IO.Path]::GetFullPath($ReportPath)
New-Item -ItemType Directory -Force -Path $EvidenceDirectory | Out-Null
$manifest = Join-Path $PSScriptRoot 'Cargo.toml'
$lockfile = Join-Path $PSScriptRoot 'Cargo.lock'
if (!(Test-Path -LiteralPath $lockfile -PathType Leaf)) { throw 'Cargo.lock is required; this gate does not generate a lockfile.' }
$packageText = Get-Content -LiteralPath $manifest -Raw
$versionMatch = [regex]::Match($packageText, '(?ms)^\[package\].*?^version\s*=\s*"([^"\r\n]+)"')
if (!$versionMatch.Success) { throw 'Could not read package version.' }
$version = $versionMatch.Groups[1].Value

function Get-SourceFingerprint {
    $files = @()
    foreach ($name in @('Cargo.toml','Cargo.lock','build.ps1','package.ps1','quality.ps1','README.md','RELEASE_NOTES.md','RENAME_NOTES.md','THIRD_PARTY.md','installer\Yas.iss')) {
        $path = Join-Path $PSScriptRoot $name
        if (Test-Path -LiteralPath $path -PathType Leaf) { $files += Get-Item -LiteralPath $path }
    }
    foreach ($name in @('src','branding','filters')) {
        $path = Join-Path $PSScriptRoot $name
        if (Test-Path -LiteralPath $path) { $files += Get-ChildItem -LiteralPath $path -File -Recurse }
    }
    $records = @($files | Sort-Object FullName -Unique | ForEach-Object {
        $relative = $_.FullName.Substring($PSScriptRoot.Length + 1).Replace('\','/')
        $relative + ' ' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    })
    $bytes = [Text.Encoding]::UTF8.GetBytes(($records -join "`n"))
    $hasher = [Security.Cryptography.SHA256]::Create()
    try { $hash = ([BitConverter]::ToString($hasher.ComputeHash($bytes))).Replace('-','') }
    finally { $hasher.Dispose() }
    return [ordered]@{sha256=$hash;fileCount=$records.Count}
}

function Set-NativeBuildEnvironment {
    # Same discovery contract as build.ps1; no app resource build or launch.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vsRoot = if (Test-Path -LiteralPath $vswhere) { & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
    if (!$vsRoot) { throw 'Install Visual Studio C++ Build Tools with the x64 MSVC compiler.' }
    $msvc = Get-ChildItem -LiteralPath (Join-Path $vsRoot 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
    $kits = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue
    $sdkRoot = if ($kits.KitsRoot10) { $kits.KitsRoot10 } else { Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10' }
    $sdk = Get-ChildItem -LiteralPath (Join-Path $sdkRoot 'Lib') -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um\x64\kernel32.lib') } | Sort-Object Name -Descending | Select-Object -First 1
    if (!$msvc -or !$sdk) { throw 'MSVC x64 tools and Windows SDK libraries are required.' }
    $env:LIB = "$($msvc.FullName)\lib\x64;$($sdk.FullName)\um\x64;$($sdk.FullName)\ucrt\x64;$env:LIB"
    $env:PATH = "$($msvc.FullName)\bin\HostX64\x64;$env:USERPROFILE\.cargo\bin;$env:PATH"
    $env:CARGO_TARGET_DIR = $TargetDirectory
}

function Invoke-GateCommand([string]$Name, [string]$File, [string[]]$Arguments, [string]$WorkingDirectory) {
    $stdout = Join-Path $EvidenceDirectory ($Name + '.stdout.log')
    $stderr = Join-Path $EvidenceDirectory ($Name + '.stderr.log')
    $started = Get-Date
    try {
        $process = Start-Process -FilePath $File -ArgumentList $Arguments -WorkingDirectory $WorkingDirectory -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -Wait -PassThru
        return [ordered]@{passed=($process.ExitCode -eq 0);exitCode=$process.ExitCode;seconds=[math]::Round(((Get-Date)-$started).TotalSeconds,3);stdout=[IO.Path]::GetFileName($stdout);stderr=[IO.Path]::GetFileName($stderr)}
    } catch {
        $_ | Out-String | Set-Content -LiteralPath $stderr -Encoding utf8
        return [ordered]@{passed=$false;exitCode=$null;seconds=[math]::Round(((Get-Date)-$started).TotalSeconds,3);stdout=[IO.Path]::GetFileName($stdout);stderr=[IO.Path]::GetFileName($stderr);error='Tool could not be executed; inspect local evidence log.'}
    }
}

$sourceBefore = Get-SourceFingerprint
$lockBefore = (Get-FileHash -LiteralPath $lockfile).Hash
Set-NativeBuildEnvironment
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (!(Test-Path -LiteralPath $cargo)) { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
$rustc = Join-Path $env:USERPROFILE '.cargo\bin\rustc.exe'
if (!(Test-Path -LiteralPath $rustc)) { $rustc = (Get-Command rustc -ErrorAction Stop).Source }
$cargoVersion = (& $cargo --version | Out-String).Trim()
$rustVersion = (& $rustc --version | Out-String).Trim()
$rustDetails = (& $rustc -vV | Out-String)
$hostMatch = [regex]::Match($rustDetails, '(?m)^host: (.+)$')
if (!$hostMatch.Success -or $hostMatch.Groups[1].Value.Trim() -ne 'x86_64-pc-windows-msvc') { throw 'Use the x86_64-pc-windows-msvc Rust toolchain for this Windows release gate.' }
$quotedManifest = '"' + $manifest + '"'
$steps = [ordered]@{}
$steps['format'] = Invoke-GateCommand 'format' $cargo @('fmt','--manifest-path',$quotedManifest,'--all','--','--check') $PSScriptRoot
$steps['strictClippy'] = Invoke-GateCommand 'strict-clippy' $cargo @('clippy','--locked','--manifest-path',$quotedManifest,'--all-targets','--','-D','warnings') $PSScriptRoot
$steps['lockedTests'] = Invoke-GateCommand 'locked-tests' $cargo @('test','--locked','--manifest-path',$quotedManifest) $PSScriptRoot
$unitCount = 0
$testOutput = Join-Path $EvidenceDirectory 'locked-tests.stdout.log'
if (Test-Path -LiteralPath $testOutput) {
    foreach ($match in [regex]::Matches((Get-Content -LiteralPath $testOutput -Raw), 'test result: ok\. (\d+) passed;')) { $unitCount += [int]$match.Groups[1].Value }
}
if ($unitCount -le 0) { $steps['lockedTests'].passed = $false; $steps['lockedTests']['error'] = 'No passing tests were discovered.' }
$audit = [ordered]@{requested=[bool]$AuditExecutable;status='not-requested'}
if ($AuditExecutable) {
    $AuditExecutable = (Resolve-Path -LiteralPath $AuditExecutable).Path
    $AdvisoryDatabase = [IO.Path]::GetFullPath($AdvisoryDatabase)
    $audit['toolSha256'] = (Get-FileHash -LiteralPath $AuditExecutable).Hash
    $audit['toolVersion'] = (& $AuditExecutable --version | Out-String).Trim()
    $steps['advisoryAudit'] = Invoke-GateCommand 'advisory-audit' $AuditExecutable @('audit','--file',('"'+$lockfile+'"'),'--db',('"'+$AdvisoryDatabase+'"'),'--json') $EvidenceDirectory
    try {
        $raw = Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'advisory-audit.stdout.log') -Raw | ConvertFrom-Json
        $audit['vulnerabilityCategoryCount'] = [int]$raw.vulnerabilities.count
        $audit['warningCount'] = 0
        $audit['warnings'] = @()
        foreach ($kind in $raw.warnings.PSObject.Properties) {
            foreach ($warning in @($kind.Value)) {
                $audit['warningCount']++
                $audit['warnings'] += [ordered]@{kind=$kind.Name;package=$warning.package.name;version=$warning.package.version;advisory=$warning.advisory.id}
            }
        }
        $audit['database'] = $raw.database
        $audit['dependencyCount'] = $raw.lockfile.'dependency-count'
        $audit['status'] = if (!$steps['advisoryAudit'].passed) { 'failed' } elseif ($audit.warningCount) { 'passed-with-advisory-warnings' } else { 'passed' }
    } catch {
        $steps['advisoryAudit'].passed = $false
        $audit['status'] = 'invalid-report'
    }
}
$sourceAfter = Get-SourceFingerprint
$lockAfter = (Get-FileHash -LiteralPath $lockfile).Hash
$sourceUnchanged = $sourceBefore.sha256 -eq $sourceAfter.sha256
$lockUnchanged = $lockBefore -eq $lockAfter
$passed = $sourceUnchanged -and $lockUnchanged -and ($steps.Values.passed -notcontains $false)
$report = [ordered]@{
    product='Yas';version=$version;testedAtUtc=(Get-Date).ToUniversalTime().ToString('o');target='x86_64-pc-windows-msvc'
    sourceFingerprint=$sourceBefore;sourceUnchanged=$sourceUnchanged;lockSha256=$lockBefore;lockUnchanged=$lockUnchanged
    toolchain=[ordered]@{cargo=$cargoVersion;rustc=$rustVersion;host=$hostMatch.Groups[1].Value.Trim()};steps=$steps;unitTestsPassed=$unitCount;audit=$audit;passed=$passed
}
$parentDirectory = [IO.Path]::GetDirectoryName($ReportPath)
New-Item -ItemType Directory -Force -Path $parentDirectory | Out-Null
$report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ReportPath -Encoding utf8
$report | ConvertTo-Json -Depth 10
if (!$passed) { throw 'Quality gate failed; inspect the report and local evidence logs.' }
