//! Trusting Prism's own publisher certificate on this PC.
//!
//! Prism is used inside a company without a paid code-signing certificate. Its programs are
//! signed with a self-made certificate instead, and setup makes *this user's* Windows trust
//! exactly that certificate, so that signed Prism programs show a known publisher:
//!
//! - Only the one certificate built into setup (`PRISM_TRUSTED_CERT`, see `signing/README.md`)
//!   is ever added, and only if it is limited to code signing. A certificate that could also
//!   vouch for websites (HTTPS) or for anything else is refused, so even a leaked key could not
//!   be used to fake secure websites.
//! - Only the current user's lists are changed (no administrator rights). Windows itself asks
//!   the user once ("Do you want to install this certificate?"); silent runs never cause that
//!   question and skip the step instead.
//! - Setup remembers which lists *it* added the certificate to, and the uninstaller removes it
//!   from exactly those. A certificate the user or IT had already added is left alone.
//!
//! The steps below are written once against [`CertStores`]: the real Windows certificate
//! lists in [`windows_store`], and a JSON file for the test sandbox in [`FileStores`].

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::PathBuf};

/// The two per-user certificate lists ("stores"). `Root` makes Windows accept a self-made
/// certificate at all; `TrustedPublisher` makes it accept that publisher's programs. Root is
/// added first and removed last, so the publisher entry never exists without its root.
pub const STORES: [&str; 2] = ["Root", "TrustedPublisher"];

/// Object identifier of the "Code Signing" purpose.
pub const CODE_SIGNING: &str = "1.3.6.1.5.5.7.3.3";

/// What happened when setup tried to trust its certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustResult {
    /// This setup was built without a certificate.
    NotConfigured,
    /// Both lists already had it.
    AlreadyTrusted,
    /// Setup added it to at least one list.
    Added,
    /// Adding would show Windows' question, which silent and passive runs must not do.
    NeedsConsent,
    /// The user answered "No" to Windows' question.
    Declined,
}

/// Certificate lists as the trust steps need them.
pub trait CertStores {
    /// A stable identifier of the certificate (its thumbprint).
    fn id(&self, certificate: &[u8]) -> io::Result<String>;
    fn contains(&self, store: &str, certificate: &[u8]) -> io::Result<bool>;
    /// Adds the certificate. `Ok(false)` when the user declined Windows' question.
    fn add(&self, store: &str, certificate: &[u8]) -> io::Result<bool>;
    /// Removes the certificate with this id. `Ok(false)` when the user declined.
    fn remove(&self, store: &str, id: &str) -> io::Result<bool>;
    /// Certificates setup added earlier, with the lists it added each one to.
    fn added(&self) -> io::Result<BTreeMap<String, Vec<String>>>;
    /// Remembers (or, with an empty list, forgets) which lists setup added `id` to.
    fn set_added(&self, id: &str, stores: &[String]) -> io::Result<()>;
}

/// Adds `certificate` to this user's trusted lists. `ask`: Windows' question may be shown.
pub fn trust(stores: &dyn CertStores, certificate: &[u8], ask: bool) -> io::Result<TrustResult> {
    if certificate.is_empty() {
        return Ok(TrustResult::NotConfigured);
    }
    check_code_signing_only(certificate)?;
    let id = stores.id(certificate)?;
    let mut ours = stores.added()?.remove(&id).unwrap_or_default();
    let mut result = TrustResult::AlreadyTrusted;
    for store in STORES {
        if stores.contains(store, certificate)? {
            continue;
        }
        // Only the root list makes Windows ask; never ask in a silent run.
        if store == "Root" && !ask {
            return Ok(TrustResult::NeedsConsent);
        }
        if !stores.add(store, certificate)? {
            return Ok(TrustResult::Declined);
        }
        // Remember right away, so an uninstall can undo it even if a later step fails.
        if !ours.iter().any(|name| name == store) {
            ours.push(store.to_owned());
        }
        stores.set_added(&id, &ours)?;
        result = TrustResult::Added;
    }
    Ok(result)
}

/// Removes what [`trust`] added, and nothing else. Lists that would make Windows ask are kept
/// (and remembered) when `ask` is false or the user declines, so a later uninstall can retry.
pub fn untrust(stores: &dyn CertStores, ask: bool) -> io::Result<()> {
    for (id, added) in stores.added()? {
        let mut kept = Vec::new();
        // Reverse of STORES: the publisher entry goes before its root.
        for store in STORES.iter().rev() {
            if !added.iter().any(|name| name == store) {
                continue;
            }
            let removed = if *store == "Root" && !ask {
                false
            } else {
                stores.remove(store, &id)?
            };
            if !removed {
                kept.push((*store).to_owned());
            }
        }
        stores.set_added(&id, &kept)?;
    }
    Ok(())
}

