use super::*;
use crate::embedding_assets::{for_platform, PACKAGES};
use flate2::{write::GzEncoder, Compression};

fn tar_file(root: &Path, entries: &[(&str, tar::EntryType, &str, &[u8])]) -> PathBuf {
    let path = root.join("fixture.tar.gz");
    let mut builder = tar::Builder::new(GzEncoder::new(
        File::create(&path).unwrap(),
        Compression::fast(),
    ));
    for (path, kind, target, body) in entries {
        let mut header = tar::Header::new_gnu();
        // Raw names deliberately allow malformed fixture paths through the writer.
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(*kind);
        header.set_size(body.len() as u64);
        header.set_mode(if path.ends_with("llama-server") {
            0o755
        } else {
            0o644
        });
        if !target.is_empty() {
            header.as_mut_bytes()[157..157 + target.len()].copy_from_slice(target.as_bytes());
        }
        header.set_cksum();
        builder.append(&header, *body).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
    path
}
fn destination(root: &Path) -> PathBuf {
    let path = root.join("runtime");
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn internal_library_aliases_are_regular_copies_and_all_installed_bytes_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let dir = destination(temp.path());
    let archive = tar_file(
        temp.path(),
        &[
            (
                "llama-b11960/llama-server",
                tar::EntryType::Regular,
                "",
                b"server",
            ),
            (
                "llama-b11960/lib.so.1.0",
                tar::EntryType::Regular,
                "",
                b"library",
            ),
            (
                "llama-b11960/lib.so.1",
                tar::EntryType::Symlink,
                "lib.so.1.0",
                b"",
            ),
            (
                "llama-b11960/lib.so",
                tar::EntryType::Symlink,
                "lib.so.1",
                b"",
            ),
            (
                "llama-b11960/server-copy",
                tar::EntryType::Link,
                "llama-b11960/llama-server",
                b"",
            ),
        ],
    );
    let package = for_platform("linux", "x86_64").unwrap();
    inspect(&archive, &dir, package, true, &|| Ok(())).unwrap();
    for name in ["lib.so.1.0", "lib.so.1", "lib.so"] {
        assert_eq!(fs::read(dir.join(name)).unwrap(), b"library");
        assert!(!fs::symlink_metadata(dir.join(name))
            .unwrap()
            .file_type()
            .is_symlink());
    }
    inspect(&archive, &dir, package, false, &|| Ok(())).unwrap();
    fs::write(dir.join("lib.so"), b"tamper!").unwrap();
    assert!(inspect(&archive, &dir, package, false, &|| Ok(())).is_err());
    fs::write(dir.join("lib.so"), b"library").unwrap();
    fs::write(dir.join("unexpected.so"), b"untrusted library").unwrap();
    assert!(inspect(&archive, &dir, package, false, &|| Ok(())).is_err());
}

#[test]
fn paths_external_links_cycles_and_duplicate_names_are_rejected() {
    for (path, kind, target) in [
        ("llama-b11960/../escape", tar::EntryType::Regular, ""),
        ("/absolute", tar::EntryType::Regular, ""),
        ("llama-b11960/file:stream", tar::EntryType::Regular, ""),
        ("llama-b11960/alias", tar::EntryType::Symlink, "../outside"),
        ("llama-b11960/alias", tar::EntryType::Symlink, "/etc/passwd"),
        ("llama-b11960/alias", tar::EntryType::Symlink, "missing.so"),
        ("llama-b11960/alias", tar::EntryType::Link, "../outside"),
        ("llama-b11960/llama-server", tar::EntryType::Regular, ""),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = destination(temp.path());
        let archive = tar_file(
            temp.path(),
            &[
                (
                    "llama-b11960/llama-server",
                    tar::EntryType::Regular,
                    "",
                    b"server",
                ),
                (path, kind, target, b""),
            ],
        );
        assert!(
            inspect(
                &archive,
                &dir,
                for_platform("linux", "x86_64").unwrap(),
                true,
                &|| Ok(())
            )
            .is_err(),
            "{path} {target}"
        );
        assert!(!temp.path().join("escape").exists());
    }
    let temp = tempfile::tempdir().unwrap();
    let dir = destination(temp.path());
    let archive = tar_file(
        temp.path(),
        &[
            (
                "llama-b11960/llama-server",
                tar::EntryType::Regular,
                "",
                b"server",
            ),
            ("llama-b11960/a", tar::EntryType::Symlink, "b", b""),
            ("llama-b11960/b", tar::EntryType::Symlink, "a", b""),
        ],
    );
    assert!(inspect(
        &archive,
        &dir,
        for_platform("linux", "x86_64").unwrap(),
        true,
        &|| Ok(())
    )
    .is_err());
}

#[test]
fn oversized_entries_and_cancelled_extraction_stop_before_unbounded_writes() {
    let temp = tempfile::tempdir().unwrap();
    let dir = destination(temp.path());
    let path = temp.path().join("oversized.tar.gz");
    let mut header = tar::Header::new_gnu();
    header.set_path("llama-b11960/llama-server").unwrap();
    header.set_size(MAX_EXPANDED + 1);
    header.set_mode(0o755);
    header.set_cksum();
    let mut writer = GzEncoder::new(File::create(&path).unwrap(), Compression::fast());
    writer.write_all(header.as_bytes()).unwrap();
    writer.finish().unwrap();
    assert!(inspect(
        &path,
        &dir,
        for_platform("linux", "x86_64").unwrap(),
        true,
        &|| Ok(())
    )
    .is_err());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    let archive = tar_file(
        temp.path(),
        &[(
            "llama-b11960/llama-server",
            tar::EntryType::Regular,
            "",
            b"server",
        )],
    );
    assert_eq!(
        inspect(
            &archive,
            &dir,
            for_platform("linux", "x86_64").unwrap(),
            true,
            &|| Err("cancelled".into())
        )
        .unwrap_err(),
        "cancelled"
    );
}

#[test]
#[ignore = "Requires PRISM_TEST_PACKAGES_DIR containing all five pinned official archives"]
fn actual_upstream_packages_extract_verify_and_materialize_all_aliases() {
    let source =
        PathBuf::from(std::env::var_os("PRISM_TEST_PACKAGES_DIR").expect("archive cache required"));
    for package in PACKAGES {
        let archive = source.join(package.asset.filename);
        let (size, digest) = hash(File::open(&archive).unwrap(), &|| Ok(())).unwrap();
        assert_eq!(size, package.asset.bytes);
        assert_eq!(digest, package.asset.sha256);
        let temp = tempfile::tempdir().unwrap();
        inspect(&archive, temp.path(), package, true, &|| Ok(())).unwrap();
        inspect(&archive, temp.path(), package, false, &|| Ok(())).unwrap();
        assert!(temp.path().join(package.executable).is_file());
        for file in fs::read_dir(temp.path()).unwrap() {
            assert!(!file.unwrap().file_type().unwrap().is_symlink());
        }
        println!("verified {}", package.target);
    }
}
