use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

fn fixture(directory: &Path) -> (Catalog, PathBuf) {
    let files = [
        ("engine.exe", b"engine".as_slice()),
        ("python/python.exe", b"python".as_slice()),
    ];
    let entries: Vec<Entry> = files
        .iter()
        .map(|(name, bytes)| Entry {
            path: name.to_string(),
            bytes: bytes.len() as u64,
            sha256: digest(bytes),
            unix_mode: None,
        })
        .collect();
    let raw = serde_json::to_vec(&entries).unwrap();
    let path = directory.join("test.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    for (name, bytes) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.start_file("inventory.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&raw).unwrap();
    zip.finish().unwrap();
    let bytes = fs::read(&path).unwrap();
    let mut value = catalog().unwrap();
    value.schema = 1;
    value.target = "windows-x64".into();
    value.python_home = None;
    value.executable = "python/python.exe".into();
    value.engine_path = "engine.exe".into();
    value.inventory_sha256 = digest(&raw);
    value.file_count = entries.len();
    value.expanded_bytes = entries.iter().map(|entry| entry.bytes).sum();
    value.archive.bytes = bytes.len() as u64;
    value.archive.sha256 = digest(&bytes);
    (value, path)
}

#[cfg(unix)]
fn linux_fixture(directory: &Path, zip_mode: u32) -> (Catalog, PathBuf) {
    let entries: Vec<Entry> = ["program/python.bin", "program/soffice.bin"]
        .iter()
        .map(|name| Entry {
            path: (*name).into(),
            bytes: 3,
            sha256: digest(b"elf"),
            unix_mode: Some(0o555),
        })
        .collect();
    let raw = serde_json::to_vec(&entries).unwrap();
    let path = directory.join("linux.zip");
    let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
    for entry in &entries {
        archive
            .start_file(
                &entry.path,
                zip::write::SimpleFileOptions::default().unix_permissions(zip_mode),
            )
            .unwrap();
        archive.write_all(b"elf").unwrap();
    }
    archive
        .start_file("inventory.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(&raw).unwrap();
    archive.finish().unwrap();
    let mut catalog: Catalog = serde_json::from_str(LINUX_CATALOG).unwrap();
    catalog.file_count = entries.len();
    catalog.expanded_bytes = 6;
    catalog.inventory_sha256 = digest(&raw);
    catalog.archive.bytes = fs::metadata(&path).unwrap().len();
    catalog.archive.sha256 = digest(&fs::read(&path).unwrap());
    (catalog, path)
}

#[cfg(unix)]
#[test]
fn linux_install_restores_pinned_modes_and_reopen_rejects_permission_changes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let (catalog, archive) = linux_fixture(directory.path(), 0o555);
    let parent = directory.path().join("cache");
    let root = install_archive(&parent, &archive, &catalog, &|| Ok(()), &|_| {}).unwrap();
    let executable = root.join("program/python.bin");
    assert_eq!(
        fs::metadata(&executable).unwrap().permissions().mode() & 0o7777,
        0o555
    );
    for changed in [0o444, 0o755, 0o4555] {
        fs::set_permissions(&executable, fs::Permissions::from_mode(changed)).unwrap();
        assert_eq!(
            verify(&root, &catalog, &|| Ok(()), &|_| {})
                .unwrap_err()
                .code,
            Code::Integrity
        );
    }
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o555)).unwrap();
    verify(&root, &catalog, &|| Ok(()), &|_| {}).unwrap();
}

