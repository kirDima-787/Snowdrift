# Signs Windows binaries with a real code-signing certificate.
#
# In CI the certificate comes from the WINDOWS_CERT_PFX_BASE64 and
# WINDOWS_CERT_PASSWORD secrets. Locally you can pass a PFX file or the
# thumbprint of a certificate in Cert:\CurrentUser\My instead.
#
# Do not use a self-signed certificate for releases: Windows does not trust it,
# so SmartScreen keeps warning and Defender may treat the file as suspicious.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string[]] $Files,

    [string] $PfxPath,
    [string] $PfxBase64,
    [string] $Password,
    [string] $Thumbprint,
    [string] $TimestampUrl = 'http://timestamp.digicert.com',

    # Only for testing the script with an untrusted test certificate.
    [switch] $SkipVerification
)

$ErrorActionPreference = 'Stop'

function Find-SignTool {
    $fromPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($fromPath) {
        return $fromPath.Source
    }

    $roots = @()
    if (${env:ProgramFiles(x86)}) {
        $roots += Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    }
    if ($env:ProgramFiles) {
        $roots += Join-Path $env:ProgramFiles 'Windows Kits\10\bin'
    }

    $found = foreach ($root in ($roots | Where-Object { Test-Path -LiteralPath $_ })) {
        Get-ChildItem -LiteralPath $root -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -like '*\x64\signtool.exe' }
    }

    $found | Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}

$signTool = Find-SignTool
if (-not $signTool) {
    throw 'signtool.exe was not found. Install the Windows SDK or add signtool to PATH.'
}

foreach ($file in $Files) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "File not found: $file"
    }
}

$temporaryPfx = $null
try {
    if ($PfxBase64) {
        if (-not $Password) {
            throw 'Password is required when PfxBase64 is used.'
        }
        $temporaryPfx = Join-Path ([IO.Path]::GetTempPath()) ('snowdrift-sign-{0}.pfx' -f [guid]::NewGuid())
        [IO.File]::WriteAllBytes($temporaryPfx, [Convert]::FromBase64String(($PfxBase64 -replace '\s', '')))
        $PfxPath = $temporaryPfx
    }

    $arguments = @('sign', '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256')
    if ($PfxPath) {
        if (-not $Password) {
            throw 'Password is required when PfxPath is used.'
        }
        $arguments += @('/f', (Resolve-Path -LiteralPath $PfxPath).Path, '/p', $Password)
    } elseif ($Thumbprint) {
        $arguments += @('/sha1', $Thumbprint)
    } else {
        throw 'Provide PfxPath, PfxBase64, or Thumbprint.'
    }
    $arguments += $Files

    & $signTool @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "signtool sign failed with exit code $LASTEXITCODE."
    }

    if (-not $SkipVerification) {
        foreach ($file in $Files) {
            & $signTool verify /pa /v $file | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "Signature verification failed for $file."
            }
            $signature = Get-AuthenticodeSignature -LiteralPath $file
            Write-Host ("Signed {0} by {1}" -f $file, $signature.SignerCertificate.Subject)
        }
    }
} finally {
    if ($temporaryPfx -and (Test-Path -LiteralPath $temporaryPfx)) {
        Remove-Item -LiteralPath $temporaryPfx -Force
    }
}
