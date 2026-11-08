//! Pre-GUI supervisor host and inert, opt-in command fixtures for owned tests.
fn main() {
    #[cfg(target_os = "linux")]
    if let Some(code) = prism_platform_qa::pdf_renderer::dispatch() {
        std::process::exit(code);
    }
    if let Some(code) = prism_platform_qa::runtime_supervisor::dispatch() {
        std::process::exit(code);
    }
    #[cfg(unix)]
    if let Some(code) = fixture() {
        std::process::exit(code);
    }
    eprintln!("Use cargo test; this binary hosts only platform QA fixtures.");
    std::process::exit(64);
}

#[cfg(unix)]
fn fixture() -> Option<i32> {
    use std::{
        fs,
        net::TcpListener,
        path::PathBuf,
        process::{Command, Stdio},
        time::Duration,
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(target_os = "linux")]
    if args
        .first()
        .is_some_and(|arg| arg == "--fixture-pdf-parent")
        && args.len() == 2
    {
        use std::io::Read;
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg(prism_platform_qa::pdf_renderer::FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = [0; 9];
        if child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .is_err()
            || &ready != b"PRISMPDF0"
        {
            let _ = child.kill();
            let _ = child.wait();
            return Some(70);
        }
        fs::write(
            &args[1],
            serde_json::to_vec(&serde_json::json!({"pid":child.id()})).unwrap(),
        )
        .unwrap();
        // Retain stdin while waiting: this is the real guarded renderer before
        // a request, not a replacement parser or a fake render response.
        loop {
            if child.try_wait().unwrap().is_some() {
                return Some(70);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(target_os = "linux")]
    if args
        .first()
        .is_some_and(|arg| arg == "--fixture-pdf-collection")
        && args.len() == 2
    {
        use base64::Engine;
        use prism_platform_qa::{
            collection_service::CollectionService,
            collection_store::{ErrorCode, UnitKind, WorkMetadata},
            ocr,
        };
        let directory = PathBuf::from(&args[1]);
        let app = directory.join("app");
        fs::create_dir_all(&app).unwrap();
        let source = directory.join("original.pdf");
        let original = include_bytes!("../../../tests/fixtures/ocr/crop-clockwise.pdf");
        fs::write(&source, original).unwrap();
        assert!(!ocr::Preferences::default().enabled);
        let service = CollectionService::open(&app, |_| {}).unwrap();
        let collection = service.store.create_collection("Actual PDF", "").unwrap();
        let work = service
            .store
            .create_work(&collection.id, "Original page", WorkMetadata::default())
            .unwrap();
        let unit = service
            .store
            .create_unit(&work.id, "Pages", UnitKind::Volume)
            .unwrap();
        let unit = service
            .import(&unit.id, unit.revision, vec![source.clone()])
            .unwrap();
        assert_eq!(unit.page_count, 1);
        let cover = service.cover(&work.id).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(
                cover
                    .data_url
                    .unwrap()
                    .strip_prefix("data:image/webp;base64,")
                    .unwrap(),
            )
            .unwrap();
        fs::write(directory.join("actual-cover.webp"), &bytes).unwrap();
        let page = service
            .store
            .pages(&unit.id, unit.revision, 0, 1)
            .unwrap()
            .items
            .remove(0);
        let media = service.media(&page.id, false).unwrap();
        assert!(media
            .data_url
            .unwrap()
            .starts_with("data:image/webp;base64,"));
        assert_eq!(fs::read(&source).unwrap(), original);
        drop(service);
        let service = CollectionService::open(&app, |_| {}).unwrap();
        assert!(service.cover(&work.id).unwrap().data_url.is_some());
        assert!(!app.join("ocr").exists());
        assert!(!app.join("content").exists());
        fs::write(&source, b"changed own fixture").unwrap();
        assert_eq!(
            service.cover(&work.id).unwrap_err().code,
            ErrorCode::SourceChanged
        );
        println!("Actual Linux PDF collection import, WebP cover, reader, cache reopen and stale-source checks passed; OCR remains disabled");
        return Some(0);
    }
    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    if args
        .first()
        .is_some_and(|arg| arg == "--fixture-paddle-parent")
        && args.len() == 3
    {
        use prism_platform_qa::{ocr, ocr_runtime};
        use std::sync::atomic::AtomicBool;
        let runtime = ocr_runtime::initialize(
            PathBuf::from(&args[1]).as_path(),
            &std::env::current_exe().unwrap(),
            |_| {},
        )
        .unwrap();
        assert!(runtime.status().available);
        let bytes = include_bytes!("../../../tests/fixtures/ocr/bilingual.png");
        let limits = ocr::Limits::default();
        let cancel = AtomicBool::new(false);
        let system = ocr::Preferences {
            enabled: true,
            engine: ocr::Engine::System,
            // Mac has a real OS engine. Request a valid but unsupported tag
            // to exercise actual language failure and public Auto fallback.
            #[cfg(target_os = "macos")]
            language: "zz-PrismUnavailable".into(),
            ..Default::default()
        };
        #[cfg(target_os = "linux")]
        let expected_absence = ocr::Code::Unavailable;
        #[cfg(target_os = "macos")]
        let expected_absence = ocr::Code::LanguageUnavailable;
        assert_eq!(
            ocr::recognize(ocr::Input::Image(bytes), &system, &limits, &cancel)
                .unwrap_err()
                .code,
            expected_absence
        );
        let automatic = ocr::Preferences {
            engine: ocr::Engine::Auto,
            ..system
        };
        let pages = ocr::recognize(ocr::Input::Image(bytes), &automatic, &limits, &cancel).unwrap();
        let recognized =
            pages.len() == 1 && pages[0].text.contains("프리즘") && pages[0].text.contains("74829");
        assert!(recognized);
        // Exercise the actual extraction caller too: typed pages stay typed,
        // scanned pages use Auto, and decrypted render bytes stay in memory.
        use prism_platform_qa::{document_extract as extract, ocr_assets};
        let sources = PathBuf::from(&args[1]).join("public-fixtures");
        fs::create_dir(&sources).unwrap();
        let options = extract::ExtractOptions {
            ocr: automatic,
            ..Default::default()
        };
        let mixed_bytes = include_bytes!("../../../tests/fixtures/ocr/mixed.pdf");
        let mixed_path = sources.join("mixed.pdf");
        fs::write(&mixed_path, mixed_bytes).unwrap();
        let document = extract::extract_document(&mixed_path, &options, &cancel).unwrap();
        assert!(document.sections[0]
            .text
            .contains("typed original page one"));
        assert!(document
            .sections
            .iter()
            .any(|s| s.page == Some(2) && s.text.contains("74829")));
        assert!(document
            .sections
            .iter()
            .any(|s| s.page == Some(3) && s.text.contains("typed original page three")));
        assert!(document.sections.iter().all(|s| s.page != Some(4)));
        assert!(document.sections.windows(2).all(|s| s[0].page <= s[1].page));
        assert_eq!(document.fingerprint.sha256, ocr_assets::digest(mixed_bytes));
        assert_eq!(fs::read(&mixed_path).unwrap(), mixed_bytes);
        let encrypted = include_bytes!("../../../tests/fixtures/ocr/scan-locked.pdf");
        let encrypted_path = sources.join("scan-locked.pdf");
        fs::write(&encrypted_path, encrypted).unwrap();
        for password in ["ocr-fixture-password", "ocr-fixture-owner"] {
            let document = extract::extract_document_with_password(
                &encrypted_path,
                &options,
                &cancel,
                password,
            )
            .unwrap();
            assert!(document.sections.iter().any(|s| s.page == Some(1)
                && s.text.contains("프리즘")
                && s.text.contains("74829")));
            assert_eq!(document.fingerprint.sha256, ocr_assets::digest(encrypted));
            assert_eq!(fs::read(&encrypted_path).unwrap(), encrypted);
        }
        assert_eq!(
            extract::extract_document_with_password(&encrypted_path, &options, &cancel, "wrong")
                .unwrap_err()
                .code,
            extract::ExtractErrorCode::PasswordRejected
        );
        assert_eq!(
            fs::read_dir(&sources).unwrap().count(),
            2,
            "no decrypted PDF file may be written"
        );
        fs::write(&args[2], serde_json::to_vec(&serde_json::json!({
            "pid": runtime.status().pid.unwrap(), "autoFallback": true, "actualKorean": recognized,
            "sourceExtractionVerified": true,
        })).unwrap()).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--fixture-model-parent")
        && args.len() == 3
    {
        let service =
            prism_platform_qa::embedding_runtime::EmbeddingRuntime::new(PathBuf::from(&args[1]))
                .unwrap();
        let model = PathBuf::from(std::env::var_os("PRISM_TEST_MODEL").expect("PRISM_TEST_MODEL"));
        let runtime =
            PathBuf::from(std::env::var_os("PRISM_TEST_RUNTIME").expect("PRISM_TEST_RUNTIME"));
        service.import_model(&model).unwrap();
        service.import_runtime(&runtime).unwrap();
        service.start().unwrap();
        assert_eq!(
            service.embed_query("제주도 여행 일정", 256).unwrap().len(),
            256
        );
        fs::write(&args[2], serde_json::to_vec(&service.status()).unwrap()).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if args.first().is_some_and(|arg| arg == "--fixture-exit") {
        return Some(23);
    }
    if args.first().is_some_and(|arg| arg == "--fixture-server") && args.len() == 3 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let data = serde_json::json!({
            "pid": std::process::id(), "address": listener.local_addr().unwrap().to_string(),
            "literal": args[2].to_str(), "cwd": std::env::current_dir().unwrap(),
            "secretEnv": std::env::var("PRISM_SYNTHETIC_SERVER_KEY").as_deref() == Ok("synthetic-only"),
            "removedEnv": std::env::var_os("PRISM_SHOULD_BE_ABSENT").is_none(),
        });
        fs::write(&args[1], serde_json::to_vec(&data).unwrap()).unwrap();
        for connection in listener.incoming() {
            drop(connection.unwrap());
        }
        return Some(0);
    }
    if args.first().is_some_and(|arg| arg == "--fixture-parent") && args.len() == 3 {
        let exe = std::env::current_exe().unwrap();
        let directory = PathBuf::from(&args[1]);
        let mut command = Command::new(&exe);
        command
            .args(["--fixture-server", "server.json", "$(literal); 한글 공백"])
            .current_dir(&directory)
            .env("PRISM_SYNTHETIC_SERVER_KEY", "synthetic-only")
            .env_remove("PRISM_SHOULD_BE_ABSENT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let owned =
            prism_platform_qa::runtime_process::OwnedRuntimeProcess::spawn(command, &exe).unwrap();
        fs::write(&args[2], owned.id().to_string()).unwrap();
        // The external test kills this Child with SIGKILL. No Drop runs here.
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    None
}
