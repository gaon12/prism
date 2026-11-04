use prism_update_verifier::{verify, MAX_ARTIFACT_BYTES, MAX_KEY_BYTES, MAX_SIGNATURE_BYTES};
use std::{ffi::OsString, fs, io::Read, path::Path};

fn bounded_text(path: &Path, limit: usize) -> Result<String, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "Verification file unavailable.")?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("Invalid verification file.");
    }
    let mut text = String::new();
    fs::File::open(path)
        .map_err(|_| "Verification file unavailable.")?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "Could not read verification file.")?;
    if text.len() > limit {
        return Err("Verification file is too large.");
    }
    Ok(text)
}

fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let mut version = None;
    let mut file = None;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or("Expected --version VERSION --file PATH.")?;
        if value.to_string_lossy().starts_with("--") {
            return Err("Missing option value.");
        }
        if arg == "--version" && version.is_none() {
            version = Some(
                value
                    .into_string()
                    .map_err(|_| "Invalid version encoding.")?,
            );
        } else if arg == "--file" && file.is_none() {
            file = Some(value);
        } else {
            return Err("Unknown or duplicate verification option.");
        }
    }
    let file = file.ok_or("An update artifact is required.")?;
    let path = Path::new(&file);
    let metadata = fs::symlink_metadata(path).map_err(|_| "Update artifact unavailable.")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err("Invalid update artifact size/type.");
    }
    let mut signature = OsString::from(&file);
    signature.push(".sig");
    let signature = bounded_text(Path::new(&signature), MAX_SIGNATURE_BYTES)?;
    let key = std::env::var("PRISM_UPDATER_PUBLIC_KEY")
        .map_err(|_| "PRISM_UPDATER_PUBLIC_KEY is required.")?;
    if key.len() > MAX_KEY_BYTES {
        return Err("Public key is too large.");
    }
    verify(
        fs::File::open(path).map_err(|_| "Update artifact unavailable.")?,
        &key,
        &signature,
        &version.ok_or("A release version is required.")?,
    )?;
    println!("Update artifact signature and release version verified.");
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Update verification failed: {error}");
        std::process::exit(1);
    }
}
