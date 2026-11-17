<#
.SYNOPSIS
Signs Prism programs with prism-publisher.pfx (Authenticode, SHA-256).

.DESCRIPTION
Uses Windows' own Set-AuthenticodeSignature, so no SDK tools are needed. The password comes from
the PRISM_SIGNING_PASSWORD environment variable (CI) or is asked for.

A time stamp from a public time-stamp service keeps signatures valid after the certificate
expires; only the files' hashes are sent there. Pass -NoTimestamp to skip it (offline).

Order for a release (see README.md): sign prism.exe, build setup with it, then sign the setup.

.EXAMPLE
pwsh signing/sign.ps1 src-tauri/target/release/prism.exe
pwsh signing/sign.ps1 installer/src-tauri/target/release/prism-setup.exe
pwsh signing/sign.ps1 a.exe,b.exe -NoTimestamp
#>
param(
    [Parameter(Mandatory, Position = 0)][string[]]$Path,
    [string]$Pfx = (Join-Path $PSScriptRoot 'prism-publisher.pfx'),
    [string]$TimestampServer = 'http://timestamp.digicert.com',
    # Skip the time stamp (offline builds); the signature then ends with the certificate.
    [switch]$NoTimestamp
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Pfx)) { throw "No signing certificate at $Pfx. Create one with new-certificate.ps1." }
$password = if ($env:PRISM_SIGNING_PASSWORD) {
    ConvertTo-SecureString $env:PRISM_SIGNING_PASSWORD -AsPlainText -Force
} else {
    Read-Host 'Password for the signing certificate' -AsSecureString
}
# Loaded in memory only: nothing is added to this PC's certificate lists.
$certificate = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 @(
    (Resolve-Path $Pfx).Path, $password,
    [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
if (-not $certificate.HasPrivateKey) { throw "$Pfx has no private key." }

foreach ($file in $Path) {
    $full = (Resolve-Path $file).Path
    $options = @{ FilePath = $full; Certificate = $certificate; HashAlgorithm = 'SHA256' }
    if (-not $NoTimestamp) { $options.TimestampServer = $TimestampServer }
    $result = Set-AuthenticodeSignature @options
    # On a PC that does not trust the certificate yet the status is "UnknownError"; what
    # matters is that the file now carries a signature by exactly this certificate.
    if (-not $result.SignerCertificate -or $result.SignerCertificate.Thumbprint -ne $certificate.Thumbprint) {
        throw "Signing failed for ${full}: $($result.StatusMessage)"
    }
    Write-Host "Signed $full ($($result.Status))"
}
