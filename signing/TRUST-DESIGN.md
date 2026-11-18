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
IT may distribute setup through managed company channels; Windows execution and reputation policies still apply.

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

- `prism-publisher.pfx` stays in a fresh ACL-restricted private output directory (the helper
  defaults to `signing/private-publisher`, or a private folder outside the checkout). Back up
  the encrypted PFX and retain its password securely. A DPAPI-protected password backup is
  tied to its Windows user profile and is not a substitute for an independent password backup.
  CI receives the key only through repository secrets.
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
| Full window, user opted in (`run_install(..., trustPublisher: true)`) | Only after the explicit choice; Windows may ask again                |
| Passive `/P` (the updater)                                            | Never                                                                |
| Silent `/S`, `/S /UPDATE`                                             | Never, not even the publisher entry for a root that is already there |

- Not opting in or declining an additional Windows prompt does not fail Prism installation.
  Certificate operations are best effort and their actual result is recorded in the setup log.
- The window must show the identity and a real choice. Opting in must be a deliberate action.
  The window must never tell users to just click "Yes".
- Only the current user's lists change (`CurrentUser\Root`, `CurrentUser\TrustedPublisher`). No
  administrator rights are needed and other users are not affected.

## Removal

- Setup records what it added under `HKCU\Software\Prism Setup\TrustedCertificates`. The value
  name is the thumbprint and the data lists the stores.
- Uninstall removes exactly those entries, the publisher entry first. A root that IT or the user
  had added before is never removed.
- A full-window uninstall may remove the root and Windows may request confirmation. A silent
  uninstall preserves the root and its ownership record. A subsequent full uninstall of a
  verified registered installation can clean up an owned root; orphan or unverified installs
  do not receive authority to remove certificates.

## Failure and concurrency boundaries

Certificate completion hooks run while the same per-user maintenance guard remains held.
The installed registration is checked again before adding trust; removal requires a typed
receipt proving the uninstalled folder owned registration and that no new registration has
appeared. Registry read errors do not authorize certificate mutation.

Only an atomic newly-created certificate is marked owned. An entry that IT or another process
added first is not claimed. Durable `Adding`/`Removing` intents are flushed under
`HKCU\Software\Prism Setup\CertificateOperations` before changing a store. File-sandbox state
uses an atomic flushed replacement. If store/ownership-record completion becomes uncertain,
`PENDING_REVIEW` is logged and automatic claim/redeletion is refused. This protects an IT
certificate re-added after an interrupted removal; certificate uncertainty is not advertised
as automatic certificate recovery. It does not roll back a successfully installed app.

### Manual review

The setup log's `PENDING_REVIEW` message lists certificate thumbprints, affected stores and
`Adding`/`Removing` intents. These describe an interrupted operation, not proof of ownership
or proof that a certificate remains installed. Inspect the Current User stores with
`certmgr.msc`: Trusted Root Certification Authorities > Certificates (`Root`) and
Trusted Publishers > Certificates (`TrustedPublisher`). Open each candidate and compare its
full thumbprint, subject and validity with the log and the company's published identity.

IT must decide whether that certificate is still needed by another installation or program
signed by the same publisher. Remove only the exact certificate from a reviewed store when
IT confirms removal is appropriate; do not delete a store or unrelated certificates. Keep
the setup log and export the specific registry values under `CertificateOperations` and
`TrustedCertificates` above before changing them. After reviewing both the actual stores
and ownership history, IT can remove only the reviewed thumbprint's values from these two
keys. Clearing an intent is an explicit administrative decision, not automatic recovery.
Do not clear records before inspection: that loses the evidence needed to distinguish
setup's entries from IT-provisioned or subsequently re-added entries.

Build input must be a single bounded public X.509 DER certificate envelope. PFX, private-key
containers, PEM, extra appended data and malformed DER lengths are rejected before embedding.
Certificate EKU/publisher policy and cryptographic signatures are validated separately.

## UI contract (for the setup window owner)

- `setup_info.trustCertificate`: `null`, or `{ subject, sha256, thumbprint, validUntil }`.
- `run_install({ options, trustPublisher })`: `trustPublisher` is optional. `true` only after
  the user opted in. Leaving it out, or passing `false`, adds nothing.
- Suggested UI: an unchecked option such as "Trust this publisher on this PC", with the subject,
  fingerprint and expiry next to it. Explain that Windows may ask once more and that declining does not block app installation.

## Evidence

- `installer/core` unit tests (`trust::tests`) cover:
  - Key-usage acceptance and refusal, checked with the real Windows API.
  - The displayed identity.
  - Silent runs never adding trust, even with an IT root present.
  - Declining.
  - Add and remove only setup's own entries.
- `tests/native/setup-trust.mjs` runs the real setup in its sandbox. It covers no opt-in,
  opt-in with window uninstall, decline, and silent install, update and uninstall. It checks that
  the real Root, TrustedPublisher and My lists are unchanged. Opt-in and decline use the
  actual identity/choice controls; decline is explicitly a file-store sandbox simulation,
  not evidence of a real Windows confirmation dialog.
