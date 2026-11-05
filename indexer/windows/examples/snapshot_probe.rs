//! Explicit read-only performance probe; never installs or elevates a service.
//! Run from an already authorized administrator shell to measure a live volume.
#[cfg(windows)]
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(2);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("This read-only probe requires Windows.");
    std::process::exit(2);
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use prism_ntfs_core::{layout::Error, snapshot};
    use prism_ntfs_protocol::DriveLetter;
    use prism_ntfs_windows::volume::Volume;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || args[0] != "--drive" || args[1].len() != 1 {
        return Err("Usage: snapshot_probe --drive C".into());
    }
    let drive = DriveLetter::new(args[1].as_bytes()[0]).map_err(|_| "Invalid drive letter.")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let begin = Instant::now();
    let mut volume = Volume::open(drive, cancel.clone()).map_err(|error| {
        format!(
            "Cannot open read-only NTFS volume: {:?}. This probe does not request elevation.",
            error.kind()
        )
    })?;
    let geometry = volume.geometry();
    let mut last = Instant::now();
    let check = || {
        if begin.elapsed() > Duration::from_secs(120) {
            cancel.store(true, Ordering::Release);
            Err(Error::TimedOut)
        } else if cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    let mut result = snapshot::collect(&mut volume, geometry, check, |progress| {
        if last.elapsed() >= Duration::from_millis(500) {
            eprintln!("{progress:?}");
            last = Instant::now();
        }
    })
    .map_err(|error| format!("Snapshot was not published: {error}."))?;
    let collected = begin.elapsed().as_millis();
    let mut count = 0u64;
    result
        .catalog
        .visit(drive, |_| {
            check()?;
            count += 1;
            Ok::<_, Error>(())
        })
        .map_err(|error| format!("Metadata export was not completed: {error}."))?;
    println!("{{\"recordsRead\":{},\"paths\":{},\"collectionMs\":{},\"totalMs\":{},\"serviceInstalled\":false}}",result.records_read,count,collected,begin.elapsed().as_millis());
    Ok(())
}
