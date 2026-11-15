// Explicit ordinary-user component diagnostic; not the GUI CPU benchmark.
// No app profile, clipboard, model, file contents or service management.
#[cfg(not(windows))]
pub(crate) fn run(_args: &[String]) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(windows)]
pub(crate) fn run(args: &[String]) -> std::io::Result<()> {
    use super::*;
    use prism_ntfs_protocol::DriveLetter;
    use prism_ntfs_windows::{administration, client, scm};
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    if std::env::var("PRISM_DEV_FILENAME_LIVE_CPU").ok().as_deref() != Some("1")
        || std::env::var("PRISM_DEV_FILENAME_CPU_TEST_MODE")
            .ok()
            .as_deref()
            != Some("1")
        || args.len() != 2
        || args[0] != "--drive"
        || args[1].len() != 1
        || !matches!(
            std::env::var("PRISM_DEV_FILENAME_CPU_PROFILE")
                .ok()
                .as_deref(),
            Some("0" | "1")
        )
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let drive = DriveLetter::new(args[1].as_bytes()[0])?;
    println!("{{\"waitingForStart\":true}}");
    io::stdout().flush()?;
    let mut gate = [0];
    io::stdin().read_exact(&mut gate)?;
    if gate != *b"G" || usable_cpus() != 1 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if administration::is_elevated()? || scm::state()? != scm::State::Running {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let root = drive.root();
    let identity = client::root_identity(Path::new(&root))?;
    let index = FileIndex::new();
    let started = Instant::now();
    let scope_id = "owned-live-cpu";
    index.configure(vec![SearchScope {
        id: scope_id.into(),
        name: "Owned read-only CPU probe".into(),
        path: root.clone(),
        filename_enabled: true,
        content_enabled: false,
        external_allowed: false,
    }]);
    let mut first_search = None;
    let mut queries = 0usize;
    let mut last_query = Instant::now();
    let mut timeline = Vec::new();
    let final_status = loop {
        if started.elapsed() > Duration::from_secs(120) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let status = index.status().map_err(|_| io::ErrorKind::Other)?;
        if status.count > 3_000_000 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if timeline.last().is_none_or(|row: &(u128, usize, bool)| {
            row.1 != status.count || row.2 != status.scanning
        }) {
            timeline.push((started.elapsed().as_millis(), status.count, status.scanning));
        }
        if status.count > 0
            && (first_search.is_none() || last_query.elapsed() >= Duration::from_secs(2))
        {
            let response = index
                .search_with_keyboard("", "", Some(scope_id), 200, true)
                .map_err(|_| io::ErrorKind::Other)?;
            queries += 1;
            if response.total > 0 {
                first_search.get_or_insert(started.elapsed().as_millis());
            }
            last_query = Instant::now();
        }
        if status.initial_scan_complete && !status.scanning {
            break status;
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let ready_ms = started.elapsed().as_millis();
    if final_status.service_snapshots != 1
        || final_status.overflow_reconciles != 0
        || final_status.count == 0
        || first_search.is_none()
        || client::root_identity(Path::new(&root))? != identity
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // Only numbers and fixed phase names are serialized; warnings/result paths
    // and the root's volume identity remain private.
    let result = serde_json::json!({
        "headlessComponent":true,"actualProductionFileIndex":true,
        "guiWebViewIpcIncluded":false,"replacedResponses":false,
        "firstSearchMs":first_search,"fullReadyMs":ready_ms,"queryCount":queries,
        "records":final_status.count,"warningCount":final_status.warnings.len(),
        "serviceSnapshots":final_status.service_snapshots,"overflowReconciles":final_status.overflow_reconciles,
        "lastServiceSnapshot":final_status.last_service_snapshot,"filenameCpuProfile":cpu_profile::snapshot(),
        "timeline":timeline,"waitingForClose":true,
    });
    println!("{result}");
    io::stdout().flush()?;
    // Keep the owned process alive while the controller reads process/Job CPU.
    // EOF or an invalid byte is a failed diagnostic, not a successful result.
    io::stdin().read_exact(&mut gate)?;
    if gate != *b"Q" {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    drop(index);
    Ok(())
}
