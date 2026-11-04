//! User-bound credential storage. Secrets are never serialized as plaintext JSON.
//! OS calls and file I/O block; run store operations on a blocking worker.
use serde::Serialize;
use std::{fmt, path::PathBuf, sync::Arc};
use zeroize::Zeroizing;
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "native_secrets.rs"]
mod native;

const MAX_SECRET_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretErrorCode {
    Unsupported,
    Input,
    Storage,
    Protection,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretError {
    pub code: SecretErrorCode,
    pub message: String,
}
impl SecretError {
    fn new(code: SecretErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for SecretError {}

/// Intentionally not Serialize or Clone. Debug never contains the value.
pub struct SecretValue(Zeroizing<String>);
impl SecretValue {
    pub fn new(value: String) -> Result<Self, SecretError> {
        let value = Zeroizing::new(value);
        if value.is_empty()
            || value.len() > MAX_SECRET_BYTES
            || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(SecretError::new(
                SecretErrorCode::Input,
                "An API key must be a nonempty header-safe value up to 8192 bytes.",
            ));
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSupport {
    pub supported: bool,
    pub provider: String,
    pub user_bound: bool,
}

/// Platform implementations must never fall back to a plaintext file.
pub trait SecretProvider: Send + Sync {
    fn support(&self) -> SecretSupport;
    fn set(&self, id: &str, value: &SecretValue) -> Result<(), SecretError>;
    fn get(&self, id: &str) -> Result<Option<SecretValue>, SecretError>;
    fn remove(&self, id: &str) -> Result<(), SecretError>;
}
#[derive(Clone)]
pub struct SecretStore {
    provider: Arc<dyn SecretProvider>,
}
impl SecretStore {
    pub fn new(directory: PathBuf) -> Self {
        #[cfg(windows)]
        let provider: Arc<dyn SecretProvider> = Arc::new(windows::DpapiProvider::new(
            directory.join("generation-secrets"),
        ));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let provider: Arc<dyn SecretProvider> = {
            let _ = directory;
            Arc::new(native::NativeProvider::new())
        };
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let provider: Arc<dyn SecretProvider> = {
            let _ = directory;
            Arc::new(UnsupportedProvider)
        };
        Self { provider }
    }
    pub fn with_provider(provider: Arc<dyn SecretProvider>) -> Self {
        Self { provider }
    }
    pub fn support(&self) -> SecretSupport {
        self.provider.support()
    }
    pub fn set(&self, id: &str, value: &SecretValue) -> Result<(), SecretError> {
        validate_id(id)?;
        self.provider.set(id, value)
    }
    pub fn get(&self, id: &str) -> Result<Option<SecretValue>, SecretError> {
        validate_id(id)?;
        self.provider.get(id)
    }
    pub fn remove(&self, id: &str) -> Result<(), SecretError> {
        validate_id(id)?;
        self.provider.remove(id)
    }
}
fn validate_id(id: &str) -> Result<(), SecretError> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(SecretError::new(
            SecretErrorCode::Input,
            "Credential identifiers must use 1 to 80 letters, digits, hyphens, or underscores.",
        ));
    }
    Ok(())
}

pub struct UnsupportedProvider;
impl SecretProvider for UnsupportedProvider {
    fn support(&self) -> SecretSupport {
        SecretSupport {
            supported: false,
            provider: "unsupported".into(),
            user_bound: false,
        }
    }
    fn set(&self, _: &str, _: &SecretValue) -> Result<(), SecretError> {
        Err(unsupported())
    }
    fn get(&self, _: &str) -> Result<Option<SecretValue>, SecretError> {
        Err(unsupported())
    }
    fn remove(&self, _: &str) -> Result<(), SecretError> {
        Err(unsupported())
    }
}
fn unsupported() -> SecretError {
    SecretError::new(SecretErrorCode::Unsupported, "Secure credential storage is not implemented for this operating system. No plaintext fallback is used.")
}

#[cfg(windows)]
mod windows {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::windows::ffi::OsStrExt,
        path::Path,
        sync::Mutex,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
        Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
    };
    use zeroize::Zeroize;

    const HEADER: &[u8] = b"PRISM-SECRET\0\x01";
    const MAX_ENCRYPTED_BYTES: usize = 64 * 1024;
    pub struct DpapiProvider {
        directory: PathBuf,
        operation: Mutex<()>,
    }
    impl DpapiProvider {
        pub fn new(directory: PathBuf) -> Self {
            Self {
                directory,
                operation: Mutex::new(()),
            }
        }
    }
    fn storage() -> SecretError {
        SecretError::new(
            SecretErrorCode::Storage,
            "Secure credential storage could not be accessed. Existing credentials were preserved.",
        )
    }
    fn protection() -> SecretError {
        SecretError::new(SecretErrorCode::Protection, "This credential cannot be protected or read for the current Windows user. Enter the key again if it was moved from another account or device.")
    }

    impl SecretProvider for DpapiProvider {
        fn support(&self) -> SecretSupport {
            SecretSupport {
                supported: true,
                provider: "windowsDpapi".into(),
                user_bound: true,
            }
        }
        fn set(&self, id: &str, value: &SecretValue) -> Result<(), SecretError> {
            validate_id(id)?;
            let _operation = self.operation.lock().map_err(|_| storage())?;
            fs::create_dir_all(&self.directory).map_err(|_| storage())?;
            let protected = crypt(value.expose().as_bytes(), id, true)?;
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).map_err(|_| protection())?;
            let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
            let name = file_name(id);
            let temporary = self.directory.join(format!("{name}.{nonce}.part"));
            let destination = self.directory.join(name);
            let result = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|_| storage())?;
                file.write_all(HEADER)
                    .and_then(|_| file.write_all(&protected))
                    .and_then(|_| file.sync_all())
                    .map_err(|_| storage())?;
                drop(file);
                // Atomic replacement preserves the old key if the new write/commit fails.
                let from = wide_path(&temporary)?;
                let to = wide_path(&destination)?;
                if unsafe {
                    MoveFileExW(
                        from.as_ptr(),
                        to.as_ptr(),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                } == 0
                {
                    return Err(storage());
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result
        }
        fn get(&self, id: &str) -> Result<Option<SecretValue>, SecretError> {
            validate_id(id)?;
            let _operation = self.operation.lock().map_err(|_| storage())?;
            let path = self.directory.join(file_name(id));
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(storage()),
            };
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > (MAX_ENCRYPTED_BYTES + HEADER.len()) as u64
            {
                return Err(protection());
            }
            let mut bytes = Vec::new();
            File::open(path)
                .map_err(|_| storage())?
                .take((MAX_ENCRYPTED_BYTES + HEADER.len() + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| storage())?;
            if bytes.len() > MAX_ENCRYPTED_BYTES + HEADER.len() || !bytes.starts_with(HEADER) {
                return Err(protection());
            }
            let plain = Zeroizing::new(crypt(&bytes[HEADER.len()..], id, false)?);
            let value = std::str::from_utf8(&plain)
                .map_err(|_| protection())?
                .to_owned();
            SecretValue::new(value).map(Some).map_err(|_| protection())
        }
        fn remove(&self, id: &str) -> Result<(), SecretError> {
            validate_id(id)?;
            let _operation = self.operation.lock().map_err(|_| storage())?;
            match fs::remove_file(self.directory.join(file_name(id))) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(storage()),
            }
        }
    }
    pub(super) fn file_name(id: &str) -> String {
        // Hash the stable profile identifier: no reserved DOS names, path aliases,
        // or case-insensitive filename collisions between distinct profile IDs.
        format!("key-{:x}.bin", Sha256::digest(id.as_bytes()))
    }
    fn wide_path(path: &Path) -> Result<Vec<u16>, SecretError> {
        let value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(storage());
        }
        Ok(value.into_iter().chain(Some(0)).collect())
    }
    struct OwnedBlob(CRYPT_INTEGER_BLOB);
    impl Drop for OwnedBlob {
        fn drop(&mut self) {
            if !self.0.pbData.is_null() {
                // DPAPI owns this allocation; wipe decrypted bytes before LocalFree.
                unsafe {
                    std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                    LocalFree(self.0.pbData.cast());
                }
            }
        }
    }
    fn crypt(input: &[u8], id: &str, protect: bool) -> Result<Vec<u8>, SecretError> {
        if input.is_empty() || input.len() > MAX_ENCRYPTED_BYTES {
            return Err(protection());
        }
        let entropy_bytes = format!("Prism/generation-credential/v1/{id}");
        let source = CRYPT_INTEGER_BLOB {
            cbData: input.len() as u32,
            pbData: input.as_ptr().cast_mut(),
        };
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy_bytes.len() as u32,
            pbData: entropy_bytes.as_ptr().cast_mut(),
        };
        let mut output = OwnedBlob(CRYPT_INTEGER_BLOB::default());
        let result = unsafe {
            if protect {
                // Deliberately omit CRYPTPROTECT_LOCAL_MACHINE: bind to this user.
                CryptProtectData(
                    &source,
                    std::ptr::null(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output.0,
                )
            } else {
                CryptUnprotectData(
                    &source,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output.0,
                )
            }
        };
        if result == 0
            || output.0.pbData.is_null()
            || output.0.cbData == 0
            || output.0.cbData as usize > MAX_ENCRYPTED_BYTES
        {
            return Err(protection());
        }
        Ok(
            unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) }
                .to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_debug_and_errors_do_not_contain_credentials() {
        let value = SecretValue::new("fake-key-never-use-in-production".into()).unwrap();
        assert!(!format!("{value:?}").contains(value.expose()));
        for invalid in ["", "key\nInjected:header", "non-ascii-키", "with space"] {
            assert!(SecretValue::new(invalid.into()).is_err());
        }
        assert!(validate_id("../../escape").is_err());
        assert!(validate_id("profile:stream").is_err());
    }
    #[test]
    fn unsupported_provider_is_explicit_and_has_no_plaintext_fallback() {
        let store = SecretStore::with_provider(Arc::new(UnsupportedProvider));
        let value = SecretValue::new("fake-only".into()).unwrap();
        assert!(!store.support().supported);
        assert_eq!(
            store.set("default", &value).unwrap_err().code,
            SecretErrorCode::Unsupported
        );
        assert_eq!(
            store.get("default").unwrap_err().code,
            SecretErrorCode::Unsupported
        );
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_replacement_profile_binding_and_delete() {
        let temporary = tempfile::tempdir().unwrap();
        let store = SecretStore::new(temporary.path().to_path_buf());
        assert!(store.support().supported && store.support().user_bound);
        assert!(store.get("profile_a").unwrap().is_none());
        let first = SecretValue::new("fake-key-one-do-not-use".into()).unwrap();
        store.set("profile_a", &first).unwrap();
        let file = temporary
            .path()
            .join("generation-secrets")
            .join(windows::file_name("profile_a"));
        let bytes = std::fs::read(&file).unwrap();
        assert!(!bytes
            .windows(first.expose().len())
            .any(|window| window == first.expose().as_bytes()));
        assert!(bytes.starts_with(b"PRISM-SECRET\0\x01"));
        assert_eq!(
            store.get("profile_a").unwrap().unwrap().expose(),
            first.expose()
        );
        let second = SecretValue::new("fake-key-two-do-not-use".into()).unwrap();
        store.set("profile_a", &second).unwrap();
        assert_eq!(
            store.get("profile_a").unwrap().unwrap().expose(),
            second.expose()
        );
        std::fs::copy(
            &file,
            temporary
                .path()
                .join("generation-secrets")
                .join(windows::file_name("profile_b")),
        )
        .unwrap();
        let error = store.get("profile_b").unwrap_err();
        assert_eq!(error.code, SecretErrorCode::Protection);
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains(second.expose()));
        let moved = temporary.path().join("moved-profile");
        std::fs::create_dir(&moved).unwrap();
        std::fs::rename(
            temporary.path().join("generation-secrets"),
            moved.join("generation-secrets"),
        )
        .unwrap();
        let store = SecretStore::new(moved.clone());
        assert_eq!(
            store.get("profile_a").unwrap().unwrap().expose(),
            second.expose()
        );
        store.remove("profile_a").unwrap();
        store.remove("profile_a").unwrap();
        assert!(store.get("profile_a").unwrap().is_none());
        let file = moved
            .join("generation-secrets")
            .join(windows::file_name("profile_a"));
        std::fs::write(&file, b"not a DPAPI credential").unwrap();
        assert_eq!(
            store.get("profile_a").unwrap_err().code,
            SecretErrorCode::Protection
        );
    }
}
