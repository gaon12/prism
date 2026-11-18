# Publisher-certificate trust: design

Status: integrated into the Windows 0.1.0 release candidate, including optional Authenticode
workflow wiring and sandbox trust qualification. This is not a claim that the new candidate has
passed release CI: prior successful artifacts predate these changes. A fresh run must qualify
the exact integrated source and final signed bytes. Development and CI trust tests do not write
to real certificate stores; real user-store consent remains a separate installed-product flow.

## Why

The user asked for this (2026-11-17). Prism is used inside a company with no budget for a
code-signing certificate. Users who are not technical complain about unknown-publisher warnings.
The user decided that setup should make Windows trust Prism's own self-made certificate.

What this does **not** fix: the SmartScreen warning on the very first run of a downloaded
setup. That warning is based on download reputation, and setup cannot act before it runs.
Copying setup from a company share usually avoids it.

## Identity: which certificate

- Exactly one certificate is built into setup at build time: `PRISM_TRUSTED_CERT`, or
  `signing/prism-publisher.cer`. Without one, the feature is off: `trustCertificate` is `null`
  and nothing is ever touched.
- It must be limited to code signing. Its extended key usage is exactly `1.3.6.1.5.5.7.3.3`.
  `new-certificate.ps1` also marks it critical, makes it not a CA and limits key usage to
  digital signature. Setup checks the key usage before showing or adding the certificate, and
  `from-ci-secret.ps1` checks it before a CI build. A certificate without a key-usage list (valid
  for every purpose) or with HTTPS usage is refused. The private key must still be protected;
  disclosure requires replacement and removal of the compromised publisher trust.
- Before the user decides, the window gets the certificate's identity from `setup_info`:
  `trustCertificate = { subject, sha256, thumbprint, validUntil }`. The SHA-256 is shown in
  `AB:CD:…` form and the thumbprint as Windows shows it. The company can publish these values,
  for example on the intranet, so users or IT can compare them.

## Ownership: who holds the key

- `prism-publisher.pfx` (the private key) stays on the build PC in the git-ignored `signing/`
  folder, with a backup kept offline. CI gets it only through repository secrets.
- Only the public DER `.cer` is inside setup. CI exports it from the validated PFX and confirms
  it contains no private key and matches the signer exactly. The PFX is never an embed input.
- On disposable hosted CI, the optional PFX/password secrets are materialized only in a fresh
  ACL-restricted nonce directory under `RUNNER_TEMP`, with exclusive file creation. The helper
  immediately cleans failed preparation; an always-running final step deletes only its known
  owned files. Nothing from this directory is cached or uploaded. No signing helper adds trust.
- App Authenticode signing precedes setup payload creation; setup Authenticode signing precedes
  native tests, packaging, the independent version-bound Minisign updater signature and hashes.
  RFC3161 timestamps, publisher pins and PE content digests are verified without trusting the
  publisher on the runner. A self-signed untrusted-root error is narrowly distinguished from
  corrupt/missing signatures and other trust failures. See `signing/README.md`.
- If the key leaks, make a new certificate, release a new setup and tell IT. The old one can then
  be removed or distrusted. Setup also removes what it added when Prism is uninstalled.

## Consent: when trust is added

| Run                                                                   | Adds trust?                                                          |
| --------------------------------------------------------------------- | -------------------------------------------------------------------- |
| Full window, user did not opt in                                      | No                                                                   |
| Full window, user opted in (`run_install(..., trustPublisher: true)`) | Yes, then Windows asks again; "No" adds nothing                      |
| Passive `/P` (the updater)                                            | Never                                                                |
| Silent `/S`, `/S /UPDATE`                                             | Never, not even the publisher entry for a root that is already there |

- There are two separate "no"s. Not opting in, or answering "No" to Windows, both still install
  Prism normally.
- The window must show the identity and a real choice. Opting in must be a deliberate action.
  The window must never tell users to just click "Yes".
- Only the current user's lists change (`CurrentUser\Root`, `CurrentUser\TrustedPublisher`). No
  administrator rights are needed and other users are not affected.

## Removal

- Setup records what it added under `HKCU\Software\Prism Setup\TrustedCertificates`. The value
  name is the thumbprint and the data lists the stores.
- Uninstall removes exactly those entries, the publisher entry first. A root that IT or the user
  had added before is never removed.
- A full-window uninstall may remove the root, and Windows asks first. A silent uninstall
  removes only the publisher entry, keeps the root and remembers it, so a later uninstall can
  remove it.

## UI contract (for the setup window owner)

- `setup_info.trustCertificate`: `null`, or `{ subject, sha256, thumbprint, validUntil }`.
- `run_install({ options, trustPublisher })`: `trustPublisher` is optional. `true` only after
  the user opted in. Leaving it out, or passing `false`, adds nothing.
- Suggested UI: an unchecked option such as "Trust this publisher on this PC", with the subject,
  fingerprint and expiry next to it. Explain that Windows will ask once more and that "No" is
  fine.

## Evidence

- `installer/core` unit tests (`trust::tests`) cover:
  - Key-usage acceptance and refusal, checked with the real Windows API.
  - The displayed identity.
  - Silent runs never adding trust, even with an IT root present.
  - Declining.
  - Add and remove only setup's own entries.
- `tests/native/setup-trust.mjs` runs the real setup in its sandbox. It covers no opt-in,
  opt-in with window uninstall, decline, and silent install, update and uninstall. It checks that
  the real certificate lists are unchanged.
