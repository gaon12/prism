//! Developer-only harness importing the exact application asset implementation.
#[path = "../../../../src-tauri/src/office_assets.rs"]
#[allow(dead_code)]
mod office_assets;
#[path = "../../../../src-tauri/src/office_preview.rs"]
#[allow(dead_code)]
mod office_preview;
#[path = "../../../../src-tauri/src/path_identity.rs"]
// The shared test module also exports Windows junction fixtures; this small
// Unix harness has no callers for those fixtures. Production linting is intact.
#[cfg_attr(not(windows), allow(dead_code))]
mod path_identity;

use office_assets::{error, Code, Result};
use std::{fs, path::PathBuf, time::Instant};

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 4 && (args[0] == "render" || args[0] == "render-cancel") {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use std::time::Duration;
        let data = PathBuf::from(&args[1]);
        if fs::read(data.join("qa-owner.txt")).map_err(|_| error(Code::Input))?
            != b"Prism Office native QA data v1\n"
        {
            return Err(error(Code::Input));
        }
        let input = PathBuf::from(&args[2]);
        let extension = input
            .extension()
            .and_then(|s| s.to_str())
            .ok_or_else(|| error(Code::Input))?;
        let bytes = fs::read(&input).map_err(|_| error(Code::Input))?;
        let runtime = office_preview::Runtime::open(data)?;
        let start = Instant::now();
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let trigger =
            PathBuf::from(&args[1]).join(format!("qa-cancel-{}.flag", std::process::id()));
        let watcher = (args[0] == "render-cancel").then(|| {
            let cancel = cancel.clone();
            let done = finished.clone();
            let trigger = trigger.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if trigger.exists() {
                        cancel.store(true, Ordering::Release);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
        });
        let result = runtime.render_pdf(
            extension,
            &bytes,
            &cancel,
            Instant::now() + Duration::from_secs(120),
            &|status| {
                if status.files == 0 || status.files == status.total_files {
                    eprintln!("office phase={} files={}", status.phase, status.files);
                }
            },
        );
        finished.store(true, Ordering::Release);
        if let Some(watcher) = watcher {
            watcher.join().map_err(|_| error(Code::Failed))?;
        }
        if args[0] == "render-cancel" {
            if result
                .as_ref()
                .is_err_and(|error| error.code == Code::Cancelled)
            {
                println!(
                    "{}",
                    serde_json::json!({"cancelled":true,"elapsedMs":start.elapsed().as_millis()})
                );
                return Ok(());
            }
            return Err(error(Code::Failed));
        }
        let pdf = result?;
        let output = PathBuf::from(&args[3]);
        use std::io::Write;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .and_then(|mut f| f.write_all(&pdf))
            .map_err(|_| error(Code::Failed))?;
        println!(
            "{}",
            serde_json::json!({"rendered":true,"pdfBytes":pdf.len(),"elapsedMs":start.elapsed().as_millis(),"fingerprint":runtime.fingerprint()})
        );
        return Ok(());
    }
    if args.len() != 3 || args[0] != "install" {
        return Err(error(Code::Input));
    }
    let archive = PathBuf::from(&args[1]);
    let data = PathBuf::from(&args[2]);
    if !data.is_absolute() || data.exists() {
        return Err(error(Code::Input));
    }
    path_identity::comparison(&data).map_err(|_| error(Code::Input))?;
    fs::create_dir_all(&data).map_err(|_| error(Code::Failed))?;
    fs::write(
        data.join("qa-owner.txt"),
        b"Prism Office native QA data v1\n",
    )
    .map_err(|_| error(Code::Failed))?;
    let catalog = office_assets::catalog()?;
    let start = Instant::now();
    office_assets::install_archive(&data, &archive, &catalog, &|| Ok(()), &|n| {
        if n % 2000 == 0 {
            eprintln!("office install files={n}");
        }
    })?;
    println!(
        "{}",
        serde_json::json!({"installed":true,"files":catalog.file_count,"bytes":catalog.expanded_bytes,"elapsedMs":start.elapsed().as_millis()})
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!(
            "office error={:?} stage={:?} nativeCode={:?}",
            error.code, error.stage, error.native_code
        );
        std::process::exit(1);
    }
}
