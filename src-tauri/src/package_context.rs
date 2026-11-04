//! Package identity is queried from Windows, never guessed from an exe path or environment.
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Windows single-instance IPC must not hand a packaged launch to an ordinary installation.
/// Family identity is stable across package versions and installation directories.
pub fn instance_identifier(base: &str, family: &str) -> String {
    let hash = Sha256::digest(family.as_bytes());
    format!("{base}.msix.p{hash:x}")
}

pub fn current_family() -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentPackageFamilyName(length: *mut u32, name: *mut u16) -> i32;
        }
        let mut length = 0u32;
        // SAFETY: first query requests only the required UTF-16 buffer size.
        let result = unsafe { GetCurrentPackageFamilyName(&mut length, std::ptr::null_mut()) };
        if result == 15700 {
            return Ok(None);
        } // APPMODEL_ERROR_NO_PACKAGE
        if result != 122 || !(2..=512).contains(&length) {
            return Err("Could not determine Windows package identity.".into());
        }
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: buffer has the capacity requested by the first call and lives through it.
        let result = unsafe { GetCurrentPackageFamilyName(&mut length, buffer.as_mut_ptr()) };
        if result != 0
            || length == 0
            || length as usize > buffer.len()
            || buffer[length as usize - 1] != 0
        {
            return Err("Could not read Windows package identity.".into());
        }
        let family = String::from_utf16(&buffer[..length as usize - 1])
            .map_err(|_| "Invalid Windows package identity.")?;
        validate_family(&family)?;
        Ok(Some(family))
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

fn validate_family(family: &str) -> Result<(), String> {
    if family.len() > 128
        || family.is_empty()
        || !family
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b".-_".contains(&value))
        || family == "."
        || family == ".."
    {
        return Err("Invalid Windows package identity.".into());
    }
    Ok(())
}

/// Use the package's physical LocalState path so managed data/exclusion paths agree.
pub fn data_directory(local_data: &Path, family: &str) -> Result<PathBuf, String> {
    validate_family(family)?;
    Ok(local_data.join("Packages").join(family).join("LocalState"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_data_cannot_escape_the_package_family_directory() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            data_directory(root.path(), "Prism.Desktop_abcdefghijklm").unwrap(),
            root.path()
                .join("Packages/Prism.Desktop_abcdefghijklm/LocalState")
        );
        for family in ["../other", "C:\\outside", ".", "", "name/other"] {
            assert!(data_directory(root.path(), family).is_err());
        }
    }
    #[test]
    fn instance_identity_is_stable_and_separate_from_other_distribution_formats() {
        let base = "app.prism.desktop";
        let id = instance_identifier(base, "Prism.Desktop_abcdefghijklm");
        assert_eq!(id, instance_identifier(base, "Prism.Desktop_abcdefghijklm"));
        assert_ne!(id, base);
        assert_ne!(
            id,
            instance_identifier(base, "Prism.Desktop_otherpublisher")
        );
    }
    #[test]
    #[cfg(windows)]
    fn ordinary_test_process_has_no_package_identity() {
        assert_eq!(current_family().unwrap(), None);
    }
}
