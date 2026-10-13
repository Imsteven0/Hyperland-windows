# Installs Caelestia for Windows per-user (no admin rights needed).
#
# Latest release (no Rust needed), also as a one-liner:
#   irm https://raw.githubusercontent.com/Imsteven0/Hyperland-windows/main/scripts/install.ps1 | iex
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -FromRelease [-AddToPath] [-NoLaunch]
#
# From this checkout (builds in release mode, needs Rust + MSVC build tools):
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 [-AddToPath] [-NoBuild] [-NoLaunch]
#
# Installs to %LOCALAPPDATA%\Programs\Caelestia, adds a Start Menu shortcut and
# launches the shell. The shell starts with Windows by default: it registers
# itself on launch (`windows.autostart` in shell.json, or Settings > Startup).
# Your dots stay in ~\.config\caelestia.
param(
    [switch]$AddToPath,   # allow `caelestia toggle launcher` etc. from any terminal
    [switch]$NoBuild,
    [switch]$NoLaunch,    # don't start the shell after installing
    [switch]$FromRelease  # download the latest installer instead of building
)
$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Caelestia'
$repo = if ($PSScriptRoot) { Resolve-Path (Join-Path $PSScriptRoot '..') } else { $null }

# Run through `irm | iex` (no checkout) or asked to: install the latest release.
if ($FromRelease -or -not $repo -or -not (Test-Path (Join-Path $repo 'Cargo.toml'))) {
    $api = 'https://api.github.com/repos/Imsteven0/Hyperland-windows/releases/latest'
    $release = Invoke-RestMethod $api -Headers @{ 'User-Agent' = 'caelestia-install' }
    $asset = $release.assets | Where-Object name -like '*-setup.exe' | Select-Object -First 1
    if (-not $asset) { throw "Release $($release.tag_name) has no installer" }
    $setup = Join-Path $env:TEMP $asset.name
    Write-Host "Downloading $($asset.name)..."
    Invoke-WebRequest $asset.browser_download_url -OutFile $setup -UseBasicParsing
    $setupArgs = @('/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
    if ($AddToPath) { $setupArgs += '/TASKS=addtopath' }
    Start-Process $setup -ArgumentList $setupArgs -Wait
    Remove-Item $setup -Force
    if (-not $NoLaunch) { Start-Process (Join-Path $dest 'caelestia.exe') -WorkingDirectory $dest }
    Write-Host "Caelestia $($release.tag_name) installed."
    return
}

if (-not $NoBuild) {
    Write-Host 'Building release...'
    Push-Location $repo
    cargo build --release
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw 'cargo build failed' }
    Pop-Location
}

$running = Get-Process caelestia -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dest*" }
if ($running) {
    Write-Host 'Stopping the installed shell...'
    & (Join-Path $dest 'caelestia.exe') quit
    Start-Sleep -Seconds 2
}

New-Item -ItemType Directory -Force (Join-Path $dest 'assets\fonts') | Out-Null
Copy-Item (Join-Path $repo 'target\release\caelestia.exe') $dest -Force
Copy-Item (Join-Path $repo 'assets\fonts\*') (Join-Path $dest 'assets\fonts') -Force
Write-Host "Installed to $dest"

$shell = New-Object -ComObject WScript.Shell
function New-Shortcut($path) {
    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = Join-Path $dest 'caelestia.exe'
    $lnk.WorkingDirectory = $dest
    $lnk.Description = 'Caelestia shell for Windows'
    $lnk.Save()
}
New-Shortcut (Join-Path ([Environment]::GetFolderPath('Programs')) 'Caelestia.lnk')
Write-Host 'Start Menu shortcut created'

# Older installs autostarted from the Startup folder; the shell owns that now.
$oldStartup = Join-Path ([Environment]::GetFolderPath('Startup')) 'Caelestia.lnk'
if (Test-Path $oldStartup) { Remove-Item $oldStartup -Force }

if ($AddToPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $dest) {
        [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ";$dest"), 'User')
        Write-Host 'Added to user PATH (open a new terminal)'
    }
}
if ($NoLaunch) {
    Write-Host 'Done. Launch "Caelestia" from the Start Menu.'
} else {
    Start-Process (Join-Path $dest 'caelestia.exe') -WorkingDirectory $dest
    Write-Host 'Done. Caelestia is running and will start with Windows.'
}
