param([switch]$Run, [switch]$Test, [switch]$Release, [switch]$Debug, [switch]$GenerateBundledCache, [ValidateSet('None','Timing','Allocations')][string]$CacheBenchmark='None', [string]$TargetDirectory = $env:CARGO_TARGET_DIR)
$ErrorActionPreference = 'Stop'
if ($Release -and $Debug) { throw 'Choose either -Release or -Debug.' }
if (!$TargetDirectory) { $TargetDirectory = Join-Path $PSScriptRoot 'target' }
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
$env:CARGO_TARGET_DIR = $TargetDirectory
# Compile Windows resources with the official SDK rather than a generated app build helper.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsRoot = if (Test-Path $vswhere) { & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
if (!$vsRoot) { throw 'Install Visual Studio C++ Build Tools with the x64 MSVC compiler.' }
$msvc = Get-ChildItem (Join-Path $vsRoot 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
$kits = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' -ErrorAction SilentlyContinue
$sdkRoot = if ($kits.KitsRoot10) { $kits.KitsRoot10 } else { Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10' }
$sdk = Get-ChildItem (Join-Path $sdkRoot 'Lib') -Directory | Where-Object {Test-Path (Join-Path $_.FullName 'um\x64\kernel32.lib')} | Sort-Object Name -Descending | Select-Object -First 1
if (!$sdk) { throw 'Install a Windows 10/11 SDK with x64 libraries and resource compiler.' }
$rc = Join-Path $sdkRoot "bin\$($sdk.Name)\x64\rc.exe"
if (!(Test-Path $rc)) { throw "Windows SDK resource compiler not found: $rc" }
$env:LIB = "$($msvc.FullName)\lib\x64;$($sdk.FullName)\um\x64;$($sdk.FullName)\ucrt\x64;$env:LIB"
$env:PATH = "$($msvc.FullName)\bin\HostX64\x64;$env:USERPROFILE\.cargo\bin;$env:PATH"
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (!(Test-Path $cargo)) { $cargo = 'cargo' }
$previousCacheStamp = $env:YAS_FILTER_CACHE_BUILD_ATTESTATION

function Get-FilterCacheBuildStamp([string]$Profile) {
    $compilerLines = @(& rustc -vV)
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify the Rust compiler.' }
    $hostLine = $compilerLines | Where-Object { $_ -like 'host: *' } | Select-Object -First 1
    if (!$hostLine) { throw 'Rust host target is missing.' }
    $target = $hostLine.Substring(6)
    if ($env:CARGO_BUILD_TARGET -and $env:CARGO_BUILD_TARGET -ne $target) { throw 'Custom target is not attested by this wrapper.' }
    $metadataText = & $cargo metadata --offline --locked --format-version 1 --filter-platform $target --manifest-path "$PSScriptRoot\Cargo.toml"
    if ($LASTEXITCODE -ne 0) { throw 'Could not resolve exact engine features.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $engine = @($metadata.packages | Where-Object { $_.name -eq 'adblock' })
    if ($engine.Count -ne 1) { throw 'Expected one resolved adblock package.' }
    $nodes = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $seen = [Collections.Generic.HashSet[string]]::new()
    $pending = [Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($engine[0].id)
    while ($pending.Count) {
        $id = $pending.Dequeue()
        if (!$seen.Add($id)) { continue }
        foreach ($dependency in $nodes[$id].dependencies) { $pending.Enqueue($dependency) }
    }
    # Dependency/features are sets. Culture-sensitive Sort-Object differs
    # between Windows PowerShell 5 and PowerShell 7 for @, - and _ package IDs.
    # An explicit ordinal order gives both hosts the same attested bytes.
    $dependencyIds = [string[]]@($seen)
    [Array]::Sort($dependencyIds, [StringComparer]::Ordinal)
    $dependencyFeatures = @(foreach ($id in $dependencyIds) {
        $features = [string[]]@($nodes[$id].features)
        [Array]::Sort($features, [StringComparer]::Ordinal)
        [ordered]@{ id = $id; features = @($features) }
    })
    $engineFeatures = [string[]]@($nodes[$engine[0].id].features)
    [Array]::Sort($engineFeatures, [StringComparer]::Ordinal)
    [ordered]@{
        schema = 1; profile = $Profile; compiler = ($compilerLines -join "`n"); target = $target
        engine_version = $engine[0].version; engine_features = @($engineFeatures)
        dependency_features = $dependencyFeatures
        rustflags = [string]$env:RUSTFLAGS; encoded_rustflags = [string]$env:CARGO_ENCODED_RUSTFLAGS
        compiler_policy = 'Cargo.toml profiles and default root features; resource link argument or example-only allocation instrumentation does not alter engine dependency compilation'
    } | ConvertTo-Json -Depth 12 -Compress
}

Push-Location $PSScriptRoot
try {
    $cacheProfile = if ($Test) {'test'} elseif ($Debug) {'debug'} else {'release'}
    try { $env:YAS_FILTER_CACHE_BUILD_ATTESTATION = Get-FilterCacheBuildStamp $cacheProfile }
    catch {
        $env:YAS_FILTER_CACHE_BUILD_ATTESTATION = $null
        if ($GenerateBundledCache) { throw }
        Write-Warning "Bundled filter cache disabled for this build: $($_.Exception.Message)"
    }
    if ($GenerateBundledCache) {
        if ($Debug -or $Test) { throw 'Cache generation uses the normal Release engine only.' }
        & $cargo build --release --offline --locked --manifest-path "$PSScriptRoot\Cargo.toml" --example generate_bundled_cache
        if ($LASTEXITCODE -ne 0) { throw 'Bundled cache generator build failed.' }
        $generator = Join-Path $TargetDirectory 'release\examples\generate_bundled_cache.exe'
        & $generator
        if ($LASTEXITCODE -ne 0) { throw 'Bundled cache generation failed.' }
        return
    }
    if ($CacheBenchmark -ne 'None') {
        if ($Debug -or $Test) { throw 'The explicit cache CPU probe uses Release only.' }
        $benchmarkArgs = @('rustc','--release','--offline','--locked','--manifest-path',"$PSScriptRoot\Cargo.toml",'--example','benchmark_bundled_cache')
        if ($CacheBenchmark -eq 'Allocations') { $benchmarkArgs += @('--','--cfg','filter_cache_alloc','--check-cfg','cfg(filter_cache_alloc)') }
        & $cargo @benchmarkArgs
        if ($LASTEXITCODE -ne 0) { throw 'Complete cache probe build failed.' }
        Write-Output (Join-Path $TargetDirectory 'release\examples\benchmark_bundled_cache.exe')
        return
    }
    if ($Test) {
        & $cargo test --locked --manifest-path "$PSScriptRoot\Cargo.toml"
        if ($LASTEXITCODE -ne 0) { throw "Cargo tests failed (exit $LASTEXITCODE)." }
        return
    }
    $resourceDir = Join-Path $TargetDirectory 'yas-resources'
    New-Item -ItemType Directory -Force -Path $resourceDir | Out-Null
    $resourceKey=((Get-FileHash 'branding\yas.rc' -Algorithm SHA256).Hash.Substring(0,12))+((Get-FileHash 'branding\yas.ico' -Algorithm SHA256).Hash.Substring(0,12))
    $resource = Join-Path $resourceDir "yas-$resourceKey.res"
    & $rc /nologo /fo $resource 'branding\yas.rc'
    if ($LASTEXITCODE -ne 0) { throw "Resource compilation failed (exit $LASTEXITCODE)." }
    $cargoArgs = @('rustc', '--locked', '--manifest-path', "$PSScriptRoot\Cargo.toml", '--bin', 'yas')
    if (!$Debug) { $cargoArgs += '--release' }
    & $cargo @cargoArgs -- -C "link-arg=$resource"
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed (exit $LASTEXITCODE)." }
    $profile = if ($Debug) {'debug'} else {'release'}
    $executable = Join-Path $TargetDirectory "$profile\yas.exe"
    Write-Host "Built full $profile executable: $executable"
    if ($Run) { Start-Process -FilePath $executable -WorkingDirectory $PSScriptRoot -WindowStyle Hidden }
} finally {
    $env:YAS_FILTER_CACHE_BUILD_ATTESTATION = $previousCacheStamp
    Pop-Location
}
