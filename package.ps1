param(
    [string]$Executable = (Join-Path $PSScriptRoot 'Yas.exe'),
    [string]$Version = '0.4.5',
    [string]$Compiler,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..'),
    [string]$RuntimeFiles = (Join-Path $PSScriptRoot 'runtime'),
    [string]$StagingDirectory = (Join-Path $PSScriptRoot '..\..\work\package-staging')
)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Version must have three numeric parts, such as 0.4.5.' }
$resolvedExe = (Resolve-Path -LiteralPath $Executable).Path
$bootstrapperRoot = Join-Path $PSScriptRoot 'installer\prerequisites'
$bootstrapperPath = Join-Path $bootstrapperRoot 'MicrosoftEdgeWebview2Setup.exe'
if (!(Test-Path -LiteralPath $bootstrapperPath)) {throw 'Missing bundled Microsoft WebView2 Evergreen Bootstrapper.'}
$bootstrapperSignature = Get-AuthenticodeSignature -FilePath $bootstrapperPath
if ($bootstrapperSignature.Status -ne 'Valid' -or $bootstrapperSignature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation(?:,|$)') {
    throw 'The WebView2 Bootstrapper must have a valid Microsoft Corporation Authenticode signature.'
}
$bootstrapperHash = (Get-FileHash -LiteralPath $bootstrapperPath -Algorithm SHA256).Hash
$bootstrapperProvenance = Get-Content -LiteralPath (Join-Path $bootstrapperRoot 'WEBVIEW2_BOOTSTRAPPER.json') -Raw | ConvertFrom-Json
if ($bootstrapperProvenance.sha256 -ne $bootstrapperHash) {throw 'WebView2 Bootstrapper hash does not match its source provenance record.'}
$exeVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($resolvedExe)
$numericVersion = '{0}.{1}.{2}.{3}' -f $exeVersion.FileMajorPart,$exeVersion.FileMinorPart,$exeVersion.FileBuildPart,$exeVersion.FilePrivatePart
if ($numericVersion -ne "$Version.0" -or $exeVersion.ProductVersion -notmatch ('^' + [regex]::Escape($Version) + '(?:[^\d.]|$)')) {
    throw "Executable version does not match package $Version. FileVersion=$($exeVersion.FileVersion), ProductVersion=$($exeVersion.ProductVersion). Build and copy the intended release first."
}
if (!$Compiler) {
    $compilerCandidates = @(
        (Join-Path $PSScriptRoot '..\..\work\installer-tools\Inno Setup 7\ISCC.exe'),
        (Join-Path $PSScriptRoot '..\..\work\installer-tools\Inno Setup 6\ISCC.exe'),
        'C:\Program Files\Inno Setup 7\ISCC.exe',
        'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
    )
    $Compiler = $compilerCandidates | Where-Object {Test-Path -LiteralPath $_} | Select-Object -First 1
    if (!$Compiler) {
        $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        if ($command) { $Compiler = $command.Source }
    }
}
if (!$Compiler -or !(Test-Path -LiteralPath $Compiler)) {
    throw 'Inno Setup compiler not found. Install Inno Setup from https://jrsoftware.org/isdl.php or pass -Compiler with the path to ISCC.exe.'
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$resolvedOutput = (Resolve-Path -LiteralPath $OutputDirectory).Path
& (Join-Path $PSScriptRoot 'installer\make-wizard-assets.ps1') -BrandRoot (Join-Path $PSScriptRoot 'branding')
$compilerArgs = @('/Q', "/DAppExe=$resolvedExe", "/DAppRoot=$PSScriptRoot", "/DAppVersion=$Version", "/DOutputDir=$resolvedOutput", "/DBootstrapperSha256=$bootstrapperHash")
if ($RuntimeFiles) {
    $resolvedRuntime = (Resolve-Path -LiteralPath $RuntimeFiles).Path
    foreach ($file in @('vcruntime140.dll','vcruntime140_1.dll')) {
        $runtimePath = Join-Path $resolvedRuntime $file
        if (!(Test-Path -LiteralPath $runtimePath)) {throw "Missing runtime file: $file"}
        $runtimeSignature = Get-AuthenticodeSignature -FilePath $runtimePath
        if ($runtimeSignature.Status -ne 'Valid' -or $runtimeSignature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation(?:,|$)') {
            throw "Runtime file must have a valid Microsoft Authenticode signature: $file"
        }
    }
    $compilerArgs += "/DRuntimeFiles=$resolvedRuntime"
}
$compilerArgs += (Join-Path $PSScriptRoot 'installer\Yas.iss')
& $Compiler @compilerArgs
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed with exit code $LASTEXITCODE" }
$installer = Join-Path $resolvedOutput "Yas-$Version-Setup-x64.exe"
if (!(Test-Path -LiteralPath $installer)) {throw 'Expected installer was not produced.'}
Get-FileHash -LiteralPath $installer -Algorithm SHA256 | ForEach-Object {"$($_.Hash)  $([IO.Path]::GetFileName($installer))"} |
    Set-Content -LiteralPath ($installer + '.sha256') -Encoding utf8
New-Item -ItemType Directory -Force -Path $StagingDirectory | Out-Null
$portableStage = Join-Path (Resolve-Path -LiteralPath $StagingDirectory).Path ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $portableStage | Out-Null
Copy-Item -LiteralPath $resolvedExe -Destination (Join-Path $portableStage 'Yas.exe')
foreach ($file in @('README.md','RELEASE_NOTES.md','THIRD_PARTY.md','RENAME_NOTES.md')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $portableStage
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'licenses') -Destination $portableStage -Recurse
Copy-Item -LiteralPath $bootstrapperRoot -Destination (Join-Path $portableStage 'prerequisites') -Recurse
if ($RuntimeFiles) {
    foreach ($file in @('vcruntime140.dll','vcruntime140_1.dll')) {
        Copy-Item -LiteralPath (Join-Path $resolvedRuntime $file) -Destination $portableStage
    }
    $runtimeNotice = Join-Path $resolvedRuntime 'README.txt'
    if (Test-Path -LiteralPath $runtimeNotice) {Copy-Item -LiteralPath $runtimeNotice -Destination (Join-Path $portableStage 'MSVC-RUNTIME.txt')}
}
$portable = Join-Path $resolvedOutput "Yas-$Version-Windows-x64.zip"
# ZIP timestamps cannot represent historical zero dates from copied assets/runtime files.
# Clamp the staging copy only; packaged file bytes and Microsoft signatures are unchanged.
foreach ($item in Get-ChildItem -LiteralPath $portableStage -Recurse -Force) {
    if ($item.LastWriteTime.Year -lt 1980) { $item.LastWriteTime = [datetime]::new(1980,1,2) }
    elseif ($item.LastWriteTime.Year -gt 2107) { $item.LastWriteTime = [datetime]::new(2107,12,30) }
}
Compress-Archive -Path (Join-Path $portableStage '*') -DestinationPath $portable -CompressionLevel Optimal -Force -WarningAction SilentlyContinue
Get-FileHash -LiteralPath $portable -Algorithm SHA256 | ForEach-Object {"$($_.Hash)  $([IO.Path]::GetFileName($portable))"} |
    Set-Content -LiteralPath ($portable + '.sha256') -Encoding utf8
Get-Item -LiteralPath $installer,$portable | Select-Object FullName,Length
