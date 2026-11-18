//! Manual helper for an explicitly named, test-owned application. Never launch
//! it against a personal installation. No default path or process discovery UI.
use std::{io, path::PathBuf};

fn main() -> io::Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let app = arguments.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::other("Expected exact executable path and --check or --allow-close.")
    })?;
    let action = arguments
        .next()
        .ok_or_else(|| io::Error::other("Explicit --check or --allow-close is required."))?;
    if arguments.next().is_some() || !app.is_absolute() {
        return Err(io::Error::other(
            "Only an absolute exact executable path and one explicit action are accepted.",
        ));
    }
    let allow = match action.to_str() {
        Some("--check") => false,
        Some("--allow-close") => true,
        _ => return Err(io::Error::other("Unknown action.")),
    };
    prism_setup_core::process_control::ensure_closed(&app, allow)?;
    println!("Exact application path is not running.");
    Ok(())
}
