# Local development helper: stops Microsoft Defender from quarantining freshly
# built Snowdrift binaries while you are working on them.
#
# This is NOT for end users and NOT part of the installer. It only excludes the
# build output, the installed copy under %LOCALAPPDATA%\Programs\Snowdrift and
# the updater's temporary file. Run it from an elevated PowerShell:
#
#   powershell -ExecutionPolicy Bypass -File scripts\allow-defender.ps1
#
# Undo with:
#
#   powershell -ExecutionPolicy Bypass -File scripts\allow-defender.ps1 -Remove

[CmdletBinding()]
param(
    [switch] $Remove
)

$ErrorActionPreference = 'Stop'

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error 'Run this script from an elevated PowerShell (Run as Administrator).'
    exit 1
}

$root = Split-Path -Parent $PSScriptRoot
$paths = @(
    (Join-Path $root 'target\release'),
    (Join-Path $root 'dist'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Snowdrift'),
    (Join-Path $env:TEMP 'Snowdrift-Setup.exe')
) | Where-Object { $_ }

if (-not $Remove) {
    Write-Warning 'These exclusions are for local development only. Any file in these paths can run without being scanned. Remove them with -Remove when you are done.'
}

$defender = Get-MpComputerStatus -ErrorAction SilentlyContinue
if (-not $defender -or -not $defender.AntivirusEnabled) {
    Write-Warning 'Microsoft Defender is not the active antivirus. Add these paths to your antivirus manually:'
    $paths | ForEach-Object { Write-Host "  $_" }
    exit 0
}

foreach ($path in $paths) {
    if ($Remove) {
        Remove-MpPreference -ExclusionPath $path -ErrorAction SilentlyContinue
        Write-Host "Removed exclusion: $path"
    } else {
        Add-MpPreference -ExclusionPath $path -ErrorAction Stop
        Write-Host "Excluded: $path"
    }
}

if (-not $Remove) {
    foreach ($folder in @((Join-Path $root 'target\release'), (Join-Path $root 'dist'))) {
        Get-ChildItem -LiteralPath $folder -Filter *.exe -ErrorAction SilentlyContinue | Unblock-File
    }
    Get-Item -LiteralPath (Join-Path $env:TEMP 'Snowdrift-Setup.exe') -ErrorAction SilentlyContinue | Unblock-File
    Write-Host 'Also cleared the "downloaded from the internet" mark from local builds.'
}
