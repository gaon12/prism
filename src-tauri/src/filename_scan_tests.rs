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

#[cfg(windows)]
fn isolated(root: &Path) -> (FileIndex, FileIndex) {
    let (tx, _) = mpsc::channel();
    let index = FileIndex {
        catalog: Arc::new(RwLock::new(Catalog {
            roots: vec![Root {
                id: "fixture".into(),
                key: path_key(root),
                path: root.to_path_buf(),
                source: root.to_string_lossy().into_owned(),
            }],
            ..Default::default()
        })),
        generation: Arc::new(AtomicU64::new(1)),
        tx,
        lifetime: Arc::new(()),
    };
    (index.clone(), index)
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
