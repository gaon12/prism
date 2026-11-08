//! Full archive and installed-byte verification, without executing an asset.
//! This is not native Mac import, inference or application qualification.
use super::*;
use std::{
    cell::Cell,
    time::{Duration, Instant},
};

#[test]
#[ignore = "Requires all 129 pinned Mac ARM64 artifacts and a new exclusive 3.3GB data output; never executes them"]
fn actual_macos_reference_extracts_and_authenticates_every_installed_file() {
    let source = PathBuf::from(
        std::env::var_os("PRISM_TEST_MAC_PADDLE_ARTIFACTS")
            .expect("Set PRISM_TEST_MAC_PADDLE_ARTIFACTS to the authenticated source directory"),
    );
    let output = PathBuf::from(
        std::env::var_os("PRISM_TEST_MAC_PADDLE_DATA_OUTPUT")
            .expect("Set PRISM_TEST_MAC_PADDLE_DATA_OUTPUT to a new data-only output directory"),
    );
    let source = crate::path_identity::canonical(&source).unwrap();
    assert!(output.is_absolute());
    let parent = crate::path_identity::canonical(output.parent().unwrap()).unwrap();
    assert_eq!(
        crate::path_identity::normal(&output),
        crate::path_identity::normal(&parent.join(output.file_name().unwrap())),
        "Output must be canonical and new"
    );
    // Keep Windows' extended prefix for the actual long runtime members. The
    // comparison above strips only that prefix, never a link or short alias.
    let output = parent.join(output.file_name().unwrap());
    // Refuse overwriting any existing data, including a previous failed run.
    fs::create_dir(&output).unwrap();
    let installed = output.join("runtime");
    fs::create_dir(&installed).unwrap();
    let catalog = assets::macos_reference_catalog().unwrap();
    let began = Instant::now();
    let check = || {
        if began.elapsed() > Duration::from_secs(1200) {
            Err(error(Code::TimedOut))
        } else {
            Ok(())
        }
    };
    let last_report = Cell::new(0);
    let progress = |count: usize| {
        if count >= last_report.get() + 5_000 {
            eprintln!("Mac reference data extracted {count} files");
            last_report.set(count);
        }
    };
    let mut entries = Vec::new();
    let mut verified = Vec::new();
    for (number, asset) in catalog.assets.iter().enumerate() {
        let path = source.join(assets::relative(&asset.path).unwrap());
        let pin = hash(regular(&path).unwrap(), asset.bytes, &check).unwrap();
        assert_eq!(
            pin,
            (asset.bytes, asset.sha256.clone()),
            "Source pin: {}",
            asset.path
        );
        if asset.kind == Kind::Model {
            let destination = installed.join(assets::relative(&asset.path).unwrap());
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            let mut input = regular(&path).unwrap();
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .unwrap();
            let mut written = 0u64;
            let mut buffer = [0; 64 << 10];
            loop {
                check().unwrap();
                let count = input.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                written += count as u64;
                assert!(written <= asset.bytes);
                output.write_all(&buffer[..count]).unwrap();
            }
            output.sync_all().unwrap();
            assert_eq!(written, asset.bytes);
            entries.push(Entry {
                path: asset.path.clone(),
                bytes: asset.bytes,
                sha256: asset.sha256.clone(),
            });
        } else {
            unpack(
                &path,
                asset.kind,
                &catalog,
                &installed,
                &mut entries,
                &check,
                &progress,
            )
            .unwrap();
        }
        verified.push(serde_json::json!({"path": asset.path, "bytes": pin.0, "sha256": pin.1}));
        eprintln!(
            "Mac reference data asset {}/{} verified: {}",
            number + 1,
            catalog.assets.len(),
            asset.path
        );
    }
    write(
        &installed,
        &catalog.bootstrap_path,
        catalog.bootstrap.as_bytes(),
        &mut entries,
    )
    .unwrap();
    write(&installed, "worker.py", assets::WORKER, &mut entries).unwrap();
    catalog.validate_inventory(&entries).unwrap();
    let mut inventory = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(installed.join("inventory.json"))
        .unwrap();
    inventory
        .write_all(&serde_json::to_vec(&entries).unwrap())
        .unwrap();
    inventory.sync_all().unwrap();
    drop(inventory);
    eprintln!("Mac reference data: verifying every installed file");
    verify(&installed, &catalog, &entries, &check).unwrap();
    // Reopen exactly what was written, then detect an unexpected importable file.
    let reopened = load(&installed, &catalog).unwrap();
    let unexpected = installed.join("python/foreign_import.py");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&unexpected)
        .unwrap();
    file.write_all(b"raise RuntimeError('data-only; never execute')\n")
        .unwrap();
    drop(file);
    assert_eq!(
        verify(&installed, &catalog, &reopened, &check)
            .unwrap_err()
            .code,
        Code::Failed
    );
    fs::remove_file(unexpected).unwrap(); // Only the exact fixture this test created.
    eprintln!("Mac reference data: rechecking installed bytes after fixture removal");
    verify(&installed, &catalog, &reopened, &check).unwrap();
    let result = serde_json::json!({
        "target": catalog.target, "pipeline": catalog.pipeline, "verifiedAssets": verified,
        "installedFiles": entries.len(), "expandedBytes": entries.iter().map(|entry|entry.bytes).sum::<u64>(),
        "inventorySha256": catalog.inventory_sha256, "workerSha256": catalog.worker_sha256,
        "everyInstalledFileVerified": true, "reopened": true, "unexpectedImportRejected": true,
        "actualOcrExecuted": false, "downloadedAssetsExecuted": false, "nativeOcrQualified": false,
        "hostOs": std::env::consts::OS, "hostArch": std::env::consts::ARCH,
        "seconds": began.elapsed().as_secs_f64(),
    });
    let mut report = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("result.json"))
        .unwrap();
    report
        .write_all(&serde_json::to_vec_pretty(&result).unwrap())
        .unwrap();
    report.sync_all().unwrap();
    eprintln!(
        "Mac reference DATA verification passed: {} files; no asset was executed",
        entries.len()
    );
    // Preserve both success and failure output for independent inspection.
}
