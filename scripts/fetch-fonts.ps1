# Downloads the fonts Caelestia's look depends on into assets/fonts.
# Both are from Google's official repositories (OFL / Apache-2.0 licensed).
$ErrorActionPreference = 'Stop'
$dest = Join-Path $PSScriptRoot '..\assets\fonts'
New-Item -ItemType Directory -Force $dest | Out-Null

$fonts = @{
    'MaterialSymbolsRounded.ttf' = 'https://github.com/google/material-design-icons/raw/master/variablefont/MaterialSymbolsRounded%5BFILL,GRAD,opsz,wght%5D.ttf'
    'Rubik.ttf'                  = 'https://github.com/google/fonts/raw/main/ofl/rubik/Rubik%5Bwght%5D.ttf'
}

foreach ($name in $fonts.Keys) {
    $out = Join-Path $dest $name
    if (Test-Path $out) { Write-Host "ok   $name (already present)"; continue }
    Write-Host "get  $name"
    Invoke-WebRequest -Uri $fonts[$name] -OutFile $out -UseBasicParsing
    Write-Host ("     {0:N1} MB" -f ((Get-Item $out).Length / 1MB))
}
