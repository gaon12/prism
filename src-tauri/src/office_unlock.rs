//! Password-to-open OOXML only. No filename, filesystem, provider, process or
//! password persistence API belongs here. The returned and partial output
//! buffers are zeroized on drop; this is not a process-memory sandbox.
use std::{
    cell::Cell,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    PasswordRejected,
    Corrupt,
    Unsupported,
    Limit,
    Cancelled,
    TimedOut,
}
pub type Result<T> = std::result::Result<T, Code>;
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    pub deadline: Instant,
}
impl Control<'_> {
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Code::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Code::TimedOut)
        } else {
            Ok(())
        }
    }
    fn io(&self) -> io::Result<()> {
        self.check().map_err(|code| {
            io::Error::new(
                if code == Code::TimedOut {
                    io::ErrorKind::TimedOut
                } else {
                    // Interrupted is retried by read_exact. An accepted cancel
                    // must terminate those loops rather than spin forever.
                    io::ErrorKind::ConnectionAborted
                },
                "Office operation stopped.",
            )
        })
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub plaintext_bytes: usize,
    pub spin_count: u32,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: 64 << 20,
            plaintext_bytes: 64 << 20,
            spin_count: 1_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrity {
    Verified,
    /// Standard encryption has a password verifier but no package HMAC.
    NotDefined,
}
// Deliberately no Debug implementation: the package contains document bytes.
pub struct Opened {
    pub package: Zeroizing<Vec<u8>>,
    pub integrity: Integrity,
}
struct Input<'a, 'b> {
    bytes: Cursor<&'a [u8]>,
    control: &'b Control<'b>,
}
impl Read for Input<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.control.io()?;
        self.bytes.read(bytes)
    }
}
impl Seek for Input<'_, '_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.control.io()?;
        self.bytes.seek(position)
    }
}

fn input<'a, 'b>(bytes: &'a [u8], control: &'b Control<'b>) -> Input<'a, 'b> {
    Input {
        bytes: Cursor::new(bytes),
        control,
    }
}
fn io_code(error: io::Error, control: &Control<'_>) -> Code {
    control.check().err().unwrap_or_else(|| match error.kind() {
        io::ErrorKind::OutOfMemory => Code::Limit,
        _ => Code::Corrupt,
    })
}

/// Bound *all* password encryptors before the codec's uncancellable KDF. Do not
/// rely on the forgiving diagnostic classifier to validate hostile XML. This
/// path supports one password recipient; certificates/IRM are not passwords.
fn agile_budget(xml: &[u8], limits: Limits, control: &Control<'_>) -> Result<()> {
    use quick_xml::{events::Event, name::ResolveResult, NsReader};
    const PASSWORD_NS: &[u8] = b"http://schemas.microsoft.com/office/2006/keyEncryptor/password";
    const CERTIFICATE_NS: &[u8] =
        b"http://schemas.microsoft.com/office/2006/keyEncryptor/certificate";
    let mut reader = NsReader::from_reader(xml);
    let mut keys = 0;
    let mut depth = 0usize;
    let mut events = 0;
    loop {
        control.check()?;
        events += 1;
        if events > 16_384 {
            return Err(Code::Limit);
        }
        let (namespace, event) = reader.read_resolved_event().map_err(|_| Code::Corrupt)?;
        let password_key =
            matches!(&namespace, ResolveResult::Bound(value) if value.as_ref() == PASSWORD_NS);
        let certificate_key =
            matches!(&namespace, ResolveResult::Bound(value) if value.as_ref() == CERTIFICATE_NS);
        let opens = matches!(&event, Event::Start(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if opens {
                    depth += 1;
                    if depth > 32 {
                        return Err(Code::Limit);
                    }
                }
                let mut spin = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| Code::Corrupt)?;
                    if attribute.key.as_ref() == b"spinCount" {
                        let value = attribute
                            .decode_and_unescape_value(reader.decoder())
                            .map_err(|_| Code::Corrupt)?;
                        let value: u32 = value.parse().map_err(|_| Code::Corrupt)?;
                        if value > limits.spin_count {
                            return Err(Code::Limit);
                        }
                        spin = Some(value);
                    }
                }
                if element.local_name().as_ref() == b"encryptedKey" {
                    if password_key {
                        keys += 1;
                        if keys > 1 || spin.is_none() {
                            return Err(Code::Unsupported);
                        }
                    } else if !certificate_key {
                        return Err(Code::Unsupported);
                    }
                }
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or(Code::Corrupt)?,
            Event::DocType(_) | Event::GeneralRef(_) => return Err(Code::Corrupt),
            Event::Eof => {
                return if keys == 1 && depth == 0 {
                    Ok(())
                } else {
                    Err(Code::Corrupt)
                };
            }
            _ => {}
        }
    }
}

