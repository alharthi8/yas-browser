param([string]$SourceDirectory=(Join-Path $PSScriptRoot '..'))
$ErrorActionPreference='Stop'
$SourceDirectory=(Resolve-Path -LiteralPath $SourceDirectory).Path
$tokens=$null;$parseErrors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory 'quality.ps1'),[ref]$tokens,[ref]$parseErrors)
if($parseErrors.Count){throw 'Quality source did not parse'}
$definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-SourceFingerprint'},$true)
if(!$definition){throw 'Quality fingerprint function is missing'}
. ([scriptblock]::Create($definition.Extent.Text.Replace('$PSScriptRoot','$SourceDirectory')))
Get-SourceFingerprint | ConvertTo-Json -Compress
