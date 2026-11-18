# Nonce-owned fixtures only. Never imports certificates into a Windows store.
param([switch]$OnlineTimestamp)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Windows helper fixtures require PowerShell 7 on Windows.' }
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
. (Join-Path $repo 'signing/sign.ps1') -LibraryOnly
Initialize-PrismSignatureVerifier
$root = Join-Path ([IO.Path]::GetTempPath()) ('prism-signing-fixture-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $root
$ownedFiles = [Collections.Generic.List[string]]::new()
$certificates = [Collections.Generic.List[Security.Cryptography.X509Certificates.X509Certificate2]]::new()
$keys = [Collections.Generic.List[Security.Cryptography.RSA]]::new()
$preparedDirectory = $null
$preparedToken = $null
$generatorDirectories = [Collections.Generic.List[string]]::new()
$generationSecret = $null
function Assert-Fixture([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Assert-Rejected([scriptblock]$Operation, [string]$Message) {
    $rejected = $false
    try { & $Operation } catch { $rejected = $true }
    Assert-Fixture $rejected $Message
}
function New-FixtureCertificate([string]$Kind = 'valid') {
    $rsa = [Security.Cryptography.RSA]::Create(2048); $keys.Add($rsa)
    $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=Prism disposable signing fixture', $rsa, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $purposes = [Security.Cryptography.OidCollection]::new()
    $null = $purposes.Add([Security.Cryptography.Oid]::new($(if ($Kind -eq 'wrong-eku') { '1.3.6.1.5.5.7.3.1' } else { '1.3.6.1.5.5.7.3.3' })))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($purposes, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new(($Kind -eq 'ca'), $false, 0, $true))
    $request.CertificateExtensions.Add([Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new([Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true))
    $start = [DateTimeOffset]::UtcNow.AddMinutes(-5); $end = [DateTimeOffset]::UtcNow.AddHours(1)
    if ($Kind -eq 'expired') { $start = $start.AddDays(-2); $end = $end.AddDays(-1) }
    $cert = $request.CreateSelfSigned($start, $end); $certificates.Add($cert)
    return $cert
}
function Invoke-FixtureChild([string]$Script, [string[]]$Arguments, [string]$DummyPfx, [string]$DummyPassword) {
    $info = [Diagnostics.ProcessStartInfo]::new((Join-Path $PSHOME 'pwsh.exe'))
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    foreach ($arg in @('-NoProfile', '-NonInteractive', '-File', $Script) + $Arguments) { $info.ArgumentList.Add($arg) }
    # Never reads, saves or restores production secrets. Child values are replaced before launch.
    $null = $info.Environment.Remove('WINDOWS_SIGNING_PFX'); $null = $info.Environment.Remove('PRISM_SIGNING_PASSWORD')
    if ($DummyPfx) { $info.Environment['WINDOWS_SIGNING_PFX'] = $DummyPfx }
    if ($DummyPassword) { $info.Environment['PRISM_SIGNING_PASSWORD'] = $DummyPassword }
    $info.Environment['GITHUB_ACTIONS'] = 'true'; $info.Environment['RUNNER_ENVIRONMENT'] = 'github-hosted'
    $info.Environment['RUNNER_TEMP'] = $root
    $info.Environment['GITHUB_OUTPUT'] = Join-Path $root 'output.txt'
    $info.Environment['GITHUB_ENV'] = Join-Path $root 'environment.txt'
    foreach ($name in 'output.txt', 'environment.txt') {
        $file = Join-Path $root $name
        if (-not $ownedFiles.Contains($file)) { $ownedFiles.Add($file) }
        [IO.File]::WriteAllText($file, '')
    }
    $process = [Diagnostics.Process]::Start($info)
    $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(60000)) {
        $process.Kill($true) # Only this fixture's retained child-process handle and its children.
        $null = $process.WaitForExit(10000)
        $process.Dispose()
        throw 'Fixture helper exceeded its 60 second bound.'
    }
    $result = @{ ExitCode=$process.ExitCode; Stdout=$stdout.GetAwaiter().GetResult(); Stderr=$stderr.GetAwaiter().GetResult() }
    $process.Dispose(); return $result
}
try {
    $valid = New-FixtureCertificate
    Assert-PrismCertificate $valid
    foreach ($kind in 'ca', 'wrong-eku', 'expired') {
        $invalid = New-FixtureCertificate $kind
        Assert-Rejected { Assert-PrismCertificate $invalid } "Accepted $kind certificate."
    }
    $public = [Security.Cryptography.X509Certificates.X509Certificate2]::new($valid.RawData); $certificates.Add($public)
    Assert-Rejected { Assert-PrismCertificate $public } 'Accepted public-only signer.'
    $password = 'Disposable-fixture-only-' + [Guid]::NewGuid().ToString('N')
    $pfx = Join-Path $root 'fixture.pfx'; $ownedFiles.Add($pfx)
    [IO.File]::WriteAllBytes($pfx, $valid.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $password))
    $pe = Join-Path $root 'inert-copy.exe'; $ownedFiles.Add($pe)
    Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32/where.exe') -Destination $pe
    $sign = Join-Path $repo 'signing/sign.ps1'
    $signed = Invoke-FixtureChild $sign @('-Path', $pe, '-Pfx', $pfx, '-NoTimestamp') '' $password
    Assert-Fixture ($signed.ExitCode -eq 0) ('Offline fixture signing/verification failed: ' + $signed.Stderr)
    [PrismReleaseSignature]::Verify($pe, $valid.RawData, $false)
    Assert-Rejected { [PrismReleaseSignature]::Verify($pe, $valid.RawData, $true) } 'Accepted missing RFC3161 timestamp.'
    $other = New-FixtureCertificate
    Assert-Rejected { [PrismReleaseSignature]::Verify($pe, $other.RawData, $false) } 'Accepted different signer pin.'
    if ($OnlineTimestamp) {
        $timed = Invoke-FixtureChild $sign @('-Path', $pe, '-Pfx', $pfx) '' $password
        Assert-Fixture ($timed.ExitCode -eq 0) ('RFC3161 fixture signing/verification failed: ' + $timed.Stderr + $timed.Stdout)
        [PrismReleaseSignature]::Verify($pe, $valid.RawData, $true)
    }
    $bytes = [IO.File]::ReadAllBytes($pe); $header = [BitConverter]::ToInt32($bytes, 60)
    $section = $header + 24 + [BitConverter]::ToUInt16($bytes, $header + 20)
    $raw = [BitConverter]::ToInt32($bytes, $section + 20); $bytes[$raw] = $bytes[$raw] -bxor 1
    [IO.File]::WriteAllBytes($pe, $bytes)
    Assert-Rejected { [PrismReleaseSignature]::Verify($pe, $valid.RawData, $false) } 'Accepted tampered executable content.'
    $prepare = Join-Path $repo 'signing/from-ci-secret.ps1'
    $unsigned = Invoke-FixtureChild $prepare @() '' ''
    Assert-Fixture ($unsigned.ExitCode -eq 0 -and ([IO.File]::ReadAllText((Join-Path $root 'output.txt')) -match 'configured=false')) 'Both-absent unsigned configuration failed.'
    Assert-Fixture ([IO.File]::ReadAllText((Join-Path $root 'environment.txt')) -ceq '') 'Unsigned configuration wrote a build environment value.'
    $pfxOnly = Invoke-FixtureChild $prepare @() 'invalid-base64-fixture' ''
    Assert-Fixture ($pfxOnly.ExitCode -ne 0) 'Accepted PFX-only partial configuration.'
    $partial = Invoke-FixtureChild $prepare @() '' $password
    Assert-Fixture ($partial.ExitCode -ne 0) 'Accepted password-only partial configuration.'
    $malformed = Invoke-FixtureChild $prepare @() 'invalid-base64-fixture' $password
    Assert-Fixture ($malformed.ExitCode -ne 0 -and @(Get-ChildItem -LiteralPath $root -Directory).Count -eq 0) 'Malformed PFX did not fail and clean up.'
    $dummyEncoded = [Convert]::ToBase64String([IO.File]::ReadAllBytes($pfx))
    $badPassword = Invoke-FixtureChild $prepare @() $dummyEncoded 'deliberately-wrong-fixture-password'
    Assert-Fixture ($badPassword.ExitCode -ne 0 -and @(Get-ChildItem -LiteralPath $root -Directory).Count -eq 0) 'Wrong PFX password did not fail and clean up.'
    $prepared = Invoke-FixtureChild $prepare @() $dummyEncoded $password
    Assert-Fixture ($prepared.ExitCode -eq 0) ('Fixture preparation failed: ' + $prepared.Stderr)
    $outputs = @{}
    Get-Content -LiteralPath (Join-Path $root 'output.txt') | ForEach-Object { $pair = $_.Split('=', 2); $outputs[$pair[0]] = $pair[1] }
    $preparedDirectory = $outputs['directory']; $preparedToken = $outputs['ownership-token']
    Assert-Fixture ($outputs['configured'] -eq 'true') 'Configured output missing.'
    $der = [Security.Cryptography.X509Certificates.X509Certificate2]::new([IO.File]::ReadAllBytes($outputs['cer'])); $certificates.Add($der)
    Assert-Fixture (-not $der.HasPrivateKey -and $der.Thumbprint -eq $valid.Thumbprint) 'CER does not contain exactly the public signer.'
    $rules = (Get-Acl -LiteralPath $outputs['pfx']).Access
    Assert-Fixture (@($rules | Where-Object { $_.AccessControlType -eq 'Allow' -and $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]) -ne [Security.Principal.WindowsIdentity]::GetCurrent().User }).Count -eq 0) 'PFX ACL grants other principals access.'
    $extra = Join-Path $preparedDirectory 'unexpected.txt'; $ownedFiles.Add($extra)
    [IO.File]::WriteAllText($extra, 'owned test entry')
    $refused = Invoke-FixtureChild $prepare @('-Cleanup', '-Directory', $preparedDirectory, '-OwnershipToken', $preparedToken) '' ''
    Assert-Fixture ($refused.ExitCode -ne 0 -and (Test-Path -LiteralPath $outputs['pfx'])) 'Cleanup deleted a directory containing an unknown entry.'
    Remove-Item -LiteralPath $extra
    $cleanup = Invoke-FixtureChild $prepare @('-Cleanup', '-Directory', $outputs['directory'], '-OwnershipToken', $outputs['ownership-token']) '' ''
    Assert-Fixture ($cleanup.ExitCode -eq 0 -and -not (Test-Path -LiteralPath $outputs['directory'])) 'Explicit owned cleanup failed.'
    $generator = Join-Path $repo 'signing/new-certificate.ps1'
    $generationSecret = ConvertTo-SecureString ([Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(48))) -AsPlainText -Force
    $generated = Join-Path $root 'new-publisher'; $generatorDirectories.Add($generated)
    foreach ($leaf in 'prism-publisher.pfx', 'prism-publisher.cer') { $ownedFiles.Add((Join-Path $generated $leaf)) }
    & $generator -Name 'Prism disposable generator fixture' -Years 1 -Password $generationSecret -OutputDirectory $generated
    Assert-Fixture ($generationSecret.Length -gt 8) 'Generator disposed caller-owned SecureString.'
    $generatedPfx = Join-Path $generated 'prism-publisher.pfx'; $generatedCer = Join-Path $generated 'prism-publisher.cer'
    Assert-Fixture ((Test-Path -LiteralPath $generatedPfx -PathType Leaf) -and (Test-Path -LiteralPath $generatedCer -PathType Leaf)) 'Generator omitted PFX or CER.'
    $generatedCert = [Security.Cryptography.X509Certificates.X509Certificate2]::new([IO.File]::ReadAllBytes($generatedPfx), $generationSecret, [Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet); $certificates.Add($generatedCert)
    Assert-PrismCertificate $generatedCert
    $publicGenerated = [Security.Cryptography.X509Certificates.X509Certificate2]::new([IO.File]::ReadAllBytes($generatedCer)); $certificates.Add($publicGenerated)
    Assert-Fixture (-not $publicGenerated.HasPrivateKey -and [Linq.Enumerable]::SequenceEqual[byte]($publicGenerated.RawData, $generatedCert.RawData)) 'Generator CER is not matching public DER.'
    $rsaGenerated = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($generatedCert)
    try { Assert-Fixture ($rsaGenerated.KeySize -eq 3072) 'Generator did not use RSA3072.' } finally { $rsaGenerated.Dispose() }
    Assert-Fixture ([Math]::Abs(($generatedCert.NotAfter.ToUniversalTime() - [DateTime]::UtcNow.AddYears(1)).TotalMinutes) -lt 2) 'Generator expiry is not the requested UTC duration.'
    foreach ($file in $generated, $generatedPfx, $generatedCer) {
        $access = Get-Acl -LiteralPath $file
        Assert-Fixture (@($access.Access | Where-Object { $_.AccessControlType -eq 'Allow' -and $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]) -ne [Security.Principal.WindowsIdentity]::GetCurrent().User }).Count -eq 0) 'Generated output ACL grants another principal access.'
    }
    $beforePfx = (Get-FileHash -LiteralPath $generatedPfx -Algorithm SHA256).Hash
    $beforeCer = (Get-FileHash -LiteralPath $generatedCer -Algorithm SHA256).Hash
    Assert-Rejected { & $generator -Password $generationSecret -OutputDirectory $generated } 'Generator overwrote an existing publisher.'
    Assert-Fixture ((Get-FileHash -LiteralPath $generatedPfx -Algorithm SHA256).Hash -eq $beforePfx -and (Get-FileHash -LiteralPath $generatedCer -Algorithm SHA256).Hash -eq $beforeCer) 'Refused overwrite changed existing certificate bytes.'
    $emptyExisting = Join-Path $root 'existing-empty'; $generatorDirectories.Add($emptyExisting); $null = New-Item -ItemType Directory -Path $emptyExisting
    Assert-Rejected { & $generator -Password $generationSecret -OutputDirectory $emptyExisting } 'Generator accepted an existing empty directory.'
    $weak = [Security.SecureString]::new(); $weak.AppendChar('x')
    try { Assert-Rejected { & $generator -Password $weak -OutputDirectory (Join-Path $root 'weak-password') } 'Generator accepted a weak password.' } finally { $weak.Dispose() }
    Assert-Fixture (-not (Test-Path -LiteralPath (Join-Path $root 'weak-password'))) 'Weak password created an output directory.'
    Assert-Rejected { & $generator -Password $null -OutputDirectory (Join-Path $root 'null-password') } 'Generator accepted a null automation password.'
    Assert-Rejected { & $generator -Password $generationSecret -OutputDirectory (Join-Path $root 'missing-parent/child') } 'Generator created a missing parent implicitly.'
    Assert-Rejected { & $generator -Name 'Prism,CN=Injected' -Password $generationSecret -OutputDirectory (Join-Path $root 'invalid-name') } 'Generator accepted a distinguished-name injection.'
    $aclFailure = Join-Path $root 'acl-failure'
    function Set-Acl { throw 'Disposable fixture: ACL denied.' }
    try { Assert-Rejected { & $generator -Password $generationSecret -OutputDirectory $aclFailure } 'Generator ignored ACL failure.' } finally { Remove-Item Function:Set-Acl }
    Assert-Fixture (-not (Test-Path -LiteralPath $aclFailure)) 'Failed private ACL preparation left an output directory.'
    $interactive = Join-Path $root 'interactive'; $generatorDirectories.Add($interactive)
    foreach ($leaf in 'prism-publisher.pfx', 'prism-publisher.cer') { $ownedFiles.Add((Join-Path $interactive $leaf)) }
    $fixturePromptState = @{ Count = 0 }
    function Read-Host { param([string]$Prompt, [switch]$AsSecureString); $fixturePromptState.Count++; return $generationSecret.Copy() }
    try { & $generator -OutputDirectory $interactive } finally { Remove-Item Function:Read-Host }
    Assert-Fixture ($fixturePromptState.Count -eq 2 -and (Test-Path -LiteralPath (Join-Path $interactive 'prism-publisher.pfx'))) 'Interactive password/confirmation flow was not preserved.'
    Write-Host ('PASS: certificate policy, public DER, unsigned/partial/wrong-password configuration, private PFX ACL, unknown-entry-safe cleanup, PE signature/pin/digest and timestamp rejection, generator RSA3072/UTC/privateACL/exclusive-output/SecureString/interactive; online RFC3161=' + [bool]$OnlineTimestamp)
} finally {
    if ($generationSecret) { $generationSecret.Dispose() }
    foreach ($cert in $certificates) { $cert.Dispose() }; foreach ($key in $keys) { $key.Dispose() }
    foreach ($file in $ownedFiles) { if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force } }
    if ($preparedDirectory -and (Test-Path -LiteralPath $preparedDirectory)) {
        $finalCleanup = Invoke-FixtureChild (Join-Path $repo 'signing/from-ci-secret.ps1') @('-Cleanup', '-Directory', $preparedDirectory, '-OwnershipToken', $preparedToken) '' ''
        if ($finalCleanup.ExitCode -ne 0) { Write-Warning 'Fixture signing directory retained for inspection after safe cleanup refused.' }
        foreach ($name in 'output.txt', 'environment.txt') { $file = Join-Path $root $name; if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file } }
    }
    foreach ($directory in $generatorDirectories) {
        if ((Test-Path -LiteralPath $directory) -and @(Get-ChildItem -LiteralPath $directory -Force).Count -eq 0) { Remove-Item -LiteralPath $directory }
    }
    # Keep unknown files/directories for inspection; never recursively remove a computed path.
    if ((Test-Path -LiteralPath $root) -and @(Get-ChildItem -LiteralPath $root -Force).Count -eq 0) { Remove-Item -LiteralPath $root }
}
