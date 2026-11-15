use super::*;
use crate::file_policy::Categories;
use prism_ntfs_protocol::FileRecord;

fn directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("prism-filename-policy-")
        .tempdir()
        .unwrap()
}
fn root(path: &Path, id: &str) -> Root {
    Root {
        id: id.into(),
        key: path_key(path),
        path: path.to_owned(),
        source: path.to_string_lossy().into_owned(),
        available: true,
    }
}
fn fixture(path: &Path, categories: Categories) -> (FileIndex, FileIndex) {
    let (tx, _) = mpsc::channel();
    let index = FileIndex {
        published: Arc::new(status::Store::default()),
        catalog: Arc::new(RwLock::new(Catalog {
            generation: 1,
            roots: vec![root(path, "fixture")],
            file_policy: categories,
            ..Default::default()
        })),
        generation: Arc::new(AtomicU64::new(1)),
        tx,
        lifetime: Arc::new(()),
    };
    (index.clone(), index)
}
fn record(path: &Path) -> FileRecord {
    FileRecord {
        path: path.to_string_lossy().into_owned(),
        is_dir: path.is_dir(),
        size: 7,
        modified_ms: Some(17),
    }
}
fn seed(index: &FileIndex, paths: &[PathBuf]) {
    let mut state = index
        .write_catalog(status_profile::WriterKind::Metadata)
        .unwrap();
    for path in paths {
        state
            .entries
            .insert(&Entry::new(catalog::raw(record(path)), 1));
    }
    state.status.count = state.entries.len();
    state.status.revision += 1;
}
fn selection(index: &FileIndex, query: &str, extension: &str) -> FilenameSearch {
    FilenameSearch {
        query: query.into(),
        extension: extension.into(),
        scope_id: Some("fixture".into()),
        index_revision: index.status().unwrap().revision,
        settings_revision: 0,
    }
}
fn snapshot(index: &FileIndex, selection: &FilenameSearch) -> (Vec<String>, bool) {
    let mut paths = vec![];
    let partial = index
        .snapshot_files(selection, true, &AtomicBool::new(false), &mut |path| {
            paths.push(path.to_owned());
            Ok::<_, ()>(())
        })
        .unwrap_or_else(|_| panic!("owned snapshot failed"));
    (paths, partial)
}

#[test]
fn cached_query_admission_backfills_before_limits_and_guards_snapshot_and_open() {
    let dir = directory();
    let categories = Categories {
        source_code: false,
        ..Default::default()
    };
    let (index, _owner) = fixture(dir.path(), categories);
    let mut paths = vec![];
    for n in 0..140 {
        let path = dir.path().join(format!("needle-a{n:03}.rs"));
        fs::write(&path, "not read by filename matching").unwrap();
        paths.push(path);
    }
    let allowed = dir.path().join("needle-z.txt");
    fs::write(&allowed, "owned").unwrap();
    paths.push(allowed.clone());
    seed(&index, &paths);
    let response = index.search("needle", "", Some("fixture"), 1).unwrap();
    assert_eq!(response.total, 1);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].name, "needle-z.txt");
    assert!(index.resolve_result(paths[0].to_str().unwrap()).is_err());
    assert!(index.resolve_result(allowed.to_str().unwrap()).is_ok());
    let (selected, _) = snapshot(&index, &selection(&index, "needle", ""));
    assert_eq!(selected.len(), 1);
    assert!(selected[0].ends_with("needle-z.txt"));
    let cancelled = AtomicBool::new(true);
    assert!(index
        .snapshot_files(
            &selection(&index, "needle", ""),
            true,
            &cancelled,
            &mut |_| Ok::<_, ()>(())
        )
        .is_err());
}

