#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !matches!(
        args.first().map(String::as_str),
        Some("--service" | "--install" | "--uninstall")
    ) || args.len() != 1
    {
        eprintln!("Prism filename index: --service, --install or --uninstall. Management requires an already elevated administrator.");
        std::process::exit(2);
    }
    #[cfg(windows)]
    if let Err(error) = match args[0].as_str() {
        "--install" => prism_ntfs_windows::administration::install(),
        "--uninstall" => prism_ntfs_windows::administration::uninstall(),
        _ => prism_ntfs_windows::service::run(std::sync::Arc::new(
            prism_ntfs_windows::host::SnapshotBackend,
        )),
    } {
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
