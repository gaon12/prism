//! Explicit OS stores, without changing keyring's process-wide default store.
//! No file fallback and no enumeration of unrelated credentials.
use super::*;
use keyring_core::api::CredentialStoreApi;
use std::sync::Mutex;

pub(super) struct NativeProvider {
    service: String,
    operation: Mutex<()>,
}
impl NativeProvider {
    pub fn new() -> Self {
        Self {
            service: "app.prism.desktop.generation.v1".into(),
            operation: Mutex::new(()),
        }
    }
    fn entry(&self, id: &str) -> Result<keyring_core::Entry, SecretError> {
        validate_id(id)?;
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new().map_err(failure)?;
        #[cfg(target_os = "linux")]
        let store = zbus_secret_service_keyring_store::Store::new().map_err(failure)?;
        // The credential ID already includes the persistent profile UUID, provider
        // slot and normalized endpoint hash. Neither URLs nor keys are labels.
        store.build(&self.service, id, None).map_err(failure)
    }
}
fn unavailable() -> SecretError {
    SecretError::new(SecretErrorCode::Storage,
        "Secure credential storage is unavailable or locked. Unlock your system keychain and try again. Existing credentials were preserved.")
}
fn failure(_error: keyring_core::Error) -> SecretError {
    unavailable()
}
fn invalid_secret() -> SecretError {
    SecretError::new(
        SecretErrorCode::Protection,
        "The stored API key is invalid. Enter it again in this profile.",
    )
}
impl SecretProvider for NativeProvider {
    fn support(&self) -> SecretSupport {
        SecretSupport {
            supported: true,
            user_bound: true,
            provider: if cfg!(target_os = "macos") {
                "macos-keychain"
            } else {
                "linux-secret-service"
            }
            .into(),
        }
    }
    fn set(&self, id: &str, value: &SecretValue) -> Result<(), SecretError> {
        let _operation = self.operation.lock().map_err(|_| unavailable())?;
        self.entry(id)?
            .set_secret(value.expose().as_bytes())
            .map_err(failure)
    }
    fn get(&self, id: &str) -> Result<Option<SecretValue>, SecretError> {
        let _operation = self.operation.lock().map_err(|_| unavailable())?;
        let bytes = match self.entry(id)?.get_secret() {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(keyring_core::Error::NoEntry) => return Ok(None),
            Err(error) => return Err(failure(error)),
        };
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(invalid_secret());
        }
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| invalid_secret())?
            .to_owned();
        SecretValue::new(value)
            .map(Some)
            .map_err(|_| invalid_secret())
    }
    fn remove(&self, id: &str) -> Result<(), SecretError> {
        let _operation = self.operation.lock().map_err(|_| unavailable())?;
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(failure(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn os_failure_messages_do_not_expose_backend_details() {
        let error = unavailable();
        assert_eq!(error.code, SecretErrorCode::Storage);
        assert!(error.message.contains("Unlock"));
        assert!(error.message.contains("preserved"));
    }
    #[test]
    #[ignore = "Requires PRISM_TEST_OS_CREDENTIALS=1 and an accessible disposable OS keychain/session"]
    fn native_store_roundtrip_replacement_isolation_and_delete() {
        assert_eq!(
            std::env::var("PRISM_TEST_OS_CREDENTIALS").as_deref(),
            Ok("1")
        );
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let nonce = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let provider = Arc::new(NativeProvider {
            service: format!("app.prism.platform-qa.{nonce}"),
            operation: Mutex::new(()),
        });
        struct Cleanup(Arc<NativeProvider>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.remove("synthetic_a");
                let _ = self.0.remove("synthetic_b");
            }
        }
        let _cleanup = Cleanup(Arc::clone(&provider));
        assert!(provider.get("synthetic_a").unwrap().is_none());
        provider
            .set(
                "synthetic_a",
                &SecretValue::new("synthetic-first-key".into()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            provider.get("synthetic_a").unwrap().unwrap().expose(),
            "synthetic-first-key"
        );
        assert!(provider.get("synthetic_b").unwrap().is_none());
        provider
            .set(
                "synthetic_a",
                &SecretValue::new("synthetic-second-key".into()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            provider.get("synthetic_a").unwrap().unwrap().expose(),
            "synthetic-second-key"
        );
        provider.remove("synthetic_a").unwrap();
        provider.remove("synthetic_a").unwrap();
        assert!(provider.get("synthetic_a").unwrap().is_none());
    }
}
