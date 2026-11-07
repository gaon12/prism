use super::*;

#[test]
fn uncertain_paths_preserve_only_covered_subtrees_and_handle_bounded_overflow() {
    let mut pending = PendingPaths::default();
    pending.push(["/a", "/a-branch/x", "/한글/문서"].map(PathBuf::from));
    let uncertain = UncertainPaths::from(pending);
    for path in ["/a", "/a/file.md", "/a-branch/x/child", "/한글/문서/파일"] {
        assert!(uncertain.contains(&path_key(Path::new(path))));
    }
    for path in ["/a-b", "/a-branch", "/a-branch/y", "/한글/문서2", "/other"] {
        assert!(!uncertain.contains(&path_key(Path::new(path))));
    }
    let pending = Mutex::new(PendingPaths::default());
    note_unreadable(
        &pending,
        Path::new("/gone"),
        Some(std::io::ErrorKind::NotFound),
    );
    note_unreadable(
        &pending,
        Path::new("/denied"),
        Some(std::io::ErrorKind::PermissionDenied),
    );
    let uncertain = UncertainPaths::from(pending.into_inner().unwrap());
    assert!(!uncertain.contains(&path_key(Path::new("/gone"))));
    assert!(uncertain.contains(&path_key(Path::new("/denied/child"))));

    let mut pending = PendingPaths::default();
    pending.push((0..4097).map(|n| PathBuf::from(format!("/failed/{n}"))));
    let uncertain = UncertainPaths::from(pending);
    assert!(uncertain.all);
    assert!(uncertain.contains(&path_key(Path::new("/cached/unknown"))));
}

fn isolated(root: &Path) -> (FileIndex, FileIndex) {
    let (tx, _) = mpsc::channel();
    let index = FileIndex {
        catalog: Arc::new(RwLock::new(Catalog {
            roots: vec![Root {
                id: "fixture".into(),
                key: path_key(root),
                path: root.to_path_buf(),
                source: root.to_string_lossy().into_owned(),
                available: true,
            }],
            ..Default::default()
        })),
        generation: Arc::new(AtomicU64::new(1)),
        tx,
        lifetime: Arc::new(()),
    };
    (index.clone(), index)
}

fn scope(id: &str, path: &Path) -> SearchScope {
    SearchScope {
        id: id.into(),
        name: id.into(),
        path: path.to_string_lossy().into_owned(),
        filename_enabled: true,
        content_enabled: false,
        external_allowed: false,
    }
}

fn wait_for(index: &FileIndex, ready: impl Fn(&IndexStatus) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = index.status().unwrap();
        if ready(&status) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "index did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn unavailable_scope_refresh_preserves_cache_recovers_and_can_be_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let parked = temp.path().join("parked");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("cached.md"), "fixture").unwrap();
    let index = FileIndex::new();
    let scopes = vec![scope("folder", &root)];
    index.configure(scopes.clone());
    wait_for(&index, |s| s.initial_scan_complete && !s.scanning);
    assert_eq!(
        index
            .search("cached.md", "", Some("folder"), 10)
            .unwrap()
            .total,
        1
    );

    fs::rename(&root, &parked).unwrap();
    index.refresh(scopes.clone());
    wait_for(&index, |s| {
        s.initial_scan_complete && !s.scanning && !s.warnings.is_empty()
    });
    assert_eq!(
        index
            .search("cached.md", "", Some("folder"), 10)
            .unwrap()
            .total,
        1
    );
    assert!(!index.status().unwrap().watching);

    fs::rename(&parked, &root).unwrap();
    fs::remove_file(root.join("cached.md")).unwrap();
    fs::write(root.join("recovered.md"), "new fixture").unwrap();
    index.refresh(scopes.clone());
    wait_for(&index, |s| {
        s.initial_scan_complete && !s.scanning && s.warnings.is_empty()
    });
    assert_eq!(index.search("cached.md", "", None, 10).unwrap().total, 0);
    assert_eq!(
        index
            .search("recovered.md", "", Some("folder"), 10)
            .unwrap()
            .total,
        1
    );

    fs::rename(&root, &parked).unwrap();
    index.refresh(scopes.clone());
    wait_for(&index, |s| {
        s.initial_scan_complete && !s.scanning && !s.warnings.is_empty()
    });
    assert_eq!(index.search("recovered.md", "", None, 10).unwrap().total, 1);
    let mut disabled = scopes;
    disabled[0].filename_enabled = false;
    index.configure(disabled);
    assert_eq!(
        index.status().unwrap().count,
        0,
        "disabling revokes cached names immediately"
    );
    wait_for(&index, |s| s.initial_scan_complete && !s.scanning);
    assert_eq!(index.status().unwrap().count, 0);
}

