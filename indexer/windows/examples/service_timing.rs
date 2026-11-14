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
    use std::{
        io::{self, Read, Write},
        path::Path,
        sync::atomic::AtomicBool,
        sync::Arc,
        time::{Duration, Instant},
    };

    let args: Vec<_> = std::env::args().skip(1).collect();
    let gated = args.len() == 3 && args[2] == "--wait-for-start";
    if !(args.len() == 2 || gated) || args[0] != "--drive" || args[1].len() != 1 {
        eprintln!("Usage: service_timing --drive C [--wait-for-start]");
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let drive = DriveLetter::new(args[1].as_bytes()[0])?;
    if gated {
        // A controller can apply and read back a Job's CPU/memory limits before
        // any service interaction. EOF or anything except G starts no request.
        println!("{{\"waitingForStart\":true}}");
        io::stdout().flush()?;
        let mut byte = [0];
        io::stdin().lock().read_exact(&mut byte)?;
        if byte[0] != b'G' {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
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
    let mut callback = Duration::ZERO;
    client::snapshot(
        Path::new(&drive.root()),
        Arc::new(AtomicBool::new(false)),
        |event| {
            match event {
                Event::Records(batch) => {
                    first.get_or_insert_with(|| start.elapsed());
                    let callback_start = Instant::now();
                    records += batch.len() as u64;
                    path_bytes += batch.iter().map(|row| row.path.len() as u64).sum::<u64>();
                    callback += callback_start.elapsed();
                    last = Some(start.elapsed());
                }
                Event::Progress { .. } => progress_events += 1,
                // The production snapshot client validates/rejects terminal
                // ordering and removals; do not manufacture a successful reply.
                Event::AttributedRecords(_) | Event::Removed(_) | Event::Finished { .. } => {
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
        "{{\"ordinaryClient\":true,\"protocolVersion\":{VERSION},\"records\":{records},\"pathBytes\":{path_bytes},\"progressEvents\":{progress_events},\"firstRecordsMs\":{first_ms},\"transferWindowMs\":{transfer_ms},\"afterRecordsMs\":{after_ms},\"receiveCallbackMs\":{},\"startGateUsed\":{gated},\"totalMs\":{}}}",
        callback.as_millis(),
        total.as_millis(),
    );
    Ok(())
}