#[test]
fn reconfiguration_detaches_cached_and_provisional_rows_and_preserves_legacy_policy() {
    let dir = directory();
    let path = dir.path().join("owned.rs");
    fs::write(&path, "owned").unwrap();
    let (index, _owner) = fixture(dir.path(), Categories::default());
    seed(&index, std::slice::from_ref(&path));
    let old = selection(&index, "", "");
    let policy = Categories {
        source_code: false,
        ..Default::default()
    };
    let scope = SearchScope {
        id: "fixture".into(),
        name: "fixture".into(),
        path: dir.path().to_string_lossy().into_owned(),
        filename_enabled: true,
        content_enabled: true,
        external_allowed: false,
    };
    index.configure_with_policy(vec![scope.clone()], policy);
    assert_eq!(index.status().unwrap().count, 0);
    assert!(index.catalog.read().unwrap().entries.is_empty());
    assert!(index
        .snapshot_files(
            &old,
            true,
            &AtomicBool::new(false),
            &mut |_| Ok::<_, ()>(())
        )
        .is_err());
    for action in [FileIndex::refresh, FileIndex::configure, FileIndex::reset] {
        action(&index, vec![scope.clone()]);
        assert_eq!(index.catalog.read().unwrap().file_policy, policy);
    }
    let (temporary, _owner) = fixture(dir.path(), policy);
    let shared = Arc::new(Mutex::new(Entries::new()));
    shared
        .lock()
        .unwrap()
        .insert(&Entry::new(catalog::raw(record(&path)), 1));
    temporary.show_provisional(1, &shared, &Arc::new(status::Progress::default()));
    assert!(temporary.catalog.read().unwrap().provisional.is_some());
    temporary.configure_with_policy(vec![scope], policy);
    assert!(temporary.catalog.read().unwrap().provisional.is_none());
    assert!(temporary.publish_provisional(1, &shared).is_err());
    assert_eq!(temporary.search("", "", None, 10).unwrap().total, 0);
}

#[test]
fn ordinary_collection_prunes_excluded_entries_before_catalog_preparation() {
    let dir = directory();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
    for name in [
        "visible.md",
        "script.rs",
        "archive.zip",
        ".hidden.md",
        "ignored.md",
    ] {
        fs::write(dir.path().join(name), "owned").unwrap();
    }
    let categories = Categories {
        hidden: false,
        system: false,
        source_code: false,
        repository_ignored: false,
        archives: false,
    };
    let (index, _owner) = fixture(dir.path(), categories);
    index.scan(1, &[dir.path().to_owned()], 1, true);
    let results = index.search("", "md", None, 20).unwrap();
    assert_eq!(results.total, 1);
    assert_eq!(results.results[0].name, "visible.md");
    let state = index.catalog.read().unwrap();
    for name in ["script.rs", "archive.zip", ".hidden.md", "ignored.md"] {
        assert!(!state
            .entries
            .contains_key(&path_key(&dir.path().join(name))));
    }
}

#[test]
fn nested_filename_root_survives_parent_pruning_and_removal_restores_parent_rules() {
    let dir = directory();
    let child = dir.path().join(".private/docs");
    fs::create_dir_all(&child).unwrap();
    let path = child.join("visible.md");
    fs::write(&path, "owned").unwrap();
    let (index, _owner) = fixture(
        dir.path(),
        Categories {
            hidden: false,
            ..Default::default()
        },
    );
    index
        .catalog
        .write()
        .unwrap()
        .roots
        .push(root(&child, "child"));
    let roots = index.catalog.read().unwrap().roots.clone();
    let paths = index.collection_roots(&roots);
    assert_eq!(paths.len(), 2);
    index.scan(1, &paths, 1, true);
    assert_eq!(index.search("visible", "md", None, 10).unwrap().total, 1);
    // This is a parent watch update, not another complete-root scan.
    index.scan_changes(1, &[dir.path().join(".private")], &[], 2);
    assert_eq!(index.search("visible", "md", None, 10).unwrap().total, 1);
    index
        .catalog
        .write()
        .unwrap()
        .roots
        .retain(|root| root.id != "child");
    assert_eq!(index.search("visible", "md", None, 10).unwrap().total, 0);
    index.scan(1, &[dir.path().to_owned()], 3, true);
    assert!(!index
        .catalog
        .read()
        .unwrap()
        .entries
        .contains_key(&path_key(&path)));
}

