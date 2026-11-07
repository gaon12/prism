//! Metadata fixtures exercise revision/permission boundaries; real old→new
//! app conversion and pixels are checked separately by office-upgrade.mjs.
use super::*;

fn fixture() -> (tempfile::TempDir, CollectionService, Work, Unit, Source) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let service = CollectionService::open(&data, |_| {}).unwrap();
    let collection = service.store.create_collection("Upgrade", "").unwrap();
    let work = service
        .store
        .create_work(&collection.id, "Book", WorkMetadata::default())
        .unwrap();
    let unit = service
        .store
        .create_unit(&work.id, "Unit", UnitKind::Document)
        .unwrap();
    let original = dir.path().join("source.docx");
    fs::write(
        &original,
        include_bytes!("../../tests/fixtures/encrypted-office/example.docx"),
    )
    .unwrap();
    let source = service
        .store
        .add_source(&unit.id, &original, SourceKind::Document)
        .unwrap();
    let unit = service.store.unit(&unit.id).unwrap();
    (dir, service, work, unit, source)
}
fn page(source: &Source, number: u32, version: char, hash: &str) -> Page {
    import::page_location(
        source,
        PageLocation {
            kind: PageKind::Office,
            entry: None,
            number,
            renderer: Some(format!(
                "{}:{}",
                version.to_string().repeat(64),
                "f".repeat(64)
            )),
        },
        number.to_string(),
        hash,
    )
}

#[test]
fn renderer_replacement_preserves_interleaved_order_and_inserts_new_pages_after_the_source() {
    let (_dir, service, _, unit, source) = fixture();
    let other = service
        .store
        .add_source(
            &unit.id,
            &Path::new(&source.path)
                .parent()
                .unwrap()
                .join("metadata-only-image.png"),
            SourceKind::Image,
        )
        .unwrap();
    let mut image = page(&other, 1, 'a', &"a".repeat(64));
    image.location.kind = PageKind::Image;
    image.location.renderer = None;
    let mut second = page(&source, 2, 'a', &"a".repeat(64));
    second.title = "Custom title".into();
    let old = vec![
        second,
        image.clone(),
        page(&source, 1, 'a', &"a".repeat(64)),
    ];
    let replacements = HashMap::from([(
        source.id.clone(),
        (1..=3)
            .map(|n| (n, page(&source, n, 'b', &"a".repeat(64))))
            .collect(),
    )]);
    let next = refreshed_order(old, replacements);
    assert_eq!(
        next.iter().map(|p| p.location.number).collect::<Vec<_>>(),
        [2, 1, 1, 3]
    );
    assert_eq!(next[1].id, image.id);
    assert_eq!(next[0].title, "Custom title");
    assert!(next.iter().filter(|p| p.source_id == source.id).all(|p| p
        .location
        .renderer
        .as_deref()
        == Some(format!("{}:{}", "b".repeat(64), "f".repeat(64)).as_str())));
}

#[test]
fn office_bookmark_remapping_is_atomic_and_never_crosses_a_changed_original() {
    let (_dir, service, work, unit, source) = fixture();
    let store = &service.store;
    let hash = import::hash(&fs::read(&source.path).unwrap());
    let old: Vec<_> = (1..=3).map(|n| page(&source, n, 'a', &hash)).collect();
    let unit = store
        .replace_pages(
            &unit.id,
            unit.revision,
            old.clone(),
            &AtomicBool::new(false),
        )
        .unwrap();
    store
        .save_reading(&work.id, Some(&old[2].id), true)
        .unwrap();
    let next = vec![page(&source, 2, 'b', &hash), page(&source, 1, 'b', &hash)];
    assert_eq!(
        store
            .replace_pages(
                &unit.id,
                unit.revision,
                next.clone(),
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(
        store.reading(&work.id).unwrap().page_id.as_deref(),
        Some(old[2].id.as_str())
    );
    assert_eq!(
        store
            .replace_pages(
                &unit.id,
                unit.revision - 1,
                next.clone(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let unit = store
        .replace_pages(
            &unit.id,
            unit.revision,
            next.clone(),
            &AtomicBool::new(false),
        )
        .unwrap();
    let reading = store.reading(&work.id).unwrap();
    assert_eq!(reading.page_id.as_deref(), Some(next[0].id.as_str()));
    assert!(reading.completed);
    assert!(!store
        .outdated_office(&unit.id, next[0].location.renderer.as_deref().unwrap())
        .unwrap());
    let changed = vec![page(&source, 1, 'c', &"e".repeat(64))];
    store
        .replace_pages(&unit.id, unit.revision, changed, &AtomicBool::new(false))
        .unwrap();
    assert!(store.reading(&work.id).unwrap().page_id.is_none());
}

#[test]
fn failed_preparation_preserves_the_unit_and_reports_a_nonretrying_terminal_phase() {
    let (_dir, mut service, work, unit, source) = fixture();
    let events = Arc::new(Mutex::new(Vec::new()));
    let output = events.clone();
    Arc::get_mut(&mut service.inner).unwrap().emit =
        Box::new(move |event| output.lock().unwrap().push(event));
    let hash = import::hash(&fs::read(&source.path).unwrap());
    let old = page(&source, 1, 'a', &hash);
    let unit = service
        .store
        .replace_pages(
            &unit.id,
            unit.revision,
            [old.clone()],
            &AtomicBool::new(false),
        )
        .unwrap();
    service
        .store
        .save_reading(&work.id, Some(&old.id), true)
        .unwrap();
    assert_eq!(
        service.prepare_unit(&unit.id).unwrap_err().code,
        ErrorCode::RendererUnavailable
    );
    assert_eq!(
        service.store.unit(&unit.id).unwrap().revision,
        unit.revision
    );
    assert_eq!(
        service.store.reading(&work.id).unwrap().page_id.as_deref(),
        Some(old.id.as_str())
    );
    assert!(service.jobs().is_empty());
    assert_eq!(
        events.lock().unwrap().last().unwrap().phase,
        "prepareFailed"
    );
    fs::write(&source.path, b"changed original").unwrap();
    assert_eq!(
        service.prepare_unit(&unit.id).unwrap_err().code,
        ErrorCode::SourceChanged
    );
    assert_eq!(
        service.store.unit(&unit.id).unwrap().revision,
        unit.revision
    );
    assert_eq!(
        service.store.reading(&work.id).unwrap().page_id.as_deref(),
        Some(old.id.as_str())
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.phase == "prepareFailed")
            .count(),
        2
    );
    assert!(service.jobs().is_empty());
}
