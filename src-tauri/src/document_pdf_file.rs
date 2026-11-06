//! Atomic replacement of a user-selected PDF destination. Only a new, random
//! staging file is removed on failure; an existing destination stays intact.
use crate::{
    document_pdf::{Code, Pdf, Result},
    document_pdf_job::Job,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn validate(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || !path
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case("pdf"))
        || path.file_name().is_none()
    {
        return Err(Code::Input);
    }
    Ok(())
}
pub fn save(path: &Path, pdf: &Pdf, job: &Job) -> Result<()> {
    validate(path)?;
    job.control.check()?;
    if pdf.bytes.len() > crate::document_pdf::MAX_PDF || !pdf.bytes.starts_with(b"%PDF-") {
        return Err(Code::Input);
    }
    let parent = fs::canonicalize(path.parent().ok_or(Code::Input)?).map_err(|_| Code::Storage)?;
    let destination = parent.join(path.file_name().ok_or(Code::Input)?);
    match fs::symlink_metadata(&destination) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => return Err(Code::Input),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(Code::Storage),
        _ => {}
    }
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| Code::Storage)?;
    let pending = Pending(parent.join(format!(
        ".prism-pdf-{:032x}.tmp",
        u128::from_le_bytes(nonce)
    )));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending.0).map_err(|_| Code::Storage)?;
    for chunk in pdf.bytes.chunks(64 << 10) {
        job.control.check()?;
        file.write_all(chunk).map_err(|_| Code::Storage)?;
    }
    file.sync_all().map_err(|_| Code::Storage)?;
    drop(file);
    job.commit(|| fs::rename(&pending.0, &destination).map_err(|_| Code::Storage))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_selected_file_replacement_and_cancel_preserve_old_bytes_without_staging_remains() {
        let _test = crate::document_pdf_job::test_guard();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("한글.pdf");
        fs::write(&path, b"previous document").unwrap();
        let pdf = Pdf {
            bytes: include_bytes!("../../tests/fixtures/ocr/crop-clockwise.pdf").to_vec(),
            pages: 1,
            omitted_images: 0,
        };
        let job = Job::start("pdf-file-replace-request", "main").unwrap();
        save(&path, &pdf, &job).unwrap();
        assert_eq!(fs::read(&path).unwrap(), pdf.bytes);
        drop(job);
        let job = Job::start("pdf-file-cancel-request", "main").unwrap();
        assert!(job.stop(Code::Cancelled));
        assert_eq!(save(&path, &pdf, &job), Err(Code::Cancelled));
        assert_eq!(fs::read(&path).unwrap(), pdf.bytes);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(validate(Path::new("relative.pdf")).is_err());
        assert!(validate(&directory.path().join("not-markdown.md")).is_err());
    }
}
