<#
.SYNOPSIS
  Erzeugt die synthetischen OCR-Fixtures unter tests/fixtures/content/ (einmalig).

.DESCRIPTION
  Zeichnet erfundenen Rechnungstext (keine echten Daten) mit System.Drawing in ein JPEG.
  Das JPEG dient direkt als Bild-Scan und wird in den Tests in ein bildbasiertes PDF
  verpackt. Nur auf Windows ausführbar.
#>
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root 'tests/fixtures/content/rechnung-scan.jpg'

$lines = @(
    'Musterfirma GmbH',
    'Rechnung Nr. 4711',
    'Rechnungsdatum: 30.09.2026',
    'Zahlbar bis 30.10.2026',
    'Gesamtbetrag: 119,00 EUR',
    'Vielen Dank fuer Ihren Auftrag. Pruefung und Groesse'
)
$bmp = New-Object System.Drawing.Bitmap 1600, 700
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::White)
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
$font = New-Object System.Drawing.Font 'Arial', 34
$y = 30
foreach ($line in $lines) {
    $g.DrawString($line, $font, [System.Drawing.Brushes]::Black, 40, $y)
    $y += 100
}
$g.Dispose()
$codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.MimeType -eq 'image/jpeg' }
$params = New-Object System.Drawing.Imaging.EncoderParameters 1
$params.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter ([System.Drawing.Imaging.Encoder]::Quality), 80L
$bmp.Save($out, $codec, $params)
$bmp.Dispose()
Write-Host "geschrieben: $out ($((Get-Item $out).Length) Bytes)"
