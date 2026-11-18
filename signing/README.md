# Signing Prism on Windows

Windows Authenticode signing is optional in `.github/workflows/release.yml`. When configured,
the same publisher certificate signs `prism.exe` and setup; setup embeds only its public DER
certificate. The installer offers a separate, unchecked choice to trust that publisher for the
current user. Building or signing never imports a certificate into a Windows certificate store.

| File                  | Contents                            | Handling                                                |
| --------------------- | ----------------------------------- | ------------------------------------------------------- |
| `prism-publisher.pfx` | Certificate and private signing key | Never commit or share. Keep a protected offline backup. |
| `prism-publisher.cer` | Public DER certificate              | May be shared; compare its SHA-256 fingerprint.         |

Certificate files are ignored by git. The scripts, instructions and design document are tracked.
A self-signed certificate does not provide public SmartScreen reputation, and the first downloaded
setup can still show a warning. A compromised private key remains a serious security incident.

## Create a certificate once

```powershell
pwsh signing/new-certificate.ps1
```

This requests a password twice and creates `signing/private-publisher/` with a private ACL,
then exclusively creates its PFX and public CER. That directory must not already exist; the
helper refuses even an existing empty directory. It never overwrites an old publisher or
changes an existing folder's ACL. Its parent must already exist and contain no aliases.
The folder and its certificates are ignored by git. It uses an in-memory RSA key and does not
install a certificate. The certificate has code-signing EKU only, is explicitly not
a CA, and permits only digital signatures. The signing helpers also require current validity,
an RSA key of at least 2048 bits, and a matching private key. Keep the same identity across
releases; changing it requires users to assess a new publisher fingerprint.

Automation calls the script in the same PowerShell process with an existing `SecureString`:

```powershell
& ./signing/new-certificate.ps1 -Password $securePassword -OutputDirectory $newPrivateFolder
```

`$newPrivateFolder` must be a new path inside a protected, ignored location. Never pass a
plaintext password through process arguments, shell history or logs. The helper copies and
disposes its own password/key objects; the caller remains responsible for its original
`SecureString` and backups. The helper does not register GitHub secrets or create backups.
Keep a protected offline copy of both the encrypted PFX and a separately protected recovery
password. A DPAPI-protected local password backup is tied to its Windows user/machine and is
not a portable disaster-recovery backup. Test recovery without changing the production key.

The default validity is five years (`-Years 1..10`), starting five minutes before generation
in UTC to tolerate minor clock skew. Rotate before expiry: an expired certificate cannot sign
new releases. A valid RFC3161 timestamp can preserve Windows verification of a signature made
within validity, but does not renew the certificate, guarantee future Windows policy behavior,
or bypass Prism's validity checks before offering publisher trust.

## Local release order

1. Build `prism.exe`, then run
   `pwsh signing/sign.ps1 src-tauri/target/release/prism.exe -Pfx signing/private-publisher/prism-publisher.pfx`.
2. Build setup with `PRISM_TRUSTED_CERT` pointing to the generated public `.cer`, for example
   `signing/private-publisher/prism-publisher.cer`. The already-signed app bytes become setup's
   payload. An unset variable retains the legacy fallback `signing/prism-publisher.cer`.
3. Run `pwsh signing/sign.ps1 installer/src-tauri/target/release/prism-setup.exe -Pfx signing/private-publisher/prism-publisher.pfx`.
4. Run the native release gates, then copy the setup and package the signed portable app.
5. Apply the **separate Minisign updater signature**, including `--app-version` matching the
   release version (currently `0.1.0`), to the final setup. Verify it against the app's updater
   public key. Generate download checksums and `latest.json` only after all byte-changing steps.

`uninstall.exe` is a copy of the signed setup. Extracted auxiliary service/cleanup executables
are not independently Authenticode-signed by this workflow.