#[test]
fn current_repository_rules_filter_cached_rows_and_fail_closed_on_missing_evidence() {
    let dir = directory();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let ignored = dir.path().join("ignored.md");
    let allowed = dir.path().join("allowed.md");
    for path in [&ignored, &allowed] {
        fs::write(path, "owned").unwrap();
    }
    let rules = dir.path().join(".gitignore");
    fs::write(&rules, "ignored.md\n").unwrap();
    let (index, _owner) = fixture(
        dir.path(),
        Categories {
            repository_ignored: false,
            ..Default::default()
        },
    );
    seed(&index, &[ignored.clone(), allowed.clone()]);
    let hits = index.search("", "md", None, 1).unwrap();
    assert_eq!(hits.total, 1);
    assert_eq!(hits.results[0].name, "allowed.md");
    fs::write(&rules, "allowed.md\n").unwrap();
    let hits = index.search("", "md", None, 1).unwrap();
    assert_eq!(hits.total, 1);
    assert_eq!(hits.results[0].name, "ignored.md");
    fs::write(&rules, [0xff, 0xfe]).unwrap();
    assert_eq!(index.search("", "md", None, 10).unwrap().total, 0);
    assert!(snapshot(&index, &selection(&index, "", "md")).0.is_empty());
    assert!(index.resolve_result(allowed.to_str().unwrap()).is_err());
}

#[test]
fn legacy_service_rows_are_filtered_and_provisional_snapshots_use_the_same_policy() {
    let dir = directory();
    let good = dir.path().join("owned.docx");
    let bad = dir.path().join("owned.rs");
    for path in [&good, &bad] {
        fs::write(path, "not parsed").unwrap();
    }
    let policy = Categories {
        source_code: false,
        archives: false,
        ..Default::default()
    };
    let (index, _owner) = fixture(dir.path(), policy);
    index
        .collect_service_snapshot(1, dir.path(), 1, |_, receive| {
            receive(Event::Records(vec![record(&good), record(&bad)]))?;
            Ok(1)
        })
        .unwrap();
    assert_eq!(index.status().unwrap().count, 1);
    assert!(!index
        .catalog
        .read()
        .unwrap()
        .entries
        .contains_key(&path_key(&bad)));
    let (temporary, _owner) = fixture(dir.path(), policy);
    let shared = Arc::new(Mutex::new(Entries::new()));
    for path in [&good, &bad] {
        shared
            .lock()
            .unwrap()
            .insert(&Entry::new(catalog::raw(record(path)), 1));
    }
    temporary.show_provisional(1, &shared, &Arc::new(status::Progress::default()));
    let (paths, partial) = snapshot(&temporary, &selection(&temporary, "owned", ""));
    assert!(partial);
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("owned.docx"));
    temporary.drop_provisional(1);
    assert_eq!(temporary.status().unwrap().count, 0);
}

#[cfg(windows)]
#[test]
fn attributed_service_keeps_known_zero_distinct_from_unavailable_leaf_attributes() {
    use prism_ntfs_protocol::AttributedFileRecord;
    let dir = directory();
    let existing = dir.path().join("exists.docx");
    fs::write(&existing, "owned").unwrap();
    let policy = Categories {
        hidden: false,
        system: false,
        archives: false,
        ..Default::default()
    };
    let (index, _owner) = fixture(dir.path(), policy);
    let rows = [
        (&existing, None),
        (&dir.path().join("known-missing.docx"), Some(0)),
        (&dir.path().join("unknown-missing.docx"), None),
        (&dir.path().join("hidden.docx"), Some(2)),
        (&dir.path().join("system.docx"), Some(4)),
        (&dir.path().join("container.hwpx"), Some(0x820)),
    ]
    .into_iter()
    .map(|(path, attributes)| AttributedFileRecord {
        record: record(path),
        attributes,
    })
    .collect();
    index
        .collect_service_snapshot_mode(1, dir.path(), 1, true, |_, receive| {
            receive(Event::AttributedRecords(rows))?;
            Ok(1)
        })
        .unwrap();
    let state = index.catalog.read().unwrap();
    assert_eq!(state.entries.len(), 3);
    assert_eq!(state.status.skipped, 1);
    assert!(state
        .entries
        .contains_key(&path_key(&dir.path().join("known-missing.docx"))));
    assert!(!state
        .entries
        .contains_key(&path_key(&dir.path().join("unknown-missing.docx"))));
    drop(state);
    assert_eq!(index.search("", "", None, 10).unwrap().total, 1);
    assert!(index
        .resolve_result(dir.path().join("known-missing.docx").to_str().unwrap())
        .is_err());
}

