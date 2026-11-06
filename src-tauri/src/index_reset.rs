//! Reset only the app-owned content cache, before any database/model worker opens it.
//! A persisted request survives interruption; settings, models and source folders are
//! never supplied as deletion targets. Filename catalogs live only in memory.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const REQUEST: &str = "index-reset.json";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    token: String,
}

fn root(directory: &Path) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("The cache folder must be absolute.".into());
    }
    let mut ancestor = directory;
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("The cache folder has no parent.")?;
    }
    crate::path_identity::no_links(ancestor)
        .map_err(|_| "The cache folder contains an unsafe link.")?;
    fs::create_dir_all(directory).map_err(|_| "Could not open the cache folder.")?;
    crate::path_identity::canonical(directory)
        .map_err(|_| "The cache folder contains an unsafe link.".into())
}
fn read_request(directory: &Path) -> Result<Option<Request>, String> {
    let path = directory.join(REQUEST);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_file() && metadata.len() <= 1024 => {}
        _ => return Err("The index reset request is unavailable or invalid.".into()),
    }
    crate::path_identity::no_links(&path).map_err(|_| "The reset request is linked.")?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(1025).read_to_end(&mut bytes))
        .map_err(|_| "Could not read the index reset request.")?;
    if bytes.len() > 1024 {
        return Err("The reset request is too large.".into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "The reset request is invalid.")?;
    if request.version != 1
        || request.token.len() != 32
        || !request.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("The reset request version or identity is invalid.".into());
    }
    Ok(Some(request))
}
fn validate_cache(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata) if metadata.is_dir() => {}
        _ => return Err("The content cache is not an accessible directory.".into()),
    }
    for (count, item) in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        if count >= 1_000_000 {
            return Err("The cache exceeds the reset file budget.".into());
        }
        let item = item.map_err(|_| "Could not inspect the content cache.")?;
        crate::path_identity::no_links(item.path())
            .map_err(|_| "The content cache contains an unsafe link.")?;
    }
    Ok(true)
}

/// Schedule a reset, never touching an open content store. Repeated requests are
/// idempotent and cannot replace a different pending journal or linked file.
pub fn schedule(directory: &Path) -> Result<(), String> {
    let directory = root(directory)?;
    if read_request(&directory)?.is_some() {
        return Ok(());
    }
    validate_cache(&directory.join("content"))?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "Could not prepare the reset identity.")?;
    let request = Request {
        version: 1,
        token: random.iter().map(|byte| format!("{byte:02x}")).collect(),
    };
    let bytes = serde_json::to_vec(&request).map_err(|_| "Could not prepare the reset request.")?;
    let path = directory.join(REQUEST);
    let temporary = directory.join(format!(
        ".prism-index-reset-request-{}.pending",
        request.token
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| "Could not create the index reset request.")?;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("Could not save the index reset request.".into());
    }
    if read_request(&directory)?.is_some() {
        let _ = fs::remove_file(&temporary);
        return Ok(());
    }
    // Publish a complete journal, never a partially written JSON file. The IPC
    // caller serializes scheduling inside this app instance.
    fs::rename(&temporary, &path).map_err(|_| "Could not publish the index reset request.".into())
}

pub fn pending(directory: &Path) -> Result<bool, String> {
    Ok(read_request(&root(directory)?)?.is_some())
}