Signing uses PowerShell 7 and the Windows SDK's x64 `signtool.exe`. The private key is loaded
with `EphemeralKeySet`; it is never imported into `My`, `Root` or `TrustedPublisher`.
`Set-AuthenticodeSignature` creates the SHA-256 signature in process. A separate
`signtool timestamp /tr ... /td SHA256` adds the RFC3161 timestamp without passing a password or
PFX to a native command. A public timestamp service receives a signature digest, not the program.
`-NoTimestamp` is available for offline fixtures only; the release workflow never uses it.

After signing, the helper independently verifies the PE content digest, CMS signature, exact
publisher DER, RFC3161 timestamp signature and message imprint, timestamp time, and existing
trusted TSA chain. `WinVerifyTrust` must succeed or return specifically `CERT_E_UNTRUSTEDROOT`
for the pinned, verified self-signed publisher. Generic `UnknownError`, hash mismatches, missing
signatures, missing timestamps and other trust failures do not pass. No publisher is added to
a store to make these checks succeed. Verification intentionally rejects ambiguous certificate
tables and unsupported noncontiguous PE layouts rather than weakening digest validation.

## GitHub Actions configuration

Configure both repository secrets, or neither:

- `WINDOWS_SIGNING_PFX`: Base64 of the PFX. On the machine holding the private key, prepare it with
  `[Convert]::ToBase64String([IO.File]::ReadAllBytes('signing/private-publisher/prism-publisher.pfx')) | Set-Clipboard`.
- `PRISM_SIGNING_PASSWORD`: the PFX password.

Both absent preserves the unsigned build. A partially configured pair fails before app build.
These secrets are independent from `TAURI_SIGNING_PRIVATE_KEY` and its password, which protect
updater downloads through Minisign. Authenticode alone does not satisfy the updater-signing gate.

The release workflow now wires preparation → app build → app Authenticode → setup build with
public CER → setup Authenticode → native gates and configured-certificate sandbox trust tests →
packaging → version-bound Minisign → checksum generation. The existing platform matrix and
Windows-only build/no-publish behavior remain in place.

Preparation refuses local and self-hosted execution. On a disposable GitHub-hosted Windows
runner it creates a fresh nonce directory under `RUNNER_TEMP`, restricts its ACL to the runner
identity, and writes PFX/CER exclusively. PFX files never enter the checkout, Cargo cache or
uploaded artifacts. `PRISM_TRUSTED_CERT` names the exported public DER only. Failure during
preparation cleans up immediately; an `always()` workflow step removes the known owned files
after later successes or failures. Cleanup verifies the directory nonce, marker, regular files
and absence of aliases, never recursively deletes, and refuses unexpected entries. A runner
terminated before its cleanup step relies on disposal of the hosted VM; do not use this on a
persistent runner.

Windows helper fixtures run even when signing secrets are absent:

```powershell
pwsh tests/scripts/signing-helpers.Tests.ps1
# Optional: also exercise the real public RFC3161 service with a disposable fixture key/PE copy.
pwsh tests/scripts/signing-helpers.Tests.ps1 -OnlineTimestamp
```

The fixtures use ephemeral test certificates and nonce-owned copies of an inert system PE.
They never import certificates or sign a production artifact. Native trust tests use setup's
sandbox JSON stores and compare the real stores before/after. A fresh successful workflow run
is required to qualify the integrated candidate; an older successful artifact is not evidence
for these new signing/trust changes.

## User trust and key rotation

The setup window shows subject, SHA-256 fingerprint and expiry before the user decides. The
trust option starts unchecked. Only explicit opt-in may request Windows confirmation for the
current user's `Root` and `TrustedPublisher` stores; silent installs and updates never add trust.
Uninstall removes only trust records owned by setup. See [TRUST-DESIGN.md](TRUST-DESIGN.md).

If the key expires or leaks, create a replacement certificate and distribute its new fingerprint
through a trusted channel. For a leak, notify IT and revoke/remove the old trust on affected PCs.
A publicly trusted signing service/certificate is a separate distribution decision.
