<#
.SYNOPSIS
  Erzeugt assets/geo/cities15000.tsv aus der GeoNames-Datei cities15000.txt.

.DESCRIPTION
  Einmalig auszuführen (die erzeugte Datei ist eingecheckt). Lädt cities15000.zip von
  download.geonames.org, behält je Ort name, country, lat, lon, population (tabgetrennt, sortiert nach
  Breite) und schreibt UTF-8 ohne BOM. Daten: GeoNames, CC BY 4.0 (siehe assets/geo/LICENSE).

.EXAMPLE
  pwsh tools/geonames.ps1
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('geonames-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp 'cities15000.zip'
    Invoke-WebRequest -Uri 'https://download.geonames.org/export/dump/cities15000.zip' -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $tmp
    $inv = [Globalization.CultureInfo]::InvariantCulture
    $rows = Get-Content -Path (Join-Path $tmp 'cities15000.txt') -Encoding UTF8 | ForEach-Object {
        $c = $_ -split "`t"
        # 1 name, 4 latitude, 5 longitude, 8 country code, 14 population
        [pscustomobject]@{
            Name = $c[1]; Country = $c[8]
            Lat = [double]::Parse($c[4], $inv); Lon = [double]::Parse($c[5], $inv)
            Pop = [long]$c[14]
        }
    } | Sort-Object Lat, Lon
    $lines = $rows | ForEach-Object {
        '{0}{1}{2}{1}{3}{1}{4}{1}{5}' -f $_.Name, "`t", $_.Country, $_.Lat.ToString('0.#####', $inv), $_.Lon.ToString('0.#####', $inv), $_.Pop
    }
    $out = Join-Path $root 'assets/geo/cities15000.tsv'
    [IO.File]::WriteAllText($out, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
    Write-Host "$($rows.Count) Orte -> $out"
}
finally {
    Remove-Item -Recurse -Force $tmp
}
