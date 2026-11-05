#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() != ["--service"] {
        eprintln!("Prism filename index host. Run only through the installed Windows service (--service).");
        std::process::exit(2);
    }
    #[cfg(windows)]
    if let Err(error) = prism_ntfs_windows::service::run(std::sync::Arc::new(
        prism_ntfs_windows::host::SnapshotBackend,
    )) {
        eprintln!(
            "Filename index operation failed: {:?} (code {:?}).",
            error.kind(),
            error.raw_os_error()
        );
        std::process::exit(2);
    }
    #[cfg(not(windows))]
    {
        eprintln!("The filename index service host requires Windows.");
        std::process::exit(2);
    }
}