fn preflight(bytes: &[u8], limits: Limits, control: &Control<'_>) -> Result<(usize, bool)> {
    let mut container =
        cfb::CompoundFile::open(input(bytes, control)).map_err(|error| io_code(error, control))?;
    let mut information = container
        .open_stream("/EncryptionInfo")
        .map_err(|error| io_code(error, control))?;
    let length = information.len();
    if length < 8 {
        return Err(Code::Corrupt);
    }
    if length > 1 << 20 {
        return Err(Code::Limit);
    }
    let mut info = Vec::with_capacity(length as usize);
    information
        .read_to_end(&mut info)
        .map_err(|error| io_code(error, control))?;
    drop(information);
    let major = u16::from_le_bytes([info[0], info[1]]);
    let minor = u16::from_le_bytes([info[2], info[3]]);
    let agile = match (major, minor) {
        (4, 4) => {
            if info[4..8] != 64u32.to_le_bytes() {
                return Err(Code::Corrupt);
            }
            agile_budget(&info[8..], limits, control)?;
            true
        }
        (2..=4, 2) => {
            if limits.spin_count < 50_000 {
                return Err(Code::Limit);
            }
            false
        }
        _ => return Err(Code::Unsupported),
    };
    let mut package = container
        .open_stream("/EncryptedPackage")
        .map_err(|error| io_code(error, control))?;
    if package.len() < 8 {
        return Err(Code::Corrupt);
    }
    if package.len() > bytes.len() as u64 {
        return Err(Code::Limit);
    }
    let mut size = [0; 8];
    package
        .read_exact(&mut size)
        .map_err(|error| io_code(error, control))?;
    let size = u64::from_le_bytes(size);
    if size == 0 {
        return Err(Code::Corrupt);
    }
    if size > limits.plaintext_bytes as u64 {
        return Err(Code::Limit);
    }
    Ok((size as usize, agile))
}

struct Output<'a> {
    bytes: Zeroizing<Vec<u8>>,
    limit: usize,
    control: &'a Control<'a>,
    failure: Cell<Option<Code>>,
}
impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.control.io()?;
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.failure.set(Some(Code::Limit));
            return Err(io::Error::other("Office output exceeds its budget."));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.control.io()
    }
}
fn crypto_code(value: msoffice_crypto::Error) -> Code {
    use msoffice_crypto::Error;
    match value {
        Error::WrongPassword => Code::PasswordRejected,
        Error::UnsupportedAlgorithm { .. } | Error::UnsupportedEncryptionVersion(..) => {
            Code::Unsupported
        }
        Error::PackageTooLarge { .. } => Code::Limit,
        // Integrity/parse/cipher failures never become an accusation that the
        // user entered the wrong password. Never forward third-party messages.
        _ => Code::Corrupt,
    }
}