#[test]
fn unavailable_parent_does_not_mask_changes_in_an_available_child_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    fs::write(root.join("outside-cached.md"), "old").unwrap();
    fs::write(child.join("deleted.md"), "remove later").unwrap();
    let (index, _owner) = isolated(&root);
    index.scan(1, std::slice::from_ref(&root), 1, true);
    // Represent a previously resolved root whose configured alias disappeared.
    // The old canonical target still exists; it must not be enumerated through
    // that unavailable scope. The real child is independently configured.
    let missing_alias = root.join("missing-alias");
    index.catalog.write().unwrap().roots[0].source = missing_alias.to_string_lossy().into_owned();
    fs::write(root.join("outside-cached.md"), "changed while unavailable").unwrap();
    fs::write(root.join("outside-new.md"), "must not discover").unwrap();
    fs::remove_file(child.join("deleted.md")).unwrap();
    fs::write(child.join("new-child.md"), "discover").unwrap();
    let (roots, warnings) =
        index.resolve_roots(&[scope("fixture", &missing_alias), scope("child", &child)]);
    assert!(!roots[0].available && roots[1].available);
    assert!(!warnings.is_empty());
    let paths = root_paths(&roots);
    assert_eq!(paths, [child]);
    index.catalog.write().unwrap().roots = roots;
    index.scan(1, &paths, 2, true);
    assert_eq!(
        index
            .search("outside-cached.md", "", None, 10)
            .unwrap()
            .results[0]
            .size,
        3
    );
    assert_eq!(
        index.search("outside-new.md", "", None, 10).unwrap().total,
        0
    );
    assert_eq!(index.search("deleted.md", "", None, 10).unwrap().total, 0);
    assert_eq!(
        index
            .search("new-child.md", "", Some("child"), 10)
            .unwrap()
            .total,
        1
    );
    assert!(index
        .resolve_result(root.join("outside-new.md").to_str().unwrap())
        .is_err());
    // Neither a new scope id nor a different source inherits the old identity.
    for changed in [
        scope("other", &missing_alias),
        scope("fixture", &root.join("different-alias")),
    ] {
        assert!(index.resolve_roots(&[changed]).0.is_empty());
    }
}

#[test]
fn missing_configured_root_keeps_known_names_until_scope_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let parked = temp.path().join("parked");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("cached.md"), "fixture").unwrap();
    let (index, _owner) = isolated(&root);
    index.scan(1, std::slice::from_ref(&root), 1, true);
    fs::rename(&root, &parked).unwrap();
    index.scan(1, std::slice::from_ref(&root), 2, false);
    assert_eq!(
        index.search("cached.md", "", None, 10).unwrap().total,
        1,
        "a configured root becoming unavailable does not revoke its cached names"
    );
    assert!(!index.status().unwrap().warnings.is_empty());
    index.configure(vec![]);
    index.scan(1, &[root], 3, true);
    assert_eq!(index.status().unwrap().count, 0);
}

#[cfg(windows)]
#[test]
fn unreadable_directories_keep_cached_filenames_and_real_deletions_still_disappear() {
    use std::os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, OPEN_EXISTING,
        },
    };
    for reconcile in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let blocked = root.join("blocked");
        fs::create_dir(&blocked).unwrap();
        let cached = blocked.join("cached.md");
        let deleted = root.join("deleted.md");
        fs::write(&cached, "retained metadata").unwrap();
        fs::write(&deleted, "removed later").unwrap();
        let (index, _owner) = isolated(&root);
        index.scan(1, std::slice::from_ref(&root), 1, true);
        assert_eq!(index.search("cached.md", "", None, 10).unwrap().total, 1);

        let wide: Vec<_> = blocked.as_os_str().encode_wide().chain(Some(0)).collect();
        // Deny directory enumeration with our own ordinary handle, not ACLs.
        // The process owns this new temporary directory and closes on unwind.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let held = unsafe { OwnedHandle::from_raw_handle(handle) };
        assert_eq!(fs::read_dir(&blocked).unwrap_err().raw_os_error(), Some(32));
        fs::remove_file(&deleted).unwrap();
        index.scan(1, std::slice::from_ref(&root), 2, reconcile);
        assert!(index.status().unwrap().skipped > 0);
        assert_eq!(
            index.search("cached.md", "", None, 10).unwrap().total,
            1,
            "an unreadable subtree is unknown, not a deletion"
        );
        assert_eq!(index.search("deleted.md", "", None, 10).unwrap().total, 0);

        drop(held);
        fs::remove_file(&cached).unwrap();
        index.scan(1, std::slice::from_ref(&root), 3, true);
        assert_eq!(index.search("cached.md", "", None, 10).unwrap().total, 0);
        assert_eq!(index.status().unwrap().skipped, 0);
        index.configure(vec![]);
        index.scan(1, &[root], 4, true);
        assert_eq!(
            index.status().unwrap().count,
            0,
            "revoked scans cannot republish"
        );
    }
}
