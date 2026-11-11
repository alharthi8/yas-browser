param(
    [string]$Executable = (Join-Path $PSScriptRoot 'Yas.exe'),
    [string]$Version = '1.0.0',
    [string]$Compiler,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..'),
    [string]$RuntimeFiles = (Join-Path $PSScriptRoot 'runtime'),
    [string]$StagingDirectory = (Join-Path $PSScriptRoot '..\..\work\package-staging'),
    [string]$AiRuntimeFiles = (Join-Path $PSScriptRoot 'ai-runtime'),
    [switch]$ValidateAiRuntimeOnly
)
$ErrorActionPreference = 'Stop'
# The same manifest is compiled into Yas. A runtime update requires an explicit
# source change and review here as well as in the application.
$aiManifestHash = 'FE26D105E6D8A9B82DA1AACC652F3EC9509591560FF5EC3807150DD8375FDDC7'
function Test-AiRuntimePayload([string]$Directory) {
    $root = (Resolve-Path -LiteralPath $Directory).Path.TrimEnd('\')
    $rootItem = Get-Item -LiteralPath $root -Force
    if (!$rootItem.PSIsContainer -or ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'AI runtime root must be a real directory.' }
    # Reject links before opening any payload, including the manifest itself.
    $linkCheck = [Collections.Generic.Stack[string]]::new()
    $linkCheck.Push($root)
    while ($linkCheck.Count -gt 0) {
        foreach ($item in Get-ChildItem -LiteralPath $linkCheck.Pop() -Force) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'AI runtime payload must not contain reparse points.' }
            if ($item.PSIsContainer) { $linkCheck.Push($item.FullName) }
        }
    }
    $manifestPath = Join-Path $root 'AI_RUNTIME_MANIFEST.json'
    if ((Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash -ne $aiManifestHash) { throw 'AI runtime manifest does not match the reviewed application manifest.' }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.vendor -ne 'Ollama' -or $manifest.version -ne '0.40.16' -or $manifest.cpuOnly -ne $true -or @($manifest.files).Count -ne 33) { throw 'Unexpected AI runtime manifest contract.' }
    $expectedFiles = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::OrdinalIgnoreCase)
    $expectedFiles.Add('AI_RUNTIME_MANIFEST.json', $null)
    $expectedDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    [long]$totalBytes = 0
    $binaryCount = 0
    foreach ($entry in $manifest.files) {
        $relative = [string]$entry.path
        if ($relative -notmatch '^[A-Za-z0-9_.+-]+(?:/[A-Za-z0-9_.+-]+)*$' -or @($relative.Split('/') | Where-Object { $_ -in @('.','..','models','blobs','manifests','profile','profiles','cache') }).Count -gt 0 -or $relative -match '(?i)\.(gguf|safetensors|bin|csv|dpapi|redb|sqlite3?)$') { throw "Unsafe AI runtime entry: $relative" }
        if ($expectedFiles.ContainsKey($relative) -or $entry.sha256 -notmatch '^[A-F0-9]{64}$' -or [long]$entry.bytes -le 0) { throw "Invalid or duplicate AI runtime entry: $relative" }
        $expectedFiles.Add($relative, $entry)
        $parent = [IO.Path]::GetDirectoryName($relative.Replace('/','\'))
        while ($parent) { [void]$expectedDirectories.Add($parent.Replace('\','/')); $parent = [IO.Path]::GetDirectoryName($parent) }
        $path = Join-Path $root $relative.Replace('/','\')
        if (!(Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path -Force).Length -ne [long]$entry.bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.sha256) { throw "AI runtime size/hash mismatch: $relative" }
        $totalBytes += [long]$entry.bytes
        if ([IO.Path]::GetExtension($relative) -in @('.dll','.exe')) {
            $stream = [IO.File]::OpenRead($path)
            $reader = [IO.BinaryReader]::new($stream)
            try {
                if ($reader.ReadUInt16() -ne 0x5A4D) { throw "AI runtime is not a PE image: $relative" }
                $stream.Position = 0x3C
                $peOffset = $reader.ReadUInt32()
                if ($peOffset -gt ($stream.Length - 6)) { throw "AI runtime has an invalid PE header: $relative" }
                $stream.Position = $peOffset
                if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw "AI runtime must be Windows x64: $relative" }
            } finally { $reader.Dispose(); $stream.Dispose() }
            $binaryCount++
        }
    }
    $requiredNotices = @('OLLAMA_LICENSE.txt','QWEN3_LICENSE.txt','lib/ollama/LLAMA_CPP_LICENSE','lib/ollama/LLAMA_CPP_VENDORS_LICENSE','lib/ollama/CPP_HTTPLIB_LICENSE','lib/ollama/GO_LICENSE')
    if ($totalBytes -ne 84640544 -or $binaryCount -ne 27 -or @($manifest.licenses).Count -ne 6) { throw 'AI runtime inventory size does not match the reviewed CPU-only distribution.' }
    foreach ($notice in $requiredNotices) {
        if (!$expectedFiles.ContainsKey($notice) -or $notice -notin $manifest.licenses) { throw "Missing AI runtime license notice: $notice" }
    }
    # Walk without following reparse points, rejecting extra directories as well
    # as files. A user model store or export can never enter this public payload.
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($root)
    $actualFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    while ($pending.Count -gt 0) {
        foreach ($item in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
            $relative = $item.FullName.Substring($root.Length + 1).Replace('\','/')
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "AI runtime contains a reparse point: $relative" }
            if ($item.PSIsContainer) {
                if (!$expectedDirectories.Contains($relative)) { throw "Unexpected AI runtime directory: $relative" }
                $pending.Push($item.FullName)
            } elseif (!$expectedFiles.ContainsKey($relative) -or !$actualFiles.Add($relative)) { throw "Unexpected AI runtime file: $relative" }
        }
    }
    if ($actualFiles.Count -ne $expectedFiles.Count) { throw 'AI runtime inventory is incomplete.' }
    return [pscustomobject]@{ Root=$root; ManifestSha256=$aiManifestHash; FileCount=$actualFiles.Count; PayloadBytes=$totalBytes; BinaryCount=$binaryCount; Files=@($expectedFiles.Keys | Sort-Object) }
}
function Copy-AiRuntimePayload($Validated, [string]$Destination) {
    New-Item -ItemType Directory -Path $Destination | Out-Null
    foreach ($relative in $Validated.Files) {
        $target = Join-Path $Destination $relative.Replace('/','\')
        New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($target)) | Out-Null
        Copy-Item -LiteralPath (Join-Path $Validated.Root $relative.Replace('/','\')) -Destination $target
    }
    return (Test-AiRuntimePayload $Destination)
}
$validatedAi = Test-AiRuntimePayload $AiRuntimeFiles
if ($ValidateAiRuntimeOnly) { return $validatedAi }
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Version must have three numeric parts, such as 1.0.0.' }
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
New-Item -ItemType Directory -Force -Path $StagingDirectory | Out-Null
$resolvedStaging = (Resolve-Path -LiteralPath $StagingDirectory).Path
$aiStage = Copy-AiRuntimePayload $validatedAi (Join-Path $resolvedStaging ('ai-runtime-' + [guid]::NewGuid().ToString('N')))
& (Join-Path $PSScriptRoot 'installer\make-wizard-assets.ps1') -BrandRoot (Join-Path $PSScriptRoot 'branding')
$compilerArgs = @('/Q', "/DAppExe=$resolvedExe", "/DAppRoot=$PSScriptRoot", "/DAppVersion=$Version", "/DOutputDir=$resolvedOutput", "/DBootstrapperSha256=$bootstrapperHash", "/DAiRuntimeFiles=$($aiStage.Root)", "/DAiRuntimeManifestSha256=$aiManifestHash")
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
$aiStage = Test-AiRuntimePayload $aiStage.Root
$installer = Join-Path $resolvedOutput "Yas-$Version-Setup-x64.exe"
if (!(Test-Path -LiteralPath $installer)) {throw 'Expected installer was not produced.'}
Get-FileHash -LiteralPath $installer -Algorithm SHA256 | ForEach-Object {"$($_.Hash)  $([IO.Path]::GetFileName($installer))"} |
    Set-Content -LiteralPath ($installer + '.sha256') -Encoding utf8
$portableStage = Join-Path $resolvedStaging ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $portableStage | Out-Null
Copy-Item -LiteralPath $resolvedExe -Destination (Join-Path $portableStage 'Yas.exe')
foreach ($file in @('README.md','RELEASE_NOTES.md','THIRD_PARTY.md','RENAME_NOTES.md')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $portableStage
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'licenses') -Destination $portableStage -Recurse
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'assistant') -Destination $portableStage -Recurse
Copy-Item -LiteralPath $bootstrapperRoot -Destination (Join-Path $portableStage 'prerequisites') -Recurse
$portableAi = Copy-AiRuntimePayload $aiStage (Join-Path $portableStage 'ai-runtime')
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
