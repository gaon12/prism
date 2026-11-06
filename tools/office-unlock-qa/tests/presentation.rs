use prism_office_unlock_qa::office_unlock::{decrypt, Code, Control, Integrity, Limits};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

// Public python-pptx fixture, independently encrypted by msoffcrypto-tool.
// The password is a published test credential, not user data.
#[test]
fn independent_presentation_and_unicode_password_preserve_the_entire_package() {
    let encrypted =
        include_bytes!("../../../tests/fixtures/encrypted-office/agile-unicode-slides.pptx");
    let original = include_bytes!("../../../tests/fixtures/encrypted-office/reference-slides.pptx");
    let cancel = AtomicBool::new(false);
    let control = Control {
        cancel: &cancel,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    let opened = decrypt(
        encrypted,
        "암호-fixture-密碼🔐",
        Limits::default(),
        &control,
    )
    .unwrap();
    assert_eq!(opened.integrity, Integrity::Verified);
    assert_eq!(opened.package.as_slice(), original);
    assert!(matches!(
        decrypt(encrypted, "암호-fixture-密碼", Limits::default(), &control),
        Err(Code::PasswordRejected)
    ));
}
