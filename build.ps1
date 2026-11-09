param([switch]$Run, [switch]$Test, [switch]$Release, [switch]$Debug, [string]$TargetDirectory = $env:CARGO_TARGET_DIR)
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
Push-Location $PSScriptRoot
try {
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
} finally { Pop-Location }
