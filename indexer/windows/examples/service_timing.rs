//! Ordinary read-only client. Report aggregate timings without exposing paths.
//! The three intervals are observed client boundaries, not a server profiler:
//! the first includes connect/prepare, and the last includes final validation.
#[cfg(not(windows))]
fn main() {
    eprintln!("The filename service timing client requires Windows.");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        // Never forward diagnostics that could contain a filesystem path.
        eprintln!("Filename service timing failed ({:?}).", error.kind());
        std::process::exit(2);
    }
}

#[cfg(windows)]
fn run() -> std::io::Result<()> {
    use prism_ntfs_protocol::{stream::Event, DriveLetter, VERSION};
    use prism_ntfs_windows::{administration, client, scm};
    use std::{io, path::Path, sync::atomic::AtomicBool, sync::Arc, time::Instant};

    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 || args[0] != "--drive" || args[1].len() != 1 {
        eprintln!("Usage: service_timing --drive C");
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let drive = DriveLetter::new(args[1].as_bytes()[0])?;
    if administration::is_elevated()? {
        eprintln!("Use an ordinary, unelevated shell for this client measurement.");
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if scm::state()? != scm::State::Running {
        // A missing/stopped service is never implicitly installed or started.
        return Err(io::ErrorKind::NotConnected.into());
    }

    let start = Instant::now();
    let mut first = None;
    let mut last = None;
    let mut records = 0u64;
    let mut path_bytes = 0u64;
    let mut progress_events = 0u64;
    client::snapshot(
        Path::new(&drive.root()),
        Arc::new(AtomicBool::new(false)),
        |event| {
            match event {
                Event::Records(batch) => {
                    first.get_or_insert_with(|| start.elapsed());
                    records += batch.len() as u64;
                    path_bytes += batch.iter().map(|row| row.path.len() as u64).sum::<u64>();
                    last = Some(start.elapsed());
                }
                Event::Progress { .. } => progress_events += 1,
                // The production snapshot client validates/rejects terminal
                // ordering and removals; do not manufacture a successful reply.
                Event::Removed(_) | Event::Finished { .. } => {
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            Ok(())
        },
    )?;
    let total = start.elapsed();
    let first_ms = first.map_or_else(|| "null".into(), |value| value.as_millis().to_string());
    let transfer_ms = first.zip(last).map_or_else(
        || "null".into(),
        |(first, last)| (last - first).as_millis().to_string(),
    );
    let after_ms = last.map_or_else(
        || "null".into(),
        |last| (total - last).as_millis().to_string(),
    );
    println!(
        "{{\"ordinaryClient\":true,\"protocolVersion\":{VERSION},\"records\":{records},\"pathBytes\":{path_bytes},\"progressEvents\":{progress_events},\"firstRecordsMs\":{first_ms},\"transferWindowMs\":{transfer_ms},\"afterRecordsMs\":{after_ms},\"totalMs\":{}}}",
        total.as_millis(),
    );
    Ok(())
}
