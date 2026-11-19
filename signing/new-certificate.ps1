<#
.SYNOPSIS
Creates a new self-signed Prism publisher in a fresh private output directory.
.DESCRIPTION
No certificates are installed in Windows stores. The key exists in memory and in the encrypted
PFX only. OutputDirectory must not exist; its parent must already be a real, non-aliased folder.
Password accepts a SecureString object in the calling PowerShell process, never a plaintext
command-line password. Without it, the existing interactive password/confirmation prompts run.
.EXAMPLE
pwsh signing/new-certificate.ps1
.EXAMPLE
# $securePassword is an existing SecureString held by the calling process.
& ./signing/new-certificate.ps1 -Password $securePassword -OutputDirectory $newPrivateFolder
#>
param(
    [string]$Name = 'Prism Publisher',
    [ValidateRange(1, 10)][int]$Years = 5,
    [System.Security.SecureString]$Password,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'private-publisher')
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Publisher generation requires PowerShell 7 on Windows.' }
if ([string]::IsNullOrWhiteSpace($Name) -or $Name.Length -gt 120 -or $Name -cne $Name.Trim() -or $Name -match '[,#+"=<>;\\\x00-\x1f]') { throw 'Use one plain publisher common name, without distinguished-name separators or control characters.' }
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { throw 'OutputDirectory must name a new directory.' }
$destination = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $destination) { throw 'OutputDirectory already exists; refusing to replace a publisher or change an existing directory.' }
$parent = Get-Item -LiteralPath ([IO.Path]::GetDirectoryName($destination)) -Force
if (-not $parent.PSIsContainer) { throw 'OutputDirectory parent must be an existing directory.' }
for ($node = $parent; $null -ne $node; $node = $node.Parent) {
    if ($node.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'OutputDirectory must not contain aliases.' }
}
$secret = $null; $first = $null; $confirmation = $null
$rsa = $null; $certificate = $null; $exportCheck = $null; $publicCheck = $null
$pfxBytes = $null; $createdDirectory = $false; $completed = $false
$createdFiles = [Collections.Generic.List[string]]::new()
try {
    if ($PSBoundParameters.ContainsKey('Password')) {
        if ($null -eq $Password -or $Password.Length -lt 8) { throw 'Supply a SecureString password of at least 8 characters.' }
        $secret = $Password.Copy() # Caller retains ownership of its original SecureString.
    } else {
        $first = Read-Host 'Password for prism-publisher.pfx' -AsSecureString
        $confirmation = Read-Host 'Same password again' -AsSecureString
        if ($first.Length -lt 8) { throw 'Use at least 8 characters.' }
        if ($first.Length -ne $confirmation.Length) { throw 'The passwords differ.' }
        $left = [IntPtr]::Zero; $right = [IntPtr]::Zero
        try {
            $left = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($first)
            $right = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($confirmation)
            $difference = 0
            for ($i = 0; $i -lt $first.Length; $i++) {
                $difference = $difference -bor ([Runtime.InteropServices.Marshal]::ReadInt16($left, $i * 2) -bxor [Runtime.InteropServices.Marshal]::ReadInt16($right, $i * 2))
            }
            if ($difference -ne 0) { throw 'The passwords differ.' }
        } finally {
            if ($left -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($left) }
            if ($right -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($right) }
        }
        $secret = $first.Copy()
    }
    $secret.MakeReadOnly()
    # No key material is created until the destination is private.
    $null = New-Item -ItemType Directory -Path $destination
    $createdDirectory = $true
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($identity); $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity, [Security.AccessControl.FileSystemRights]::FullControl, ([Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit), [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow))
    Set-Acl -LiteralPath $destination -AclObject $acl

    $rsa = [Security.Cryptography.RSA]::Create(3072)
    $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new("CN=$Name", $rsa, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $purposes = [Security.Cryptography.OidCollection]::new()
    $null = $purposes.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3'))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($purposes, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($false, $false, 0, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new([Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509SubjectKeyIdentifierExtension]::new($request.PublicKey, $false))
    $now = [DateTimeOffset]::UtcNow
    $certificate = $request.CreateSelfSigned($now.AddMinutes(-5), $now.AddYears($Years))
    $pfxBytes = $certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $secret)
    $der = $certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Cert)
    # Round-trip in memory only: public DER must never be a PFX/private-key container.
    $publicCheck = [Security.Cryptography.X509Certificates.X509Certificate2]::new($der)
    $exportCheck = [Security.Cryptography.X509Certificates.X509Certificate2]::new($pfxBytes, $secret, [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
    if ($publicCheck.HasPrivateKey -or -not $exportCheck.HasPrivateKey -or -not [Linq.Enumerable]::SequenceEqual[byte]($der, $certificate.RawData) -or -not [Linq.Enumerable]::SequenceEqual[byte]($der, $exportCheck.RawData)) { throw 'Generated certificate export does not match its publisher identity.' }
    $pfx = Join-Path $destination 'prism-publisher.pfx'; $cer = Join-Path $destination 'prism-publisher.cer'
    foreach ($payload in @(@{Path=$pfx; Bytes=$pfxBytes}, @{Path=$cer; Bytes=$der})) {
        $stream = [IO.File]::Open($payload.Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $createdFiles.Add($payload.Path)
        try { $stream.Write($payload.Bytes); $stream.Flush($true) } finally { $stream.Dispose() }
    }
    $completed = $true
    Write-Host "Created private PFX and public DER in: $destination"
    Write-Host "Publisher thumbprint: $($certificate.Thumbprint)"
    Write-Host "Public DER SHA-256: $([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($der)))"
    Write-Host "Valid until (UTC): $($certificate.NotAfter.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))"
} finally {
    foreach ($item in @($publicCheck, $exportCheck, $certificate, $rsa, $secret, $first, $confirmation)) { if ($null -ne $item) { $item.Dispose() } }
    if ($pfxBytes) { [Array]::Clear($pfxBytes, 0, $pfxBytes.Length) }
    if (-not $completed -and $createdDirectory) {
        $directory = Get-Item -LiteralPath $destination -Force
        if (-not $directory.PSIsContainer -or $directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Failed generation output became aliased; refusing cleanup.' }
        foreach ($file in $createdFiles) {
            if (Test-Path -LiteralPath $file) {
                $entry = Get-Item -LiteralPath $file -Force
                if ($entry.PSIsContainer -or $entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Failed generation file became aliased; refusing cleanup.' }
                Remove-Item -LiteralPath $file -Force
            }
        }
        # Never recursively delete; unexpected contents stay for inspection.
        if (@(Get-ChildItem -LiteralPath $destination -Force).Count -eq 0) { Remove-Item -LiteralPath $destination }
    }
}
