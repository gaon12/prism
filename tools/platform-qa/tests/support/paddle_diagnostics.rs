//! Failure-only evidence from the test's own public-fixture parent process.
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAX_LOG_BYTES: u64 = 16 * 1024;

pub fn last_phase(ready: &Path) -> String {
    fs::read_to_string(ready.with_extension("phase")).unwrap_or_else(|_| "unavailable".into())
}

fn log_tail(path: &Path) -> std::io::Result<String> {
    let mut input = fs::File::open(path)?;
    let start = input.metadata()?.len().saturating_sub(MAX_LOG_BYTES);
    input.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    input.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub struct FailureDiagnostics<'a> {
    pub ready: &'a Path,
    pub log: &'a Path,
}

impl Drop for FailureDiagnostics<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        // Declared before the owned Child guard: cleanup runs first, diagnostics
        // second, and TempDir deletion last. Diagnostic I/O must not panic again.
        let mut output = std::io::stderr().lock();
        let _ = writeln!(
            output,
            "Paddle parent last phase: {}",
            last_phase(self.ready)
        );
        match log_tail(self.log) {
            Ok(tail) => {
                let _ = writeln!(
                    output,
                    "Paddle parent fixture.log tail (max 16 KiB):\n{tail}"
                );
            }
            Err(error) => {
                let _ = writeln!(output, "Paddle parent fixture.log unavailable: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn log_reads_are_bounded_and_keep_the_end() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.log");
        let mut bytes = vec![b'x'; MAX_LOG_BYTES as usize * 2];
        bytes.extend_from_slice(b"final fixture diagnostic");
        fs::write(&path, bytes).unwrap();
        let tail = log_tail(&path).unwrap();
        assert_eq!(tail.len(), MAX_LOG_BYTES as usize);
        assert!(tail.ends_with("final fixture diagnostic"));
    }

    #[test]
    fn failed_child_preserves_phase_and_log_before_temp_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["diagnostic_child", "--ignored", "--nocapture"])
            .env("PRISM_QA_DIAGNOSTIC_CHILD", directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("controlled diagnostic failure"));
        assert!(stderr.contains("Paddle parent last phase:"));
        assert!(stderr.contains("auto-image"));
        assert!(stderr.contains("retained fixture detail"));
        assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
    }

    #[test]
    #[ignore = "Controlled child of the failure diagnostics regression"]
    fn diagnostic_child() {
        let parent = std::env::var_os("PRISM_QA_DIAGNOSTIC_CHILD").unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let ready = directory.path().join("ready.json");
        let log = directory.path().join("fixture.log");
        fs::write(ready.with_extension("phase"), r#"{"phase":"auto-image"}"#).unwrap();
        fs::write(&log, "retained fixture detail").unwrap();
        let _diagnostics = FailureDiagnostics {
            ready: &ready,
            log: &log,
        };
        panic!("controlled diagnostic failure");
    }
}
