# Update artifact verifier

An independent CI tool, not part of the desktop executable. It streams an updater artifact
through the same `minisign-verify` 0.3 library used by Prism's Tauri updater and checks the
authenticated version field after both signatures pass. It needs no private key or network.

```sh
cargo build --manifest-path tools/update-verifier/Cargo.toml --release --locked
# Set PRISM_UPDATER_PUBLIC_KEY to the public key embedded in the app build.
tools/update-verifier/target/release/prism-update-verifier --version 0.2.0 --file path/to/artifact
```

The adjacent `artifact.sig` must use Tauri's base64-wrapped Minisign format. Sign custom
artifacts with `tauri signer sign --app-version <version>`. Current prehashed signatures
are required; old signatures without a signed version are rejected. Files larger than
Prism's 512 MiB updater limit are rejected as well. The tool never starts the artifact.

Tests use the repository's existing public fixtures. They reject changed bytes, wrong keys,
changed or mismatched version comments, unbound signatures, read errors and oversized input.
No production signing material is included. This verifies updater integrity, not Windows
Authenticode, MSIX certificate trust, Apple notarization or successful installation.
