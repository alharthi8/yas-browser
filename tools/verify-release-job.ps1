param([Parameter(Mandatory=$true)][string]$Version)
# CI only. Installs into an isolated hosted-runner directory, never a user's PC.
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:GITHUB_REPOSITORY -ne 'alharthi8/yas-browser' -or $env:GITHUB_REF_NAME -ne 'main'){throw 'Run this release job only from the reviewed main branch on its hosted GitHub runner'}
if($Version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$'){throw 'Invalid version'}
$project=(Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$root=Join-Path $env:RUNNER_TEMP ('yas-release-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$info=(& gh release view "v$Version" --repo alharthi8/yas-browser --json isDraft,targetCommitish | ConvertFrom-Json)
if($LASTEXITCODE -ne 0 -or !$info.isDraft){throw 'Prepare a draft release with the tested installer and prevalidated.json first; published releases are never replaced'}
& gh release download "v$Version" --repo alharthi8/yas-browser --pattern prevalidated.json --pattern "Yas-$Version-Setup-x64.exe" --dir $root
if($LASTEXITCODE -ne 0){throw 'Draft artifact download failed'}
Remove-Item Env:GH_TOKEN -ErrorAction SilentlyContinue
$attestation=Get-Content -LiteralPath (Join-Path $root 'prevalidated.json') -Raw | ConvertFrom-Json
$fingerprint=(& (Join-Path $PSScriptRoot 'source-fingerprint.ps1') | ConvertFrom-Json)
if($attestation.schema -ne 1 -or $attestation.version -ne $Version -or $attestation.releaseGroupsPassed -lt 12 -or $attestation.sourceFingerprint.sha256 -ne $fingerprint.sha256 -or $attestation.sourceFingerprint.fileCount -ne $fingerprint.fileCount){throw 'The reviewed source differs from the locally tested release'}
if((Get-FileHash -LiteralPath (Join-Path $project 'Cargo.lock')).Hash -ne $attestation.lockSha256 -or (Get-FileHash -LiteralPath (Join-Path $project 'release.pub')).Hash -ne $attestation.publicKeySha256){throw 'Dependency or signing identity changed'}
$installer=Join-Path $root "Yas-$Version-Setup-x64.exe"
if((Get-FileHash -LiteralPath $installer).Hash -ne $attestation.installerSha256){throw 'Draft installer digest differs from the tested artifact'}
$registration='HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{E290A0CE-DB17-4BD2-9724-90AB04BC61F7}_is1'
if(Test-Path -LiteralPath $registration){throw 'Hosted runner unexpectedly has a Yas installation; no installer was launched'}
$installed=Join-Path $root 'installed'
$arguments=@('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-',('/DIR="'+$installed+'"'),('/LOG="'+(Join-Path $root 'install.log')+'"'))
$process=Start-Process -FilePath $installer -ArgumentList $arguments -WindowStyle Hidden -PassThru
$null=$process.Handle;$process.WaitForExit()
if($process.ExitCode -ne 0){throw 'Isolated runner installer validation failed'}
$executable=Join-Path $installed 'Yas.exe'
if((Get-FileHash -LiteralPath $executable).Hash -ne $attestation.releaseSha256){throw 'Installed executable differs from the locally tested candidate'}
$versionOutput=Join-Path $root 'version.txt'
$versionProcess=Start-Process -FilePath $executable -ArgumentList '--yas-version' -WindowStyle Hidden -PassThru -RedirectStandardOutput $versionOutput
$null=$versionProcess.Handle;$versionProcess.WaitForExit()
$actualVersion=(Get-Content -LiteralPath $versionOutput -Raw).Trim()
if($versionProcess.ExitCode -ne 0 -or $actualVersion -ne $Version){throw 'Installed version check failed'}
$runtime=Join-Path $installed 'ai-runtime'
if((Get-FileHash -LiteralPath (Join-Path $runtime 'AI_RUNTIME_MANIFEST.json')).Hash -ne (Get-FileHash -LiteralPath (Join-Path $project 'ai-runtime\AI_RUNTIME_MANIFEST.json')).Hash){throw 'Installed AI runtime manifest differs from source'}
Copy-Item -LiteralPath $runtime -Destination $project -Recurse -Force
& (Join-Path $project 'package.ps1') -AiRuntimeFiles (Join-Path $project 'ai-runtime') -ValidateAiRuntimeOnly | Out-Null
& cargo install cargo-audit --version 0.22.2 --locked
if($LASTEXITCODE -ne 0){throw 'Pinned advisory-audit tool build failed'}
& (Join-Path $project 'quality.ps1') -TargetDirectory (Join-Path $root 'target') -EvidenceDirectory (Join-Path $root 'quality') -AuditExecutable (Join-Path $env:USERPROFILE '.cargo\bin\cargo-audit.exe') -AdvisoryDatabase (Join-Path $root 'rustsec')
$quality=Get-Content -LiteralPath (Join-Path $project 'QUALITY_GATE_RESULTS.json') -Raw | ConvertFrom-Json
if(!$quality.passed -or $quality.unitTestsPassed -ne $attestation.unitTestsPassed -or $quality.sourceFingerprint.sha256 -ne $attestation.sourceFingerprint.sha256){throw 'Independent runner source gate did not match the reviewed release'}
& node (Join-Path $PSScriptRoot 'test-update-ui.mjs')
if($LASTEXITCODE -ne 0){throw 'Updater UI tests failed'}
& node (Join-Path $PSScriptRoot 'test-update-publisher.mjs')
if($LASTEXITCODE -ne 0){throw 'Publisher tests failed'}
$attestation.build | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $project 'RELEASE_BUILD.json') -Encoding utf8
$attestation.packageAudit | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $project 'PACKAGE_AUDIT_RESULTS.json') -Encoding utf8
$receipt=Join-Path $root 'ci-verified.json'
[ordered]@{version=$Version;sourceCommit=$env:GITHUB_SHA;installerSha256=$attestation.installerSha256;releaseSha256=$attestation.releaseSha256;quality=$quality;build=$attestation.build;packageAudit=$attestation.packageAudit} | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $receipt -Encoding utf8
"version=$Version" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
"installer_sha=$($attestation.installerSha256)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
"release_sha=$($attestation.releaseSha256)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
"receipt_sha=$((Get-FileHash -LiteralPath $receipt).Hash)" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Append -Encoding utf8
"YAS_CI_RECEIPT=$receipt" | Out-File -LiteralPath $env:GITHUB_ENV -Append -Encoding utf8
"YAS_VERIFIED_INSTALLER=$installer" | Out-File -LiteralPath $env:GITHUB_ENV -Append -Encoding utf8
"YAS_VERIFIED_EXE=$executable" | Out-File -LiteralPath $env:GITHUB_ENV -Append -Encoding utf8
"YAS_RELEASE_STAGE=$(Join-Path $root 'signed')" | Out-File -LiteralPath $env:GITHUB_ENV -Append -Encoding utf8
