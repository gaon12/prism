param(
    [string[]]$Path,
    [string]$Pfx = (Join-Path $PSScriptRoot 'prism-publisher.pfx'),
    [string]$TimestampServer = 'http://timestamp.digicert.com',
    [switch]$NoTimestamp,
    [switch]$LibraryOnly
)
$ErrorActionPreference = 'Stop'

function Assert-PrismRegularFile([string]$File) {
    $full = [IO.Path]::GetFullPath($File)
    $item = Get-Item -LiteralPath $full -Force
    if ($item.PSIsContainer) { throw 'Expected a regular file.' }
    for ($node = $item; $null -ne $node; $node = $node.Parent) {
        if ($node.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Signing paths must not contain links.' }
        if ($node -is [IO.FileInfo]) { $node = $node.Directory; if ($null -eq $node) { break } }
        if ($node.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Signing paths must not contain links.' }
    }
    return $full
}

function Assert-PrismCertificate([System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate) {
    $now = [DateTime]::UtcNow
    if ($now -lt $Certificate.NotBefore.ToUniversalTime() -or $now -ge $Certificate.NotAfter.ToUniversalTime()) { throw 'Signing certificate is not currently valid.' }
    $eku = @($Certificate.Extensions | Where-Object Oid -ne $null | Where-Object { $_.Oid.Value -eq '2.5.29.37' })
    if ($eku.Count -ne 1) { throw 'Exactly one code-signing EKU extension is required.' }
    $purposes = @(([System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]$eku[0]).EnhancedKeyUsages | ForEach-Object Value)
    if ($purposes.Count -ne 1 -or $purposes[0] -ne '1.3.6.1.5.5.7.3.3') { throw 'Certificate must be restricted to code signing.' }
    $basic = @($Certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.19' })
    if ($basic.Count -ne 1 -or ([System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]$basic[0]).CertificateAuthority) { throw 'Signing certificate must explicitly be a non-CA certificate.' }
    $usage = @($Certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.15' })
    if ($usage.Count -ne 1 -or ([System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]$usage[0]).KeyUsages -ne [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature) { throw 'Certificate key usage must be digital signature only.' }
    if (-not $Certificate.HasPrivateKey) { throw 'Signing certificate has no private key.' }
    $key = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Certificate)
    $public = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($Certificate)
    try {
        if ($null -eq $key -or $key.KeySize -lt 2048) { throw 'An RSA signing key of at least 2048 bits is required.' }
        $probe = [Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
        $proof = $key.SignData($probe, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
        if (-not $public.VerifyData($probe, $proof, [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)) { throw 'Certificate and private key do not match.' }
    } finally { if ($key) { $key.Dispose() }; if ($public) { $public.Dispose() } }
}

function Initialize-PrismSignatureVerifier {
    if ('PrismReleaseSignature' -as [type]) { return }
    $references = @(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
    $references += [System.Security.Cryptography.Pkcs.SignedCms].Assembly.Location
    Add-Type -ReferencedAssemblies $references -TypeDefinition @"
#pragma warning disable SYSLIB0057
using System;
using System.IO;
using System.Linq;
using System.Formats.Asn1;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.Pkcs;
using System.Security.Cryptography.X509Certificates;
public static class PrismReleaseSignature {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct FileInfo {
        public uint Size; [MarshalAs(UnmanagedType.LPWStr)] public string Path;
        public IntPtr Handle, Subject;
    }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct TrustData {
        public uint Size; public IntPtr Policy, Sip; public uint UI, Revocation, Choice;
        public IntPtr File; public uint State; public IntPtr StateData, Url;
        public uint Flags, Context; public IntPtr SignatureSettings;
    }
    [DllImport("wintrust.dll", ExactSpelling=true)] static extern int WinVerifyTrust(IntPtr window, ref Guid action, ref TrustData data);
    static void Require(bool value, string message) { if (!value) throw new CryptographicException(message); }
    static uint U32(byte[] b, int p) { Require(p>=0 && p<=b.Length-4,"Truncated PE."); return BitConverter.ToUInt32(b,p); }
    // Intentionally accept only one embedded PKCS#7 signature and an ordinary, contiguous PE.
    // Reject ambiguous certificate tables, overlapping sections and unsigned table padding.
    public static void Verify(string path, byte[] expectedDer, bool requireTimestamp) {
        byte[] b=System.IO.File.ReadAllBytes(path);
        Require(b.Length>=64 && b[0]==77 && b[1]==90,"Not a PE file.");
        int pe=checked((int)U32(b,60)); Require(U32(b,pe)==0x4550,"Invalid PE signature.");
        int opt=checked(pe+24), magic=BitConverter.ToUInt16(b,opt);
        Require(magic==0x10b || magic==0x20b,"Unsupported PE format.");
        int directories=opt+(magic==0x20b?112:96), security=directories+32, checksum=opt+64;
        int optSize=BitConverter.ToUInt16(b,pe+20), sections=BitConverter.ToUInt16(b,pe+6);
        Require(optSize>=(magic==0x20b?152:136) && U32(b,directories-4)>=5,"Missing certificate directory.");
        int headers=checked((int)U32(b,opt+60)), table=checked((int)U32(b,security)), size=checked((int)U32(b,security+4));
        Require(headers>=opt+optSize+sections*40 && headers<=b.Length && table>=headers && size>=8 && table%8==0 && (long)table+size==b.Length,"Invalid certificate bounds.");
        int length=checked((int)U32(b,table)); Require(length>=8 && length<=size && ((length+7L)&~7L)==size && BitConverter.ToUInt16(b,table+4)==0x200 && BitConverter.ToUInt16(b,table+6)==2,"Ambiguous certificate table.");
        for(int i=table+length;i<b.Length;i++) Require(b[i]==0,"Nonzero certificate padding.");
        byte[] signature=b.AsSpan(table+8,length-8).ToArray();
        var cms=new SignedCms(); cms.Decode(signature);
        Require(cms.ContentInfo.ContentType.Value=="1.3.6.1.4.1.311.2.1.4" && cms.SignerInfos.Count==1,"Expected one Authenticode signer.");
        var signer=cms.SignerInfos[0]; Require(signer.DigestAlgorithm.Value=="2.16.840.1.101.3.4.2.1","SHA-256 CMS signer digest required."); Require(signer.Certificate!=null && signer.Certificate.RawData.SequenceEqual(expectedDer),"Signer does not match embedded publisher certificate.");
        cms.CheckSignature(true);
        var reader=new AsnReader(cms.ContentInfo.Content,AsnEncodingRules.DER); var indirect=reader.ReadSequence();
        indirect.ReadEncodedValue(); var digestInfo=indirect.ReadSequence(); var algorithm=digestInfo.ReadSequence();
        Require(algorithm.ReadObjectIdentifier()=="2.16.840.1.101.3.4.2.1","SHA-256 Authenticode digest required.");
        if(algorithm.HasData) algorithm.ReadNull(); algorithm.ThrowIfNotEmpty();
        byte[] expectedHash=digestInfo.ReadOctetString(); digestInfo.ThrowIfNotEmpty(); indirect.ThrowIfNotEmpty(); reader.ThrowIfNotEmpty();
        using(var hash=IncrementalHash.CreateHash(HashAlgorithmName.SHA256)) {
            hash.AppendData(b,0,checksum); hash.AppendData(b,checksum+4,security-checksum-4); hash.AppendData(b,security+8,headers-security-8);
            var ranges=Enumerable.Range(0,sections).Select(i=>opt+optSize+i*40).Where(p=>U32(b,p+16)>0).OrderBy(p=>U32(b,p+20));
            int cursor=headers;
            foreach(int p in ranges) { int start=checked((int)U32(b,p+20)), count=checked((int)U32(b,p+16)); Require(start==cursor && (long)start+count<=table,"Unsupported noncontiguous PE sections."); hash.AppendData(b,start,count); cursor=checked(start+count); }
            hash.AppendData(b,cursor,table-cursor);
            Require(CryptographicOperations.FixedTimeEquals(hash.GetHashAndReset(),expectedHash),"Authenticode content digest mismatch.");
        }
        var timestamps=signer.UnsignedAttributes.Cast<CryptographicAttributeObject>().Where(a=>a.Oid.Value=="1.3.6.1.4.1.311.3.3.1").ToArray();
        if(requireTimestamp || timestamps.Length>0) {
            Require(timestamps.Length==1 && timestamps[0].Values.Count==1,"One RFC3161 timestamp is required.");
            byte[] encoded=timestamps[0].Values[0].RawData; Rfc3161TimestampToken token; int consumed;
            Require(Rfc3161TimestampToken.TryDecode(encoded,out token,out consumed) && consumed==encoded.Length,"Invalid RFC3161 timestamp.");
            Require(token.TokenInfo.HashAlgorithmId.Value=="2.16.840.1.101.3.4.2.1","SHA-256 timestamp required.");
            X509Certificate2 tsa; Require(token.VerifySignatureForSignerInfo(signer,out tsa,token.AsSignedCms().Certificates),"Timestamp signature or message imprint mismatch.");
            DateTime time=token.TokenInfo.Timestamp.UtcDateTime;
            Require(time<=DateTime.UtcNow.AddMinutes(5) && time>=signer.Certificate.NotBefore.ToUniversalTime() && time<=signer.Certificate.NotAfter.ToUniversalTime(),"Timestamp outside signer validity.");
            using(var chain=new X509Chain()) {
                chain.ChainPolicy.RevocationMode=X509RevocationMode.NoCheck;
                chain.ChainPolicy.DisableCertificateDownloads=true; chain.ChainPolicy.VerificationTime=time;
                chain.ChainPolicy.ApplicationPolicy.Add(new Oid("1.3.6.1.5.5.7.3.8"));
                chain.ChainPolicy.ExtraStore.AddRange(token.AsSignedCms().Certificates);
                Require(chain.Build(tsa),"Timestamp authority chain is not trusted in the existing runner trust store.");
            }
        }
        var fi=new FileInfo { Size=(uint)Marshal.SizeOf<FileInfo>(), Path=path };
        IntPtr memory=Marshal.AllocHGlobal(Marshal.SizeOf<FileInfo>());
        var data=new TrustData { Size=(uint)Marshal.SizeOf<TrustData>(),UI=2,Choice=1,File=memory,State=1,Flags=0x1000 };
        var action=new Guid("00AAC56B-CD44-11d0-8CC2-00C04FC295EE");
        try {
            Marshal.StructureToPtr(fi,memory,false); int status=WinVerifyTrust(new IntPtr(-1),ref action,ref data);
            if(status==unchecked((int)0x800B0109)) {
                // Only an explicitly pinned, cryptographically verified self-signed leaf may lack OS trust.
                using(var cert=new X509Certificate2(expectedDer)) using(var chain=new X509Chain()) {
                    Require(cert.SubjectName.RawData.SequenceEqual(cert.IssuerName.RawData),"Untrusted publisher is not self-signed.");
                    chain.ChainPolicy.TrustMode=X509ChainTrustMode.CustomRootTrust; chain.ChainPolicy.CustomTrustStore.Add(cert);
                    chain.ChainPolicy.RevocationMode=X509RevocationMode.NoCheck; chain.ChainPolicy.DisableCertificateDownloads=true;
                    Require(chain.Build(cert),"Invalid self-signed publisher certificate.");
                }
            } else Require(status==0,"WinVerifyTrust rejected the signature: 0x"+unchecked((uint)status).ToString("X8"));
        } finally { data.State=2; WinVerifyTrust(new IntPtr(-1),ref action,ref data); Marshal.DestroyStructure<FileInfo>(memory); Marshal.FreeHGlobal(memory); }
    }
}
"@
}

if ($LibraryOnly) { return }
if (-not $IsWindows) { throw 'Authenticode signing requires Windows PowerShell 7.' }
if (-not $Path -or $Path.Count -eq 0) { throw 'Supply at least one executable.' }
$resolvedPfx = Assert-PrismRegularFile $Pfx
$password = if ($env:PRISM_SIGNING_PASSWORD) { ConvertTo-SecureString $env:PRISM_SIGNING_PASSWORD -AsPlainText -Force } elseif ($env:GITHUB_ACTIONS -eq 'true') { throw 'CI signing password is missing.' } else { Read-Host 'Password for the signing certificate' -AsSecureString }
$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($resolvedPfx, $password, [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet)
try {
    Assert-PrismCertificate $certificate
    Initialize-PrismSignatureVerifier
    $timestampTool = $null
    if (-not $NoTimestamp) {
        # Timestamp only: no PFX path or password is ever passed to a native process.
        $sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
        $timestampTool = Get-ChildItem -LiteralPath $sdk -Directory | Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName 'x64/signtool.exe' } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
        if (-not $timestampTool) { throw 'Windows SDK x64 signtool.exe is required for RFC3161 timestamping.' }
        $timestampTool = Assert-PrismRegularFile $timestampTool
    }
    foreach ($file in $Path) {
        $full = Assert-PrismRegularFile $file
        $null = Set-AuthenticodeSignature -LiteralPath $full -Certificate $certificate -HashAlgorithm SHA256
        if (-not $NoTimestamp) {
            & $timestampTool timestamp /tr $TimestampServer /td SHA256 $full
            if ($LASTEXITCODE -ne 0) { throw "RFC3161 timestamping failed (exit $LASTEXITCODE)." }
        }
        [PrismReleaseSignature]::Verify($full, $certificate.RawData, (-not $NoTimestamp))
        Write-Host "Verified Authenticode: $full (publisher $($certificate.Thumbprint))"
    }
} finally { $certificate.Dispose(); $password.Dispose() }
