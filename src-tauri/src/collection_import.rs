//! Local, reference-only manifests. Archives are read in memory, never extracted
//! to caller-selected paths. These reads grant no content-index or AI permission.
use crate::collection_store::{
    error, ErrorCode, Page, PageKind, PageLocation, Result, Source, SourceKind,
};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering as Compare,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const MAX_BYTES: u64 = 128 << 20;
const MAX_PAGES: usize = 250_000;
pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(error(ErrorCode::Cancelled))
    } else {
        Ok(())
    }
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn image_extension(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff"
    )
}
fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}
pub(crate) fn canonical(path: &Path, excluded: &Path) -> Result<PathBuf> {
    let path = crate::path_identity::canonical(path).map_err(|issue| {
        error(if issue.kind() == std::io::ErrorKind::InvalidInput {
            ErrorCode::UnsafePath
        } else {
            ErrorCode::SourceChanged
        })
    })?;
    if path.starts_with(excluded) {
        return Err(error(ErrorCode::UnsafePath));
    }
    Ok(path)
}
pub(crate) fn source_kind(path: &Path) -> Result<SourceKind> {
    if path.is_dir() {
        return Ok(SourceKind::Folder);
    }
    if image_extension(path) {
        return Ok(SourceKind::Image);
    }
    match extension(path).as_str() {
        "pdf" => Ok(SourceKind::Pdf),
        "zip" | "cbz" => Ok(SourceKind::Archive),
        "epub" => Ok(SourceKind::Epub),
        "txt" | "md" | "markdown" | "html" | "htm" => Ok(SourceKind::Text),
        ext if crate::document_extract::is_supported_extension(ext) => Ok(SourceKind::Document),
        _ => Err(error(ErrorCode::Unsupported)),
    }
}
/// Immutable Windows sharing, no-follow final opens on both platforms, and
/// before/after canonical path checks keep a manifest bound to its source.
pub(crate) fn read(path: &Path, excluded: &Path, cancel: &AtomicBool) -> Result<Vec<u8>> {
    check(cancel)?;
    let resolved = canonical(path, excluded)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(&resolved)
        .map_err(|_| error(ErrorCode::SourceChanged))?;
    let metadata = file
        .metadata()
        .map_err(|_| error(ErrorCode::SourceChanged))?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(error(ErrorCode::Limit));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut buffer = vec![0; 256 * 1024];
    loop {
        check(cancel)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| error(ErrorCode::SourceChanged))?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) as u64 > MAX_BYTES {
            return Err(error(ErrorCode::Limit));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    check(cancel)?;
    let now = canonical(path, excluded)?;
    let after = fs::metadata(&now).map_err(|_| error(ErrorCode::SourceChanged))?;
    if resolved != now
        || metadata.len() != after.len()
        || metadata.modified().ok() != after.modified().ok()
        || bytes.len() as u64 != metadata.len()
    {
        return Err(error(ErrorCode::SourceChanged));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != after.dev() || metadata.ino() != after.ino() {
            return Err(error(ErrorCode::SourceChanged));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let current = options
            .open(&now)
            .map_err(|_| error(ErrorCode::SourceChanged))?;
        let (mut a, mut b) = (
            BY_HANDLE_FILE_INFORMATION::default(),
            BY_HANDLE_FILE_INFORMATION::default(),
        );
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut a) } == 0
            || unsafe { GetFileInformationByHandle(current.as_raw_handle(), &mut b) } == 0
            || (a.dwVolumeSerialNumber, a.nFileIndexHigh, a.nFileIndexLow)
                != (b.dwVolumeSerialNumber, b.nFileIndexHigh, b.nFileIndexLow)
            || a.dwFileAttributes & 0x400 != 0
        {
            return Err(error(ErrorCode::SourceChanged));
        }
    }
    Ok(bytes)
}
pub(crate) fn natural(a: &str, b: &str) -> Compare {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let x: String = std::iter::from_fn(|| a.next_if(|c| c.is_ascii_digit())).collect();
                let y: String = std::iter::from_fn(|| b.next_if(|c| c.is_ascii_digit())).collect();
                let (nx, ny) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = nx
                    .len()
                    .cmp(&ny.len())
                    .then_with(|| nx.cmp(ny))
                    .then_with(|| x.len().cmp(&y.len()));
                if order != Compare::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Compare::Equal {
                    return order;
                }
            }
            (None, None) => return Compare::Equal,
            (None, _) => return Compare::Less,
            (_, None) => return Compare::Greater,
        }
    }
}
fn page(
    source: &Source,
    entry: Option<String>,
    number: u32,
    kind: PageKind,
    title: String,
    source_hash: &str,
) -> Page {
    let location = PageLocation {
        kind,
        entry,
        number,
    };
    let identity =
        serde_json::to_vec(&(&source.id, &location)).expect("serializable page identity");
    Page {
        id: hash(&identity),
        unit_id: source.unit_id.clone(),
        source_id: source.id.clone(),
        position: 0,
        location,
        title,
        source_hash: source_hash.into(),
    }
}
pub(crate) fn text(
    source: &Source,
    excluded: &Path,
    cancel: &AtomicBool,
) -> Result<crate::document_extract::ExtractedDocument> {
    use crate::document_extract::{ExtractErrorCode, ExtractOptions};
    let options = ExtractOptions {
        max_file_bytes: MAX_BYTES,
        ..Default::default()
    };
    let path = canonical(Path::new(&source.path), excluded)?;
    let document =
        crate::document_extract::extract_document(&path, &options, cancel).map_err(|e| {
            error(match e.code {
                ExtractErrorCode::Cancelled => ErrorCode::Cancelled,
                ExtractErrorCode::Encrypted | ExtractErrorCode::PasswordRejected => {
                    ErrorCode::Encrypted
                }
                ExtractErrorCode::LimitExceeded => ErrorCode::Limit,
                _ => ErrorCode::Unsupported,
            })
        })?;
    if canonical(Path::new(&source.path), excluded)? != path {
        return Err(error(ErrorCode::SourceChanged));
    }
    Ok(document)
}
pub(crate) fn text_pages(document: &crate::document_extract::ExtractedDocument) -> Vec<String> {
    let mut pages = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for section in &document.sections {
        for c in section.text.chars().chain(['\n', '\n']) {
            current.push(c);
            count += 1;
            if count == 2400 {
                pages.push(std::mem::take(&mut current));
                count = 0;
            }
        }
    }
    if !current.trim().is_empty() {
        pages.push(current);
    }
    pages
}
pub(crate) fn manifest(
    source: &Source,
    excluded: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<Page>> {
    check(cancel)?;
    let path = canonical(Path::new(&source.path), excluded)?;
    let mut pages = Vec::new();
    match source.kind {
        SourceKind::Folder => {
            let mut entries = Vec::new();
            let mut visited = 0;
            let mut path_bytes = 0usize;
            for entry in walkdir::WalkDir::new(&path)
                .follow_links(false)
                .max_open(16)
            {
                check(cancel)?;
                visited += 1;
                if visited > MAX_PAGES {
                    return Err(error(ErrorCode::Limit));
                }
                let entry = entry.map_err(|_| error(ErrorCode::SourceChanged))?;
                if entry.file_type().is_symlink() {
                    return Err(error(ErrorCode::UnsafePath));
                }
                if entry.file_type().is_file() && image_extension(entry.path()) {
                    let current = canonical(entry.path(), excluded)?;
                    let relative = current
                        .strip_prefix(&path)
                        .map_err(|_| error(ErrorCode::UnsafePath))?
                        .to_str()
                        .ok_or_else(|| error(ErrorCode::Unsupported))?
                        .replace('\\', "/");
                    PageLocation {
                        kind: PageKind::Image,
                        entry: Some(relative.clone()),
                        number: 1,
                    }
                    .validate()?;
                    path_bytes = path_bytes.saturating_add(relative.len());
                    if path_bytes > 32 << 20 {
                        return Err(error(ErrorCode::Limit));
                    }
                    entries.push((relative, current));
                }
            }
            entries.sort_by(|a, b| natural(&a.0, &b.0).then_with(|| a.0.cmp(&b.0)));
            for (entry, path) in entries {
                let bytes = read(&path, excluded, cancel)?;
                pages.push(page(
                    source,
                    Some(entry.clone()),
                    1,
                    PageKind::Image,
                    entry,
                    &hash(&bytes),
                ));
                progress(pages.len() as u64);
            }
        }
        SourceKind::Archive => {
            let bytes = read(&path, excluded, cancel)?;
            let digest = hash(&bytes);
            let mut archive = zip::ZipArchive::new(Cursor::new(&bytes))
                .map_err(|_| error(ErrorCode::Unsupported))?;
            if archive.len() > MAX_PAGES {
                return Err(error(ErrorCode::Limit));
            }
            let mut entries = Vec::new();
            let mut expanded = 0u64;
            let mut seen = std::collections::HashSet::new();
            for i in 0..archive.len() {
                check(cancel)?;
                let file = archive
                    .by_index(i)
                    .map_err(|_| error(ErrorCode::Encrypted))?;
                if file.is_dir() {
                    continue;
                }
                expanded = expanded.saturating_add(file.size());
                if file.size() > MAX_BYTES || expanded > 512 << 20 {
                    return Err(error(ErrorCode::Limit));
                }
                let name = file
                    .name()
                    .map_err(|_| error(ErrorCode::Unsupported))?
                    .into_owned();
                PageLocation {
                    kind: PageKind::Image,
                    entry: Some(name.clone()),
                    number: 1,
                }
                .validate()?;
                if !seen.insert(name.clone()) {
                    return Err(error(ErrorCode::Input));
                }
                if image_extension(Path::new(&name)) {
                    entries.push(name);
                }
            }
            entries.sort_by(|a, b| natural(a, b).then_with(|| a.cmp(b)));
            for entry in entries {
                pages.push(page(
                    source,
                    Some(entry.clone()),
                    1,
                    PageKind::Image,
                    entry,
                    &digest,
                ));
                progress(pages.len() as u64);
            }
        }
        SourceKind::Image => {
            let bytes = read(&path, excluded, cancel)?;
            pages.push(page(
                source,
                None,
                1,
                PageKind::Image,
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                &hash(&bytes),
            ));
        }
        SourceKind::Pdf => {
            let bytes = read(&path, excluded, cancel)?;
            let digest = hash(&bytes);
            let doc = pdf_extract::Document::load_mem(&bytes).map_err(|e| {
                error(match e {
                    pdf_extract::Error::InvalidPassword | pdf_extract::Error::Decryption(_) => {
                        ErrorCode::Encrypted
                    }
                    _ => ErrorCode::Unsupported,
                })
            })?;
            let count = doc.get_pages().len();
            if count > MAX_PAGES {
                return Err(error(ErrorCode::Limit));
            }
            for number in 1..=count as u32 {
                check(cancel)?;
                pages.push(page(
                    source,
                    None,
                    number,
                    PageKind::Pdf,
                    number.to_string(),
                    &digest,
                ));
            }
        }
        SourceKind::Epub => return Err(error(ErrorCode::Unsupported)),
        SourceKind::Text | SourceKind::Document => {
            let document = text(source, excluded, cancel)?;
            for (i, _) in text_pages(&document).into_iter().enumerate() {
                check(cancel)?;
                if i >= MAX_PAGES {
                    return Err(error(ErrorCode::Limit));
                }
                pages.push(page(
                    source,
                    None,
                    i as u32 + 1,
                    PageKind::Text,
                    (i + 1).to_string(),
                    &document.fingerprint.sha256,
                ));
            }
        }
    }
    check(cancel)?;
    if pages.is_empty() {
        return Err(error(ErrorCode::Unsupported));
    }
    Ok(pages)
}
pub(crate) fn image_bytes(
    source: &Source,
    page: &Page,
    excluded: &Path,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    page.location.validate()?;
    let original = canonical(Path::new(&source.path), excluded)?;
    let path = if matches!(source.kind, SourceKind::Folder) {
        let child = original.join(
            page.location
                .entry
                .as_deref()
                .ok_or_else(|| error(ErrorCode::Input))?,
        );
        let resolved = canonical(&child, excluded)?;
        if !resolved.starts_with(&original) {
            return Err(error(ErrorCode::UnsafePath));
        }
        resolved
    } else {
        original
    };
    let bytes = read(&path, excluded, cancel)?;
    if hash(&bytes) != page.source_hash {
        return Err(error(ErrorCode::SourceChanged));
    }
    if matches!(source.kind, SourceKind::Archive) {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(&bytes)).map_err(|_| error(ErrorCode::Unsupported))?;
        let mut entry = archive
            .by_name(
                page.location
                    .entry
                    .as_deref()
                    .ok_or_else(|| error(ErrorCode::Input))?,
            )
            .map_err(|_| error(ErrorCode::SourceChanged))?;
        if entry.size() > MAX_BYTES {
            return Err(error(ErrorCode::Limit));
        }
        let mut data = Vec::new();
        let mut buf = vec![0; 256 * 1024];
        loop {
            check(cancel)?;
            let n = entry
                .read(&mut buf)
                .map_err(|_| error(ErrorCode::Unsupported))?;
            if n == 0 {
                break;
            }
            if data.len().saturating_add(n) as u64 > MAX_BYTES {
                return Err(error(ErrorCode::Limit));
            }
            data.extend_from_slice(&buf[..n]);
        }
        Ok(data)
    } else {
        Ok(bytes)
    }
}
