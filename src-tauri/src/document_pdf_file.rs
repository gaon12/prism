//! Atomic replacement of a user-selected PDF destination. Only a new, random
//! staging file is removed on failure; an existing destination stays intact.
use crate::{
    document_export_file,
    document_pdf::{Code, Pdf, Result},
    document_pdf_job::Job,
};
use std::{fs, path::Path};
impl From<document_export_file::Error> for Code {
    fn from(value: document_export_file::Error) -> Self {
        match value {
            document_export_file::Error::Input => Self::Input,
            document_export_file::Error::Storage => Self::Storage,
        }
    }
}
pub fn validate(path: &Path) -> Result<()> {
    document_export_file::validate(path, "pdf").map_err(Into::into)
}
pub fn save(path: &Path, pdf: &Pdf, job: &Job) -> Result<()> {
    validate(path)?;
    job.control.check()?;
    if pdf.bytes.len() > crate::document_pdf::MAX_PDF || !pdf.bytes.starts_with(b"%PDF-") {
        return Err(Code::Input);
    }
    document_export_file::save(
        path,
        "pdf",
        &pdf.bytes,
        crate::document_pdf::MAX_PDF,
        || job.control.check(),
        |stage, destination| {
            job.commit(|| fs::rename(stage, destination).map_err(|_| Code::Storage))
        },
    )
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
