//! CI verifies every updater artifact with the same Minisign library as the app.
//! No private key, installer execution, GUI or network access is needed here.
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use std::io::Read;

pub const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_KEY_BYTES: usize = 8192;
pub const MAX_SIGNATURE_BYTES: usize = 16384;

fn decode(value: &str, limit: usize) -> Result<String, &'static str> {
    if value.len() > limit || value.trim().is_empty() {
        return Err("Invalid public verification data.");
    }
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|_| "Invalid base64 verification data.")?;
    String::from_utf8(bytes).map_err(|_| "Invalid verification text.")
}

pub fn verify(
    reader: impl Read,
    public_key: &str,
    signature: &str,
    version: &str,
) -> Result<(), &'static str> {
    verify_bounded(reader, public_key, signature, version, MAX_ARTIFACT_BYTES)
}

fn verify_bounded(
    mut reader: impl Read,
    key: &str,
    signature: &str,
    version: &str,
    limit: u64,
) -> Result<(), &'static str> {
    if version.len() > 128 {
        return Err("Invalid release version.");
    }
    let expected = semver::Version::parse(version).map_err(|_| "Invalid release version.")?;
    let public_key =
        PublicKey::decode(&decode(key, MAX_KEY_BYTES)?).map_err(|_| "Invalid public key.")?;
    let signature = Signature::decode(&decode(signature, MAX_SIGNATURE_BYTES)?)
        .map_err(|_| "Invalid signature.")?;
    // Streaming accepts modern prehashed Minisign signatures produced by the pinned Tauri CLI.
    // The library checks both the artifact signature and the signed trusted comment.
    let mut verifier = public_key
        .verify_stream(&signature)
        .map_err(|_| "Unsupported signature or wrong signing key.")?;
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| "Could not read update artifact.")?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("Update artifact is too large.")?;
        if total > limit {
            return Err("Update artifact is too large.");
        }
        verifier.update(&buffer[..count]);
    }
    if total == 0 {
        return Err("Update artifact is empty.");
    }
    verifier
        .finalize()
        .map_err(|_| "Artifact or trusted-comment signature verification failed.")?;

    // A trusted comment is usable only after cryptographic verification succeeds.
    // Tauri 2.14.1 stores tab-separated key:value fields. We additionally reject duplicates.
    let versions: Vec<_> = signature
        .trusted_comment()
        .split('\t')
        .filter_map(|field| field.strip_prefix("version:"))
        .collect();
    if versions.len() != 1 {
        return Err("Signature must bind exactly one release version.");
    }
    let signed = semver::Version::parse(versions[0].trim_start_matches('v'))
        .map_err(|_| "Invalid signed version.")?;
    if signed != expected {
        return Err("Signature was made for a different release version.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = include_str!("../../../tests/fixtures/updates/public.pub");
    const OTHER_KEY: &str = include_str!("../../../tests/fixtures/updates/other-public.pub");
    const SIGNATURE: &str = include_str!("../../../tests/fixtures/updates/payload.bin.sig");
    const PAYLOAD: &[u8] = include_bytes!("../../../tests/fixtures/updates/payload.bin");

    #[test]
    fn actual_fixture_signature_binds_bytes_key_and_version() {
        verify(PAYLOAD, KEY, SIGNATURE, "0.2.0").unwrap();
        assert!(verify(PAYLOAD, OTHER_KEY, SIGNATURE, "0.2.0").is_err());
        assert!(verify(PAYLOAD, KEY, SIGNATURE, "0.3.0").is_err());
        let mut changed = PAYLOAD.to_vec();
        changed[0] ^= 1;
        assert!(verify(changed.as_slice(), KEY, SIGNATURE, "0.2.0").is_err());
    }

    #[test]
    fn legacy_unbound_signatures_are_never_release_eligible() {
        let legacy = include_str!("../../../tests/fixtures/updates/legacy.bin.sig");
        assert!(verify(PAYLOAD, KEY, legacy, "0.2.0").is_err());
    }

    #[test]
    fn modifying_the_version_comment_cannot_relabel_a_signed_artifact() {
        let text = decode(SIGNATURE, MAX_SIGNATURE_BYTES).unwrap();
        let altered = STANDARD.encode(text.replace("version:0.2.0", "version:0.3.0"));
        assert!(verify(PAYLOAD, KEY, &altered, "0.3.0")
            .unwrap_err()
            .contains("signature verification failed"));
    }

    #[test]
    fn bounds_invalid_text_empty_artifacts_and_read_failures_are_explicit() {
        assert!(verify_bounded(PAYLOAD, KEY, SIGNATURE, "0.2.0", 1)
            .unwrap_err()
            .contains("too large"));
        assert!(verify(&[][..], KEY, SIGNATURE, "0.2.0")
            .unwrap_err()
            .contains("empty"));
        assert!(verify(PAYLOAD, &"a".repeat(MAX_KEY_BYTES + 1), SIGNATURE, "0.2.0").is_err());
        assert!(verify(PAYLOAD, KEY, "not-base64", "0.2.0").is_err());
        assert!(verify(PAYLOAD, KEY, SIGNATURE, "01.2.0").is_err());
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture"))
            }
        }
        assert_eq!(
            verify(Broken, KEY, SIGNATURE, "0.2.0").unwrap_err(),
            "Could not read update artifact."
        );
    }
}
