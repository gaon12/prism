# Signing Prism (self-made certificate)

Prism is signed with a certificate we make ourselves, because we do not buy one. Setup asks each
user's Windows to trust that certificate, so signed Prism programs then show a known publisher.

Only the files listed in `.gitignore` here (this README and the two scripts) are in git. The
certificate files stay in this folder on the build PC and are never committed.

| File | What it is | Share? |
|---|---|---|
| `prism-publisher.pfx` | certificate + private key, used to sign | **Never.** Back it up offline. |
| `prism-publisher.cer` | public certificate, built into setup | Fine, it is public. |

## Once: create the certificate

```powershell
pwsh signing/new-certificate.ps1            # asks for a .pfx password
```

The certificate is limited to code signing and cannot issue other certificates, so even if the
key leaked it could not be used to fake websites. Setup refuses any other kind of certificate.
Nothing is added to the Windows certificate lists of the PC that creates it.

## Every release

1. Build the app, then sign it:
   `pwsh signing/sign.ps1 src-tauri/target/release/prism.exe`
2. Build setup. It picks up `signing/prism-publisher.cer` by itself (or the file named by the
   `PRISM_TRUSTED_CERT` environment variable) and prints
   `Setup will offer to trust the publisher certificate.`
3. Sign setup: `pwsh signing/sign.ps1 installer/src-tauri/target/release/prism-setup.exe`
4. Only now make `latest.json` (`scripts/make-latest-json.mjs`): its hashes must be of the
   signed files.

`uninstall.exe` is a copy of the signed setup, so it is signed too.

## In CI (GitHub Actions)

Add two repository secrets once:

- `WINDOWS_SIGNING_PFX`: the .pfx as Base64. Make it with
  `[Convert]::ToBase64String([IO.File]::ReadAllBytes('signing/prism-publisher.pfx')) | Set-Clipboard`
- `PRISM_SIGNING_PASSWORD`: its password.

Without the secrets the build runs unsigned exactly as before. The Windows job then needs these
steps (`release.yml`):

```yaml
      # right after "Build Windows app"
      - name: Prepare the code-signing certificate (optional)
        id: signing
        if: runner.os == 'Windows'
        shell: pwsh
        env:
          WINDOWS_SIGNING_PFX: ${{ secrets.WINDOWS_SIGNING_PFX }}
          PRISM_SIGNING_PASSWORD: ${{ secrets.PRISM_SIGNING_PASSWORD }}
        run: signing/from-ci-secret.ps1 >> $env:GITHUB_OUTPUT
      - name: Sign the Windows app
        if: steps.signing.outputs.configured == 'true'
        shell: pwsh
        env:
          PRISM_SIGNING_PASSWORD: ${{ secrets.PRISM_SIGNING_PASSWORD }}
        run: signing/sign.ps1 src-tauri/target/release/prism.exe
      # "Build the Windows setup" then embeds signing/prism-publisher.cer automatically.
      # right after "Build the Windows setup", before any setup test or copy
      - name: Sign the Windows setup
        if: steps.signing.outputs.configured == 'true'
        shell: pwsh
        env:
          PRISM_SIGNING_PASSWORD: ${{ secrets.PRISM_SIGNING_PASSWORD }}
        run: signing/sign.ps1 installer/src-tauri/target/release/prism-setup.exe
      # with the other setup tests (needs the WebView debug-port policy step before it)
      - name: Verify publisher-certificate trust in the setup sandbox
        if: steps.signing.outputs.configured == 'true'
        env:
          PRISM_SETUP_EXE: ${{ github.workspace }}/installer/src-tauri/target/release/prism-setup.exe
          PRISM_SETUP_EXPECTED_EXE: ${{ github.workspace }}/src-tauri/target/release/prism.exe
          PRISM_SETUP_TRUSTED_CERT: ${{ github.workspace }}/signing/prism-publisher.cer
        run: node tests/native/setup-trust.mjs
```

The updater signature and the hashes in `latest.json` come after these steps, so they cover the
signed files.

## What users see

- **First install:** Windows SmartScreen may still warn once ("Windows protected your PC" →
  *More info* → *Run anyway*). It judges downloads by reputation, which a self-made certificate
  does not have. Copying setup from a company network share usually avoids the warning.
- **During that install:** the window shows the certificate's name, SHA-256 fingerprint and
  expiry, and offers an unchecked option to trust this publisher on this PC. Only if the user
  opts in does Windows ask *"Do you want to install this certificate?"*. Not opting in, or
  answering "No" there, is fine: Prism installs either way, just without the trusted
  publisher. Publish the fingerprint (e.g. on the intranet) so people can compare it.
- **Updates** never add trust, also not for a certificate IT already added. Trust only grows
  through that visible choice. See `TRUST-DESIGN.md`.
- **Uninstall** removes the certificate again, but only if setup added it. Windows asks once
  more before removing it from the trusted roots.

The certificate goes into the user's own lists (`Root` and `TrustedPublisher` of the current
user). No administrator rights are needed. Setup remembers what it added under
`HKCU\Software\Prism Setup\TrustedCertificates`.

## When the certificate expires or leaks

Make a new one (move the old files away first) and release a new setup. Users who install it
get asked to trust the new certificate. If the key **leaked**, also tell IT, so the old
certificate can be removed or distrusted on company PCs.

## Later: public release

For public downloads, prefer a certificate that Windows trusts by default. SignPath Foundation
offers one free for open-source projects, which needs an open-source license first. Paid
options are Azure Trusted Signing and OV certificates.