/// Refuses any certificate that is not limited to code signing. A certificate without the
/// purpose list ("extended key usage") is valid for *every* purpose, so it is refused too.
#[cfg(windows)]
pub fn check_code_signing_only(certificate: &[u8]) -> io::Result<()> {
    let purposes = windows_store::purposes(certificate)?;
    if purposes.is_empty() || purposes.iter().any(|purpose| purpose != CODE_SIGNING) {
        return Err(io::Error::other(
            "The built-in certificate must be limited to code signing; it was not trusted.",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn check_code_signing_only(_certificate: &[u8]) -> io::Result<()> {
    Err(io::Error::other(
        "Certificates are only trusted on Windows.",
    ))
}

/// The sandbox's certificate lists: one JSON file inside the sandbox, never the real lists.
pub struct FileStores {
    pub file: PathBuf,
    /// Pretend the user answers "No" to Windows' root question (tests).
    pub decline_root: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct FileState {
    stores: BTreeMap<String, Vec<String>>,
    added: BTreeMap<String, Vec<String>>,
}

impl FileStores {
    fn load(&self) -> io::Result<FileState> {
        crate::safe_path::no_links(&self.file)?;
        match std::fs::read(&self.file) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileState::default()),
            Err(error) => Err(error),
        }
    }
    fn save(&self, state: &FileState) -> io::Result<()> {
        crate::safe_path::no_links(&self.file)?;
        std::fs::write(&self.file, serde_json::to_vec_pretty(state)?)
    }
}

impl CertStores for FileStores {
    fn id(&self, certificate: &[u8]) -> io::Result<String> {
        use sha2::{Digest, Sha256};
        Ok(format!("{:x}", Sha256::digest(certificate)))
    }
    fn contains(&self, store: &str, certificate: &[u8]) -> io::Result<bool> {
        let id = self.id(certificate)?;
        Ok(self
            .load()?
            .stores
            .get(store)
            .is_some_and(|ids| ids.contains(&id)))
    }
    fn add(&self, store: &str, certificate: &[u8]) -> io::Result<bool> {
        if store == "Root" && self.decline_root {
            return Ok(false);
        }
        let id = self.id(certificate)?;
        let mut state = self.load()?;
        let ids = state.stores.entry(store.to_owned()).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
        self.save(&state)?;
        Ok(true)
    }
    fn remove(&self, store: &str, id: &str) -> io::Result<bool> {
        if store == "Root" && self.decline_root {
            return Ok(false);
        }
        let mut state = self.load()?;
        if let Some(ids) = state.stores.get_mut(store) {
            ids.retain(|known| known != id);
        }
        self.save(&state)?;
        Ok(true)
    }
    fn added(&self) -> io::Result<BTreeMap<String, Vec<String>>> {
        Ok(self.load()?.added)
    }
    fn set_added(&self, id: &str, stores: &[String]) -> io::Result<()> {
        let mut state = self.load()?;
        if stores.is_empty() {
            state.added.remove(id);
        } else {
            state.added.insert(id.to_owned(), stores.to_vec());
        }
        self.save(&state)
    }
}

/// The real per-user certificate lists of Windows.
#[cfg(windows)]
pub mod windows_store {
    use super::*;
    use windows::{core::HSTRING, Win32::Security::Cryptography::*};
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    /// Where setup remembers which certificates it added: value name = thumbprint,
    /// data = the lists, comma separated.
    pub const MARKER_KEY: &str = r"Software\Prism Setup\TrustedCertificates";

    /// The "Operation canceled by the user" error, returned when the user answers "No".
    const CANCELLED: windows::core::HRESULT = windows::core::HRESULT(0x8007_04C7_u32 as i32);

    const ENCODING: CERT_QUERY_ENCODING_TYPE = X509_ASN_ENCODING;

    /// A parsed certificate, freed when dropped.
    struct Cert(*mut CERT_CONTEXT);
    impl Drop for Cert {
        fn drop(&mut self) {
            // SAFETY: the context came from CertCreateCertificateContext or a store search and
            // is freed exactly once.
            unsafe {
                let _ = CertFreeCertificateContext(Some(self.0));
            }
        }
    }

    fn parse(certificate: &[u8]) -> io::Result<Cert> {
        // SAFETY: Windows copies the bytes into the new context.
        let context = unsafe { CertCreateCertificateContext(ENCODING, certificate) };
        if context.is_null() {
            return Err(io::Error::other("The built-in certificate cannot be read."));
        }
        Ok(Cert(context))
    }

    /// The purposes listed in the certificate's own "extended key usage" extension.
    pub fn purposes(certificate: &[u8]) -> io::Result<Vec<String>> {
        let cert = parse(certificate)?;
        let mut size = 0u32;
        // SAFETY: the first call asks for the size, the second fills a buffer of that size.
        unsafe {
            if CertGetEnhancedKeyUsage(
                cert.0,
                CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG.0,
                None,
                &mut size,
            )
            .is_err()
            {
                return Ok(Vec::new());
            }
            // u64 storage keeps the CTL_USAGE header correctly aligned.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
            if CertGetEnhancedKeyUsage(
                cert.0,
                CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG.0,
                Some(usage),
                &mut size,
            )
            .is_err()
            {
                return Ok(Vec::new());
            }
            let usage = &*usage;
            if usage.cUsageIdentifier == 0 {
                return Ok(Vec::new());
            }
            let ids = std::slice::from_raw_parts(
                usage.rgpszUsageIdentifier,
                usage.cUsageIdentifier as usize,
            );
            Ok(ids
                .iter()
                .map(|id| String::from_utf8_lossy(id.as_bytes()).into_owned())
                .collect())
        }
    }

    /// An open per-user certificate list, closed when dropped.
    struct Store(HCERTSTORE);
    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: opened by CertOpenStore and closed once.
            unsafe {
                let _ = CertCloseStore(Some(self.0), 0);
            }
        }
    }

    fn open(name: &str) -> io::Result<Store> {
        let name = HSTRING::from(name);
        // SAFETY: `name` outlives the call; Windows copies what it needs.
        let store = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                CERT_QUERY_ENCODING_TYPE(0),
                None,
                CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_CURRENT_USER),
                Some(name.as_ptr().cast()),
            )
        }
        .map_err(|error| io::Error::other(error.message()))?;
        Ok(Store(store))
    }

    fn sha1(cert: &Cert) -> io::Result<Vec<u8>> {
        let mut hash = [0u8; 20];
        let mut size = hash.len() as u32;
        // SAFETY: the buffer is 20 bytes, the size of a SHA-1 hash.
        unsafe {
            CertGetCertificateContextProperty(
                cert.0,
                CERT_SHA1_HASH_PROP_ID,
                Some(hash.as_mut_ptr().cast()),
                &mut size,
            )
        }
        .map_err(|error| io::Error::other(error.message()))?;
        Ok(hash[..size as usize].to_vec())
    }

    fn find_by_hash(store: &Store, hash: &[u8]) -> Option<Cert> {
        let blob = CRYPT_INTEGER_BLOB {
            cbData: hash.len() as u32,
            pbData: hash.as_ptr() as *mut u8,
        };
        // SAFETY: `blob` points at `hash`, which outlives the call.
        let found = unsafe {
            CertFindCertificateInStore(
                store.0,
                ENCODING,
                0,
                CERT_FIND_SHA1_HASH,
                Some((&blob as *const CRYPT_INTEGER_BLOB).cast()),
                None,
            )
        };
        (!found.is_null()).then_some(Cert(found))
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn unhex(text: &str) -> io::Result<Vec<u8>> {
        (0..text.len())
            .step_by(2)
            .map(|i| {
                text.get(i..i + 2)
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                    .ok_or_else(|| io::Error::other("Bad certificate thumbprint."))
            })
            .collect()
    }

    pub struct WindowsStores;

    impl CertStores for WindowsStores {
        fn id(&self, certificate: &[u8]) -> io::Result<String> {
            sha1(&parse(certificate)?).map(|hash| hex(&hash))
        }
        fn contains(&self, store: &str, certificate: &[u8]) -> io::Result<bool> {
            let hash = sha1(&parse(certificate)?)?;
            Ok(find_by_hash(&open(store)?, &hash).is_some())
        }
        fn add(&self, store: &str, certificate: &[u8]) -> io::Result<bool> {
            let cert = parse(certificate)?;
            let list = open(store)?;
            // SAFETY: both handles are valid; Windows copies the certificate into the store.
            match unsafe {
                CertAddCertificateContextToStore(
                    Some(list.0),
                    cert.0,
                    CERT_STORE_ADD_USE_EXISTING,
                    None,
                )
            } {
                Ok(()) => Ok(true),
                Err(error) if error.code() == CANCELLED => Ok(false),
                Err(error) => Err(io::Error::other(error.message())),
            }
        }
        fn remove(&self, store: &str, id: &str) -> io::Result<bool> {
            let list = open(store)?;
            let Some(found) = find_by_hash(&list, &unhex(id)?) else {
                return Ok(true);
            };
            // CertDeleteCertificateFromStore frees the context itself, even on failure.
            let context = found.0;
            std::mem::forget(found);
            // SAFETY: `context` was found in the open store and is not used afterwards.
            match unsafe { CertDeleteCertificateFromStore(context) } {
                Ok(()) => Ok(true),
                Err(error) if error.code() == CANCELLED => Ok(false),
                Err(error) => Err(io::Error::other(error.message())),
            }
        }
        fn added(&self) -> io::Result<BTreeMap<String, Vec<String>>> {
            let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(MARKER_KEY) {
                Ok(key) => key,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
                Err(error) => return Err(error),
            };
            let mut added = BTreeMap::new();
            for value in key.enum_values() {
                let (name, _) = value?;
                let stores: String = key.get_value(&name)?;
                let stores = stores
                    .split(',')
                    .filter(|store| STORES.contains(store))
                    .map(str::to_owned)
                    .collect();
                added.insert(name.to_ascii_lowercase(), stores);
            }
            Ok(added)
        }
        fn set_added(&self, id: &str, stores: &[String]) -> io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(MARKER_KEY)?;
            if !stores.is_empty() {
                return key.set_value(id, &stores.join(","));
            }
            match key.delete_value(id) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            // Leave no empty key behind once nothing is remembered.
            if key.enum_values().next().is_none() {
                drop(key);
                let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey(MARKER_KEY);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway self-made test certificates (public parts only; their keys were discarded):
    // code signing only, HTTPS only, and one without a purpose list (= every purpose).
    const CODE_ONLY: &str = "308201af30820154a003020102021428b5b71517da52d6156e896861f0f0690b3f3a41300a06082a8648ce3d040302301a3118301606035504030c0f507269736d207465737420636f6465301e170d3236313131373131303931335a170d3336313131343131303931335a301a3118301606035504030c0f507269736d207465737420636f64653059301306072a8648ce3d020106082a8648ce3d03010703420004db8baabe5356fc9bf75163025e589d1ba306883decd27a7d50854d1016ac3530fbf6441ffd1c131ef0a40589f93537f255e4c495ed84d8cb029f95f37d3b7c36a3783076301d0603551d0e04160414beed09432591f4fa2437342dfb2c5b8d38f84a6b301f0603551d23041830168014beed09432591f4fa2437342dfb2c5b8d38f84a6b30160603551d250101ff040c300a06082b06010505070303300c0603551d130101ff04023000300e0603551d0f0101ff040403020780300a06082a8648ce3d04030203490030460221009722e9ce3bb74cf9b389c0b3a67d1d8274f04337ef4489380f65f1a72eada0f5022100fba78b25f3adabac5ad5f867971268ae1ed01ee5d329de4773290fc211225dfb";
    const HTTPS_ONLY: &str = "308201ac30820152a00302010202140951353dbeb0daa1eb55d4aa8358533e72674e72300a06082a8648ce3d04030230193117301506035504030c0e507269736d207465737420746c73301e170d3236313131373131303931335a170d3336313131343131303931335a30193117301506035504030c0e507269736d207465737420746c733059301306072a8648ce3d020106082a8648ce3d03010703420004705d4d53b149dcd21896bb45c56b64666730c88dadb6177d5201969f8a3f891977cf15431703b9426c32fc0e77b746bcaef95940fc5c4ae94aa5055dcb3e86e0a3783076301d0603551d0e0416041416f1d51631b04a317737c79831e7e6ddd40c692e301f0603551d2304183016801416f1d51631b04a317737c79831e7e6ddd40c692e30160603551d250101ff040c300a06082b06010505070301300c0603551d130101ff04023000300e0603551d0f0101ff040403020780300a06082a8648ce3d040302034800304502201d7032cd56e8edc87616f2c381923f224b14335ea764b3106246ef1697e9331d022100fcae9a7045c78c4d060360a4532b3bf8310a6cd3ac93206007ae1e431a58b16c";
    const ANY_PURPOSE: &str = "308201893082012fa003020102021407a893e76c92e1bb45c3dde3f4a973134be926b6300a06082a8648ce3d040302301a3118301606035504030c0f507269736d2074657374206e6f6e65301e170d3236313131373131303931335a170d3336313131343131303931335a301a3118301606035504030c0f507269736d2074657374206e6f6e653059301306072a8648ce3d020106082a8648ce3d030107034200046267bc1a069a0eead886cd118a5e4ca4e4bb5d032abf1fb45e0a48f6f74fa5e5f1542aa6a06da169bbae553a7542a0e8ad5627df8fc1aec2cf9d247d3f48b16aa3533051301d0603551d0e041604142b1824feecdc5baec0f2c8a3919c7ed40ca1db6b301f0603551d230418301680142b1824feecdc5baec0f2c8a3919c7ed40ca1db6b300f0603551d130101ff040530030101ff300a06082a8648ce3d04030203480030450220740f96537e500960b6bd133245a5f674983e08cdcaadb65046f6ce21b529488e0221009e9208c2134109a166ff7cd85ad508063107422e978333836bfae021dfb287b0";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn stores(dir: &tempfile::TempDir) -> FileStores {
        FileStores {
            file: dir.path().join("trust.json"),
            decline_root: false,
        }
    }

    #[cfg(windows)]
    #[test]
    fn only_a_code_signing_certificate_is_accepted() {
        assert_eq!(
            windows_store::purposes(&bytes(CODE_ONLY)).unwrap(),
            [CODE_SIGNING]
        );
        assert!(check_code_signing_only(&bytes(CODE_ONLY)).is_ok());
        assert!(check_code_signing_only(&bytes(HTTPS_ONLY)).is_err());
        assert!(check_code_signing_only(&bytes(ANY_PURPOSE)).is_err());
        assert!(check_code_signing_only(b"not a certificate").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_wrong_certificate_never_reaches_the_lists() {
        let dir = tempfile::tempdir().unwrap();
        let lists = stores(&dir);
        assert!(trust(&lists, &bytes(HTTPS_ONLY), true).is_err());
        assert!(!lists.file.exists());
        assert_eq!(
            trust(&lists, &[], true).unwrap(),
            TrustResult::NotConfigured
        );
    }

    #[cfg(windows)]
    #[test]
    fn setup_adds_once_and_removes_only_what_it_added() {
        let dir = tempfile::tempdir().unwrap();
        let lists = stores(&dir);
        let cert = bytes(CODE_ONLY);
        // A silent run never causes Windows' question.
        assert_eq!(
            trust(&lists, &cert, false).unwrap(),
            TrustResult::NeedsConsent
        );
        assert!(lists.added().unwrap().is_empty());
        assert_eq!(trust(&lists, &cert, true).unwrap(), TrustResult::Added);
        assert!(lists.contains("Root", &cert).unwrap());
        assert!(lists.contains("TrustedPublisher", &cert).unwrap());
        // Updates find it already trusted, also silently.
        assert_eq!(
            trust(&lists, &cert, false).unwrap(),
            TrustResult::AlreadyTrusted
        );
        // A silent uninstall removes the publisher entry but keeps the root for later.
        untrust(&lists, false).unwrap();
        assert!(lists.contains("Root", &cert).unwrap());
        assert!(!lists.contains("TrustedPublisher", &cert).unwrap());
        untrust(&lists, true).unwrap();
        assert!(!lists.contains("Root", &cert).unwrap());
        assert!(lists.added().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn a_root_the_user_already_had_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let lists = stores(&dir);
        let cert = bytes(CODE_ONLY);
        lists.add("Root", &cert).unwrap(); // added by IT, not by setup
        assert_eq!(trust(&lists, &cert, false).unwrap(), TrustResult::Added);
        untrust(&lists, true).unwrap();
        assert!(lists.contains("Root", &cert).unwrap());
        assert!(!lists.contains("TrustedPublisher", &cert).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn declining_windows_question_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let lists = FileStores {
            decline_root: true,
            ..stores(&dir)
        };
        let cert = bytes(CODE_ONLY);
        assert_eq!(trust(&lists, &cert, true).unwrap(), TrustResult::Declined);
        assert!(!lists.contains("TrustedPublisher", &cert).unwrap());
        assert!(lists.added().unwrap().is_empty());
    }
}
