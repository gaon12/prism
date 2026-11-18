<#
.SYNOPSIS
CI only: writes the signing files of this folder from repository secrets.

.DESCRIPTION
Reads WINDOWS_SIGNING_PFX (the .pfx file, Base64) from the environment and writes
  prism-publisher.pfx  for sign.ps1 (deleted again by the CI runner with the workspace)
  prism-publisher.cer  its public part, which the setup build embeds.
The password stays in PRISM_SIGNING_PASSWORD for sign.ps1 and is only used here to read the
file. Prints "configured=true|false" for the workflow; without the secret nothing is written and
the build continues unsigned.

Make the secret on the PC that holds the certificate:
  [Convert]::ToBase64String([IO.File]::ReadAllBytes('signing/prism-publisher.pfx')) | Set-Clipboard
#>
$ErrorActionPreference = 'Stop'
if (-not $env:WINDOWS_SIGNING_PFX) {
    Write-Host 'No WINDOWS_SIGNING_PFX secret: building unsigned.'
    'configured=false'
    return
}
if (-not $env:PRISM_SIGNING_PASSWORD) { throw 'WINDOWS_SIGNING_PFX needs PRISM_SIGNING_PASSWORD as well.' }

$pfx = Join-Path $PSScriptRoot 'prism-publisher.pfx'
$cer = Join-Path $PSScriptRoot 'prism-publisher.cer'
[IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_SIGNING_PFX))
$password = ConvertTo-SecureString $env:PRISM_SIGNING_PASSWORD -AsPlainText -Force
$certificate = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 @(
    $pfx, $password,
    [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
if (-not $certificate.HasPrivateKey) { throw 'The signing secret has no private key.' }
# Same rule as setup: code signing only. Fail here rather than ship a setup that refuses it.
$purposes = @($certificate.Extensions |
    Where-Object { $_ -is [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] } |
    ForEach-Object { $_.EnhancedKeyUsages } | ForEach-Object Value)
if ($purposes.Count -eq 0 -or ($purposes | Where-Object { $_ -ne '1.3.6.1.5.5.7.3.3' })) {
    throw 'The signing certificate must be limited to code signing (make it with new-certificate.ps1).'
}
[IO.File]::WriteAllBytes($cer, $certificate.Export('Cert'))
Write-Host "Signing certificate ready: $($certificate.Subject), thumbprint $($certificate.Thumbprint)"
'configured=true'
