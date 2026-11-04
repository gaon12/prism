//! Small native worker. It removes only a validated temporary uninstall copy.
#![cfg_attr(windows, windows_subsystem = "windows")]
#[allow(dead_code)]
mod safe_path;
#[allow(dead_code)]
mod temp_cleanup;

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--cleanup-temp" {
        return std::process::ExitCode::FAILURE;
    }
    match temp_cleanup::worker(std::path::Path::new(&args[1])) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
