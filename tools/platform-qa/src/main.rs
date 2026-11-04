//! Pre-GUI supervisor host and inert, opt-in command fixtures for owned tests.
fn main() {
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
