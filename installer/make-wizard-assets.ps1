param(
    [string]$BrandRoot = (Join-Path $PSScriptRoot '..\branding'),
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'assets')
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$logo = [System.Drawing.Image]::FromFile((Join-Path (Resolve-Path $BrandRoot).Path 'yas-logo.png'))
try {
    $bitmap = New-Object System.Drawing.Bitmap 492,942
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.Clear([System.Drawing.ColorTranslator]::FromHtml('#10254B'))
        $blue = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#2563EB'))
        $soft = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#3B82F6'))
        $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
        $muted = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#BBD2FF'))
        $font = New-Object System.Drawing.Font 'Dubai',34,([System.Drawing.FontStyle]::Bold),([System.Drawing.GraphicsUnit]::Pixel)
        $body = New-Object System.Drawing.Font 'Dubai',25,([System.Drawing.FontStyle]::Regular),([System.Drawing.GraphicsUnit]::Pixel)
        try {
            $graphics.FillEllipse($blue,160,660,520,520)
            $graphics.FillEllipse($soft,-260,780,480,480)
            $graphics.DrawImage($logo,54,72,150,150)
            $graphics.DrawString('Yas',$font,$white,54,260)
            $graphics.DrawString("A clearer view`nof the web.",$body,$muted,54,330)
            $graphics.DrawString('Windows browser',$body,$muted,54,580)
            $bitmap.Save((Join-Path $OutputDirectory 'wizard.bmp'),[System.Drawing.Imaging.ImageFormat]::Bmp)
        } finally {
            $font.Dispose();$body.Dispose();$blue.Dispose();$soft.Dispose();$white.Dispose();$muted.Dispose()
        }
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    $small = New-Object System.Drawing.Bitmap 165,165
    $g = [System.Drawing.Graphics]::FromImage($small)
    try {
        $g.Clear([System.Drawing.Color]::White)
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.DrawImage($logo,8,8,149,149)
        $small.Save((Join-Path $OutputDirectory 'wizard-small.bmp'),[System.Drawing.Imaging.ImageFormat]::Bmp)
    } finally { $g.Dispose();$small.Dispose() }
} finally { $logo.Dispose() }