/// Called before opening content.sqlite3 or any Lance table. Exact fixed child
/// names and canonical-root checks keep recursive removal inside this cache.
/// A failed removal leaves the request/quarantine for a later retry; callers
/// must not open a new content store after an error.
pub fn apply_pending(directory: &Path) -> Result<bool, String> {
    let directory = root(directory)?;
    let Some(request) = read_request(&directory)? else {
        return Ok(false);
    };
    let content = directory.join("content");
    let retired = directory.join(format!(".prism-index-reset-{}", request.token));
    let has_content = validate_cache(&content)?;
    let has_retired = validate_cache(&retired)?;
    if has_content && has_retired {
        return Err(
            "Reset interrupted: both old and new caches exist; neither was removed.".into(),
        );
    }
    if has_content {
        fs::rename(&content, &retired).map_err(|_| {
            "Could not retire the content cache. Close other Prism copies and retry."
        })?;
    }
    if has_content || has_retired {
        // Revalidate the final absolute deletion target after the rename.
        let resolved = crate::path_identity::canonical(&retired)
            .map_err(|_| "The retired cache path is unsafe.")?;
        if resolved.parent() != Some(directory.as_path())
            || resolved.file_name() != retired.file_name()
        {
            return Err("The retired cache escaped its owned folder.".into());
        }
        validate_cache(&resolved)?;
        fs::remove_dir_all(&resolved).map_err(|_| "Could not remove the retired content cache.")?;
    }
    if read_request(&directory)?.is_none_or(|current| current.token != request.token) {
        return Err("The reset request changed during cleanup.".into());
    }
    fs::remove_file(directory.join(REQUEST)).map_err(|_| "Could not finish the reset request.")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cache(root: &Path) {
        fs::create_dir_all(root.join("content/vectors/prism-fixture.lance")).unwrap();
        fs::write(root.join("content/content.sqlite3"), "inert index bytes").unwrap();
        fs::write(
            root.join("content/vectors/prism-fixture.lance/data"),
            "inert vectors",
        )
        .unwrap();
    }
    #[test]
    fn only_requested_content_is_removed_and_other_data_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        cache(root);
        for name in [
            "settings.json",
            "session.json",
            "chats/history",
            "documents/draft",
            "collections/media",
            "clipboard/item",
            "embedding/model",
            "source.md",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
        assert!(!apply_pending(root).unwrap());
        assert!(root.join("content/content.sqlite3").is_file());
        schedule(root).unwrap();
        let request = fs::read(root.join(REQUEST)).unwrap();
        schedule(root).unwrap();
        assert_eq!(request, fs::read(root.join(REQUEST)).unwrap());
        assert!(
            root.join("content/content.sqlite3").is_file(),
            "scheduling never deletes live caches"
        );
        assert!(apply_pending(root).unwrap());
        assert!(!root.join("content").exists());
        assert!(!root.join(REQUEST).exists());
        for name in [
            "settings.json",
            "session.json",
            "chats/history",
            "documents/draft",
            "collections/media",
            "clipboard/item",
            "embedding/model",
            "source.md",
        ] {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), name);
        }
        assert!(!apply_pending(root).unwrap());
    }
    #[test]
    fn interruption_after_retirement_resumes_but_ambiguous_new_cache_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        cache(root);
        schedule(root).unwrap();
        let request = read_request(root).unwrap().unwrap();
        let retired = root.join(format!(".prism-index-reset-{}", request.token));
        fs::rename(root.join("content"), &retired).unwrap();
        cache(root);
        assert!(apply_pending(root).is_err());
        assert!(root.join("content/content.sqlite3").exists());
        assert!(retired.exists());
        fs::remove_file(root.join("content/content.sqlite3")).unwrap();
        // Remove only this test-created replacement, after verifying its resolved parent.
        let replacement = root.join("content").canonicalize().unwrap();
        assert_eq!(
            replacement.parent(),
            Some(root.canonicalize().unwrap().as_path())
        );
        fs::remove_dir_all(replacement).unwrap();
        assert!(apply_pending(root).unwrap());
        assert!(!retired.exists());
    }
    #[test]
    fn invalid_requests_never_authorize_removal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        cache(root);
        for bytes in [
            r#"{"version":2,"token":"00000000000000000000000000000000"}"#,
            r#"{"version":1,"token":"../../outside"}"#,
            r#"{"version":1,"token":"00000000000000000000000000000000","path":"elsewhere"}"#,
            "not json",
        ] {
            fs::write(root.join(REQUEST), bytes).unwrap();
            assert!(apply_pending(root).is_err());
            assert!(root.join("content/content.sqlite3").is_file());
        }
    }
    #[test]
    fn empty_cache_reset_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        schedule(dir.path()).unwrap();
        assert!(apply_pending(dir.path()).unwrap());
        assert!(!apply_pending(dir.path()).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn linked_cache_or_child_cannot_delete_originals() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("original"), "keep").unwrap();
        symlink(outside.path(), dir.path().join("content")).unwrap();
        assert!(schedule(dir.path()).is_err());
        fs::remove_file(dir.path().join("content")).unwrap();
        cache(dir.path());
        symlink(outside.path(), dir.path().join("content/linked")).unwrap();
        assert!(schedule(dir.path()).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("original")).unwrap(),
            "keep"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_junctions_never_authorize_cache_removal() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("original"), "keep").unwrap();
        // mklink /J needs no administrator permission. Refuse shell metacharacters
        // even though these are generated, owned fixture paths, never user inputs.
        for path in [dir.path(), outside.path()] {
            assert!(!path
                .to_string_lossy()
                .contains(['&', '|', '<', '>', '^', '%', '!', '(', ')', '\r', '\n']));
        }
        let link = dir.path().join("content");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(schedule(dir.path()).is_err());
        assert!(!dir.path().join(REQUEST).exists());
        fs::remove_dir(&link).unwrap(); // only the owned junction, not its target
        cache(dir.path());
        let nested = dir.path().join("content").join("linked");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&nested)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(schedule(dir.path()).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("original")).unwrap(),
            "keep"
        );
        fs::remove_dir(nested).unwrap();
    }
}