#[cfg(unix)]
#[test]
fn linux_zip_mode_disagreement_never_promotes_a_stage() {
    let directory = tempfile::tempdir().unwrap();
    let (catalog, archive) = linux_fixture(directory.path(), 0o755);
    let parent = directory.path().join("cache");
    assert_eq!(
        install_archive(&parent, &archive, &catalog, &|| Ok(()), &|_| {})
            .unwrap_err()
            .code,
        Code::Integrity
    );
    assert!(!parent.join(catalog.directory()).exists());
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}
#[test]
fn archive_install_and_reopen_reject_extra_or_changed_modules() {
    let directory = tempfile::tempdir().unwrap();
    let (catalog, archive) = fixture(directory.path());
    let parent = directory.path().join("new/cache");
    let installed = install_archive(&parent, &archive, &catalog, &|| Ok(()), &|_| {}).unwrap();
    verify(&installed, &catalog, &|| Ok(()), &|_| {}).unwrap();
    assert_eq!(
        fs::read(installed.join("python/python.exe")).unwrap(),
        b"python"
    );
    assert_eq!(
        install_archive(&parent, &archive, &catalog, &|| Ok(()), &|_| {}).unwrap(),
        installed
    );
    fs::write(installed.join("python/sitecustomize.py"), "untrusted extra").unwrap();
    assert_eq!(
        verify(&installed, &catalog, &|| Ok(()), &|_| {})
            .unwrap_err()
            .code,
        Code::Integrity
    );
    fs::remove_file(installed.join("python/sitecustomize.py")).unwrap();
    fs::write(installed.join("engine.exe"), b"change").unwrap();
    assert_eq!(
        verify(&installed, &catalog, &|| Ok(()), &|_| {})
            .unwrap_err()
            .code,
        Code::Integrity
    );
    assert_eq!(fs::read(installed.join("engine.exe")).unwrap(), b"change");
}
#[test]
fn tampering_or_cancellation_never_promotes_or_escapes_a_stage() {
    let directory = tempfile::tempdir().unwrap();
    let (catalog, archive) = fixture(directory.path());
    let parent = directory.path().join("cache");
    let cancel = AtomicBool::new(false);
    let check = || {
        if cancel.load(Ordering::SeqCst) {
            Err(error(Code::Cancelled))
        } else {
            Ok(())
        }
    };
    assert_eq!(
        install_archive(&parent, &archive, &catalog, &check, &|_| {
            cancel.store(true, Ordering::SeqCst);
        })
        .unwrap_err()
        .code,
        Code::Cancelled
    );
    assert!(fs::read_dir(&parent).unwrap().next().is_none());
    let mut bytes = fs::read(&archive).unwrap();
    bytes[20] ^= 1;
    fs::write(&archive, bytes).unwrap();
    assert_eq!(
        install_archive(&parent, &archive, &catalog, &|| Ok(()), &|_| {})
            .unwrap_err()
            .code,
        Code::Integrity
    );
    assert!(fs::read_dir(&parent).unwrap().next().is_none());
    let outside = directory.path().join("unowned");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"user").unwrap();
    cleanup(&parent, &outside, &catalog.inventory_sha256);
    assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"user");
}
#[test]
fn inventory_contract_rejects_aliases_and_unknown_counts() {
    for value in [
        "",
        "../outside",
        "/absolute",
        "a\\b",
        "C:/x",
        "x/CON.txt",
        "x/a.",
        "x//b",
        "x/NUL",
    ] {
        assert!(relative(value).is_err(), "{value}");
    }
    let directory = tempfile::tempdir().unwrap();
    let (mut catalog, _) = fixture(directory.path());
    catalog.file_count += 1;
    let raw = serde_json::to_vec(&vec![Entry {
        path: "../bad".into(),
        bytes: 0,
        sha256: digest(b""),
        unix_mode: None,
    }])
    .unwrap();
    catalog.inventory_sha256 = digest(&raw);
    assert!(catalog.entries(&raw).is_err());
}
#[cfg(windows)]
#[test]
fn promotion_waits_for_our_live_reader_without_replacing_any_destination() {
    use std::{os::windows::fs::OpenOptionsExt, thread, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    let stage = directory.path().join("stage");
    let installed = directory.path().join("installed");
    fs::create_dir(&stage).unwrap();
    let path = stage.join("fixture.bin");
    fs::write(&path, b"owned").unwrap();
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    assert!(fs::rename(&stage, &installed).is_err());
    let closer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        drop(reader);
    });
    promote(&stage, &installed, &|| Ok(())).unwrap();
    closer.join().unwrap();
    assert_eq!(fs::read(installed.join("fixture.bin")).unwrap(), b"owned");
    fs::create_dir(&stage).unwrap();
    assert!(promote(&stage, &installed, &|| Ok(())).is_err());
    assert_eq!(fs::read(installed.join("fixture.bin")).unwrap(), b"owned");
}
#[cfg(unix)]
#[test]
fn linked_cache_ancestors_are_rejected_before_any_writes() {
    let directory = tempfile::tempdir().unwrap();
    let (catalog, archive) = fixture(directory.path());
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let link = directory.path().join("linked");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(install_archive(&link.join("new"), &archive, &catalog, &|| Ok(()), &|_| {}).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}