pub fn decrypt(
    bytes: &[u8],
    password: &str,
    limits: Limits,
    control: &Control<'_>,
) -> Result<Opened> {
    control.check()?;
    if bytes.len() > limits.input_bytes
        || limits.input_bytes > 128 << 20
        || limits.plaintext_bytes > 128 << 20
        || limits.spin_count > 1_000_000
        || password.len() > 1024
    {
        return Err(Code::Limit);
    }
    let classification = msoffice_crypto::classify_from(input(bytes, control));
    control.check()?;
    if classification.container_read != msoffice_crypto::ContainerRead::Opened {
        return Err(Code::Corrupt);
    }
    if classification.document != msoffice_crypto::Document::OoxmlPackage {
        return Err(Code::Unsupported);
    }
    match classification.family {
        msoffice_crypto::Family::Agile | msoffice_crypto::Family::Standard => {}
        msoffice_crypto::Family::Unknown => return Err(Code::Corrupt),
        _ => return Err(Code::Unsupported),
    }
    let (length, agile) = preflight(bytes, limits, control)?;
    let mut output = Output {
        // The declared plaintext bound is checked before allocating; no
        // plaintext-containing reallocation is needed during streaming.
        bytes: Zeroizing::new(Vec::with_capacity(length)),
        limit: length,
        control,
        failure: Cell::new(None),
    };
    let outcome = msoffice_crypto::decrypt_ooxml_to(
        input(bytes, control),
        &mut output,
        password,
        msoffice_crypto::IntegrityPolicy::RequireWhereDefined,
    );
    control.check()?;
    if let Some(code) = output.failure.get() {
        return Err(code);
    }
    let outcome = outcome.map_err(crypto_code)?;
    if output.bytes.len() != length || !output.bytes.starts_with(b"PK\x03\x04") {
        return Err(Code::Corrupt);
    }
    let integrity = if outcome.integrity.is_authenticated() {
        Integrity::Verified
    } else if !agile && outcome.integrity == msoffice_crypto::IntegrityOutcome::NotApplicable {
        Integrity::NotDefined
    } else {
        return Err(Code::Corrupt);
    };
    Ok(Opened {
        package: output.bytes,
        integrity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DOCX: &[u8] =
        include_bytes!("../../tests/fixtures/encrypted-office/example_password.docx");
    const XLSX: &[u8] =
        include_bytes!("../../tests/fixtures/encrypted-office/example_password.xlsx");
    const STANDARD: &[u8] =
        include_bytes!("../../tests/fixtures/encrypted-office/ecma376standard_password.docx");
    const PASSWORD: &str = "Password1234_"; // Public upstream fixture credential.
    fn open(bytes: &[u8], password: &str, limits: Limits) -> Result<Opened> {
        let cancel = AtomicBool::new(false);
        decrypt(
            bytes,
            password,
            limits,
            &Control {
                cancel: &cancel,
                deadline: Instant::now() + Duration::from_secs(10),
            },
        )
    }
    fn failure(result: Result<Opened>) -> Code {
        match result {
            Ok(_) => panic!("Expected a typed failure, not plaintext"),
            Err(code) => code,
        }
    }
    fn edit_stream(bytes: &[u8], name: &str, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut container = cfb::CompoundFile::open(Cursor::new(bytes.to_vec())).unwrap();
        let mut payload = Vec::new();
        container
            .open_stream(name)
            .unwrap()
            .read_to_end(&mut payload)
            .unwrap();
        edit(&mut payload);
        {
            let mut stream = container.open_stream(name).unwrap();
            stream.set_len(payload.len() as u64).unwrap();
            stream.write_all(&payload).unwrap();
        }
        container.into_inner().into_inner()
    }

    #[test]
    fn independent_agile_and_standard_files_match_complete_known_plaintext_bytes() {
        for (encrypted, expected, integrity) in [
            (
                DOCX,
                include_bytes!("../../tests/fixtures/encrypted-office/example.docx").as_slice(),
                Integrity::Verified,
            ),
            (
                XLSX,
                include_bytes!("../../tests/fixtures/encrypted-office/example.xlsx").as_slice(),
                Integrity::Verified,
            ),
            (
                include_bytes!(
                    "../../tests/fixtures/encrypted-office/ecma376agile_sha1_password.xlsx"
                )
                .as_slice(),
                include_bytes!("../../tests/fixtures/encrypted-office/example.xlsx").as_slice(),
                Integrity::Verified,
            ),
            (
                STANDARD,
                include_bytes!(
                    "../../tests/fixtures/encrypted-office/ecma376standard_password_plain.docx"
                )
                .as_slice(),
                Integrity::NotDefined,
            ),
        ] {
            let opened = open(encrypted, PASSWORD, Limits::default()).unwrap();
            assert_eq!(opened.integrity, integrity);
            assert_eq!(opened.package.as_slice(), expected);
            assert_eq!(
                failure(open(encrypted, "wrong-password", Limits::default())),
                Code::PasswordRejected
            );
        }
    }

    #[test]
    fn changed_ciphertext_and_missing_integrity_are_not_wrong_passwords() {
        let changed = edit_stream(DOCX, "/EncryptedPackage", |bytes| bytes[128] ^= 1);
        assert_eq!(
            failure(open(&changed, PASSWORD, Limits::default())),
            Code::Corrupt
        );
        let missing = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            let xml = String::from_utf8(bytes[8..].to_vec()).unwrap();
            let start = xml.find("<dataIntegrity ").unwrap();
            let end = start + xml[start..].find("/>").unwrap() + 2;
            let xml = format!("{}{}", &xml[..start], &xml[end..]);
            bytes.truncate(8);
            bytes.extend_from_slice(xml.as_bytes());
        });
        assert_eq!(
            failure(open(&missing, PASSWORD, Limits::default())),
            Code::Corrupt
        );
    }

    #[test]
    fn certificate_co_recipient_does_not_replace_the_password_parameters() {
        let mixed = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            let xml = String::from_utf8(bytes[8..].to_vec()).unwrap();
            let certificate = "<keyEncryptor uri=\"http://schemas.microsoft.com/office/2006/keyEncryptor/certificate\"><c:encryptedKey xmlns:c=\"http://schemas.microsoft.com/office/2006/keyEncryptor/certificate\" encryptedKeyValue=\"AA==\"/></keyEncryptor>";
            assert!(xml.contains("</keyEncryptors>"));
            bytes.truncate(8);
            bytes.extend_from_slice(
                xml.replace(
                    "</keyEncryptors>",
                    &format!("{certificate}</keyEncryptors>"),
                )
                .as_bytes(),
            );
        });
        let opened = open(&mixed, PASSWORD, Limits::default()).unwrap();
        assert_eq!(opened.integrity, Integrity::Verified);
        assert_eq!(
            opened.package.as_slice(),
            include_bytes!("../../tests/fixtures/encrypted-office/example.docx")
        );
    }

    #[test]
    fn unsupported_algorithm_or_version_is_not_a_rejected_password() {
        let algorithm = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            let xml = String::from_utf8(bytes[8..].to_vec()).unwrap();
            assert!(xml.contains("cipherAlgorithm=\"AES\""));
            bytes.truncate(8);
            bytes.extend_from_slice(
                xml.replace(
                    "cipherAlgorithm=\"AES\"",
                    "cipherAlgorithm=\"unsupported-cipher\"",
                )
                .as_bytes(),
            );
        });
        assert_eq!(
            failure(open(&algorithm, PASSWORD, Limits::default())),
            Code::Unsupported
        );
        let version = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            bytes[..4].copy_from_slice(&[9, 0, 9, 0]);
        });
        assert_eq!(
            failure(open(&version, PASSWORD, Limits::default())),
            Code::Unsupported
        );
    }

    #[test]
    fn file_declared_plaintext_and_kdf_cost_are_bounded_before_decryption() {
        assert_eq!(
            failure(open(
                STANDARD,
                PASSWORD,
                Limits {
                    spin_count: 49_999,
                    ..Default::default()
                }
            )),
            Code::Limit
        );
        assert_eq!(
            failure(open(
                DOCX,
                PASSWORD,
                Limits {
                    input_bytes: DOCX.len() - 1,
                    ..Default::default()
                }
            )),
            Code::Limit
        );
        assert_eq!(
            failure(open(
                DOCX,
                PASSWORD,
                Limits {
                    plaintext_bytes: 4,
                    ..Default::default()
                }
            )),
            Code::Limit
        );
        let inflated = edit_stream(DOCX, "/EncryptedPackage", |bytes| {
            bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        });
        assert_eq!(
            failure(open(&inflated, PASSWORD, Limits::default())),
            Code::Limit
        );
        let excessive = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            let xml = String::from_utf8(bytes[8..].to_vec()).unwrap();
            assert!(xml.contains("spinCount=\"100000\""));
            bytes.truncate(8);
            bytes.extend_from_slice(
                xml.replace("spinCount=\"100000\"", "spinCount=\"1000001\"")
                    .as_bytes(),
            );
        });
        assert_eq!(
            failure(open(&excessive, PASSWORD, Limits::default())),
            Code::Limit
        );
        let invalid = edit_stream(DOCX, "/EncryptionInfo", |bytes| {
            let xml = String::from_utf8(bytes[8..].to_vec()).unwrap();
            bytes.truncate(8);
            bytes.extend_from_slice(
                xml.replace("spinCount=\"100000\"", "spinCount=\"nonnumeric\"")
                    .as_bytes(),
            );
        });
        assert_eq!(
            failure(open(&invalid, PASSWORD, Limits::default())),
            Code::Corrupt
        );
        assert_eq!(
            failure(open(b"truncated input", PASSWORD, Limits::default())),
            Code::Corrupt
        );
    }

    #[test]
    fn xml_depth_entities_and_ambiguous_password_recipients_are_bounded() {
        let cancel = AtomicBool::new(false);
        let control = Control {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let limits = Limits::default();
        assert_eq!(
            agile_budget(b"<!DOCTYPE e [<!ENTITY a 'data'>]><e/>", limits, &control),
            Err(Code::Corrupt)
        );
        assert_eq!(
            agile_budget(
                b"<e xmlns:p='http://schemas.microsoft.com/office/2006/keyEncryptor/password'><p:encryptedKey spinCount='1'/><p:encryptedKey spinCount='2'/></e>",
                limits,
                &control
            ),
            Err(Code::Unsupported)
        );
        assert_eq!(
            agile_budget(
                b"<e><encryptedKey spinCount='1000001'/><encryptedKey spinCount='1'/></e>",
                limits,
                &control
            ),
            Err(Code::Limit)
        );
        let deep = format!(
            "{}<encryptedKey spinCount='0'/>{}",
            "<e>".repeat(33),
            "</e>".repeat(33)
        );
        assert_eq!(
            agile_budget(deep.as_bytes(), limits, &control),
            Err(Code::Limit)
        );
    }

    #[test]
    fn cancellation_and_deadline_never_return_plaintext() {
        let cancel = AtomicBool::new(true);
        let control = Control {
            cancel: &cancel,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            failure(decrypt(DOCX, PASSWORD, Limits::default(), &control)),
            Code::Cancelled
        );
        cancel.store(false, Ordering::Release);
        let control = Control {
            cancel: &cancel,
            deadline: Instant::now() - Duration::from_secs(1),
        };
        assert_eq!(
            failure(decrypt(DOCX, PASSWORD, Limits::default(), &control)),
            Code::TimedOut
        );
    }
}
