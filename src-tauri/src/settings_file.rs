//! JSON persistence shared by settings, search sessions and update records.
use serde::Serialize;
use std::{fs, path::Path};

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let parent = path.parent().ok_or("No parent directory for settings.")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    crate::atomic_file::write(path, &contents)
        .map_err(|error| format!("Could not save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_json_keeps_the_existing_utf8_format_and_replaces_the_record() {
        #[derive(Serialize)]
        struct Record<'a> {
            revision: u64,
            label: &'a str,
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("profile/settings.json");
        write_json(
            &path,
            &Record {
                revision: 1,
                label: "before",
            },
        )
        .unwrap();
        write_json(
            &path,
            &Record {
                revision: 7,
                label: "한글",
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\n  \"revision\": 7,\n  \"label\": \"한글\"\n}"
        );
    }

    #[test]
    fn settings_save_preserves_a_file_linked_at_the_old_pending_name() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let original = root.path().join("unrelated.json");
        fs::write(&original, b"preserve another record").unwrap();
        let stale = path.with_extension("pending");
        fs::hard_link(&original, &stale).unwrap();
        write_json(&path, &serde_json::json!({"revision": 7})).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"preserve another record");
        assert_eq!(fs::read(&stale).unwrap(), b"preserve another record");
        assert_eq!(fs::read_to_string(path).unwrap(), "{\n  \"revision\": 7\n}");
    }

    #[test]
    fn serialization_failure_keeps_all_existing_files_and_creates_nothing() {
        struct Rejected;
        impl Serialize for Rejected {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture serialization failure"))
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        fs::write(&path, b"original record").unwrap();
        fs::write(path.with_extension("pending"), b"previous temporary data").unwrap();
        assert!(write_json(&path, &Rejected).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original record");
        assert_eq!(
            fs::read(path.with_extension("pending")).unwrap(),
            b"previous temporary data"
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