#[test]
fn attributed_mode_rejects_legacy_or_mixed_events_without_publishing_rows() {
    use prism_ntfs_protocol::AttributedFileRecord;
    let dir = directory();
    let path = dir.path().join("owned.md");
    fs::write(&path, "owned").unwrap();
    for mixed in [false, true] {
        let (index, _owner) = fixture(dir.path(), Categories::default());
        let result = index.collect_service_snapshot_mode(1, dir.path(), 1, true, |_, receive| {
            if mixed {
                receive(Event::AttributedRecords(vec![AttributedFileRecord {
                    record: record(&path),
                    attributes: Some(0),
                }]))?;
            }
            receive(Event::Records(vec![record(&path)]))?;
            Ok(1)
        });
        assert!(result.is_err());
        assert!(index.catalog.read().unwrap().entries.is_empty());
        assert!(index.catalog.read().unwrap().provisional.is_none());
    }
}

#[test]
fn repository_rule_watch_change_restores_a_previously_pruned_file() {
    let dir = directory();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let rules = dir.path().join(".gitignore");
    fs::write(&rules, "restore.md\n").unwrap();
    fs::write(dir.path().join("restore.md"), "owned").unwrap();
    let index = FileIndex::new();
    index.configure_with_policy(
        vec![SearchScope {
            id: "fixture".into(),
            name: "fixture".into(),
            path: dir.path().to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }],
        Categories {
            repository_ignored: false,
            ..Default::default()
        },
    );
    let wait = |ready: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "owned watch did not settle"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(&|| {
        index
            .status()
            .is_ok_and(|status| status.initial_scan_complete && !status.scanning)
    });
    assert_eq!(index.search("restore", "md", None, 10).unwrap().total, 0);
    fs::write(&rules, "").unwrap();
    wait(&|| {
        index
            .search("restore", "md", None, 10)
            .is_ok_and(|result| result.total == 1)
    });
    fs::write(&rules, "restore.md\n").unwrap();
    wait(&|| {
        !index
            .catalog
            .read()
            .unwrap()
            .entries
            .contains_key(&path_key(&dir.path().join("restore.md")))
    });
    assert_eq!(index.search("restore", "md", None, 10).unwrap().total, 0);
}

#[cfg(windows)]
#[test]
fn native_hidden_attribute_watch_removes_and_restores_an_unchanged_file() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW};
    struct Restore {
        wide: Vec<u16>,
        flags: u32,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            // Only this fixture's own file; restore even when an assertion fails.
            unsafe {
                SetFileAttributesW(self.wide.as_ptr(), self.flags);
            }
        }
    }
    let dir = directory();
    let path = dir.path().join("attribute.md");
    fs::write(&path, "owned").unwrap();
    let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = unsafe { GetFileAttributesW(wide.as_ptr()) };
    assert_ne!(flags, u32::MAX);
    let restore = Restore { wide, flags };
    let index = FileIndex::new();
    index.configure_with_policy(
        vec![SearchScope {
            id: "fixture".into(),
            name: "fixture".into(),
            path: dir.path().to_string_lossy().into_owned(),
            filename_enabled: true,
            content_enabled: false,
            external_allowed: false,
        }],
        Categories {
            hidden: false,
            ..Default::default()
        },
    );
    let wait = |present: bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while index
            .catalog
            .read()
            .unwrap()
            .entries
            .contains_key(&path_key(&path))
            != present
        {
            assert!(
                std::time::Instant::now() < deadline,
                "owned attribute watch did not settle"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(true);
    let before = fs::metadata(&path).unwrap();
    assert_ne!(
        unsafe { SetFileAttributesW(restore.wide.as_ptr(), flags | 2) },
        0
    );
    wait(false);
    let hidden = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), hidden.len());
    assert_eq!(before.modified().unwrap(), hidden.modified().unwrap());
    assert_eq!(index.search("attribute", "md", None, 10).unwrap().total, 0);
    assert_ne!(
        unsafe { SetFileAttributesW(restore.wide.as_ptr(), flags) },
        0
    );
    wait(true);
    assert_eq!(index.search("attribute", "md", None, 10).unwrap().total, 1);
}
