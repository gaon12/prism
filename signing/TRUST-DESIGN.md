# Publisher-certificate trust: design

Status: implemented on branch `claude/setup-trust` and tested in the sandbox only. Not part of the
qualified 0.1.0 release. It needs review and the user's go-ahead before it is merged.

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
  for every purpose) or with HTTPS usage is refused. So a leaked key can sign programs but can
  never vouch for a website.
- Before the user decides, the window gets the certificate's identity from `setup_info`:
  `trustCertificate = { subject, sha256, thumbprint, validUntil }`. The SHA-256 is shown in
  `AB:CD:…` form and the thumbprint as Windows shows it. The company can publish these values,
  for example on the intranet, so users or IT can compare them.

## Ownership: who holds the key

- `prism-publisher.pfx` (the private key) stays on the build PC in the git-ignored `signing/`
  folder, with a backup kept offline. CI gets it only through repository secrets.
- Only the public `.cer` is inside setup.
- If the key leaks, make a new certificate, release a new setup and tell IT. The old one can then
  be removed or distrusted. Setup also removes what it added when Prism is uninstalled.

## Consent: when trust is added

| Run | Adds trust? |
|---|---|
| Full window, user did not opt in | No |
| Full window, user opted in (`run_install(..., trustPublisher: true)`) | Yes, then Windows asks again; "No" adds nothing |
| Passive `/P` (the updater) | Never |
| Silent `/S`, `/S /UPDATE` | Never, not even the publisher entry for a root that is already there |

- There are two separate "no"s. Not opting in, or answering "No" to Windows, both still install
  Prism normally.
- The window must show the identity and a real choice. Opting in must be a deliberate action.
  The window must never tell users to just click "Yes".
- Only the current user's lists change (`CurrentUser\Root`, `CurrentUser\TrustedPublisher`). No
  administrator rights are needed and other users are not affected.

## Ownership records and races

- Every trust step runs while setup holds this user's maintenance lock. This is the same lock
  that install, repair, restore and uninstall hold. Inside the lock the step first checks that
  the installation is still the one it was meant for:
  - trusting needs the install folder to still be the registered one;
  - removing needs no Prism to be registered any more.
  If another setup is still busy after about 30 seconds, the step is skipped and logged.
- Setup only claims an entry that its own add created. The Windows add uses `ADD_NEW`. If the
  certificate is already there, for example because IT added it after setup's check, the entry
  is not claimed and is never removed by setup.
- Setup writes the record before it adds (`pending:<list>`) and confirms it afterwards. So an
  entry setup added is never left without a record, even if saving the confirmation fails or
  the process stops in between.
  - The next run keeps a pending entry when the certificate is really in that list, and forgets
    it otherwise.
  - An uninstall treats a pending entry as setup's own.

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
