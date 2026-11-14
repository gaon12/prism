//! Explicit ordinary-user metadata capture, never a host/helper launcher.
#[cfg(windows)]
#[path = "../../core/examples/support/captured_rows.rs"]
mod captured_rows;
#[cfg(not(windows))]
fn main() {
    eprintln!("The ordinary service capture requires Windows.");
    std::process::exit(2);
}
#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        // The capture contains paths; diagnostics deliberately contain no paths.
        eprintln!("Service metadata capture failed ({:?}).", error.kind());
        std::process::exit(2);
    }
}
#[cfg(windows)]
fn run() -> std::io::Result<()> {
    use prism_ntfs_protocol::{stream::Event, DriveLetter};
    use prism_ntfs_windows::{administration, client, scm};
    use std::{
        fs::OpenOptions,
        io::{self, BufReader, BufWriter, Read, Write},
        os::windows::fs::OpenOptionsExt,
        path::Path,
        sync::{atomic::AtomicBool, Arc},
        time::{Duration, Instant},
    };
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 || args[0] != "--drive" || args[1].len() != 1 || args[2] != "--output" {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let drive = DriveLetter::new(args[1].as_bytes()[0])?;
    println!("{{\"waitingForStart\":true}}");
    io::stdout().flush()?;
    let mut byte = [0];
    io::stdin().read_exact(&mut byte)?;
    if byte != *b"G" {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if administration::is_elevated()? {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if scm::state()? != scm::State::Running {
        return Err(io::ErrorKind::NotConnected.into());
    }
    let started = Instant::now();
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    let mut capture =
        captured_rows::Writer::new(BufWriter::with_capacity(1024 * 1024, file), drive)?;
    let revision = client::snapshot(
        Path::new(&drive.root()),
        Arc::new(AtomicBool::new(false)),
        |event| match event {
            Event::Records(rows) => capture.push(rows),
            Event::Progress { .. } => Ok(()),
            Event::AttributedRecords(_) | Event::Removed(_) | Event::Finished { .. } => {
                Err(io::ErrorKind::InvalidData.into())
            }
        },
    )?;
    // Only the fully authenticated, terminal and root-revalidated exchange can
    // append success. Any prior failure leaves an unreadable partial capture.
    let (writer, summary) = capture.finish(revision)?;
    writer.get_ref().sync_all()?;
    drop(writer);
    let capture_ms = started.elapsed().as_millis();
    let input = OpenOptions::new().read(true).share_mode(1).open(&args[3])?;
    let parsed = captured_rows::read(
        BufReader::with_capacity(1024 * 1024, input),
        || {
            if started.elapsed() > Duration::from_secs(150) {
                Err(io::ErrorKind::TimedOut.into())
            } else {
                Ok(())
            }
        },
        |_| Ok(()),
    )?;
    if parsed != summary {
        return Err(io::ErrorKind::InvalidData.into());
    }
    println!("{{\"ordinaryClient\":true,\"metadataOnly\":true,\"startGateUsed\":true,\"captureComplete\":true,\"readbackVerified\":true,\"records\":{},\"pathBytes\":{},\"captureBytes\":{},\"captureMs\":{capture_ms},\"totalMs\":{}}}",summary.records,summary.path_bytes,summary.bytes,started.elapsed().as_millis());
    Ok(())
}
