<#
.SYNOPSIS
Creates Prism's self-made code-signing certificate in this folder.

.DESCRIPTION
Writes two files next to this script (both ignored by git):
  prism-publisher.pfx  certificate + private key, used to sign. Keep it secret and back it up
                       somewhere safe offline; whoever has it can sign programs as Prism.
  prism-publisher.cer  public certificate only. Setup is built with it and asks each user's
                       Windows to trust it.

The certificate is limited to code signing (it cannot vouch for websites) and cannot issue other
certificates. Nothing is added to any Windows certificate list on this PC.

.EXAMPLE
pwsh signing/new-certificate.ps1
pwsh signing/new-certificate.ps1 -Name "Prism (Example Corp)" -Years 5
#>
param(
    # Publisher name Windows shows ("CN=..."). Keep it the same for the certificate's lifetime.
    [string]$Name = 'Prism Publisher',
    # Validity in years. When it ends, make a new one; every PC must then trust the new one.
    [ValidateRange(1, 10)][int]$Years = 5
)
$ErrorActionPreference = 'Stop'
$X509 = 'System.Security.Cryptography.X509Certificates'

$pfx = Join-Path $PSScriptRoot 'prism-publisher.pfx'
$cer = Join-Path $PSScriptRoot 'prism-publisher.cer'
if ((Test-Path $pfx) -or (Test-Path $cer)) {
    throw "A certificate already exists in $PSScriptRoot. Move it away first: a new certificate means every PC has to trust it again."
}

# The password protects the .pfx file; signing asks for it.
$password = Read-Host 'Password for prism-publisher.pfx' -AsSecureString
$again = Read-Host 'Same password again' -AsSecureString
$plain = [Net.NetworkCredential]::new('', $password).Password
if ($plain.Length -lt 8) { throw 'Use at least 8 characters.' }
if ($plain -ne [Net.NetworkCredential]::new('', $again).Password) { throw 'The passwords differ.' }
$plain = $null

$rsa = [System.Security.Cryptography.RSA]::Create(3072)
$request = New-Object "$X509.CertificateRequest" @(
    "CN=$Name",
    $rsa,
    [System.Security.Cryptography.HashAlgorithmName]::SHA256,
    [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
)
# Purpose: code signing only (1.3.6.1.5.5.7.3.3), marked critical so nothing else accepts it.
$purposes = New-Object System.Security.Cryptography.OidCollection
[void]$purposes.Add((New-Object System.Security.Cryptography.Oid '1.3.6.1.5.5.7.3.3'))
$request.CertificateExtensions.Add((New-Object "$X509.X509EnhancedKeyUsageExtension" @($purposes, $true)))
# Not a certificate authority: it cannot be used to create other certificates.
$request.CertificateExtensions.Add((New-Object "$X509.X509BasicConstraintsExtension" @($false, $false, 0, $true)))
$request.CertificateExtensions.Add((New-Object "$X509.X509KeyUsageExtension" @(
    [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true)))
$request.CertificateExtensions.Add((New-Object "$X509.X509SubjectKeyIdentifierExtension" @($request.PublicKey, $false)))

$now = [DateTimeOffset]::UtcNow
$certificate = $request.CreateSelfSigned($now.AddMinutes(-5), $now.AddYears($Years))
[IO.File]::WriteAllBytes($pfx, $certificate.Export('Pfx', $password))
[IO.File]::WriteAllBytes($cer, $certificate.Export('Cert'))

Write-Host "Created:"
Write-Host "  $pfx  (secret: signing key)"
Write-Host "  $cer  (public: built into setup)"
Write-Host "Thumbprint: $($certificate.Thumbprint)"
Write-Host "Valid until: $($certificate.NotAfter.ToString('yyyy-MM-dd'))"
