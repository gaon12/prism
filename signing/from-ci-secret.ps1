<#
Hosted-CI preparation only. Both secrets absent means unsigned; a partial pair fails.
The PFX is created exclusively in a fresh private RUNNER_TEMP directory, never the checkout.
Only the public DER certificate is exported for PRISM_TRUSTED_CERT. No certificate store writes.
-Cleanup deletes only known regular files in a directory authenticated by its nonce marker.
#>
param([switch]$Cleanup, [string]$Directory, [string]$OwnershipToken)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'sign.ps1') -LibraryOnly

function Remove-PrismSigningDirectory([string]$OwnedDirectory, [string]$Token) {
    if (-not $OwnedDirectory -or -not $Token) { throw 'Cleanup requires directory and ownership token.' }
    if ($Token -notmatch '^[a-f0-9]{32}$') { throw 'Invalid ownership token.' }
    $base = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $expected = Join-Path $base "prism-signing-$Token"
    if ([IO.Path]::GetFullPath($OwnedDirectory) -cne $expected) { throw 'Cleanup directory is outside its owned temp path.' }
    if (-not (Test-Path -LiteralPath $expected)) { return }
    $folder = Get-Item -LiteralPath $expected -Force
    if (-not $folder.PSIsContainer -or $folder.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Cleanup refuses an aliased directory.' }
    $marker = Assert-PrismRegularFile (Join-Path $expected 'owner.txt')
    if ([IO.File]::ReadAllText($marker) -cne $Token) { throw 'Cleanup ownership marker does not match.' }
    $known = @('owner.txt', 'prism-publisher.pfx', 'prism-publisher.cer')
    $children = @(Get-ChildItem -LiteralPath $expected -Force)
    foreach ($child in $children) {
        if ($child.Name -notin $known -or $child.PSIsContainer -or $child.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected entry in signing directory; refusing deletion.' }
        $null = Assert-PrismRegularFile $child.FullName
    }
    foreach ($name in @('prism-publisher.pfx', 'prism-publisher.cer', 'owner.txt')) {
        $file = Join-Path $expected $name
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    }
    Remove-Item -LiteralPath $expected # Empty directory only. Never recurse.
}

if ($Cleanup) { Remove-PrismSigningDirectory $Directory $OwnershipToken; return }
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'Secret preparation is restricted to disposable GitHub-hosted Windows runners.' }
$hasPfx = -not [string]::IsNullOrEmpty($env:WINDOWS_SIGNING_PFX)
$hasPassword = -not [string]::IsNullOrEmpty($env:PRISM_SIGNING_PASSWORD)
if ($hasPfx -ne $hasPassword) { throw 'Set WINDOWS_SIGNING_PFX and PRISM_SIGNING_PASSWORD together, or neither.' }
if (-not $hasPfx) {
    if ($env:GITHUB_OUTPUT) { 'configured=false' | Out-File -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 -Append }
    Write-Host 'Windows Authenticode is not configured; building unsigned.'
    return
}
if (-not $env:RUNNER_TEMP -or -not $env:GITHUB_OUTPUT -or -not $env:GITHUB_ENV) { throw 'CI output and temp paths are required.' }
$base = Get-Item -LiteralPath $env:RUNNER_TEMP -Force
for ($node = $base; $null -ne $node; $node = $node.Parent) {
    if ($node.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Runner temp must not contain aliases.' }
}
$token = [Guid]::NewGuid().ToString('N')
$owned = Join-Path $base.FullName "prism-signing-$token"
# Directory.CreateDirectory alone accepts an existing directory: New-Item without Force must fail.
$null = New-Item -ItemType Directory -Path $owned
$completed = $false
$certificate = $null
$password = $null
$bytes = $null
try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($identity)
    $acl.SetAccessRuleProtection($true, $false)
    $rule = [Security.AccessControl.FileSystemAccessRule]::new($identity, [Security.AccessControl.FileSystemRights]::FullControl, ([Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit), [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
    Set-Acl -LiteralPath $owned -AclObject $acl
    # Exclusive marker and payload writes; never replace an existing file or follow a link.
    $stream = [IO.File]::Open((Join-Path $owned 'owner.txt'), [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $encoded = [Text.Encoding]::UTF8.GetBytes($token); $stream.Write($encoded) } finally { $stream.Dispose() }
    "directory=$owned", "ownership-token=$token" | Out-File -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 -Append
    $bytes = [Convert]::FromBase64String($env:WINDOWS_SIGNING_PFX)
    $password = ConvertTo-SecureString $env:PRISM_SIGNING_PASSWORD -AsPlainText -Force
    $certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes, $password, [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
    Assert-PrismCertificate $certificate
    $pfx = Join-Path $owned 'prism-publisher.pfx'
    $cer = Join-Path $owned 'prism-publisher.cer'
    $publicDer = $certificate.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert)
    $publicOnly = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($publicDer)
    try {
        if ($publicOnly.HasPrivateKey -or -not [Linq.Enumerable]::SequenceEqual[byte]($publicDer, $certificate.RawData)) { throw 'Public DER export does not match signer.' }
    } finally { $publicOnly.Dispose() }
    foreach ($payload in @(@{Path=$pfx; Bytes=$bytes}, @{Path=$cer; Bytes=$publicDer})) {
        $stream = [IO.File]::Open($payload.Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($payload.Bytes) } finally { $stream.Dispose() }
    }
    "pfx=$pfx", "cer=$cer", 'configured=true' | Out-File -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8 -Append
    "PRISM_TRUSTED_CERT=$cer" | Out-File -LiteralPath $env:GITHUB_ENV -Encoding utf8 -Append
    Write-Host "Prepared public publisher certificate: $($certificate.Thumbprint)"
    $completed = $true
} finally {
    if ($certificate) { $certificate.Dispose() }
    if ($password) { $password.Dispose() }
    if ($bytes) { [Array]::Clear($bytes, 0, $bytes.Length) }
    if (-not $completed) {
        # Before a marker exists, only the fresh, still-empty owned directory may be removed.
        if (Test-Path -LiteralPath (Join-Path $owned 'owner.txt')) { Remove-PrismSigningDirectory $owned $token }
        elseif (@(Get-ChildItem -LiteralPath $owned -Force).Count -eq 0) { Remove-Item -LiteralPath $owned }
    }
}
