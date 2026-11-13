//! Bound decoder threads by the CPUs this process may actually use.
const PARALLEL_MFT_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn workers(mft_bytes: u64) -> usize {
    // Small scans cannot amortize a pool. Only pure decoding, never source
    // volume/journal operations, runs on the selected worker threads.
    if mft_bytes < PARALLEL_MFT_BYTES {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    #[cfg(windows)]
    let affinity = {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: the current-process pseudo handle needs no closing and the
        // output pointers refer to valid locals. Read outputs only on success.
        if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } != 0 {
            Some(process)
        } else {
            None
        }
    };
    #[cfg(not(windows))]
    let affinity = None;
    limited(available, affinity)
}

fn limited(available: usize, affinity: Option<usize>) -> usize {
    // Zero masks can mean a multi-group process rather than zero CPUs. Failed
    // queries and that case keep the standard estimate and the existing cap.
    let usable = match affinity {
        Some(mask) if mask != 0 => available.min(mask.count_ones() as usize),
        _ => available,
    };
    // Four decoders matched or beat eight in the owned-buffer benchmark,
    // without doubling its bounded raw working set.
    usable.clamp(1, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_budget_handles_unknown_masks_and_preserves_small_scan_and_pool_bounds() {
        for (available, affinity, expected) in [
            (8, Some(0b1000), 1),
            (8, Some(0b1010), 2),
            (1, Some(usize::MAX), 1),
            (8, Some(usize::MAX), 4),
            (8, Some(0), 4),
            (8, None, 4),
            (0, None, 1),
        ] {
            assert_eq!(limited(available, affinity), expected);
        }
        assert_eq!(workers(0), 1);
        assert_eq!(workers(PARALLEL_MFT_BYTES - 1), 1);
        assert!((1..=4).contains(&workers(PARALLEL_MFT_BYTES)));
    }

    #[cfg(windows)]
    #[test]
    fn process_affinity_limits_cold_scan_workers() {
        use std::{
            io::{Read, Write},
            os::windows::{io::AsRawHandle, process::CommandExt},
            process::{Child, Command, Stdio},
            time::{Duration, Instant},
        };
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetProcessAffinityMask, SetProcessAffinityMask,
        };
        const CHILD_ENV: &str = "PRISM_TEST_NTFS_AFFINITY_CHILD";
        if let Ok(expected) = std::env::var(CHILD_ENV) {
            let expected: usize = expected.parse().unwrap();
            assert!([1, 2, 4].contains(&expected));
            let mut gate = [0];
            std::io::stdin().read_exact(&mut gate).unwrap();
            assert_eq!(gate, *b"G");
            let (mut process, mut system) = (0usize, 0usize);
            // SAFETY: current-process pseudo handle and valid output locals.
            assert_ne!(
                unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) },
                0
            );
            assert_eq!(process.count_ones() as usize, expected);
            let standard = std::thread::available_parallelism().map_or(1, usize::from);
            let selected = workers(PARALLEL_MFT_BYTES);
            assert!(selected <= expected);
            assert_eq!(selected, standard.min(expected).clamp(1, 4));
            assert_eq!(workers(PARALLEL_MFT_BYTES - 1), 1);
            println!(
                "NTFS_AFFINITY_PROBE standard={standard} allowed={expected} workers={selected}"
            );
            return;
        }
        // Change only a newly owned child, never the parent test process or
        // an installed service. Drop also terminates a child after failures.
        struct Probe(Option<Child>);
        impl Drop for Probe {
            fn drop(&mut self) {
                if let Some(child) = self.0.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        let (mut process, mut system) = (0usize, 0usize);
        // SAFETY: current-process pseudo handle and valid output locals.
        assert_ne!(
            unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) },
            0
        );
        assert_ne!(process, 0);
        for count in [1, 2, 4] {
            if process.count_ones() < count {
                continue;
            }
            let (mut rest, mut mask) = (process, 0);
            for _ in 0..count {
                let bit = rest & rest.wrapping_neg();
                mask |= bit;
                rest &= !bit;
            }
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "snapshot::parallelism::tests::process_affinity_limits_cold_scan_workers",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, count.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn()
                .unwrap();
            let mut probe = Probe(Some(child));
            let child = probe.0.as_mut().unwrap();
            // SAFETY: Child owns a live process handle; mask is a nonempty
            // subset of the inherited parent process's available CPUs.
            assert_ne!(
                unsafe { SetProcessAffinityMask(child.as_raw_handle() as _, mask) },
                0
            );
            child.stdin.take().unwrap().write_all(b"G").unwrap();
            let start = Instant::now();
            while child.try_wait().unwrap().is_none() {
                assert!(
                    start.elapsed() < Duration::from_secs(15),
                    "Affinity probe timed out"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = probe.0.take().unwrap().wait_with_output().unwrap();
            assert!(output.status.success(), "{:?}", output);
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains("NTFS_AFFINITY_PROBE"), "{stdout}");
            eprintln!("{stdout}");
        }
    }
}
