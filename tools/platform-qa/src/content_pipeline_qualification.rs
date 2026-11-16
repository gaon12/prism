//! An opt-in component benchmark using exact production extraction, window and
//! SQL publication. It is not an app admission, embedding or GUI benchmark.
use crate::{content_pipeline, content_store::ContentStore, document_extract, resource_policy};
use sha2::{Digest, Sha256};
use std::{fs, sync::atomic::AtomicBool, time::Instant};

#[test]
#[ignore = "opt-in actual extraction throughput; creates only synthetic temporary documents"]
fn actual_extraction_window_qualification() {
    if std::env::var("PRISM_DEV_CONTENT_PIPELINE_QUALIFY").as_deref() != Ok("1") {
        return;
    }
    let parallel = resource_policy::WorkBudget::current().workers_for(512 * 1024 * 1024);
    assert!(
        parallel > 1,
        "qualification requires budget for at least two extraction slots"
    );
    resource_policy::background_thread();
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("sources");
    fs::create_dir(&source).unwrap();
    let source = fs::canonicalize(source).unwrap();
    let mut paths = Vec::new();
    let mut input_hash = Sha256::new();
    // Identical, bounded input bytes are reused across all four cases. No live
    // service, private capture, user documents, model or download is involved.
    for id in 0..96 {
        let mut html = format!("<!doctype html><html><body><h1>Document {id}</h1>");
        for paragraph in 0..1200 {
            html.push_str(&format!(
                "<p>Content qualification {id} paragraph {paragraph}: bounded extraction preserves text and source identity.</p>"
            ));
        }
        html.push_str("</body></html>");
        input_hash.update((html.len() as u64).to_le_bytes());
        input_hash.update(html.as_bytes());
        let path = source.join(format!("document-{id:03}.html"));
        fs::write(&path, html).unwrap();
        paths.push(path);
    }
    let input_sha = format!("{:x}", input_hash.finalize());
    let roots = vec![source];
    let mut storage = crate::content_storage::ReadPolicy::default();
    assert!(
        storage.all_roots_nonseek(&roots),
        "parallel qualification requires a verified non-seek source volume"
    );
    let cancel = AtomicBool::new(false);
    let mut cases = Vec::new();
    let mut expected = None;
    for (case, workers) in [1, parallel, parallel, 1].into_iter().enumerate() {
        let store =
            ContentStore::open(&temporary.path().join(format!("index-{case}.sqlite3"))).unwrap();
        let mut output = Sha256::new();
        let mut published = 0usize;
        let started = Instant::now();
        let outcome = content_pipeline::run(
            workers,
            || true,
            |path: std::path::PathBuf| {
                document_extract::extract_document(&path, &Default::default(), &cancel).unwrap()
            },
            |document| {
                let bytes = serde_json::to_vec(&document).unwrap();
                output.update((bytes.len() as u64).to_le_bytes());
                output.update(bytes);
                store
                    .replace_document_versioned(
                        &document,
                        &roots,
                        Some("component-qualification"),
                        &cancel,
                    )
                    .unwrap();
                published += 1;
                true
            },
            |window| {
                for path in &paths {
                    assert!(window.push(path.clone()));
                }
            },
        );
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(outcome, content_pipeline::Outcome::Completed);
        assert_eq!(published, paths.len());
        let summaries = store.documents(&roots, &cancel).unwrap();
        assert_eq!(summaries.len(), paths.len());
        output.update(serde_json::to_vec(&summaries).unwrap());
        let output_sha = format!("{:x}", output.finalize());
        if let Some(expected) = &expected {
            assert_eq!(&output_sha, expected);
        } else {
            expected = Some(output_sha.clone());
        }
        cases.push(serde_json::json!({
            "case": case, "workers": workers, "milliseconds": elapsed_ms,
            "published": published, "semanticSha256": output_sha,
        }));
    }
    let mut checked_input = Sha256::new();
    for path in &paths {
        let bytes = fs::read(path).unwrap();
        checked_input.update((bytes.len() as u64).to_le_bytes());
        checked_input.update(bytes);
    }
    assert_eq!(format!("{:x}", checked_input.finalize()), input_sha);
    println!(
        "{}",
        serde_json::json!({
            "componentOnly": true, "actualExtraction": true, "actualSqlPublication": true,
            "appAdmissionExercised": false, "embeddingExercised": false, "guiExercised": false,
            "syntheticInputSha256": input_sha, "cases": cases,
        })
    );
}
